use std::cell::RefCell;
use std::f64::consts::PI;
use std::rc::Rc;
use std::time::Duration;
use glow::HasContext;
use gtk4::cairo;
use gtk4::prelude::*;
use gtk4::{
    glib, Align, Box as GtkBox, Button, DrawingArea, GLArea, Label, MenuButton, Orientation,
    Overlay, Popover, PositionType, Separator,
};

use crate::player::PlayerManager;

/// Состояние цветов и параметров шейдера «Плавающего нечто»
pub struct WaveShaderState {
    pub cur_core: [f32; 3],
    pub cur_deep: [f32; 3],
    pub cur_ridge: [f32; 3],
    pub target_core: [f32; 3],
    pub target_deep: [f32; 3],
    pub target_ridge: [f32; 3],
    pub time: f32,
    pub bass: f32,
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

    pub fn set_target_colors(&mut self, core: [f32; 3], deep: [f32; 3], ridge: [f32; 3]) {
        self.target_core = core;
        self.target_deep = deep;
        self.target_ridge = ridge;
    }

    pub fn update_from_track(&mut self, track: &crate::api::models::Track) {
        self.set_target_colors(track.core_rgb(), track.deep_rgb(), track.ridge_rgb());
    }

    pub fn tick(&mut self, dt: f32) {
        // Плавная интерполяция текущих цветов к целевым: cur = cur + (target - cur) * 0.05
        let interp = 0.05_f32;
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
            // Пауза: замедление течения плазмы и мягкое затухание пульсации
            self.time += dt * 0.18;
            self.bass *= 0.92;
        }
    }
}

/// Выделенные GPU ресурсы OpenGL для фонового шейдера
struct GlResources {
    gl: glow::Context,
    program: glow::Program,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    loc_res: Option<glow::UniformLocation>,
    loc_time: Option<glow::UniformLocation>,
    loc_bass: Option<glow::UniformLocation>,
    loc_core: Option<glow::UniformLocation>,
    loc_ridge: Option<glow::UniformLocation>,
    loc_deep: Option<glow::UniformLocation>,
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
    /// Обновляет целевые цвета шейдера при переключении трека
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

    // ================= Фоновый GLArea шейдер живого «Плавающего нечто» =================
    let gl_area = GLArea::new();
    gl_area.set_has_depth_buffer(false);
    gl_area.set_has_stencil_buffer(false);
    gl_area.set_hexpand(true);
    gl_area.set_vexpand(true);
    gl_area.set_auto_render(true);

    let gl_res: Rc<RefCell<Option<GlResources>>> = Rc::new(RefCell::new(None));

    // Инициализация OpenGL ресурсов
    {
        let gl_res_clone = gl_res.clone();
        gl_area.connect_realize(move |area| {
            area.make_current();
            if let Some(err) = area.error() {
                eprintln!("GLArea context error: {}", err);
                return;
            }
            match init_gl_resources() {
                Ok(res) => {
                    *gl_res_clone.borrow_mut() = Some(res);
                }
                Err(e) => {
                    eprintln!("Ошибка компиляции шейдера Волны: {}", e);
                }
            }
        });
    }

    // Рендеринг кадра шейдера
    {
        let gl_res_clone = gl_res.clone();
        let state_clone = shader_state.clone();
        gl_area.connect_render(move |area, _ctx| {
            if let Some(res) = gl_res_clone.borrow().as_ref() {
                let state = state_clone.borrow();
                render_shader(res, area.width(), area.height(), &state);
            }
            glib::Propagation::Proceed
        });
    }

    // Освобождение GPU ресурсов
    {
        let gl_res_clone = gl_res.clone();
        gl_area.connect_unrealize(move |area| {
            area.make_current();
            if let Some(res) = gl_res_clone.borrow_mut().take() {
                cleanup_gl_resources(res);
            }
        });
    }

    // Таймер анимации 60 FPS на базе GdkFrameClock (add_tick_callback)
    {
        let state_clone = shader_state.clone();
        let is_playing_clone = is_playing_state.clone();
        let last_time_ref = Rc::new(RefCell::new(0_i64));

        gl_area.add_tick_callback(move |area, frame_clock| {
            let now = frame_clock.frame_time();
            let mut last = last_time_ref.borrow_mut();
            let dt = if *last == 0 {
                0.016_f32
            } else {
                ((now - *last) as f32 / 1_000_000.0).clamp(0.001, 0.1)
            };
            *last = now;

            {
                let mut state = state_clone.borrow_mut();
                state.is_playing = *is_playing_clone.borrow();
                state.tick(dt);
            }

            area.queue_render();
            glib::ControlFlow::Continue
        });
    }

    root_overlay.set_child(Some(&gl_area));

