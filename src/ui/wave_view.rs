use std::cell::RefCell;
use std::rc::Rc;
use gtk4::prelude::*;
use gtk4::{glib, Align, Box as GtkBox, Button, Label, Orientation, Overlay};

use crate::player::PlayerManager;

/// Состояние цветов и параметров живого рендерера «Плавающего нечто»
#[derive(Clone)]
pub struct WaveShaderState {
    pub cur_core: [f64; 3],
    pub cur_deep: [f64; 3],
    pub cur_ridge: [f64; 3],
    pub target_core: [f64; 3],
    pub target_deep: [f64; 3],
    pub target_ridge: [f64; 3],
    pub time: f64,
    pub bass: f64,
    pub is_playing: bool,
}

impl WaveShaderState {
    pub fn new() -> Self {
        let core = [0.95, 0.42, 0.02];
        let deep = [0.36, 0.10, 0.00];
        let ridge = [1.0, 0.84, 0.42];
        Self {
            cur_core: core,
            cur_deep: deep,
            cur_ridge: ridge,
            target_core: core,
            target_deep: deep,
            target_ridge: ridge,
            time: 0.0,
            bass: 0.0,
            is_playing: false,
        }
    }

    pub fn set_target_colors(&mut self, core: [f64; 3], deep: [f64; 3], ridge: [f64; 3]) {
        self.target_core = core;
        self.target_deep = deep;
        self.target_ridge = ridge;
    }

    pub fn update_from_track(&mut self, track: &crate::api::models::Track) {
        let c = track.core_rgb();
        let d = track.deep_rgb();
        let r = track.ridge_rgb();
        self.set_target_colors(
            [c[0] as f64, c[1] as f64, c[2] as f64],
            [d[0] as f64, d[1] as f64, d[2] as f64],
            [r[0] as f64, r[1] as f64, r[2] as f64],
        );
    }

    /// Цвета по самой обложке: у Яндекса `derivedColors` — среднее по картинке, и у яркой обложки
    /// на тёмном фоне (неон, закат) выходит серо-лиловая каша. Серую обложку оставляем как есть.
    pub fn update_from_cover(&mut self, path: &str) {
        let Ok(pixbuf) = gtk4::gdk_pixbuf::Pixbuf::from_file_at_scale(path, 48, 48, false) else { return };
        let (n, stride) = (pixbuf.n_channels() as usize, pixbuf.rowstride() as usize);
        let bytes = pixbuf.read_pixel_bytes();
        let mut px = Vec::with_capacity(48 * 48);
        for y in 0..pixbuf.height() as usize {
            for x in 0..pixbuf.width() as usize {
                let i = y * stride + x * n;
                if let Some(p) = bytes.get(i..i + 3) {
                    px.push([p[0] as f64 / 255.0, p[1] as f64 / 255.0, p[2] as f64 / 255.0]);
                }
            }
        }
        if let Some((core, deep, ridge)) = palette_from_pixels(&px) {
            self.set_target_colors(core, deep, ridge);
        }
    }

    /// `bass_level` — реальный уровень баса 0..1 из аудио-бэкенда
    pub fn tick(&mut self, dt: f64, bass_level: f64) {
        // Плавная интерполяция текущих цветов к целевым цветам текущего трека:
        // cur = cur + (target - cur) * 0.05
        let interp = 0.05_f64;
        for i in 0..3 {
            self.cur_core[i] += (self.target_core[i] - self.cur_core[i]) * interp;
            self.cur_deep[i] += (self.target_deep[i] - self.cur_deep[i]) * interp;
            self.cur_ridge[i] += (self.target_ridge[i] - self.cur_ridge[i]) * interp;
        }

        if self.is_playing {
            // Воспроизведение: течение плазмы + пульсация по реальному басу (0..1 → 0..0.35);
            // атака/спад уже в анализаторе, тут только сглаживаем 30 Гц обновлений до 60 FPS
            self.time += dt * 1.0;
            self.bass += (bass_level * 0.35 - self.bass) * 0.5;
        } else {
            // Пауза: замедление скорости течения плазмы и плавное затухание баса
            self.time += dt * 0.18;
            self.bass *= 0.92;
        }
    }
}

