use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk4::prelude::*;
use libadwaita::prelude::*;
use gtk4::{
    glib, Align, Box as GtkBox, Button, Label, ListBox, Orientation, ScrolledWindow,
    SelectionMode, Stack,
};

use super::collection::{album_tile, ask_playlist_name, clear_box, ensure_liked, new_playlist_tile, open_playlist, playlist_tile, section_title, tile_row, track_actions, track_row, DetailPage};
use crate::api::models::Playlist;
use crate::player::{PlayerManager, PlayerUiEvent, PlaySource, SourceTarget};

pub struct LibraryViewWidgets {
    pub container: GtkBox,
    pub open_liked: Rc<dyn Fn()>,
    pub set_layout_margins: Rc<dyn Fn(i32, i32)>,
}

const TILE_SIZE: i32 = 160;
const LIKED_PREVIEW: usize = 6;
const LIBRARY_REFRESH_TTL: Duration = Duration::from_secs(300);

pub fn create_library_view(player: &PlayerManager) -> LibraryViewWidgets {
    let container = GtkBox::new(Orientation::Vertical, 0);

    let stack = Stack::new();
    stack.set_hexpand(true);
    stack.set_vexpand(true);
    stack.set_transition_type(gtk4::StackTransitionType::Crossfade);
    container.append(&stack);

    let page = {
        let stack = stack.clone();
        DetailPage::new(player, move || stack.set_visible_child_name("main"))
    };
    stack.add_named(&page.widget, Some("detail"));
    page.set_nav(&stack);

    let main = GtkBox::new(Orientation::Vertical, 26);
    let header_row = GtkBox::new(Orientation::Horizontal, 8);
    let heading = Label::new(Some("Коллекция"));
    heading.set_halign(Align::Start);
    heading.set_hexpand(true);
    heading.add_css_class("detail-title");
    let refresh_btn = Button::from_icon_name("view-refresh-symbolic");
    refresh_btn.add_css_class("flat");
    refresh_btn.add_css_class("circular");
    refresh_btn.set_valign(Align::Center);
    refresh_btn.set_tooltip_text(Some("Обновить медиатеку"));
    header_row.append(&heading);
    header_row.append(&refresh_btn);
    main.append(&header_row);

    let status = Label::new(None);
    status.set_halign(Align::Start);
    status.add_css_class("dim-label");
    status.set_visible(false);
    main.append(&status);

    let yt_login_btn = Button::with_label("Войти в Google");
    yt_login_btn.add_css_class("suggested-action");
    yt_login_btn.add_css_class("pill");
    yt_login_btn.set_halign(Align::Start);
    yt_login_btn.set_visible(false);
    main.append(&yt_login_btn);

    // Мне нравится: превью и переход к полному списку
    let liked_title = Button::with_label("Мне нравится");
    liked_title.add_css_class("flat");
    liked_title.add_css_class("section-title");
    liked_title.set_halign(Align::Start);
    let liked_count = Label::new(None);
    liked_count.set_halign(Align::Start);
    liked_count.add_css_class("dim-label");
    let liked_empty_msg = Label::new(Some("Ставьте лайк трекам ❤️, чтобы наполнить медиатеку"));
    liked_empty_msg.set_halign(Align::Start);
    liked_empty_msg.add_css_class("dim-label");
    liked_empty_msg.set_visible(false);
    let liked_list = ListBox::new();
    liked_list.add_css_class("boxed-list");
    liked_list.set_selection_mode(SelectionMode::None);
    let liked_all = Button::with_label("Показать все");
    liked_all.add_css_class("pill");
    liked_all.set_halign(Align::Start);

    let liked_box = GtkBox::new(Orientation::Vertical, 8);
    liked_box.append(&liked_title);
    liked_box.append(&liked_count);
    liked_box.append(&liked_empty_msg);
    liked_box.append(&liked_list);
    liked_box.append(&liked_all);
    liked_box.set_visible(false);
    main.append(&liked_box);

    let liked_tracks: Rc<RefCell<Vec<crate::api::models::Track>>> = Rc::default();
    let liked_total: Rc<RefCell<Option<u32>>> = Rc::default();
    let open_liked_fn: Rc<dyn Fn()> = {
        let page = page.clone();
        let stack = stack.clone();
        let tracks = liked_tracks.clone();
        let liked_total = liked_total.clone();
        let p = player.clone();
        Rc::new(move || {
            let tracks = tracks.borrow().clone();
            let total_opt = *liked_total.borrow();
            let sub = match total_opt {
                Some(total) if total > tracks.len() as u32 => format!("{} треков ({} доступно)", total, tracks.len()),
                Some(total) if total > 0 => format!("{} треков", total),
                _ => format!("{} треков", tracks.len()),
            };
            let cover_url = if p.providers.current().provider_id() == crate::api::provider::ProviderId::YouTubeMusic {
                Some("https://www.gstatic.com/youtube/media/ytm/images/pbg/liked-songs-delhi-576.png".to_string())
            } else {
                tracks.first().and_then(|t| t.cover_uri.clone())
            };
            page.show("Коллекция", "Мне нравится", &sub, cover_url, tracks);
            stack.set_visible_child_name("detail");
        })
    };
    for open_liked in [liked_title.clone(), liked_all.clone()] {
        let f = open_liked_fn.clone();
        open_liked.connect_clicked(move |_| {
            f();
        });
    }

    // Загруженные треки: играют без интернета
    let offline_list = ListBox::new();
    offline_list.add_css_class("boxed-list");
    offline_list.set_selection_mode(SelectionMode::None);
    let offline_box = GtkBox::new(Orientation::Vertical, 10);
    offline_box.append(&section_title("Загруженные"));
    offline_box.append(&offline_list);
    offline_box.set_visible(false);
    main.append(&offline_box);

    // Плейлисты и любимые альбомы
    let (playlists_scroll, playlists_row) = tile_row();
    let playlists_box = GtkBox::new(Orientation::Vertical, 10);
    playlists_box.append(&section_title("Плейлисты"));
    playlists_box.append(&playlists_scroll);
    playlists_box.set_visible(false);
    main.append(&playlists_box);

    let (albums_scroll, albums_row) = tile_row();
    let albums_box = GtkBox::new(Orientation::Vertical, 10);
    albums_box.append(&section_title("Любимые альбомы"));
    albums_box.append(&albums_scroll);
    albums_box.set_visible(false);
    main.append(&albums_box);

    let scrolled = ScrolledWindow::new();
    scrolled.set_vexpand(true);
    // отступ под плавающий плеер даёт само содержимое прокрутки: оно уезжает под плеер и докручивается выше него
    scrolled.set_margin_top(24);
    scrolled.set_margin_start(32);
    scrolled.set_margin_end(32);
    main.set_margin_bottom(130);
    main.set_margin_end(14); // полоса прокрутки не наезжает на строки
    scrolled.set_child(Some(&main));
    stack.add_named(&scrolled, Some("main"));
    stack.set_visible_child_name("main");

    let set_layout_margins: Rc<dyn Fn(i32, i32)> = {
        let (container, scrolled, page) = (container.clone(), scrolled.clone(), page.clone());
        Rc::new(move |sidebar_w, gutter| {
            container.set_margin_start(sidebar_w);
            container.set_margin_end(0);
            scrolled.set_margin_start(gutter);
            scrolled.set_margin_end(gutter);
            page.set_side_margins(gutter);
        })
    };

    let dirty = Rc::new(Cell::new(true));
    let last_loaded: Rc<RefCell<Option<Instant>>> = Rc::default();
    let reload_in_flight = Rc::new(Cell::new(false));
    let reload_gen = Rc::new(Cell::new(0u64));
    let pending_reason: Rc<Cell<Option<&'static str>>> = Rc::new(Cell::new(None));

    let reload_library: Rc<dyn Fn(&'static str)> = {
        let (p, stack, page, status, yt_login_btn, refresh_btn) = (
            player.clone(),
            stack.clone(),
            page.clone(),
            status.clone(),
            yt_login_btn.clone(),
            refresh_btn.clone(),
        );
        let (liked_box, liked_count, liked_empty_msg, liked_list, liked_all, liked_tracks, liked_total) =
            (liked_box.clone(), liked_count.clone(), liked_empty_msg.clone(), liked_list.clone(), liked_all.clone(), liked_tracks.clone(), liked_total.clone());
        let (playlists_box, playlists_row) = (playlists_box.clone(), playlists_row.clone());
        let (albums_box, albums_row) = (albums_box.clone(), albums_row.clone());
        let (offline_box, offline_list) = (offline_box.clone(), offline_list.clone());
        let (dirty, last_loaded, reload_in_flight, reload_gen, pending_reason) = (
            dirty.clone(),
            last_loaded.clone(),
            reload_in_flight.clone(),
            reload_gen.clone(),
            pending_reason.clone(),
        );

        Rc::new(move |reason: &'static str| {
            pending_reason.set(None);
            eprintln!("[ui] library reload: reason={}", reason);
            let my_gen = reload_gen.get().wrapping_add(1);
            reload_gen.set(my_gen);
            reload_in_flight.set(true);
            refresh_btn.set_sensitive(false);

            let (p, stack, page, status, yt_login_btn, refresh_btn) = (
                p.clone(),
                stack.clone(),
                page.clone(),
                status.clone(),
                yt_login_btn.clone(),
                refresh_btn.clone(),
            );
            let (liked_box, liked_count, liked_empty_msg, liked_list, liked_all, liked_tracks, liked_total) =
                (liked_box.clone(), liked_count.clone(), liked_empty_msg.clone(), liked_list.clone(), liked_all.clone(), liked_tracks.clone(), liked_total.clone());
            let (playlists_box, playlists_row) = (playlists_box.clone(), playlists_row.clone());
            let (albums_box, albums_row) = (albums_box.clone(), albums_row.clone());
            let (offline_box, offline_list) = (offline_box.clone(), offline_list.clone());
            let (dirty, last_loaded, reload_in_flight, reload_gen) = (
                dirty.clone(),
                last_loaded.clone(),
                reload_in_flight.clone(),
                reload_gen.clone(),
            );

            // офлайн-треки показываются сразу и без сети, отфильтрованные по активному провайдеру
            offline_list.remove_all();
            let active_id = p.providers.active_id();
            let saved: Vec<crate::api::models::Track> = crate::cache::list_offline()
                .into_iter()
                .filter(|track| match active_id {
                    crate::api::provider::ProviderId::Vk => {
                        track.source == crate::api::models::TrackSource::Vk
                    }
                    crate::api::provider::ProviderId::YouTubeMusic => {
                        track.source == crate::api::models::TrackSource::YouTubeMusic
                    }
                    crate::api::provider::ProviderId::Yandex => {
                        track.source == crate::api::models::TrackSource::Yandex
                    }
                })
                .collect();
            let offline_empty = saved.is_empty();
            // Общий Rc разделяется между всеми строками офлайн-списка без O(N^2) клонирования.
            let saved_rc = Rc::new(saved);
            for (i, track) in saved_rc.iter().enumerate() {
                let (p_cb, queue) = (p.clone(), saved_rc.clone());
                let row = track_row(track, move || {
                    p_cb.set_queue_and_play_from(
                        (*queue).clone(),
                        i,
                        Some(PlaySource::new(
                            "Загруженные",
                            Some(SourceTarget::Library),
                        )),
                    )
                });
                row.add_suffix(&track_actions(&p, Some((&stack, &page)), track));
                offline_list.append(&row);
            }
            offline_box.set_visible(!offline_empty);

            glib::spawn_future_local(async move {
                let provider = p.providers.current();
                let is_yandex = provider.provider_id() == crate::api::provider::ProviderId::Yandex;
                let is_yt = provider.provider_id() == crate::api::provider::ProviderId::YouTubeMusic;

                if is_yandex && !provider.is_authenticated() {
                    if reload_gen.get() != my_gen {
                        return;
                    }
                    status.set_text("Укажите токен Яндекс Музыки в настройках ⚙ для доступа к коллекции");
                    status.set_visible(true);
                    yt_login_btn.set_visible(false);
                    liked_box.set_visible(false);
                    playlists_box.set_visible(false);
                    albums_box.set_visible(false);
                    dirty.set(false);
                    *last_loaded.borrow_mut() = Some(Instant::now());
                    reload_in_flight.set(false);
                    refresh_btn.set_sensitive(true);
                    return;
                }

                if is_yt && !provider.is_authenticated() {
                    if reload_gen.get() != my_gen {
                        return;
                    }
                    status.set_text("Войдите в Google Аккаунт для доступа к вашей медиатеке YouTube Music 🔴");
                    status.set_visible(true);
                    yt_login_btn.set_visible(true);
                    liked_box.set_visible(false);
                    playlists_box.set_visible(false);
                    albums_box.set_visible(false);
                    dirty.set(false);
                    *last_loaded.borrow_mut() = Some(Instant::now());
                    reload_in_flight.set(false);
                    refresh_btn.set_sensitive(true);
                    return;
                }

                yt_login_btn.set_visible(false);
                status.set_text("");
                if is_yandex {
                    ensure_liked(&p).await;
                }

                let (liked_res, playlists_res, albums_res, total_cnt_opt) = tokio::join!(
                    provider.get_liked_tracks(),
                    provider.get_playlists(),
                    provider.get_liked_albums(),
                    provider.get_liked_total_count()
                );

                if reload_gen.get() != my_gen {
                    return;
                }

                *liked_total.borrow_mut() = total_cnt_opt;

                // Пустая медиатека при "авторизованном" аккаунте — почти всегда
                // протухшая кука Google: приложение считает вход живым и молчит.
                let library_empty = liked_res
                    .as_ref()
                    .map(|tracks| tracks.is_empty())
                    .unwrap_or(true)
                    && playlists_res
                        .as_ref()
                        .map(|playlists| playlists.is_empty())
                        .unwrap_or(true)
                    && albums_res.as_ref().map(|albums| albums.is_empty()).unwrap_or(true);
                let session_expired = is_yt && provider.is_authenticated() && library_empty;
                if session_expired {
                    status.set_text(
                        "Не удалось загрузить медиатеку: похоже, сессия Google истекла. Войдите заново через карточку сервиса → «Аккаунт»",
                    );
                    status.set_visible(true);
                    yt_login_btn.set_visible(true);
                }

                let any_ok = liked_res.is_ok() || playlists_res.is_ok() || albums_res.is_ok();
                let mut liked_cnt = 0usize;
                let mut playlists_cnt = 0usize;
                let mut albums_cnt = 0usize;

                if let Ok(tracks) = liked_res {
                    let tracks_len = tracks.len();
                    liked_cnt = tracks_len;
                    *liked_tracks.borrow_mut() = tracks.clone();
                    if tracks_len > 0 {
                        liked_empty_msg.set_visible(false);
                        let count_str = match total_cnt_opt {
                            Some(total) if total > tracks_len as u32 => format!("{} треков ({} доступно)", total, tracks_len),
                            Some(total) if total > 0 => format!("{} треков", total),
                            _ => format!("{} треков", tracks_len),
                        };
                        liked_count.set_text(&count_str);
                        liked_count.set_visible(true);
                        liked_list.remove_all();
                        let queue_rc = Rc::new(tracks);
                        for (i, track) in queue_rc.iter().take(LIKED_PREVIEW).enumerate() {
                            let (p_cb, queue) = (p.clone(), queue_rc.clone());
                            let row = track_row(track, move || {
                                p_cb.set_queue_and_play_from(
                                    (*queue).clone(),
                                    i,
                                    Some(PlaySource::new("Мне нравится", Some(SourceTarget::Library))),
                                )
                            });
                            row.add_suffix(&track_actions(&p, Some((&stack, &page)), track));
                            liked_list.append(&row);
                        }
                        liked_list.set_visible(true);
                        liked_all.set_visible(tracks_len > LIKED_PREVIEW);
                        liked_box.set_visible(true);
                    } else if is_yt && provider.is_authenticated() {
                        liked_empty_msg.set_visible(true);
                        liked_count.set_visible(false);
                        liked_list.remove_all();
                        liked_list.set_visible(false);
                        liked_all.set_visible(false);
                        liked_box.set_visible(true);
                    } else {
                        liked_empty_msg.set_visible(false);
                        liked_box.set_visible(false);
                    }
                } else {
                    liked_empty_msg.set_visible(false);
                    liked_box.set_visible(false);
                }

                if let Ok(playlists) = playlists_res {
                    playlists_cnt = playlists.len();
                    clear_box(&playlists_row);

                    if is_yandex {
                        let uid = {
                            let client = p.client.lock().unwrap().clone();
                            client.get_uid().await.unwrap_or_else(|_| "user".to_string())
                        };
                        if reload_gen.get() != my_gen {
                            return;
                        }
                        let (p2, stack2, page2, row2, uid2) = (p.clone(), stack.clone(), page.clone(), playlists_row.clone(), uid.clone());
                        let new_tile = new_playlist_tile(TILE_SIZE, {
                            let anchor = playlists_row.clone();
                            move || {
                                let (p, stack, page, row, uid) = (p2.clone(), stack2.clone(), page2.clone(), row2.clone(), uid2.clone());
                                ask_playlist_name(&anchor, move |name| {
                                    glib::spawn_future_local(async move {
                                        let client = p.client.lock().unwrap().clone();
                                        match client.create_playlist(&uid, &name).await {
                                            Ok(kind) => {
                                                let pl = Playlist { uid: uid.clone(), kind, title: name, track_count: Some(0), ..Default::default() };
                                                open_playlist(&p, &stack, &page, &pl);
                                                fill_playlists(&p, &stack, &page, &row, &uid).await;
                                            }
                                            Err(e) => p.emit(PlayerUiEvent::Error(format!("Не удалось создать плейлист: {}", e))),
                                        }
                                    });
                                });
                            }
                        });
                        playlists_row.append(&new_tile);
                    }

                    for pl in &playlists {
                        playlists_row.append(&playlist_tile(&p, &stack, &page, pl, TILE_SIZE));
                    }
                    playlists_box.set_visible(!playlists.is_empty() || is_yandex);
                } else {
                    playlists_box.set_visible(false);
                }

                if let Ok(albums) = albums_res {
                    albums_cnt = albums.len();
                    clear_box(&albums_row);
                    for album in &albums {
                        albums_row.append(&album_tile(&p, &stack, &page, album, TILE_SIZE));
                    }
                    albums_box.set_visible(!albums.is_empty());
                } else {
                    albums_box.set_visible(false);
                }

                if any_ok && !session_expired {
                    dirty.set(false);
                    *last_loaded.borrow_mut() = Some(Instant::now());
                } else {
                    dirty.set(true);
                }
                reload_in_flight.set(false);
                refresh_btn.set_sensitive(true);
                eprintln!(
                    "[ui] library ready: liked={} playlists={} albums={} rss_mb={:.1}",
                    liked_cnt,
                    playlists_cnt,
                    albums_cnt,
                    crate::current_rss_mb()
                );
            });
        })
    };

    {
        let (dirty, reload) = (dirty.clone(), reload_library.clone());
        refresh_btn.connect_clicked(move |_| {
            dirty.set(true);
            reload("manual");
        });
    }

    {
        let (dirty, last_loaded, container, reload, pending_reason) = (
            dirty.clone(),
            last_loaded.clone(),
            container.clone(),
            reload_library.clone(),
            pending_reason.clone(),
        );
        page.set_on_removed(move || {
            dirty.set(true);
            *last_loaded.borrow_mut() = None;
            if container.is_mapped() {
                reload("dirty");
            } else {
                pending_reason.set(Some("dirty"));
            }
        });
    }

    {
        let p = player.clone();
        let btn = yt_login_btn.clone();
        let (reload, dirty, last_loaded) = (reload_library.clone(), dirty.clone(), last_loaded.clone());
        yt_login_btn.connect_clicked(move |_| {
            let (reload, dirty, last_loaded) = (reload.clone(), dirty.clone(), last_loaded.clone());
            super::preferences_dialog::show_ytmusic_auth_dialog(&btn, &p, move || {
                dirty.set(true);
                *last_loaded.borrow_mut() = None;
                reload("auth");
            });
        });
    }

    let rx = player.subscribe();
    {
        let (player, page, stack, container, liked_tracks, liked_count, liked_empty_msg, liked_list, liked_all, liked_box, liked_total, reload, dirty, last_loaded, pending_reason) = (
            player.clone(),
            page.clone(),
            stack.clone(),
            container.clone(),
            liked_tracks.clone(),
            liked_count.clone(),
            liked_empty_msg.clone(),
            liked_list.clone(),
            liked_all.clone(),
            liked_box.clone(),
            liked_total.clone(),
            reload_library.clone(),
            dirty.clone(),
            last_loaded.clone(),
            pending_reason.clone(),
        );
        glib::spawn_future_local(async move {
            while let Ok(event) = rx.recv().await {
                match event {
                    PlayerUiEvent::ProviderChanged(_)
                    | PlayerUiEvent::UserLoggedIn(_)
                    | PlayerUiEvent::UserLoggedOut => {
                        let reason = match &event {
                            PlayerUiEvent::ProviderChanged(_) => "provider",
                            PlayerUiEvent::UserLoggedIn(_) | PlayerUiEvent::UserLoggedOut => "auth",
                            _ => unreachable!(),
                        };
                        dirty.set(true);
                        *last_loaded.borrow_mut() = None;
                        if container.is_mapped() {
                            reload(reason);
                        } else {
                            pending_reason.set(Some(reason));
                        }
                    }
                    PlayerUiEvent::TrackLiked { track, liked } => {
                        dirty.set(true);
                        page.handle_track_liked(&track, liked);

                        if liked {
                            if !liked_tracks.borrow().iter().any(|t| t.id == track.id) {
                                liked_tracks.borrow_mut().insert(0, track.clone());
                            }
                        } else {
                            liked_tracks.borrow_mut().retain(|t| t.id != track.id);
                        }

                        let count = liked_tracks.borrow().len();
                        if count > 0 {
                            liked_empty_msg.set_visible(false);
                            let total_opt = *liked_total.borrow();
                            let count_str = match total_opt {
                                Some(total) if total > count as u32 => format!("{} треков ({} доступно)", total, count),
                                Some(total) if total > 0 => format!("{} треков", total),
                                _ => format!("{} треков", count),
                            };
                            liked_count.set_text(&count_str);
                            liked_count.set_visible(true);

                            liked_list.remove_all();
                            for (i, t) in liked_tracks.borrow().iter().take(LIKED_PREVIEW).enumerate() {
                                let (p, queue) = (player.clone(), liked_tracks.clone());
                                let row = track_row(t, move || {
                                    p.set_queue_and_play_from(
                                        queue.borrow().clone(),
                                        i,
                                        Some(PlaySource::new("Мне нравится", Some(SourceTarget::Library))),
                                    )
                                });
                                row.add_suffix(&track_actions(&player, Some((&stack, &page)), t));
                                liked_list.append(&row);
                            }
                            liked_list.set_visible(true);
                            liked_all.set_visible(count > LIKED_PREVIEW);
                            liked_box.set_visible(true);
                        } else {
                            let provider = player.providers.current();
                            let is_yt = provider.provider_id() == crate::api::provider::ProviderId::YouTubeMusic;
                            if is_yt && provider.is_authenticated() {
                                liked_empty_msg.set_visible(true);
                                liked_count.set_visible(false);
                                liked_list.remove_all();
                                liked_list.set_visible(false);
                                liked_all.set_visible(false);
                                liked_box.set_visible(true);
                            } else {
                                liked_empty_msg.set_visible(false);
                                liked_box.set_visible(false);
                            }
                        }
                    }
                    _ => {}
                }
            }
        });
    }

    // Данные подгружаются при первом показе, если помечены dirty или устарел 5-минутный TTL
    {
        let (reload, dirty, last_loaded, in_flight, pending_reason) = (
            reload_library.clone(),
            dirty.clone(),
            last_loaded.clone(),
            reload_in_flight.clone(),
            pending_reason.clone(),
        );
        container.connect_map(move |_| {
            let had_loaded = last_loaded.borrow().is_some();
            let expired = last_loaded
                .borrow()
                .is_none_or(|t| t.elapsed() >= LIBRARY_REFRESH_TTL);
            if (dirty.get() || expired) && !in_flight.get() {
                let reason = pending_reason.take().unwrap_or_else(|| {
                    if !had_loaded {
                        "initial"
                    } else if dirty.get() {
                        "dirty"
                    } else {
                        "ttl"
                    }
                });
                reload(reason);
            }
        });
    }

    LibraryViewWidgets {
        container,
        open_liked: open_liked_fn,
        set_layout_margins,
    }
}

