use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use async_channel::Receiver;
use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Button, Label, Orientation, Overlay, ScrolledWindow, PolicyType,
};
use libadwaita::prelude::*;
use libadwaita::{
    Application, ApplicationWindow, HeaderBar, Toast, ToastOverlay, ViewStack, ViewSwitcher,
};

use crate::api::models::{parse_lrc, LrcLine};
use crate::player::{PlayerManager, PlayerUiEvent};
use super::floating_island::{create_floating_island, FloatingIslandWidgets};
use super::library_view::create_library_view;
use super::preferences_dialog::show_preferences_dialog;
use super::search_view::create_search_view;
use super::wave_view::{create_wave_view, WaveViewWidgets};

pub struct VolnaWindow {
    pub window: ApplicationWindow,
}

impl VolnaWindow {
    pub fn new(
        app: &Application,
        player: PlayerManager,
        event_receiver: Receiver<PlayerUiEvent>,
    ) -> Self {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("Волна")
            .default_width(960)
            .default_height(680)
            .build();

        window.add_css_class("volna-main-window");

        let toast_overlay = ToastOverlay::new();

        // Главный GtkOverlay
        let root_overlay = Overlay::new();

        // Фоновый контейнер с радиальным градиентом
        let background_box = GtkBox::new(Orientation::Vertical, 0);
        background_box.add_css_class("ambient-backdrop");
        background_box.set_vexpand(true);
        background_box.set_hexpand(true);

        // HeaderBar
        let header_bar = HeaderBar::new();
        header_bar.add_css_class("fluid-header");

        // Бренд слева
        let brand_box = GtkBox::new(Orientation::Horizontal, 8);
        brand_box.set_valign(Align::Center);

        let brand_title = Label::new(Some("🌊 Волна"));
        brand_title.add_css_class("heading");
        brand_box.append(&brand_title);

        let brand_badge = Label::new(Some("Wave-First"));
        brand_badge.add_css_class("brand-badge");
        brand_box.append(&brand_badge);

        header_bar.pack_start(&brand_box);

        // ViewStack
        let view_stack = ViewStack::new();
        view_stack.set_vexpand(true);
        view_stack.set_hexpand(true);

        let view_switcher = ViewSwitcher::builder()
            .stack(&view_stack)
            .policy(libadwaita::ViewSwitcherPolicy::Wide)
            .build();
        header_bar.set_title_widget(Some(&view_switcher));

        let settings_btn = Button::from_icon_name("emblem-system-symbolic");
        settings_btn.set_tooltip_text(Some("Настройки Волны и токен"));
        settings_btn.add_css_class("flat");
        settings_btn.add_css_class("circular");
        header_bar.pack_end(&settings_btn);

        background_box.append(&header_bar);

        let is_playing_state = Rc::new(RefCell::new(false));
        let accent_color = Rc::new(RefCell::new([1.0_f64, 0.42_f64, 0.29_f64])); // #ff6b4a

        // Экран 1: Моя волна
        let wave_view = create_wave_view(&player, is_playing_state.clone(), accent_color);
        let wave_page = view_stack.add_titled(&wave_view.container, Some("wave"), "Моя волна");
        wave_page.set_icon_name(Some("audio-volume-high-symbolic"));

        // Экран 2: Поиск
        let search_view = create_search_view(&player);
        let search_page = view_stack.add_titled(&search_view.container, Some("search"), "Поиск");
        search_page.set_icon_name(Some("system-search-symbolic"));

        // Экран 3: Медиатека
        let library_view = create_library_view(&player);
        let lib_page = view_stack.add_titled(&library_view.container, Some("library"), "Медиатека");
        lib_page.set_icon_name(Some("folder-music-symbolic"));

        background_box.append(&view_stack);
        root_overlay.set_child(Some(&background_box));

        // Нижний плавающий остров плеера
        let island = create_floating_island(&player);
        root_overlay.add_overlay(&island.container);

        // ================= Оверлей текста песни (Karaoke Overlay) =================
        let karaoke_box = GtkBox::new(Orientation::Vertical, 12);
        karaoke_box.add_css_class("karaoke-sheet");
        karaoke_box.set_visible(false);
        karaoke_box.set_vexpand(true);
        karaoke_box.set_hexpand(true);

        let k_header = GtkBox::new(Orientation::Horizontal, 12);
        k_header.set_margin_top(16);
        k_header.set_margin_start(20);
        k_header.set_margin_end(20);

        let k_title = Label::new(Some("Синхронизированный текст песни"));
        k_title.add_css_class("heading");
        k_header.append(&k_title);

        let k_badge = Label::new(Some("LRC Karaoke"));
        k_badge.add_css_class("brand-badge");
        k_header.append(&k_badge);

        let k_close_btn = Button::from_icon_name("window-close-symbolic");
        k_close_btn.set_halign(Align::End);
        k_close_btn.set_hexpand(true);
        k_close_btn.add_css_class("flat");
        k_close_btn.add_css_class("circular");
        k_header.append(&k_close_btn);
        karaoke_box.append(&k_header);

        let lyrics_scrolled = ScrolledWindow::new();
        lyrics_scrolled.set_policy(PolicyType::Never, PolicyType::Automatic);
        lyrics_scrolled.set_vexpand(true);

        let lyrics_container = GtkBox::new(Orientation::Vertical, 14);
        lyrics_container.set_valign(Align::Center);
        lyrics_container.set_halign(Align::Center);
        lyrics_container.set_margin_top(32);
        lyrics_container.set_margin_bottom(80);

        lyrics_scrolled.set_child(Some(&lyrics_container));
        karaoke_box.append(&lyrics_scrolled);
        root_overlay.add_overlay(&karaoke_box);

        let lyrics_data: Rc<RefCell<Vec<LrcLine>>> = Rc::new(RefCell::new(Vec::new()));
        let lyrics_labels: Rc<RefCell<Vec<Label>>> = Rc::new(RefCell::new(Vec::new()));

        // Управление показом караоке
        {
            let k_box_clone = karaoke_box.clone();
            island.karaoke_btn.connect_clicked(move |_| {
                k_box_clone.set_visible(true);
            });
        }
        {
            let k_box_clone = karaoke_box.clone();
            k_close_btn.connect_clicked(move |_| {
                k_box_clone.set_visible(false);
            });
        }

        toast_overlay.set_child(Some(&root_overlay));
        window.set_content(Some(&toast_overlay));

        // Открытие настроек
        {
            let p_clone = player.clone();
            let win_weak = window.downgrade();
            settings_btn.connect_clicked(move |_| {
                if let Some(w) = win_weak.upgrade() {
                    show_preferences_dialog(&w, &p_clone);
                }
            });
        }
        {
            let p_clone = player.clone();
            let win_weak = window.downgrade();
            wave_view.filter_btn.connect_clicked(move |_| {
                if let Some(w) = win_weak.upgrade() {
                    show_preferences_dialog(&w, &p_clone);
                }
            });
        }

        // Таймер тика прогресса и синхронизации караоке
        {
            let player_clone = player.clone();
            let island_seek = island.seek_scale.clone();
            let island_curr_time = island.curr_time_label.clone();
            let island_total_time = island.total_time_label.clone();
            let is_playing_ref = is_playing_state.clone();
            let l_data = lyrics_data.clone();
            let l_labels = lyrics_labels.clone();
            let sc_weak = lyrics_scrolled.downgrade();

            gtk4::glib::timeout_add_local(Duration::from_millis(500), move || {
                if *is_playing_ref.borrow() {
                    if let Some(pos) = player_clone.audio.position_seconds() {
                        let dur = player_clone.audio.duration_seconds().unwrap_or(240);
                        island_seek.set_range(0.0, dur as f64);
                        island_seek.set_value(pos as f64);
                        island_curr_time.set_text(&format_seconds(pos));
                        island_total_time.set_text(&format_seconds(dur));

                        // Синхронизация караоке
                        let pos_ms = pos * 1000;
                        let lines = l_data.borrow();
                        let labels = l_labels.borrow();
                        if !lines.is_empty() && lines.len() == labels.len() {
                            let active_idx = lines.iter().rposition(|l| l.time_ms <= pos_ms);
                            if let Some(idx) = active_idx {
                                for (i, lbl) in labels.iter().enumerate() {
                                    if i == idx {
                                        lbl.add_css_class("active");
                                    } else {
                                        lbl.remove_css_class("active");
                                    }
                                }
                                if let Some(sc) = sc_weak.upgrade() {
                                    let vadj = sc.vadjustment();
                                    let target_val = (idx as f64 * 45.0).min(vadj.upper() - vadj.page_size());
                                    vadj.set_value(target_val.max(0.0));
                                }
                            }
                        }
                    }
                }
                gtk4::glib::ControlFlow::Continue
            });
        }

        // Обработка асинхронных событий
        setup_event_listener(
            event_receiver,
            toast_overlay,
            wave_view,
            island,
            is_playing_state,
            lyrics_container,
            lyrics_data,
            lyrics_labels,
        );

        Self { window }
    }