    // ================= Центральный блок интерфейса =================
    let container = GtkBox::new(Orientation::Vertical, 16);
    container.set_valign(Align::Center);
    container.set_halign(Align::Fill);
    container.add_css_class("hero-container");

    // Оверлей для визуализатора и центральной кнопки
    let visualizer_overlay = Overlay::new();
    visualizer_overlay.set_halign(Align::Center);
    visualizer_overlay.set_valign(Align::Center);

    let drawing_area = DrawingArea::new();
    drawing_area.set_content_width(320);
    drawing_area.set_content_height(320);
    visualizer_overlay.set_child(Some(&drawing_area));

    // Центральная кнопка Play/Pause
    let play_btn = Button::new();
    play_btn.add_css_class("hero-play-badge");
    play_btn.set_halign(Align::Center);
    play_btn.set_valign(Align::Center);
    let play_btn_icon = gtk4::Image::from_icon_name("media-playback-start-symbolic");
    play_btn_icon.set_pixel_size(32);
    play_btn.set_child(Some(&play_btn_icon));
    visualizer_overlay.add_overlay(&play_btn);

    // Логика отрисовки органической волны в DrawingArea
    let angle_state = Rc::new(RefCell::new(0.0_f64));
    {
        let angle_ref = angle_state.clone();
        let playing_ref = is_playing_state.clone();
        let accent_ref = accent_color.clone();

        drawing_area.set_draw_func(move |_area, cr, width, height| {
            let cx = width as f64 / 2.0;
            let cy = height as f64 / 2.0;
            let angle = *angle_ref.borrow();
            let is_playing = *playing_ref.borrow();
            let rgb = *accent_ref.borrow();

            // Очищаем область
            cr.set_operator(cairo::Operator::Clear);
            let _ = cr.paint();
            cr.set_operator(cairo::Operator::Over);

            // 3 слоя органической волны
            for layer in 0..3 {
                let points = 8;
                let base_radius = 100.0 - (layer as f64 * 20.0);
                cr.new_path();

                for i in 0..=points {
                    let theta = (i as f64 / points as f64) * PI * 2.0;
                    let speed_mult = 1.0 + (layer as f64 * 0.4);
                    let amp = if is_playing { 14.0 } else { 4.0 };
                    let offset = (theta * 3.0 + angle * speed_mult).sin() * amp;
                    let r = base_radius + offset;
                    let x = cx + theta.cos() * r;
                    let y = cy + theta.sin() * r;

                    if i == 0 {
                        cr.move_to(x, y);
                    } else {
                        cr.line_to(x, y);
                    }
                }
                cr.close_path();

                // Радиальный градиент
                let pattern = cairo::RadialGradient::new(cx, cy, 10.0, cx, cy, 140.0);
                match layer {
                    0 => {
                        pattern.add_color_stop_rgba(0.0, rgb[0], rgb[1], rgb[2], 0.7);
                        pattern.add_color_stop_rgba(1.0, 0.61, 0.15, 0.69, 0.2);
                    }
                    1 => {
                        pattern.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.6);
                        pattern.add_color_stop_rgba(1.0, rgb[0], rgb[1], rgb[2], 0.35);
                    }
                    _ => {
                        pattern.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.8);
                        pattern.add_color_stop_rgba(1.0, rgb[0], rgb[1], rgb[2], 0.5);
                    }
                }
                let _ = cr.set_source(&pattern);
                let _ = cr.fill();
            }
        });
    }

    // Анимация центральной ауры волны по таймеру
    {
        let angle_ref = angle_state.clone();
        let playing_ref = is_playing_state.clone();
        let da_weak = drawing_area.downgrade();

        glib::timeout_add_local(Duration::from_millis(30), move || {
            if let Some(da) = da_weak.upgrade() {
                if *playing_ref.borrow() {
                    let mut a = angle_ref.borrow_mut();
                    *a += 0.04;
                    if *a > PI * 2.0 * 100.0 {
                        *a = 0.0;
                    }
                    da.queue_draw();
                }
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        });
    }

    container.append(&visualizer_overlay);

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

    container.append(&meta_box);

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

                // Обновляем целевые цвета шейдера
                let core_f = [rgb[0] as f32, rgb[1] as f32, rgb[2] as f32];
                let deep_f = [core_f[0] * 0.35, core_f[1] * 0.35, core_f[2] * 0.35];
                let ridge_f = [
                    (core_f[0] * 0.6 + 0.4).min(1.0),
                    (core_f[1] * 0.6 + 0.4).min(1.0),
                    (core_f[2] * 0.6 + 0.4).min(1.0),
                ];
                shader_state_clone.borrow_mut().set_target_colors(core_f, deep_f, ridge_f);

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

    container.append(&menu_button);

    // Обработчик центральной кнопки Play
    {
        let player_clone = player.clone();
        play_btn.connect_clicked(move |_| {
            player_clone.toggle_play();
        });
    }

    root_overlay.add_overlay(&container);

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
// Вспомогательные функции компиляции и рендеринга OpenGL шейдера
// ==============================================================================

const VS_SRC: &str = r#"#version 130
in vec2 position;
out vec2 vUv;
void main() {
    vUv = position * 0.5 + 0.5;
    gl_Position = vec4(position, 0.0, 1.0);
}
"#;

const FS_SRC: &str = r#"#version 130
precision highp float;

uniform vec2 uResolution;
uniform float uTime;
uniform float uBass;
uniform vec3 uColorCore;
uniform vec3 uColorRidge;
uniform vec3 uColorDeep;

out vec4 fragColor;

mat2 rot(float a) {
    float c = cos(a);
    float s = sin(a);
    return mat2(c, -s, s, c);
}

void main() {
    vec2 res = max(uResolution, vec2(1.0, 1.0));
    vec2 uv = (gl_FragCoord.xy - 0.5 * res) / min(res.x, res.y);

    float t = uTime * 0.45;
    float bass = uBass;

    // 4-итерационный Domain Warping: rot(0.38 * fi) с синусами и косинусами
    vec2 p = uv * 2.1;
    float totalFold = 0.0;
    float weight = 1.0;

    for (int i = 1; i <= 4; i++) {
        float fi = float(i);
        p = rot(0.38 * fi + t * 0.07) * p;
        vec2 shift = vec2(
            sin(p.y * 1.6 + t * 0.45 + fi * 0.8) * 0.65,
            cos(p.x * 1.6 - t * 0.40 + fi * 1.2) * 0.65
        );
        p += shift * (1.0 + bass * 0.35);

        float fold = sin(p.x * 2.4 + p.y * 2.0 + t * 0.5);
        totalFold += abs(fold) * weight;
        weight *= 0.62;
    }

    // Каустические складки (Fresnel Silk Ridges)
    float foldNorm = totalFold * 0.38;
    float ridge = pow(clamp(1.0 - abs(foldNorm), 0.0, 1.0), 2.6);

    // Радиальные световые лучи из центра
    float dist = length(uv);
    float angle = atan(uv.y, uv.x);
    float rayWave = sin(angle * 7.0 + t * 0.5 + sin(dist * 5.0 - t * 0.8)) * 0.5 + 0.5;
    float rays = pow(rayWave, 3.2) * exp(-dist * 1.9);

    // Смешивание глубокого фона и акцентного ядра
    float coreWeight = clamp(exp(-dist * 1.35) * (1.05 + bass * 0.45) - totalFold * 0.22, 0.0, 1.0);
    vec3 col = mix(uColorDeep, uColorCore, coreWeight);

    // Наложение шелковых гребней и световых лучей
    col += uColorRidge * (ridge * 0.88 + rays * 0.5);

    // Центральное мягкое свечение
    col += uColorCore * (exp(-dist * 2.0) * 0.38);

    // Плавное затухание в глубокий черный цвет в нижней части экрана
    float screenY = gl_FragCoord.y / res.y;
    float bottomVignette = smoothstep(0.02, 0.48, screenY);

    // Мягкое виньетирование по периметру
    float vignette = clamp(1.45 - dist * 0.85, 0.0, 1.0);
    col *= bottomVignette * vignette;

    fragColor = vec4(clamp(col, 0.0, 1.0), 1.0);
}
"#;

unsafe fn create_glow_context() -> glow::Context {
    unsafe extern "C" {
        fn dlsym(handle: *mut std::ffi::c_void, symbol: *const std::os::raw::c_char) -> *mut std::ffi::c_void;
    }

    unsafe {
        glow::Context::from_loader_function(|name| {
            let c_str = std::ffi::CString::new(name).unwrap();
            let ptr = dlsym(std::ptr::null_mut(), c_str.as_ptr());
            if !ptr.is_null() {
                return ptr as *const _;
            }
            let epoxy_str = std::ffi::CString::new(format!("epoxy_{}", name)).unwrap();
            let ptr_epoxy = dlsym(std::ptr::null_mut(), epoxy_str.as_ptr());
            ptr_epoxy as *const _
        })
    }
}

fn init_gl_resources() -> Result<GlResources, String> {
    unsafe {
        let gl = create_glow_context();
        let version_str = gl.get_parameter_string(glow::VERSION);
        let is_gles = version_str.contains("OpenGL ES");

        let vs_code = if is_gles {
            VS_SRC.replace("#version 130", "#version 300 es\nprecision highp float;")
        } else {
            VS_SRC.to_string()
        };

        let fs_code = if is_gles {
            FS_SRC.replace("#version 130", "#version 300 es\nprecision highp float;")
        } else {
            FS_SRC.to_string()
        };

        let vs = compile_shader(&gl, glow::VERTEX_SHADER, &vs_code)?;
        let fs = compile_shader(&gl, glow::FRAGMENT_SHADER, &fs_code)?;
        let program = link_program(&gl, vs, fs)?;

        let (vao, vbo) = create_fullscreen_quad(&gl)?;

        let loc_res = gl.get_uniform_location(program, "uResolution");
        let loc_time = gl.get_uniform_location(program, "uTime");
        let loc_bass = gl.get_uniform_location(program, "uBass");
        let loc_core = gl.get_uniform_location(program, "uColorCore");
        let loc_ridge = gl.get_uniform_location(program, "uColorRidge");
        let loc_deep = gl.get_uniform_location(program, "uColorDeep");

        Ok(GlResources {
            gl,
            program,
            vao,
            vbo,
            loc_res,
            loc_time,
            loc_bass,
            loc_core,
            loc_ridge,
            loc_deep,
        })
    }
}

fn compile_shader(gl: &glow::Context, shader_type: u32, source: &str) -> Result<glow::Shader, String> {
    unsafe {
        let shader = gl.create_shader(shader_type)?;
        gl.shader_source(shader, source);
        gl.compile_shader(shader);
        if !gl.get_shader_compile_status(shader) {
            let log = gl.get_shader_info_log(shader);
            gl.delete_shader(shader);
            return Err(format!("Shader compile error: {}", log));
        }
        Ok(shader)
    }
}

fn link_program(gl: &glow::Context, vs: glow::Shader, fs: glow::Shader) -> Result<glow::Program, String> {
    unsafe {
        let program = gl.create_program()?;
        gl.attach_shader(program, vs);
        gl.attach_shader(program, fs);
        gl.link_program(program);
        if !gl.get_program_link_status(program) {
            let log = gl.get_program_info_log(program);
            gl.delete_program(program);
            return Err(format!("Program link error: {}", log));
        }
        gl.delete_shader(vs);
        gl.delete_shader(fs);
        Ok(program)
    }
}

fn create_fullscreen_quad(gl: &glow::Context) -> Result<(glow::VertexArray, glow::Buffer), String> {
    unsafe {
        let vertices: [f32; 8] = [
            -1.0, -1.0,
             1.0, -1.0,
            -1.0,  1.0,
             1.0,  1.0,
        ];
        let bytes: &[u8] = core::slice::from_raw_parts(
            vertices.as_ptr() as *const u8,
            vertices.len() * core::mem::size_of::<f32>(),
        );

        let vao = gl.create_vertex_array()?;
        gl.bind_vertex_array(Some(vao));

        let vbo = gl.create_buffer()?;
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STATIC_DRAW);

        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, (2 * core::mem::size_of::<f32>()) as i32, 0);

        gl.bind_buffer(glow::ARRAY_BUFFER, None);
        gl.bind_vertex_array(None);

        Ok((vao, vbo))
    }
}