pub struct WaveViewWidgets {
    pub container: Overlay,
    pub title_label: Label,
    pub explicit_badge: Label,
    pub artist_label: Label,
    pub play_btn: Button,
    pub play_btn_icon: gtk4::Image,
    pub menu_button: Button,
    pub drawing_area: gtk4::GLArea,
    pub shader_state: Rc<RefCell<WaveShaderState>>,
    pub content_box: GtkBox,
    pub filter_pill: GtkBox,
    /// Клип на фоне (превью-ролик трека)
    pub clip: Rc<super::wave_clip::ClipController>,
    pub pill_label: Label,
    pill_close: Button,
    /// Куда ведёт клик по плашке (источник очереди или станция)
    pub source_target: Rc<RefCell<Option<crate::player::SourceTarget>>>,
    option_buttons: Rc<RefCell<Vec<Button>>>,
}

impl WaveViewWidgets {
    /// Плашка типа Волны (по артисту или треку); `None` — убрать плашку
    pub fn set_context(&self, label: Option<&str>) {
        match label {
            Some(text) => {
                self.pill_label.set_text(text);
                self.pill_close.set_tooltip_text(Some("Сбросить фильтр"));
                self.pill_close.set_visible(true);
                self.filter_pill.set_visible(true);
                for b in self.option_buttons.borrow().iter() {
                    b.remove_css_class("selected");
                }
            }
            None => self.filter_pill.set_visible(false),
        }
    }

    /// Подпись источника очереди (плейлист, альбом, топ артиста); крестик включает обычную Волну
    pub fn set_source(&self, label: Option<&str>) {
        match label {
            Some(text) => {
                self.pill_label.set_text(text);
                self.pill_close.set_tooltip_text(Some("Сбросить: включить обычную Волну"));
                self.pill_close.set_visible(true);
                self.filter_pill.set_visible(true);
            }
            None => self.filter_pill.set_visible(false),
        }
    }

    /// Обновляет целевые цвета шейдера при переключении трека из derivedColors
    pub fn update_track_colors(&self, track: &crate::api::models::Track) {
        self.shader_state.borrow_mut().update_from_track(track);
    }
}

