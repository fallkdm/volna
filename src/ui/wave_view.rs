use std::cell::RefCell;
use std::f64::consts::PI;
use std::rc::Rc;
use gtk4::cairo;
use gtk4::prelude::*;
use gtk4::{glib, Align, Box as GtkBox, Button, DrawingArea, Label, Orientation, Overlay};

use crate::player::PlayerManager;

/// Состояние цветов и параметров живого рендерера «Плавающего нечто»
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
    pub artist_label: Label,
    pub play_btn: Button,
    pub play_btn_icon: gtk4::Image,
    pub menu_button: Button,
    pub drawing_area: DrawingArea,
    pub shader_state: Rc<RefCell<WaveShaderState>>,
}

impl WaveViewWidgets {
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
    let drawing_area = DrawingArea::new();
    drawing_area.set_hexpand(true);
    drawing_area.set_vexpand(true);

    // Центральная кнопка Play/Pause (фиксированный размер 72x72)
    let play_btn = Button::new();
    play_btn.add_css_class("hero-play-badge");
    play_btn.set_size_request(72, 72);
    play_btn.set_halign(Align::Center);
    play_btn.set_valign(Align::Center);
    let play_btn_icon = gtk4::Image::from_icon_name("media-playback-start-symbolic");
    play_btn_icon.set_pixel_size(32);
    play_btn.set_child(Some(&play_btn_icon));

    // Отрисовка органического живого фона на Cairo
    {
        let state_ref = shader_state.clone();
        let play_btn_ref = play_btn.clone();
        drawing_area.set_draw_func(move |area, cr, width, height| {
            let state = state_ref.borrow();
            // Вычисляем точный центр кнопки Play в координатах холста
            let (cx, cy) = play_btn_ref
                .compute_point(area, &gtk4::graphene::Point::new(36.0, 36.0))
                .map(|p| (p.x() as f64, p.y() as f64))
                .unwrap_or((width as f64 / 2.0, height as f64 * 0.38));
            render_live_wave_cairo(cr, width as f64, height as f64, cx, cy, &state);
        });
    }

