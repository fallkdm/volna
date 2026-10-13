use std::cell::RefCell;
use std::f64::consts::PI;
use std::rc::Rc;
use gtk4::cairo;
use gtk4::prelude::*;
use gtk4::{
    glib, Align, Box as GtkBox, Button, DrawingArea, Label, MenuButton, Orientation,
    Overlay, Popover, PositionType, Separator,
};

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

    pub fn tick(&mut self, dt: f64) {
        // Плавная интерполяция текущих цветов к целевым цветам текущего трека:
        // cur = cur + (target - cur) * 0.05
        let interp = 0.05_f64;
        for i in 0..3 {
            self.cur_core[i] += (self.target_core[i] - self.cur_core[i]) * interp;
            self.cur_deep[i] += (self.target_deep[i] - self.cur_deep[i]) * interp;
            self.cur_ridge[i] += (self.target_ridge[i] - self.cur_ridge[i]) * interp;
        }

        if self.is_playing {
            // Воспроизведение: динамичное течение плазмы + легкая ритмичная пульсация баса
            self.time += dt * 1.0;
            self.bass = (self.time * 3.5).sin().max(0.0) * 0.28;
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
    pub menu_button: MenuButton,
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
                state.tick(dt);
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
    title_label.set_justify(gtk4::Justification::Center);
    bottom_box.append(&title_label);

    let artist_label = Label::new(Some("Нажмите для запуска персонального потока 🌊"));
    artist_label.add_css_class("hero-track-artist");
    artist_label.set_justify(gtk4::Justification::Center);
    bottom_box.append(&artist_label);

    // Центральная кнопка фильтров Ротора «Настроить волну»
    let menu_button = MenuButton::new();
    menu_button.set_label("⚙ Настроить волну ▾");
    menu_button.add_css_class("fluid-pill");
    menu_button.set_halign(Align::Center);
    menu_button.set_margin_top(14);

    // Всплывающее Popover меню настройки Моей волны
    let popover = Popover::new();
    popover.set_autohide(true);
    popover.set_position(PositionType::Bottom);

    let popover_box = GtkBox::new(Orientation::Vertical, 2);
    popover_box.add_css_class("rotor-popover-box");

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
        if s_idx > 0 {
            let sep = Separator::new(Orientation::Horizontal);
            sep.set_margin_top(4);
            sep.set_margin_bottom(4);
            popover_box.append(&sep);
        }

        let section_header = Label::new(Some(section.title));
        section_header.add_css_class("rotor-section-title");
        section_header.set_halign(Align::Start);
        popover_box.append(&section_header);

        for opt in section.options {
            let btn = Button::with_label(opt.label);
            btn.add_css_class("flat");
            btn.add_css_class("rotor-filter-btn");
            btn.set_halign(Align::Fill);

            let player_clone = player.clone();
            let accent_clone = accent_color.clone();
            let da_weak = drawing_area.downgrade();
            let pop_weak = popover.downgrade();
            let mb_weak = menu_button.downgrade();
            let shader_state_clone = shader_state.clone();

            let opt_label = opt.label;
            let mood_val = opt.mood.map(|s| s.to_string());
            let lang_val = opt.lang.map(|s| s.to_string());
            let char_val = opt.charac.map(|s| s.to_string());
            let rgb = opt.color;

            btn.connect_clicked(move |_| {
                if let Some(pop) = pop_weak.upgrade() {
                    pop.popdown();
                }
                if let Some(mb) = mb_weak.upgrade() {
                    mb.set_label(&format!("{} ▾", opt_label));
                }
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

            popover_box.append(&btn);
        }
    }

    // Разделитель и кнопка сброса фильтров
    let reset_sep = Separator::new(Orientation::Horizontal);
    reset_sep.set_margin_top(6);
    reset_sep.set_margin_bottom(4);
    popover_box.append(&reset_sep);

    let reset_btn = Button::with_label("↺ Сбросить фильтры");
    reset_btn.add_css_class("flat");
    reset_btn.add_css_class("rotor-reset-btn");
    reset_btn.set_halign(Align::Fill);

    {
        let player_clone = player.clone();
        let accent_clone = accent_color.clone();
        let da_weak = drawing_area.downgrade();
        let pop_weak = popover.downgrade();
        let mb_weak = menu_button.downgrade();
        let shader_state_clone = shader_state.clone();

        reset_btn.connect_clicked(move |_| {
            if let Some(pop) = pop_weak.upgrade() {
                pop.popdown();
            }
            if let Some(mb) = mb_weak.upgrade() {
                mb.set_label("⚙ Настроить волну ▾");
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
    popover.set_child(Some(&popover_box));
    menu_button.set_popover(Some(&popover));

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
    // 1. Полноэкранное атмосферное свечение (Ambient Aurora Mesh)
    // --------------------------------------------------------------------------
    cr.set_operator(cairo::Operator::Source);
    let bg_pattern = cairo::RadialGradient::new(cx, cy, 30.0, cx, cy, max_dim * 0.90);
    bg_pattern.add_color_stop_rgba(0.0, deep[0] * 0.70, deep[1] * 0.70, deep[2] * 0.70, 0.75);
    bg_pattern.add_color_stop_rgba(0.40, deep[0] * 0.30, deep[1] * 0.30, deep[2] * 0.30, 0.40);
    bg_pattern.add_color_stop_rgba(0.80, 0.04, 0.04, 0.06, 1.0);
    bg_pattern.add_color_stop_rgba(1.0, 0.02, 0.02, 0.03, 1.0);
    let _ = cr.set_source(&bg_pattern);
    let _ = cr.paint();
    cr.set_operator(cairo::Operator::Over);

    // 3–4 мягких дрейфующих световых фокуса (attractors) по краям экрана (Lissajous)
    let attractors = [
        (
            w * 0.22 + (time * 0.35).sin() * (w * 0.16),
            h * 0.20 + (time * 0.28).cos() * (h * 0.14),
            max_dim * 0.44,
            core,
            0.15 + bass * 0.06,
        ),
        (
            w * 0.80 + (time * 0.30 + 1.5).cos() * (w * 0.15),
            h * 0.30 + (time * 0.40 + 2.0).sin() * (h * 0.16),
            max_dim * 0.48,
            deep,
            0.22 + bass * 0.08,
        ),
        (
            w * 0.48 + (time * 0.25 + 3.0).sin() * (w * 0.20),
            h * 0.72 + (time * 0.32 + 0.8).cos() * (h * 0.12),
            max_dim * 0.42,
            ridge,
            0.11 + bass * 0.05,
        ),
        (
            w * 0.14 + (time * 0.38 + 4.0).cos() * (w * 0.12),
            h * 0.82 + (time * 0.24 + 1.2).sin() * (h * 0.14),
            max_dim * 0.38,
            core,
            0.09,
        ),
    ];

    for (ax, ay, ar, acolor, aalpha) in attractors {
        let a_pat = cairo::RadialGradient::new(ax, ay, 0.0, ax, ay, ar);
        a_pat.add_color_stop_rgba(0.0, acolor[0], acolor[1], acolor[2], aalpha);
        a_pat.add_color_stop_rgba(0.55, acolor[0] * 0.5, acolor[1] * 0.5, acolor[2] * 0.5, aalpha * 0.30);
        a_pat.add_color_stop_rgba(1.0, acolor[0], acolor[1], acolor[2], 0.0);
        cr.new_path();
        cr.arc(ax, ay, ar, 0.0, PI * 2.0);
        let _ = cr.set_source(&a_pat);
        let _ = cr.fill();
    }

    // --------------------------------------------------------------------------
    // 2. Крупное объемное плазменное ядро (Luminous Fluid Orb)
    // --------------------------------------------------------------------------
    let base_r = (175.0 + bass * 18.0).max(120.0);

    // Внешняя корона мягкого свечения (Corona Bloom)
    let corona_r = base_r * 1.55;
    let corona_pat = cairo::RadialGradient::new(cx, cy, base_r * 0.45, cx, cy, corona_r);
    corona_pat.add_color_stop_rgba(0.0, core[0], core[1], core[2], 0.35 + bass * 0.14);
    corona_pat.add_color_stop_rgba(0.42, core[0], core[1], core[2], 0.14);
    corona_pat.add_color_stop_rgba(0.75, ridge[0], ridge[1], ridge[2], 0.04);
    corona_pat.add_color_stop_rgba(1.0, core[0], core[1], core[2], 0.0);
    cr.new_path();
    cr.arc(cx, cy, corona_r, 0.0, PI * 2.0);
    let _ = cr.set_source(&corona_pat);
    let _ = cr.fill();

    // Органический контур деформации Безье (36 контрольных точек через Catmull-Rom)
    let points_count = 36;
    let mut pts = Vec::with_capacity(points_count);
    for i in 0..points_count {
        let theta = (i as f64 / points_count as f64) * PI * 2.0;
        let warp = (theta * 3.0 + time * 1.2).sin() * 14.0
            + (theta * 5.0 - time * 0.9).cos() * 10.0
            + (theta * 2.0 + time * 0.5).sin() * 8.0;
        let r = base_r + warp * (1.0 + bass * 0.30);
        let px = cx + theta.cos() * r;
        let py = cy + theta.sin() * r;
        pts.push((px, py));
    }

    draw_smooth_closed_path(cr, &pts);

    // Объемная 3D-заливка сферы смещенным радиальным градиентом
    let orb_pat = cairo::RadialGradient::new(
        cx - base_r * 0.32,
        cy - base_r * 0.32,
        10.0,
        cx,
        cy,
        base_r * 1.08,
    );
    orb_pat.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.95);
    orb_pat.add_color_stop_rgba(0.22, core[0], core[1], core[2], 0.95);
    orb_pat.add_color_stop_rgba(0.60, ridge[0], ridge[1], ridge[2], 0.85);
    orb_pat.add_color_stop_rgba(0.92, deep[0], deep[1], deep[2], 0.90);
    orb_pat.add_color_stop_rgba(1.0, deep[0] * 0.4, deep[1] * 0.4, deep[2] * 0.4, 0.95);

    let _ = cr.set_source(&orb_pat);
    let _ = cr.fill();

    // Внутренний флюидный плазменный вихрь (Secondary Plasma Swirl)
    let swirl_cx = cx + (time * 0.8).cos() * (base_r * 0.22);
    let swirl_cy = cy + (time * 0.7).sin() * (base_r * 0.22);
    let swirl_r = base_r * 0.65;
    let swirl_pat = cairo::RadialGradient::new(swirl_cx, swirl_cy, 5.0, swirl_cx, swirl_cy, swirl_r);
    swirl_pat.add_color_stop_rgba(0.0, ridge[0], ridge[1], ridge[2], 0.42);
    swirl_pat.add_color_stop_rgba(0.50, core[0], core[1], core[2], 0.22);
    swirl_pat.add_color_stop_rgba(1.0, deep[0], deep[1], deep[2], 0.0);

    cr.new_path();
    cr.arc(swirl_cx, swirl_cy, swirl_r, 0.0, PI * 2.0);
    let _ = cr.set_source(&swirl_pat);
    let _ = cr.fill();

    // Зеркальный блик стекла (Specular Glass Highlight)
    let hl_cx = cx - base_r * 0.32;
    let hl_cy = cy - base_r * 0.32;
    let hl_rx = base_r * 0.30;
    let hl_ry = base_r * 0.18;
    let hl_pat = cairo::RadialGradient::new(hl_cx, hl_cy, 2.0, hl_cx, hl_cy, hl_rx);
    hl_pat.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.58);
    hl_pat.add_color_stop_rgba(0.45, 1.0, 1.0, 1.0, 0.22);
    hl_pat.add_color_stop_rgba(1.0, 1.0, 1.0, 1.0, 0.0);

    cr.save().ok();
    cr.translate(hl_cx, hl_cy);
    cr.rotate(-PI * 0.18);
    cr.scale(1.0, hl_ry / hl_rx);
    cr.new_path();
    cr.arc(0.0, 0.0, hl_rx, 0.0, PI * 2.0);
    let _ = cr.set_source(&hl_pat);
    let _ = cr.fill();
    cr.restore().ok();

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
