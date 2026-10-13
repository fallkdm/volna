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

    // Отрисовка органического живого фона на Cairo
    {
        let state_ref = shader_state.clone();
        drawing_area.set_draw_func(move |_area, cr, width, height| {
            let state = state_ref.borrow();
            render_live_wave_cairo(cr, width as f64, height as f64, &state);
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
    let content_box = GtkBox::new(Orientation::Vertical, 16);
    content_box.set_valign(Align::Center);
    content_box.set_halign(Align::Fill);
    content_box.add_css_class("hero-container");

    // Центральная кнопка Play/Pause
    let play_btn = Button::new();
    play_btn.add_css_class("hero-play-badge");
    play_btn.set_halign(Align::Center);
    play_btn.set_valign(Align::Center);
    let play_btn_icon = gtk4::Image::from_icon_name("media-playback-start-symbolic");
    play_btn_icon.set_pixel_size(36);
    play_btn.set_child(Some(&play_btn_icon));
    content_box.append(&play_btn);

    // Метаданные текущего трека
    let meta_box = GtkBox::new(Orientation::Vertical, 4);
    meta_box.set_halign(Align::Center);

    let title_label = Label::new(Some("Моя волна"));
    title_label.add_css_class("hero-track-title");
    title_label.set_wrap(true);
    title_label.set_justify(gtk4::Justification::Center);
    meta_box.append(&title_label);

    let artist_label = Label::new(Some("Нажмите для запуска персонального потока 🌊"));
    artist_label.add_css_class("hero-track-artist");
    artist_label.set_justify(gtk4::Justification::Center);
    meta_box.append(&artist_label);

    content_box.append(&meta_box);

    // Центральная кнопка фильтров Ротора «Настроить волну»
    let menu_button = MenuButton::new();
    menu_button.set_label("⚙ Настроить волну ▾");
    menu_button.add_css_class("fluid-pill");
    menu_button.set_halign(Align::Center);

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

    content_box.append(&menu_button);

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
// Высокопроизводительный и 100% безопасный рендеринг «Плавающего нечто» на Cairo
// ==============================================================================

fn render_live_wave_cairo(cr: &cairo::Context, w: f64, h: f64, state: &WaveShaderState) {
    let cx = w / 2.0;
    let cy = h * 0.42; // Центр визуализатора чуть выше центра экрана
    let max_r = w.max(h);
    let time = state.time;
    let bass = state.bass;
    let core = state.cur_core;
    let deep = state.cur_deep;
    let ridge = state.cur_ridge;

    // 1. Очистка и отрисовка глубокого фонового радиального градиента на весь экран
    cr.set_operator(cairo::Operator::Source);
    let bg_pattern = cairo::RadialGradient::new(cx, cy, 20.0, cx, cy, max_r * 0.85);
    bg_pattern.add_color_stop_rgba(0.0, deep[0], deep[1], deep[2], 0.65);
    bg_pattern.add_color_stop_rgba(0.4, deep[0] * 0.4, deep[1] * 0.4, deep[2] * 0.4, 0.35);
    bg_pattern.add_color_stop_rgba(0.85, 0.06, 0.06, 0.06, 1.0);
    bg_pattern.add_color_stop_rgba(1.0, 0.04, 0.04, 0.04, 1.0);
    let _ = cr.set_source(&bg_pattern);
    let _ = cr.paint();
    cr.set_operator(cairo::Operator::Over);

    // 2. Радиальные световые лучи из центра
    let ray_count = 8;
    for i in 0..ray_count {
        let angle = (i as f64 / ray_count as f64) * PI * 2.0 + time * 0.06;
        let ray_r = 160.0 + (time * 0.3 + i as f64).sin() * 40.0;
        let spread = 0.22;

        cr.new_path();
        cr.move_to(cx, cy);
        cr.line_to(cx + (angle - spread).cos() * ray_r, cy + (angle - spread).sin() * ray_r);
        cr.line_to(cx + (angle + spread).cos() * ray_r, cy + (angle + spread).sin() * ray_r);
        cr.close_path();

        let ray_pat = cairo::RadialGradient::new(cx, cy, 10.0, cx, cy, ray_r);
        ray_pat.add_color_stop_rgba(0.0, core[0], core[1], core[2], 0.18 + bass * 0.12);
        ray_pat.add_color_stop_rgba(0.7, ridge[0], ridge[1], ridge[2], 0.06);
        ray_pat.add_color_stop_rgba(1.0, deep[0], deep[1], deep[2], 0.0);
        let _ = cr.set_source(&ray_pat);
        let _ = cr.fill();
    }

    // 3. 4-итерационный Domain Warping с каустическими складками (Fresnel Silk Ridges)
    let points_count = 28;
    for layer in 0..4 {
        let fi = (layer + 1) as f64;
        let base_radius = (145.0 - fi * 22.0) * (1.0 + bass * 0.35);
        let rot = 0.38 * fi + time * 0.07;

        let mut pts = Vec::with_capacity(points_count);
        for i in 0..points_count {
            let theta = (i as f64 / points_count as f64) * PI * 2.0;
            // 4-итерационный сдвиг синусов/косинусов
            let shift1 = (theta * 3.0 + rot * 1.6).sin() * (16.0 + fi * 4.0);
            let shift2 = (theta * 5.0 - time * 0.35 + fi).cos() * (9.0 + fi * 2.5);
            let r = base_radius + (shift1 + shift2) * (1.0 + bass * 0.22);
            let x = cx + theta.cos() * r;
            let y = cy + theta.sin() * r;
            pts.push((x, y));
        }

        // Рисуем непрерывный плавный Catmull-Rom/Bezier замкнутый контур
        draw_smooth_closed_path(cr, &pts);

        // Градиент заливки складок
        let pat = cairo::RadialGradient::new(cx, cy, 15.0, cx, cy, base_radius + 35.0);
        match layer {
            0 => {
                pat.add_color_stop_rgba(0.0, core[0], core[1], core[2], 0.60);
                pat.add_color_stop_rgba(0.6, deep[0], deep[1], deep[2], 0.30);
                pat.add_color_stop_rgba(1.0, deep[0], deep[1], deep[2], 0.0);
            }
            1 => {
                pat.add_color_stop_rgba(0.0, ridge[0], ridge[1], ridge[2], 0.45);
                pat.add_color_stop_rgba(0.5, core[0], core[1], core[2], 0.35);
                pat.add_color_stop_rgba(1.0, deep[0], deep[1], deep[2], 0.0);
            }
            2 => {
                pat.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.50);
                pat.add_color_stop_rgba(0.4, ridge[0], ridge[1], ridge[2], 0.35);
                pat.add_color_stop_rgba(1.0, core[0], core[1], core[2], 0.0);
            }
            _ => {
                pat.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.70);
                pat.add_color_stop_rgba(0.3, ridge[0], ridge[1], ridge[2], 0.50);
                pat.add_color_stop_rgba(1.0, core[0], core[1], core[2], 0.0);
            }
        }
        let _ = cr.set_source(&pat);
        let _ = cr.fill_preserve();

        // Тонкий контур каустических складок (Silk Ridges)
        cr.set_line_width(2.2 - fi * 0.3);
        cr.set_source_rgba(ridge[0], ridge[1], ridge[2], 0.28 + (3.0 - fi) * 0.08);
        let _ = cr.stroke();
    }

    // 4. Мягкое центральное свечение ядра
    let core_r = 50.0 + bass * 14.0;
    let core_pat = cairo::RadialGradient::new(cx, cy, 5.0, cx, cy, core_r);
    core_pat.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.85);
    core_pat.add_color_stop_rgba(0.35, ridge[0], ridge[1], ridge[2], 0.65);
    core_pat.add_color_stop_rgba(0.7, core[0], core[1], core[2], 0.35);
    core_pat.add_color_stop_rgba(1.0, deep[0], deep[1], deep[2], 0.0);
    cr.new_path();
    cr.arc(cx, cy, core_r, 0.0, PI * 2.0);
    let _ = cr.set_source(&core_pat);
    let _ = cr.fill();

    // 5. Плавное затухание в глубокий черный цвет в нижней части экрана
    // (для идеальной читаемости острова-плеера и кнопки настроек)
    let vignette_start = h * 0.52;
    let vignette_pat = cairo::LinearGradient::new(0.0, vignette_start, 0.0, h);
    vignette_pat.add_color_stop_rgba(0.0, 0.05, 0.05, 0.05, 0.0);
    vignette_pat.add_color_stop_rgba(0.35, 0.06, 0.06, 0.06, 0.65);
    vignette_pat.add_color_stop_rgba(0.75, 0.06, 0.06, 0.06, 0.92);
    vignette_pat.add_color_stop_rgba(1.0, 0.05, 0.05, 0.05, 0.98);

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
