use std::cell::RefCell;
use std::f64::consts::PI;
use std::rc::Rc;
use std::time::Duration;
use gtk4::cairo;
use gtk4::prelude::*;
use gtk4::{
    glib, Align, Box as GtkBox, Button, DrawingArea, Label, MenuButton, Orientation, Overlay,
    Popover, PositionType, Separator,
};

use crate::player::PlayerManager;

pub struct WaveViewWidgets {
    pub container: GtkBox,
    pub title_label: Label,
    pub artist_label: Label,
    pub play_btn: Button,
    pub play_btn_icon: gtk4::Image,
    pub menu_button: MenuButton,
    pub drawing_area: DrawingArea,
}

pub fn create_wave_view(
    player: &PlayerManager,
    is_playing_state: Rc<RefCell<bool>>,
    accent_color: Rc<RefCell<[f64; 3]>>, // RGB: 0.0 .. 1.0
) -> WaveViewWidgets {
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

    // Анимация волны по таймеру
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

        reset_btn.connect_clicked(move |_| {
            if let Some(pop) = pop_weak.upgrade() {
                pop.popdown();
            }
            if let Some(mb) = mb_weak.upgrade() {
                mb.set_label("⚙ Настроить волну ▾");
            }
            *accent_clone.borrow_mut() = [1.0, 0.42, 0.29]; // дефолтный цвет
            if let Some(da) = da_weak.upgrade() {
                da.queue_draw();
            }
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

    WaveViewWidgets {
        container,
        title_label,
        artist_label,
        play_btn,
        play_btn_icon,
        menu_button,
        drawing_area,
    }
}