/// Плитка создания и плейлисты пользователя; после создания список перерисовывается
async fn fill_playlists(p: &PlayerManager, stack: &Stack, page: &Rc<DetailPage>, row: &GtkBox, uid: &str) {
    let client = p.client.lock().unwrap().clone();
    let playlists = client.get_user_playlists(uid).await.unwrap_or_default();
    clear_box(row);
    let (p2, stack2, page2, row2, uid2) = (p.clone(), stack.clone(), page.clone(), row.clone(), uid.to_string());
    let new_tile = new_playlist_tile(TILE_SIZE, {
        let anchor = row.clone();
        move || {
            let (p, stack, page, row, uid) = (p2.clone(), stack2.clone(), page2.clone(), row2.clone(), uid2.clone());
            ask_playlist_name(&anchor, move |name| {
                glib::spawn_future_local(async move {
                    let client = p.client.lock().unwrap().clone();
                    match client.create_playlist(&uid, &name).await {
                        Ok(kind) => {
                            // у нового плейлиста сразу открывается его страница
                            let pl = Playlist { uid: uid.clone(), kind, title: name, track_count: Some(0), ..Default::default() };
                            open_playlist(&p, &stack, &page, &pl);
                            fill_playlists(&p, &stack, &page, &row, &uid).await
                        }
                        Err(e) => p.emit(PlayerUiEvent::Error(format!("Не удалось создать плейлист: {}", e))),
                    }
                });
            });
        }
    });
    row.append(&new_tile);
    for pl in &playlists {
        row.append(&playlist_tile(p, stack, page, pl, TILE_SIZE));
    }
}