pub fn create_wave_view(
    player: &PlayerManager,
    is_playing_state: Rc<RefCell<bool>>,
    accent_color: Rc<RefCell<[f64; 3]>>, // RGB: 0.0 .. 1.0
) -> WaveViewWidgets {
    let shader_state = Rc::new(RefCell::new(WaveShaderState::new()));

    // Корневой оверлей экрана Волны
    let root_overlay = Overlay::new();
    root_overlay.set_hexpand(true);
    root_overlay.set_vexpand(true);

    // ================= Полноэкранный рендерер живого «Плавающего нечто» =================
    // Рисует видеокарта (шейдер в wave_gl.rs); процессор только двигает время и цвета
    let drawing_area = gtk4::GLArea::new();
    drawing_area.set_hexpand(true);
    drawing_area.set_vexpand(true);
    let play_center = Rc::new(std::cell::Cell::new((0.0_f64, 0.0_f64)));
    // Клип на фоне: декодер пишет кадры в общую ячейку, шейдер их показывает
    let clip_shared = std::sync::Arc::new(super::wave_clip::ClipShared::default());
    let clip = Rc::new(super::wave_clip::ClipController::new(clip_shared.clone()));
    // Запасной фон под GLArea: статичный градиент темы; показывается, только если OpenGL недоступен
    let fallback = GtkBox::new(Orientation::Vertical, 0);
    fallback.add_css_class("ambient-backdrop");
    fallback.set_hexpand(true);
    fallback.set_vexpand(true);
    fallback.set_visible(false);
    {
        let fb = fallback.clone();
        super::wave_gl::attach(&drawing_area, shader_state.clone(), play_center.clone(), clip_shared.clone(), move || fb.set_visible(true));
    }

    // Центральная кнопка Play/Pause (фиксированный размер 72x72)
    let play_btn = Button::new();
    play_btn.add_css_class("hero-play-badge");
    play_btn.set_size_request(72, 72);
    play_btn.set_halign(Align::Center);
    play_btn.set_valign(Align::Center);
    let play_btn_icon = gtk4::Image::from_icon_name("media-playback-start-symbolic");
    play_btn_icon.set_pixel_size(32);
    play_btn.set_child(Some(&play_btn_icon));

    // Анимация 60 FPS через add_tick_callback на базе GdkFrameClock: шаг состояния и запрос кадра у видеокарты
    {
        let state_ref = shader_state.clone();
        let is_playing_clone = is_playing_state.clone();
        let audio = player.audio.clone();
        let last_time_ref = Rc::new(RefCell::new(0_i64));
        let play_btn_ref = play_btn.clone();
        let tick_n = std::cell::Cell::new(0u32);
        let clip_for_tick = clip_shared.clone();

        drawing_area.add_tick_callback(move |area, frame_clock| {
            let now = frame_clock.frame_time(); // время в микросекундах
            let mut last = last_time_ref.borrow_mut();
            let dt = if *last == 0 {
                0.016_f64
            } else {
                ((now - *last) as f64 / 1_000_000.0).clamp(0.001, 0.1)
            };
            *last = now;

            {
                let mut state = state_ref.borrow_mut();
                state.is_playing = *is_playing_clone.borrow();
                state.tick(dt, audio.bass_level() as f64);
            }

            // На паузе плазма еле дрейфует: бас затух — кадр каждый 4-й тик (≈15 FPS)
            let n = tick_n.get().wrapping_add(1);
            tick_n.set(n);
            let idle = {
                let st = state_ref.borrow();
                !st.is_playing && st.bass < 0.005 && !clip_for_tick.active()
            };
            if idle && n % 4 != 0 {
                return glib::ControlFlow::Continue;
            }

            // Центр кнопки Play в координатах фона
            let center = play_btn_ref
                .compute_point(area, &gtk4::graphene::Point::new(36.0, 36.0))
                .map(|p| (p.x() as f64, p.y() as f64))
                .unwrap_or((area.width() as f64 / 2.0, area.height() as f64 * 0.38));
            play_center.set(center);
            area.queue_render();
            glib::ControlFlow::Continue
        });
    }

    root_overlay.set_child(Some(&fallback));
    root_overlay.add_overlay(&drawing_area);

    // ================= Центральный блок интерфейса поверх фона =================
    // Вертикальная компоновка: кнопка Play по центру сферы, а текст и настройки строго ПОД сферой!
    let content_box = GtkBox::new(Orientation::Vertical, 0);
    content_box.set_valign(Align::Center);
    content_box.set_halign(Align::Center);
    content_box.set_margin_bottom(50); // Учитываем нижний плавающий остров плеера

    content_box.append(&play_btn);

    // Блок метаданных и кнопки настроек: размещен под кнопкой Play; отступ подобран, чтобы кнопка настроек не уходила под плеер
    let bottom_box = GtkBox::new(Orientation::Vertical, 6);
    bottom_box.set_halign(Align::Center);
    bottom_box.set_margin_top(110);

    let title_label = Label::new(Some("Волна"));
    title_label.add_css_class("hero-track-title");
    title_label.set_wrap(true);
    title_label.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
    title_label.set_justify(gtk4::Justification::Center);
    title_label.set_max_width_chars(24);
    let explicit_badge = super::collection::explicit_badge();
    explicit_badge.set_margin_start(12);
    explicit_badge.add_css_class("hero");
    let title_row = GtkBox::new(Orientation::Horizontal, 0);
    title_row.set_halign(Align::Center);
    title_row.append(&title_label);
    title_row.append(&explicit_badge);
    bottom_box.append(&title_row);

    let artist_label = Label::new(Some("Нажмите для запуска персонального потока 🌊"));
    artist_label.add_css_class("hero-track-artist");
    artist_label.set_wrap(true);
    artist_label.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
    artist_label.set_justify(gtk4::Justification::Center);
    artist_label.set_max_width_chars(48);
    bottom_box.append(&artist_label);

    // Центральная кнопка фильтров Ротора «Настроить волну»
    let menu_button = Button::new();
    let menu_label = Label::new(Some("Настроить волну"));
    let menu_content = GtkBox::new(Orientation::Horizontal, 8);
    menu_content.append(&gtk4::Image::from_icon_name("preferences-system-symbolic"));
    menu_content.append(&menu_label);
    menu_content.append(&gtk4::Image::from_icon_name("pan-down-symbolic"));
    menu_button.set_child(Some(&menu_content));
    menu_button.add_css_class("fluid-pill");
    menu_button.set_halign(Align::Center);
    menu_button.set_margin_top(14);

    // Плашка сверху: какой тип Волны включён; у простой Волны плашки нет, крестик сбрасывает фильтр
    let filter_pill = GtkBox::new(Orientation::Horizontal, 6);
    filter_pill.add_css_class("wave-filter-pill");
    filter_pill.set_halign(Align::Center);
    filter_pill.set_valign(Align::Start);
    filter_pill.set_margin_top(18);
    filter_pill.set_visible(false);
    let pill_label = Label::new(None);
    let pill_close = Button::from_icon_name("window-close-symbolic");
    pill_close.add_css_class("flat");
    pill_close.add_css_class("circular");
    pill_close.set_tooltip_text(Some("Сбросить фильтр"));
    filter_pill.append(&pill_label);
    filter_pill.append(&pill_close);

    // Панель настройки Волны внутри окна: затемнение + карточка по центру экрана волны
    let popover = GtkBox::new(Orientation::Vertical, 0);
    popover.add_css_class("wave-scrim");
    popover.set_visible(false);
    popover.set_hexpand(true);
    popover.set_vexpand(true);
    popover.set_halign(Align::Fill);
    popover.set_valign(Align::Fill);

    // выбранный пункт подсвечивается; общий список нужен, чтобы снять подсветку с прежнего
    let option_buttons: Rc<RefCell<Vec<Button>>> = Rc::default();
    // куда ведёт клик по плашке; выбор фильтра сбрасывает цель (плашка тогда показывает фильтры)
    let source_target: Rc<RefCell<Option<crate::player::SourceTarget>>> = Rc::default();

    let popover_box = GtkBox::new(Orientation::Vertical, 14);
    popover_box.add_css_class("wave-settings-card");
    popover_box.set_halign(Align::Center);
    popover_box.set_valign(Align::Center);
    popover_box.set_vexpand(true);

    let card_header = GtkBox::new(Orientation::Horizontal, 8);
    let card_title = Label::new(Some("Настроить волну"));
    card_title.add_css_class("wave-settings-title");
    card_title.set_hexpand(true);
    card_title.set_halign(Align::Start);
    let close_btn = Button::from_icon_name("window-close-symbolic");
    close_btn.add_css_class("karaoke-close-btn");
    close_btn.set_tooltip_text(Some("Закрыть"));
    card_header.append(&card_title);
    card_header.append(&close_btn);
    popover_box.append(&card_header);

    struct RotorFilterOption {
        label: &'static str,
        mood: Option<&'static str>,
        lang: Option<&'static str>,
        charac: Option<&'static str>,
        color: [f64; 3],
    }

    struct FilterSection {
        title: &'static str,
        options: Vec<RotorFilterOption>,
    }

    let sections = vec![
        FilterSection {
            title: "Настроение",
            options: vec![
                RotorFilterOption {
                    label: "⚡ Энергичное",
                    mood: Some("active"),
                    lang: None,
                    charac: None,
                    color: [1.0, 0.42, 0.29],
                },
                RotorFilterOption {
                    label: "☕ Спокойное",
                    mood: Some("calm"),
                    lang: None,
                    charac: None,
                    color: [0.21, 0.52, 0.89],
                },
                RotorFilterOption {
                    label: "🎉 Весёлое",
                    mood: Some("fun"),
                    lang: None,
                    charac: None,
                    color: [0.90, 0.65, 0.04],
                },
                RotorFilterOption {
                    label: "🌧 Грустное",
                    mood: Some("sad"),
                    lang: None,
                    charac: None,
                    color: [0.45, 0.50, 0.68],
                },
            ],
        },
        FilterSection {
            title: "Характер",
            options: vec![
                RotorFilterOption {
                    label: "❤️ Моё любимое",
                    mood: None,
                    lang: None,
                    charac: Some("favorite"),
                    color: [0.95, 0.20, 0.40],
                },
                RotorFilterOption {
                    label: "✨ Незнакомое",
                    mood: None,
                    lang: None,
                    charac: Some("discover"),
                    color: [0.61, 0.15, 0.69],
                },
                RotorFilterOption {
                    label: "🔥 Популярное",
                    mood: None,
                    lang: None,
                    charac: Some("popular"),
                    color: [0.95, 0.35, 0.10],
                },
            ],
        },
        FilterSection {
            title: "Язык",
            options: vec![
                RotorFilterOption {
                    label: "🇷🇺 На русском",
                    mood: None,
                    lang: Some("russian"),
                    charac: None,
                    color: [0.15, 0.64, 0.41],
                },
                RotorFilterOption {
                    label: "🌐 Иностранный",
                    mood: None,
                    lang: Some("not-russian"),
                    charac: None,
                    color: [0.12, 0.68, 0.82],
                },
                RotorFilterOption {
                    label: "🎵 Без слов",
                    mood: None,
                    lang: Some("without-words"),
                    charac: None,
                    color: [0.55, 0.35, 0.75],
                },
            ],
        },
    ];

    // выбор по одному пункту в каждом разделе (настроение, характер, язык); разделы сочетаются
    type Pick = Option<(&'static str, &'static str, [f64; 3])>;
    let selection: Rc<RefCell<[Pick; 3]>> = Rc::new(RefCell::new([None; 3]));
    let slot_buttons: Rc<RefCell<Vec<(Button, usize, &'static str)>>> = Rc::default();

    for (s_idx, section) in sections.into_iter().enumerate() {
        let section_flow = gtk4::FlowBox::new();
        section_flow.set_selection_mode(gtk4::SelectionMode::None);
        section_flow.set_max_children_per_line(4);
        section_flow.set_column_spacing(8);
        section_flow.set_row_spacing(8);

        let section_header = Label::new(Some(section.title));
        section_header.add_css_class("rotor-section-title");
        section_header.set_halign(Align::Start);
        popover_box.append(&section_header);
        popover_box.append(&section_flow);

        for opt in section.options {
            let btn = Button::new();
            let text = Label::new(Some(opt.label));
            text.set_halign(Align::Start);
            text.set_xalign(0.0);
            btn.set_child(Some(&text));
            btn.add_css_class("flat");
            btn.add_css_class("rotor-filter-btn");
            btn.set_halign(Align::Fill);
            option_buttons.borrow_mut().push(btn.clone());

            let player_clone = player.clone();
            let accent_clone = accent_color.clone();
            let da_weak = drawing_area.downgrade();
            let (pill_label_clone, pill_clone, pill_close_clone) = (pill_label.clone(), filter_pill.clone(), pill_close.clone());
            let source_target_clone = source_target.clone();
            let shader_state_clone = shader_state.clone();

            let opt_label = opt.label;
            let opt_value = opt.mood.or(opt.lang).or(opt.charac).unwrap_or_default();
            let rgb = opt.color;
            slot_buttons.borrow_mut().push((btn.clone(), s_idx, opt_value));
            let selection_clone = selection.clone();
            let slot_buttons_clone = slot_buttons.clone();

            btn.connect_clicked(move |_| {
                // повторный клик по выбранному пункту снимает его
                let (mood_val, char_val, lang_val, core) = {
                    let mut sel = selection_clone.borrow_mut();
                    let same = sel[s_idx].map(|p| p.0) == Some(opt_value);
                    sel[s_idx] = if same { None } else { Some((opt_value, opt_label, rgb)) };
                    for (b, slot, val) in slot_buttons_clone.borrow().iter() {
                        if sel[*slot].map(|p| p.0) == Some(*val) {
                            b.add_css_class("selected");
                        } else {
                            b.remove_css_class("selected");
                        }
                    }
                    let labels: Vec<&str> = sel.iter().flatten().map(|p| p.1).collect();
                    pill_label_clone.set_text(&labels.join(" · "));
                    pill_close_clone.set_tooltip_text(Some("Сбросить фильтр"));
                    *source_target_clone.borrow_mut() = None;
                    pill_clone.set_visible(!labels.is_empty());
                    // цвет фона: последний выбранный пункт, иначе стандартный оранжевый
                    let core = if same {
                        sel.iter().flatten().last().map(|p| p.2).unwrap_or([0.95, 0.42, 0.02])
                    } else {
                        rgb
                    };
                    (sel[0].map(|p| p.0.to_string()), sel[1].map(|p| p.0.to_string()), sel[2].map(|p| p.0.to_string()), core)
                };
                *accent_clone.borrow_mut() = core;
                if let Some(da) = da_weak.upgrade() {
                    da.queue_draw();
                }

                // Плавный перелив цветов шейдера под выбранный фильтр
                let deep = [core[0] * 0.35, core[1] * 0.35, core[2] * 0.35];
                let ridge = [
                    (core[0] * 0.6 + 0.4).min(1.0),
                    (core[1] * 0.6 + 0.4).min(1.0),
                    (core[2] * 0.6 + 0.4).min(1.0),
                ];
                shader_state_clone.borrow_mut().set_target_colors(core, deep, ridge);

                player_clone.apply_rotor_filter(mood_val, lang_val, char_val);
            });

            section_flow.append(&btn);
        }
    }

    // Разделитель и кнопка сброса фильтров
    let reset_btn = Button::new();
    let reset_text = Label::new(Some("↺ Сбросить фильтры"));
    reset_text.set_halign(Align::Start);
    reset_text.set_xalign(0.0);
    reset_btn.set_child(Some(&reset_text));
    reset_btn.add_css_class("flat");
    reset_btn.add_css_class("rotor-reset-btn");
    reset_btn.set_halign(Align::Start);

    {
        let player_clone = player.clone();
        let accent_clone = accent_color.clone();
        let da_weak = drawing_area.downgrade();
        let pop_weak = popover.downgrade();
        let pill_clone = filter_pill.clone();
        let buttons_clone = option_buttons.clone();
        let selection_clone = selection.clone();
        let shader_state_clone = shader_state.clone();

        let reset_filters: Rc<dyn Fn()> = Rc::new(move || {
            if let Some(pop) = pop_weak.upgrade() {
                pop.set_visible(false);
            }
            pill_clone.set_visible(false);
            *selection_clone.borrow_mut() = [None; 3];
            for b in buttons_clone.borrow().iter() {
                b.remove_css_class("selected");
            }
            *accent_clone.borrow_mut() = [1.0, 0.42, 0.29];
            if let Some(da) = da_weak.upgrade() {
                da.queue_draw();
            }

            shader_state_clone.borrow_mut().set_target_colors(
                [0.95, 0.42, 0.02],
                [0.36, 0.10, 0.00],
                [1.0, 0.84, 0.42],
            );

            player_clone.apply_rotor_filter(None, None, None);
        });
        let r = reset_filters.clone();
        reset_btn.connect_clicked(move |_| r());
        pill_close.connect_clicked(move |_| reset_filters());
    }

    popover_box.append(&reset_btn);
    popover.append(&popover_box);

    // Открытие по кнопке; закрытие — крестиком или кликом по затемнению
    {
        let pop = popover.clone();
        menu_button.connect_clicked(move |_| pop.set_visible(true));
        let pop = popover.clone();
        close_btn.connect_clicked(move |_| pop.set_visible(false));
        let click = gtk4::GestureClick::new();
        let pop = popover.clone();
        click.connect_released(move |_, _, x, y| {
            if pop.pick(x, y, gtk4::PickFlags::DEFAULT).as_ref() == Some(pop.upcast_ref::<gtk4::Widget>()) {
                pop.set_visible(false);
            }
        });
        popover.add_controller(click);
    }

    bottom_box.append(&menu_button);
    content_box.append(&bottom_box);

    // Обработчик центральной кнопки Play
    {
        let player_clone = player.clone();
        play_btn.connect_clicked(move |_| {
            player_clone.toggle_play();
        });
    }

    root_overlay.add_overlay(&content_box);
    root_overlay.add_overlay(&filter_pill);
    root_overlay.add_overlay(&popover);

    WaveViewWidgets {
        container: root_overlay,
        title_label,
        explicit_badge,
        artist_label,
        play_btn,
        play_btn_icon,
        menu_button,
        drawing_area,
        shader_state,
        content_box,
        clip,
        filter_pill,
        pill_label,
        pill_close: pill_close.clone(),
        source_target: source_target.clone(),
        option_buttons,
    }
}

/// Основной цвет обложки: самый «весомый» оттенок, где вес пикселя растёт с насыщенностью и яркостью.
/// `None`, если обложка почти бесцветная (тогда остаются цвета Яндекса).
fn palette_from_pixels(px: &[[f64; 3]]) -> Option<([f64; 3], [f64; 3], [f64; 3])> {
    const BINS: usize = 18;
    let mut bins = [(0.0_f64, [0.0_f64; 3]); BINS];
    let mut total = 0.0;
    for &[r, g, b] in px {
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        if max < 0.12 {
            continue;
        }
        let s = (max - min) / max;
        let hue = if max == min {
            0.0
        } else if max == r {
            ((g - b) / (max - min)).rem_euclid(6.0)
        } else if max == g {
            (b - r) / (max - min) + 2.0
        } else {
            (r - g) / (max - min) + 4.0
        } / 6.0;
        let w = s * s * max;
        let bin = &mut bins[((hue * BINS as f64) as usize).min(BINS - 1)];
        bin.0 += w;
        for c in 0..3 {
            bin.1[c] += [r, g, b][c] * w;
        }
        total += w;
    }
    if total < 0.03 * px.len() as f64 {
        return None;
    }
    // лучший оттенок вместе с соседями по кругу, чтобы граница корзин не рвала цвет
    let score = |i: usize| bins[(i + BINS - 1) % BINS].0 + bins[i].0 + bins[(i + 1) % BINS].0;
    let best = (0..BINS).max_by(|&a, &b| score(a).total_cmp(&score(b)))?;
    let (mut w, mut rgb) = (0.0, [0.0; 3]);
    for i in [(best + BINS - 1) % BINS, best, (best + 1) % BINS] {
        w += bins[i].0;
        for c in 0..3 {
            rgb[c] += bins[i].1[c];
        }
    }
    let mut core = rgb.map(|c| c / w);
    // яркость к 0.9 по старшему каналу: оттенок сохраняется, фон не выходит тусклым
    let peak = core[0].max(core[1]).max(core[2]).max(1e-6);
    core = core.map(|c| (c / peak * 0.9).min(1.0));
    let deep = core.map(|c| c * 0.32);
    let ridge = core.map(|c| (c * 0.6 + 0.4).min(1.0));
    Some((core, deep, ridge))
}

#[cfg(test)]
mod palette_tests {
    use super::palette_from_pixels;

    #[test]
    fn neon_on_dark_wins_over_gray() {
        // много серо-синего и тёмного, немного яркого розового неона
        let mut px = vec![[0.35, 0.36, 0.42]; 300];
        px.extend(vec![[0.05, 0.05, 0.1]; 300]);
        px.extend(vec![[1.0, 0.2, 0.6]; 120]);
        let (core, _, _) = palette_from_pixels(&px).unwrap();
        assert!(core[0] > core[1] && core[2] > core[1], "{core:?}");
    }

    #[test]
    fn gray_cover_keeps_yandex_colors() {
        assert!(palette_from_pixels(&vec![[0.5, 0.5, 0.52]; 500]).is_none());
    }
}