    pub fn present(&self) {
        self.window.present();
    }
}

fn format_seconds(secs: u64) -> String {
    let m = secs / 60;
    let s = secs % 60;
    format!("{:02}:{:02}", m, s)
}

fn setup_event_listener(
    receiver: Receiver<PlayerUiEvent>,
    toast_overlay: ToastOverlay,
    wave_view: WaveViewWidgets,
    island: FloatingIslandWidgets,
    is_playing_state: Rc<RefCell<bool>>,
    lyrics_container: GtkBox,
    lyrics_data: Rc<RefCell<Vec<LrcLine>>>,
    lyrics_labels: Rc<RefCell<Vec<Label>>>,
) {
    gtk4::glib::spawn_future_local(async move {
        while let Ok(event) = receiver.recv().await {
            match event {
                PlayerUiEvent::TrackStarted { track, .. } => {
                    let artists = track.artists_string();
                    let album = track.albums.first().and_then(|a| a.title.as_ref()).map(|s| s.as_str()).unwrap_or("");
                    let subtitle = if album.is_empty() {
                        artists.clone()
                    } else {
                        format!("{} • {}", artists, album)
                    };

                    wave_view.title_label.set_text(&track.title);
                    wave_view.artist_label.set_text(&subtitle);

                    island.title_label.set_text(&track.title);
                    island.artist_label.set_text(&artists);

                    if let Some(dur_ms) = track.duration_ms {
                        let total_secs = dur_ms / 1000;
                        island.total_time_label.set_text(&format_seconds(total_secs));
                        island.seek_scale.set_range(0.0, total_secs as f64);
                    }
                }
                PlayerUiEvent::PlaybackToggled(is_playing) => {
                    *is_playing_state.borrow_mut() = is_playing;
                    let icon_name = if is_playing {
                        "media-playback-pause-symbolic"
                    } else {
                        "media-playback-start-symbolic"
                    };

                    wave_view.play_btn_icon.set_icon_name(Some(icon_name));
                    island.play_btn_icon.set_icon_name(Some(icon_name));
                    wave_view.drawing_area.queue_draw();
                }
                PlayerUiEvent::Progress { current_sec, total_sec } => {
                    island.curr_time_label.set_text(&format_seconds(current_sec));
                    island.total_time_label.set_text(&format_seconds(total_sec));
                    island.seek_scale.set_value(current_sec as f64);
                }
                PlayerUiEvent::LikeStateChanged(liked) => {
                    if liked {
                        island.like_btn.add_css_class("liked");
                    } else {
                        island.like_btn.remove_css_class("liked");
                    }
                }
                PlayerUiEvent::DislikeStateChanged(disliked) => {
                    if disliked {
                        island.dislike_btn.add_css_class("disliked");
                    } else {
                        island.dislike_btn.remove_css_class("disliked");
                    }
                }
                PlayerUiEvent::CoverUpdated(path) => {
                    island.cover_image.set_from_file(Some(&path));
                }
                PlayerUiEvent::LyricsUpdated(lrc_text) => {
                    let parsed = parse_lrc(&lrc_text);
                    // Очищаем старые строки
                    while let Some(child) = lyrics_container.first_child() {
                        lyrics_container.remove(&child);
                    }

                    let mut new_labels = Vec::new();
                    for line in &parsed {
                        let lbl = Label::new(Some(&line.text));
                        lbl.add_css_class("karaoke-line");
                        lbl.set_wrap(true);
                        lbl.set_justify(gtk4::Justification::Center);
                        lyrics_container.append(&lbl);
                        new_labels.push(lbl);
                    }

                    *lyrics_data.borrow_mut() = parsed;
                    *lyrics_labels.borrow_mut() = new_labels;
                }
                PlayerUiEvent::StatusMessage(msg) => {
                    let toast = Toast::new(&msg);
                    toast_overlay.add_toast(toast);
                }
                PlayerUiEvent::Error(err) => {
                    let toast = Toast::new(&format!("Ошибка: {}", err));
                    toast_overlay.add_toast(toast);
                }
                _ => {}
            }
        }
    });
}
