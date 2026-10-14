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
use super::auth_dialog::show_auth_dialog;
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

        // Правая часть HeaderBar: Кнопка профиля/авторизации + Настройки
        let right_header_box = GtkBox::new(Orientation::Horizontal, 8);

        let auth_btn = Button::with_label("Войти в Яндекс ID");
        auth_btn.add_css_class("suggested-action");
        right_header_box.append(&auth_btn);

        let settings_btn = Button::from_icon_name("emblem-system-symbolic");
        settings_btn.set_tooltip_text(Some("Настройки Волны и токен"));
        settings_btn.add_css_class("flat");
        settings_btn.add_css_class("circular");
        right_header_box.append(&settings_btn);

        header_bar.pack_end(&right_header_box);
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
        let karaoke_overlay = Overlay::new();
        karaoke_overlay.add_css_class("karaoke-sheet");
        karaoke_overlay.set_visible(false);
        karaoke_overlay.set_vexpand(true);
        karaoke_overlay.set_hexpand(true);

        // 1. Скроллируемый контейнер текста песни (базовый слой)
        let lyrics_scrolled = ScrolledWindow::new();
        lyrics_scrolled.add_css_class("karaoke-lyrics-scrolled");
        lyrics_scrolled.set_policy(PolicyType::Never, PolicyType::Automatic);
        lyrics_scrolled.set_vexpand(true);
        lyrics_scrolled.set_hexpand(true);

        let lyrics_container = GtkBox::new(Orientation::Vertical, 14);
        lyrics_container.set_valign(Align::Center);
        lyrics_container.set_halign(Align::Center);
        lyrics_container.set_margin_top(80);
        lyrics_container.set_margin_bottom(90);

        lyrics_scrolled.set_child(Some(&lyrics_container));
        karaoke_overlay.set_child(Some(&lyrics_scrolled));

        // 2. Абсолютно зафиксированная шапка (слой поверх текста, Align::Start по вертикали)
        let k_header = GtkBox::new(Orientation::Horizontal, 16);
        k_header.set_valign(Align::Start);
        k_header.set_hexpand(true);
        k_header.set_margin_top(20);
        k_header.set_margin_start(24);
        k_header.set_margin_end(24);

        let k_track_info = GtkBox::new(Orientation::Vertical, 2);
        k_track_info.set_valign(Align::Center);
        k_track_info.set_halign(Align::Start);

        let k_track_title = Label::new(Some("Моя волна"));
        k_track_title.add_css_class("heading");
        k_track_title.set_halign(Align::Start);
        k_track_title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        k_track_title.set_max_width_chars(28);

        let k_track_artist = Label::new(Some("Яндекс Музыка"));
        k_track_artist.add_css_class("dim-label");
        k_track_artist.set_halign(Align::Start);
        k_track_artist.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        k_track_artist.set_max_width_chars(28);

        k_track_info.append(&k_track_title);
        k_track_info.append(&k_track_artist);
        k_header.append(&k_track_info);

        let k_close_btn = Button::from_icon_name("window-close-symbolic");
        k_close_btn.set_halign(Align::End);
        k_close_btn.set_valign(Align::Center);
        k_close_btn.set_hexpand(true);
        k_close_btn.add_css_class("circular");
        k_close_btn.add_css_class("karaoke-close-btn");
        k_close_btn.set_tooltip_text(Some("Закрыть текст песни"));
        k_header.append(&k_close_btn);
        karaoke_overlay.add_overlay(&k_header);

        // 3. Крупная неоновая цифра обратного отсчета (парит в верхней трети, не дергает верстку)
        let countdown_label = Label::new(Some("3"));
        countdown_label.add_css_class("karaoke-countdown-digit");
        countdown_label.set_halign(Align::Center);
        countdown_label.set_valign(Align::Start);
        countdown_label.set_margin_top(90);
        countdown_label.set_visible(false);
        karaoke_overlay.add_overlay(&countdown_label);

        root_overlay.add_overlay(&karaoke_overlay);

        let lyrics_data: Rc<RefCell<Vec<LrcLine>>> = Rc::new(RefCell::new(Vec::new()));
        let lyrics_labels: Rc<RefCell<Vec<Label>>> = Rc::new(RefCell::new(Vec::new()));

        // Управление показом караоке
        {
            let k_overlay_clone = karaoke_overlay.clone();
            island.karaoke_btn.connect_clicked(move |_| {
                k_overlay_clone.set_visible(true);
            });
        }
        {
            let k_overlay_clone = karaoke_overlay.clone();
            k_close_btn.connect_clicked(move |_| {
                k_overlay_clone.set_visible(false);
            });
        }

        toast_overlay.set_child(Some(&root_overlay));
        window.set_content(Some(&toast_overlay));

        // Клик по кнопке авторизации
        {
            let p_clone = player.clone();
            let win_weak = window.downgrade();
            auth_btn.connect_clicked(move |_| {
                if let Some(w) = win_weak.upgrade() {
                    let is_logged_in = p_clone.client.lock().unwrap().has_token();
                    if is_logged_in {
                        // Диалог с предложением выйти
                        let alert = libadwaita::AlertDialog::new(
                            Some("Управление аккаунтом"),
                            Some("Вы авторизованы в Яндекс Музыке."),
                        );
                        alert.add_response("cancel", "Отмена");
                        alert.add_response("logout", "Выйти из аккаунта");
                        alert.set_response_appearance("logout", libadwaita::ResponseAppearance::Destructive);

                        let p_inner = p_clone.clone();
                        alert.choose(Some(&w), None::<&gio::Cancellable>, move |response| {
                            if response == "logout" {
                                p_inner.logout();
                            }
                        });
                    } else {
                        show_auth_dialog(&w, &p_clone);
                    }
                }
            });
        }

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
            let cnt_label = countdown_label.clone();
            let sc_clone = lyrics_scrolled.clone();

            gtk4::glib::timeout_add_local(Duration::from_millis(200), move || {
                if *is_playing_ref.borrow() {
                    if let Some(pos) = player_clone.audio.position_seconds() {
                        let dur = player_clone.audio.duration_seconds().unwrap_or(240);
                        island_seek.set_range(0.0, dur as f64);
                        island_seek.set_value(pos as f64);
                        island_curr_time.set_text(&format_seconds(pos));
                        island_total_time.set_text(&format_seconds(dur));

                        // Синхронизация караоке и индикатор обратного отсчета 3..2..1
                        let pos_ms = pos * 1000;
                        let lines = l_data.borrow();
                        let labels = l_labels.borrow();
                        if !lines.is_empty() && lines.len() == labels.len() {
                            // Проверяем обратный отсчет перед вступлением или длинным проигрышем (> 4 сек)
                            let next_line_info = match lines.iter().position(|l| l.time_ms > pos_ms) {
                                Some(0) => {
                                    // До первой строки песни
                                    Some(lines[0].time_ms)
                                }
                                Some(next_idx) => {
                                    let prev_time = lines[next_idx - 1].time_ms;
                                    let next_time = lines[next_idx].time_ms;
                                    if next_time.saturating_sub(prev_time) >= 4000 {
                                        Some(next_time)
                                    } else {
                                        None
                                    }
                                }
                                None => None,
                            };

                            let mut counting_down = false;
                            if let Some(target_time) = next_line_info {
                                if target_time > pos_ms {
                                    let diff_ms = target_time - pos_ms;
                                    if diff_ms <= 3000 && diff_ms > 0 {
                                        counting_down = true;
                                        cnt_label.set_visible(true);
                                        let digit_str = if diff_ms > 2000 {
                                            "3"
                                        } else if diff_ms > 1000 {
                                            "2"
                                        } else {
                                            "1"
                                        };
                                        cnt_label.set_text(digit_str);
                                        sc_clone.set_opacity(0.0);
                                    }
                                }
                            }

                            if !counting_down {
                                cnt_label.set_visible(false);
                                sc_clone.set_opacity(1.0);
                            }

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
                        } else {
                            cnt_label.set_visible(false);
                            sc_clone.set_opacity(1.0);
                        }
                    }
                }
                gtk4::glib::ControlFlow::Continue
            });
        }

        // Обработка асинхронных событий
        setup_event_listener(
            event_receiver,
            window.clone(),
            toast_overlay,
            wave_view,
            island,
            auth_btn,
            player.clone(),
            is_playing_state,
            lyrics_container,
            lyrics_data,
            lyrics_labels,
            countdown_label,
            lyrics_scrolled,
            k_track_title,
            k_track_artist,
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
    window: ApplicationWindow,
    toast_overlay: ToastOverlay,
    wave_view: WaveViewWidgets,
    island: FloatingIslandWidgets,
    auth_btn: Button,
    player: PlayerManager,
    is_playing_state: Rc<RefCell<bool>>,
    lyrics_container: GtkBox,
    lyrics_data: Rc<RefCell<Vec<LrcLine>>>,
    lyrics_labels: Rc<RefCell<Vec<Label>>>,
    countdown_label: Label,
    lyrics_scrolled: ScrolledWindow,
    k_track_title: Label,
    k_track_artist: Label,
) {
    let win_weak = window.downgrade();
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
                    wave_view.update_track_colors(&track);

                    island.title_label.set_text(&track.title);
                    island.artist_label.set_text(&artists);

                    k_track_title.set_text(&track.title);
                    k_track_artist.set_text(&artists);

                    // Сбрасываем текст песни и обратный отсчет для нового трека
                    while let Some(child) = lyrics_container.first_child() {
                        lyrics_container.remove(&child);
                    }
                    *lyrics_data.borrow_mut() = Vec::new();
                    *lyrics_labels.borrow_mut() = Vec::new();
                    countdown_label.set_visible(false);
                    lyrics_scrolled.set_opacity(1.0);

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
                    while let Some(child) = lyrics_container.first_child() {
                        lyrics_container.remove(&child);
                    }

                    let parsed = parse_lrc(&lrc_text);
                    if !parsed.is_empty() {
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
                    } else {
                        countdown_label.set_visible(false);
                        lyrics_scrolled.set_opacity(1.0);
                        *lyrics_data.borrow_mut() = Vec::new();
                        *lyrics_labels.borrow_mut() = Vec::new();

                        let trimmed = lrc_text.trim();
                        if !trimmed.is_empty() {
                            for line in lrc_text.lines() {
                                let line_trimmed = line.trim();
                                if line_trimmed.is_empty() {
                                    let spacer = GtkBox::new(Orientation::Vertical, 0);
                                    spacer.set_size_request(-1, 10);
                                    lyrics_container.append(&spacer);
                                } else {
                                    let lbl = Label::new(Some(line_trimmed));
                                    lbl.add_css_class("plain-lyrics-line");
                                    lbl.set_wrap(true);
                                    lbl.set_justify(gtk4::Justification::Center);
                                    lyrics_container.append(&lbl);
                                }
                            }
                        } else {
                            let empty_box = GtkBox::new(Orientation::Vertical, 12);
                            empty_box.set_valign(Align::Center);
                            empty_box.set_halign(Align::Center);
                            empty_box.set_margin_top(60);

                            let empty_icon = gtk4::Image::from_icon_name("audio-x-generic-symbolic");
                            empty_icon.set_pixel_size(48);
                            empty_icon.add_css_class("dim-label");
                            empty_box.append(&empty_icon);

                            let empty_label = Label::new(Some("У этого трека текст песни пока не добавлен"));
                            empty_label.add_css_class("heading");
                            empty_label.add_css_class("dim-label");
                            empty_box.append(&empty_label);

                            lyrics_container.append(&empty_box);
                        }
                    }
                }
                PlayerUiEvent::AuthRequired => {
                    auth_btn.set_label("Войти в Яндекс ID");
                    auth_btn.remove_css_class("flat");
                    auth_btn.add_css_class("suggested-action");
                    if let Some(w) = win_weak.upgrade() {
                        show_auth_dialog(&w, &player);
                    }
                }
                PlayerUiEvent::UserLoggedIn(login) => {
                    auth_btn.set_label(&format!("👤 {}", login));
                    auth_btn.remove_css_class("suggested-action");
                    auth_btn.add_css_class("flat");
                }
                PlayerUiEvent::UserLoggedOut => {
                    auth_btn.set_label("Войти в Яндекс ID");
                    auth_btn.remove_css_class("flat");
                    auth_btn.add_css_class("suggested-action");

                    wave_view.title_label.set_text("Моя волна");
                    wave_view.artist_label.set_text("Войдите в Яндекс ID для запуска потока");
                    island.title_label.set_text("Моя волна");
                    island.artist_label.set_text("Яндекс Музыка");
                    island.cover_image.set_icon_name(Some("audio-x-generic-symbolic"));
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
