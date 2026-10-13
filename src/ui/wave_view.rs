use std::cell::RefCell;
use std::f64::consts::PI;
use std::rc::Rc;
use std::time::Duration;
use gtk4::cairo;
use gtk4::prelude::*;
use gtk4::{glib, Align, Box as GtkBox, Button, DrawingArea, Label, Orientation, Overlay, ScrolledWindow, PolicyType};

use crate::player::PlayerManager;

pub struct WaveViewWidgets {
    pub container: GtkBox,
    pub title_label: Label,
    pub artist_label: Label,
    pub play_btn: Button,
    pub play_btn_icon: gtk4::Image,
    pub filter_btn: Button,
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
    let play_btn_icon = gtk4::Image::from_icon_name("media-playback-pause-symbolic");
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

    let title_label = Label::new(Some("Midnight City"));
    title_label.add_css_class("hero-track-title");
    title_label.set_wrap(true);
    title_label.set_justify(gtk4::Justification::Center);
    meta_box.append(&title_label);

    let artist_label = Label::new(Some("M83 • Hurry Up, We're Dreaming"));
    artist_label.add_css_class("hero-track-artist");
    artist_label.set_justify(gtk4::Justification::Center);
    meta_box.append(&artist_label);

    container.append(&meta_box);

    // Горизонтальная лента чипов настроений
    let scroll_wrap = ScrolledWindow::new();
    scroll_wrap.set_policy(PolicyType::Automatic, PolicyType::Never);
    scroll_wrap.set_halign(Align::Center);

    let pill_bar = GtkBox::new(Orientation::Horizontal, 8);
    pill_bar.add_css_class("fluid-pill-bar");

    let moods = [
        ("⚡ Энергичное", "energetic", [1.0, 0.42, 0.29]), // #ff6b4a
        ("☕ Спокойное", "calm", [0.21, 0.52, 0.89]),      // #3584e4
        ("🇷🇺 На русском", "russian", [0.15, 0.64, 0.41]), // #26a269
        ("✨ Новое", "discover", [0.61, 0.15, 0.69]),       // #9c27b0
        ("🎉 Вечеринка", "party", [0.90, 0.65, 0.04]),     // #e5a50a
    ];

    for (label_text, tag, rgb) in moods {
        let pill = Button::with_label(label_text);
        pill.add_css_class("fluid-pill");
        if tag == "energetic" {
            pill.add_css_class("active");
        }

        let player_clone = player.clone();
        let accent_clone = accent_color.clone();
        let da_weak = drawing_area.downgrade();
        let pill_bar_weak = pill_bar.downgrade();

        pill.connect_clicked(move |clicked_btn| {
            if let Some(bar) = pill_bar_weak.upgrade() {
                // Снимаем класс active со всех дочерних кнопок
                let mut child = bar.first_child();
                while let Some(w) = child {
                    w.remove_css_class("active");
                    child = w.next_sibling();
                }
            }
            clicked_btn.add_css_class("active");
            *accent_clone.borrow_mut() = rgb;
            if let Some(da) = da_weak.upgrade() {
                da.queue_draw();
            }
            player_clone.fetch_wave_tracks(tag);
        });

        pill_bar.append(&pill);
    }

    let filter_btn = Button::with_label("⚙ Фильтры");
    filter_btn.add_css_class("fluid-pill");
    pill_bar.append(&filter_btn);

    scroll_wrap.set_child(Some(&pill_bar));
    container.append(&scroll_wrap);

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
        filter_btn,
        drawing_area,
    }
}