    // Анимация 60 FPS через add_tick_callback на базе GdkFrameClock
    {
        let state_ref = shader_state.clone();
        let is_playing_clone = is_playing_state.clone();
        let audio = player.audio.clone();
        let last_time_ref = Rc::new(RefCell::new(0_i64));

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

            area.queue_draw();
            glib::ControlFlow::Continue
        });
    }

    root_overlay.set_child(Some(&drawing_area));

    // ================= Центральный блок интерфейса поверх фона =================
    // Вертикальная компоновка: кнопка Play по центру сферы, а текст и настройки строго ПОД сферой!
    let content_box = GtkBox::new(Orientation::Vertical, 0);
    content_box.set_valign(Align::Center);
    content_box.set_halign(Align::Center);
    content_box.set_margin_bottom(50); // Учитываем нижний плавающий остров плеера

    content_box.append(&play_btn);

    // Блок метаданных и кнопки настроек: размещен строго под сферой с отступом 175px
    let bottom_box = GtkBox::new(Orientation::Vertical, 6);
    bottom_box.set_halign(Align::Center);
    bottom_box.set_margin_top(175);

    let title_label = Label::new(Some("Моя волна"));
    title_label.add_css_class("hero-track-title");
    title_label.set_wrap(true);
    title_label.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
    title_label.set_justify(gtk4::Justification::Center);
    title_label.set_max_width_chars(24);
    bottom_box.append(&title_label);

    let artist_label = Label::new(Some("Нажмите для запуска персонального потока 🌊"));
    artist_label.add_css_class("hero-track-artist");
    artist_label.set_wrap(true);
    artist_label.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
    artist_label.set_justify(gtk4::Justification::Center);
    artist_label.set_max_width_chars(38);
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

    // Панель настройки Моей волны внутри окна: затемнение + карточка по центру экрана волны
    let popover = GtkBox::new(Orientation::Vertical, 0);
    popover.add_css_class("wave-scrim");
    popover.set_visible(false);
    popover.set_hexpand(true);
    popover.set_vexpand(true);
    popover.set_halign(Align::Fill);
    popover.set_valign(Align::Fill);

    // выбранный пункт подсвечивается; общий список нужен, чтобы снять подсветку с прежнего
    let option_buttons: Rc<RefCell<Vec<Button>>> = Rc::default();

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
                    mood: Some("energetic"),
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
                    lang: Some("foreign"),
                    charac: None,
                    color: [0.12, 0.68, 0.82],
                },
                RotorFilterOption {
                    label: "🎵 Без слов",
                    mood: None,
                    lang: Some("instrumental"),
                    charac: None,
                    color: [0.55, 0.35, 0.75],
                },
            ],
        },
    ];

    for (s_idx, section) in sections.into_iter().enumerate() {
        let _ = s_idx;
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
            let pop_weak = popover.downgrade();
            let label_clone = menu_label.clone();
            let buttons_clone = option_buttons.clone();
            let shader_state_clone = shader_state.clone();

            let opt_label = opt.label;
            let mood_val = opt.mood.map(|s| s.to_string());
            let lang_val = opt.lang.map(|s| s.to_string());
            let char_val = opt.charac.map(|s| s.to_string());
            let rgb = opt.color;

            btn.connect_clicked(move |clicked| {
                if let Some(pop) = pop_weak.upgrade() {
                    pop.set_visible(false);
                }
                label_clone.set_text(opt_label);
                for b in buttons_clone.borrow().iter() {
                    b.remove_css_class("selected");
                }
                clicked.add_css_class("selected");
                *accent_clone.borrow_mut() = rgb;
                if let Some(da) = da_weak.upgrade() {
                    da.queue_draw();
                }

                // Плавный перелив цветов шейдера под выбранный фильтр
                let core = rgb;
                let deep = [core[0] * 0.35, core[1] * 0.35, core[2] * 0.35];
                let ridge = [
                    (core[0] * 0.6 + 0.4).min(1.0),
                    (core[1] * 0.6 + 0.4).min(1.0),
                    (core[2] * 0.6 + 0.4).min(1.0),
                ];
                shader_state_clone.borrow_mut().set_target_colors(core, deep, ridge);

                player_clone.apply_rotor_filter(
                    mood_val.clone(),
                    lang_val.clone(),
                    char_val.clone(),
                );
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
        let label_clone = menu_label.clone();
        let buttons_clone = option_buttons.clone();
        let shader_state_clone = shader_state.clone();

        reset_btn.connect_clicked(move |_| {
            if let Some(pop) = pop_weak.upgrade() {
                pop.set_visible(false);
            }
            label_clone.set_text("Настроить волну");
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
    root_overlay.add_overlay(&popover);

    WaveViewWidgets {
        container: root_overlay,
        title_label,
        artist_label,
        play_btn,
        play_btn_icon,
        menu_button,
        drawing_area,
        shader_state,
    }
}

// ==============================================================================
// Роскошный, кинематографичный рендеринг «Luminous Fluid Orb» на Cairo (100% safe)
// ==============================================================================

fn render_live_wave_cairo(
    cr: &cairo::Context,
    w: f64,
    h: f64,
    cx: f64,
    cy: f64,
    state: &WaveShaderState,
) {
    let max_dim = w.max(h);
    let time = state.time;
    let bass = state.bass;
    let core = state.cur_core;
    let deep = state.cur_deep;
    let ridge = state.cur_ridge;

    // --------------------------------------------------------------------------
    // 1. Сплошная заливка цветом трека (как у Яндекс Музыки): без шара и твёрдых форм
    // --------------------------------------------------------------------------
    let base = [
        core[0] * 0.55 + deep[0] * 0.45,
        core[1] * 0.55 + deep[1] * 0.45,
        core[2] * 0.55 + deep[2] * 0.45,
    ];
    cr.set_operator(cairo::Operator::Source);
    let bg_pattern = cairo::RadialGradient::new(cx, cy, 0.0, cx, cy, max_dim * 0.95);
    bg_pattern.add_color_stop_rgba(0.0, base[0] * 1.25 + ridge[0] * 0.10, base[1] * 1.25 + ridge[1] * 0.10, base[2] * 1.25 + ridge[2] * 0.10, 1.0);
    bg_pattern.add_color_stop_rgba(0.55, base[0], base[1], base[2], 1.0);
    bg_pattern.add_color_stop_rgba(1.0, base[0] * 0.45, base[1] * 0.45, base[2] * 0.45, 1.0);
    let _ = cr.set_source(&bg_pattern);
    let _ = cr.paint();
    cr.set_operator(cairo::Operator::Over);

    // Мягкий свет у кнопки Play, усиливается басом
    let glow_r = max_dim * 0.42 * (1.0 + bass * 0.35);
    let glow_pat = cairo::RadialGradient::new(cx, cy, 0.0, cx, cy, glow_r);
    glow_pat.add_color_stop_rgba(0.0, ridge[0], ridge[1], ridge[2], 0.38 + bass * 0.40);
    glow_pat.add_color_stop_rgba(0.45, core[0], core[1], core[2], 0.14 + bass * 0.12);
    glow_pat.add_color_stop_rgba(1.0, core[0], core[1], core[2], 0.0);
    cr.new_path();
    cr.arc(cx, cy, glow_r, 0.0, PI * 2.0);
    let _ = cr.set_source(&glow_pat);
    let _ = cr.fill();

    // --------------------------------------------------------------------------
    // 2. Текучие светящиеся нити: замкнутые контуры-«жилки», медленно меняющие форму
    // --------------------------------------------------------------------------
    const FILAMENTS: usize = 7;
    const FILAMENT_POINTS: usize = 72;
    let light = [
        ridge[0] * 0.7 + 0.3,
        ridge[1] * 0.7 + 0.3,
        ridge[2] * 0.7 + 0.3,
    ];
    cr.set_operator(cairo::Operator::Screen);
    for k in 0..FILAMENTS {
        let kf = k as f64;
        let frac = kf / FILAMENTS as f64;
        let radius = max_dim * (0.14 + 0.14 * kf);
        let drift_x = (time * 0.17 + kf * 1.9).sin() * 36.0;
        let drift_y = (time * 0.13 + kf * 1.1).cos() * 24.0;
        let speed = 1.0 + kf * 0.09;

        let mut pts = Vec::with_capacity(FILAMENT_POINTS);
        for i in 0..FILAMENT_POINTS {
            let theta = (i as f64 / FILAMENT_POINTS as f64) * PI * 2.0;
            let wave = 0.16 * (theta * 3.0 + time * 0.50 * speed + kf * 1.3).sin()
                + 0.10 * (theta * 5.0 - time * 0.40 * speed + kf * 0.7).cos()
                + 0.06 * (theta * 2.0 + time * 0.30 * speed + kf).sin();
            let r = radius * (1.0 + wave) + bass * 22.0 * (0.4 + frac);
            pts.push((cx + drift_x + theta.cos() * r, cy + drift_y + theta.sin() * r));
        }

        let strength = (0.16 + bass * 0.70) * (1.0 - frac * 0.55);
        // Три обводки разной толщины имитируют размытие нити
        for (width, alpha) in [(18.0, 0.16), (7.0, 0.34), (2.2, 0.85)] {
            draw_smooth_closed_path(cr, &pts);
            cr.set_line_width(width);
            cr.set_source_rgba(light[0], light[1], light[2], (alpha * strength).min(1.0));
            let _ = cr.stroke();
        }
    }
    cr.set_operator(cairo::Operator::Over);

    // --------------------------------------------------------------------------
    // 3. Плавное затухание в глубокий ночной цвет в нижней части экрана
    // --------------------------------------------------------------------------
    let vignette_start = h * 0.54;
    let vignette_pat = cairo::LinearGradient::new(0.0, vignette_start, 0.0, h);
    vignette_pat.add_color_stop_rgba(0.0, 0.02, 0.02, 0.03, 0.0);
    vignette_pat.add_color_stop_rgba(0.35, 0.03, 0.03, 0.04, 0.65);
    vignette_pat.add_color_stop_rgba(0.70, 0.03, 0.03, 0.05, 0.90);
    vignette_pat.add_color_stop_rgba(1.0, 0.02, 0.02, 0.03, 0.98);

    cr.new_path();
    cr.rectangle(0.0, vignette_start, w, h - vignette_start);
    let _ = cr.set_source(&vignette_pat);
    let _ = cr.fill();
}

/// Рисует замкнутый гладкий контур Безье через переданные контрольные точки
fn draw_smooth_closed_path(cr: &cairo::Context, pts: &[(f64, f64)]) {
    let n = pts.len();
    if n < 3 {
        return;
    }

    cr.new_path();
    let start_mid = (
        (pts[0].0 + pts[n - 1].0) / 2.0,
        (pts[0].1 + pts[n - 1].1) / 2.0,
    );
    cr.move_to(start_mid.0, start_mid.1);

    for i in 0..n {
        let p_curr = pts[i];
        let p_next = pts[(i + 1) % n];
        let mid = ((p_curr.0 + p_next.0) / 2.0, (p_curr.1 + p_next.1) / 2.0);
        cr.curve_to(p_curr.0, p_curr.1, p_curr.0, p_curr.1, mid.0, mid.1);
    }
    cr.close_path();
}