fn render_shader(res: &GlResources, width: i32, height: i32, state: &WaveShaderState) {
    unsafe {
        res.gl.viewport(0, 0, width, height);
        res.gl.use_program(Some(res.program));

        if let Some(loc) = &res.loc_res {
            res.gl.uniform_2_f32(Some(loc), width as f32, height as f32);
        }
        if let Some(loc) = &res.loc_time {
            res.gl.uniform_1_f32(Some(loc), state.time);
        }
        if let Some(loc) = &res.loc_bass {
            res.gl.uniform_1_f32(Some(loc), state.bass);
        }
        if let Some(loc) = &res.loc_core {
            res.gl.uniform_3_f32(Some(loc), state.cur_core[0], state.cur_core[1], state.cur_core[2]);
        }
        if let Some(loc) = &res.loc_ridge {
            res.gl.uniform_3_f32(Some(loc), state.cur_ridge[0], state.cur_ridge[1], state.cur_ridge[2]);
        }
        if let Some(loc) = &res.loc_deep {
            res.gl.uniform_3_f32(Some(loc), state.cur_deep[0], state.cur_deep[1], state.cur_deep[2]);
        }

        res.gl.bind_vertex_array(Some(res.vao));
        res.gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
        res.gl.bind_vertex_array(None);
        res.gl.use_program(None);
    }
}

fn cleanup_gl_resources(res: GlResources) {
    unsafe {
        res.gl.delete_program(res.program);
        res.gl.delete_vertex_array(res.vao);
        res.gl.delete_buffer(res.vbo);
    }
}
