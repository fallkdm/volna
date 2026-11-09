use std::cell::RefCell;
use std::rc::Rc;

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
}

const TILE_SIZE: i32 = 160;
const LIKED_PREVIEW: usize = 6;

pub fn create_library_view(player: &PlayerManager) -> LibraryViewWidgets {
    let container = GtkBox::new(Orientation::Vertical, 0);
    container.set_margin_top(24);
    // отступ под плавающий плеер даёт само содержимое прокрутки: оно уезжает под плеер и докручивается выше него
    container.set_margin_start(32);
    container.set_margin_end(32);

    let stack = Stack::new();
    stack.set_transition_type(gtk4::StackTransitionType::Crossfade);
    container.append(&stack);

    let page = {
        let stack = stack.clone();
        DetailPage::new(player, move || stack.set_visible_child_name("main"))
    };
    stack.add_named(&page.widget, Some("detail"));
    page.set_nav(&stack);

    let main = GtkBox::new(Orientation::Vertical, 26);
    let heading = Label::new(Some("Коллекция"));
    heading.set_halign(Align::Start);
    heading.add_css_class("detail-title");
    main.append(&heading);

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
    main.set_margin_bottom(130);
    main.set_margin_end(14); // полоса прокрутки не наезжает на строки
    scrolled.set_child(Some(&main));
    stack.add_named(&scrolled, Some("main"));
    stack.set_visible_child_name("main");

    let reload_library: Rc<dyn Fn()> = {
        let (p, stack, page, status, yt_login_btn) = (player.clone(), stack.clone(), page.clone(), status.clone(), yt_login_btn.clone());
        let (liked_box, liked_count, liked_empty_msg, liked_list, liked_all, liked_tracks, liked_total) =
            (liked_box.clone(), liked_count.clone(), liked_empty_msg.clone(), liked_list.clone(), liked_all.clone(), liked_tracks.clone(), liked_total.clone());
        let (playlists_box, playlists_row) = (playlists_box.clone(), playlists_row.clone());
        let (albums_box, albums_row) = (albums_box.clone(), albums_row.clone());
        let (offline_box, offline_list) = (offline_box.clone(), offline_list.clone());

        Rc::new(move || {
            let (p, stack, page, status, yt_login_btn) = (p.clone(), stack.clone(), page.clone(), status.clone(), yt_login_btn.clone());
            let (liked_box, liked_count, liked_empty_msg, liked_list, liked_all, liked_tracks, liked_total) =
                (liked_box.clone(), liked_count.clone(), liked_empty_msg.clone(), liked_list.clone(), liked_all.clone(), liked_tracks.clone(), liked_total.clone());
            let (playlists_box, playlists_row) = (playlists_box.clone(), playlists_row.clone());
            let (albums_box, albums_row) = (albums_box.clone(), albums_row.clone());
            let (offline_box, offline_list) = (offline_box.clone(), offline_list.clone());

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
            for (i, track) in saved.iter().enumerate() {
                let (p, queue) = (p.clone(), saved.clone());
                let row = track_row(track, {
                    let (p, queue) = (p.clone(), queue.clone());
                    move || {
                        p.set_queue_and_play_from(
                            queue.clone(),
                            i,
                            Some(PlaySource::new(
                                "Загруженные",
                                Some(SourceTarget::Library),
                            )),
                        )
                    }
                });
                row.add_suffix(&track_actions(&p, Some((&stack, &page)), track));
                offline_list.append(&row);
            }
            offline_box.set_visible(!saved.is_empty());

            glib::spawn_future_local(async move {
                let provider = p.providers.current();
                let is_yandex = provider.provider_id() == crate::api::provider::ProviderId::Yandex;
                let is_yt = provider.provider_id() == crate::api::provider::ProviderId::YouTubeMusic;

                if is_yandex && !provider.is_authenticated() {
                    status.set_text("Укажите токен Яндекс Музыки в настройках ⚙ для доступа к коллекции");
                    status.set_visible(true);
                    yt_login_btn.set_visible(false);
                    liked_box.set_visible(false);
                    playlists_box.set_visible(false);
                    albums_box.set_visible(false);
                    return;
                }

                if is_yt && !provider.is_authenticated() {
                    status.set_text("Войдите в Google Аккаунт для доступа к вашей медиатеке YouTube Music 🔴");
                    status.set_visible(true);
                    yt_login_btn.set_visible(true);
                    liked_box.set_visible(false);
                    playlists_box.set_visible(false);
                    albums_box.set_visible(false);
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
                if is_yt && provider.is_authenticated() && library_empty {
                    status.set_text(
                        "Не удалось загрузить медиатеку: похоже, сессия Google истекла. Войдите заново через карточку сервиса → «Аккаунт»",
                    );
                    status.set_visible(true);
                    yt_login_btn.set_visible(true);
                }

                if let Ok(tracks) = liked_res {
                    *liked_tracks.borrow_mut() = tracks.clone();
                    if !tracks.is_empty() {
                        liked_empty_msg.set_visible(false);
                        let count_str = match total_cnt_opt {
                            Some(total) if total > tracks.len() as u32 => format!("{} треков ({} доступно)", total, tracks.len()),
                            Some(total) if total > 0 => format!("{} треков", total),
                            _ => format!("{} треков", tracks.len()),
                        };
                        liked_count.set_text(&count_str);
                        liked_count.set_visible(true);
                        liked_list.remove_all();
                        for (i, track) in tracks.iter().take(LIKED_PREVIEW).enumerate() {
                            let (p, queue) = (p.clone(), tracks.clone());
                            let row = track_row(track, { let (p, queue) = (p.clone(), queue.clone()); move || p.set_queue_and_play_from(queue.clone(), i, Some(PlaySource::new("Мне нравится", Some(SourceTarget::Library)))) });
                            row.add_suffix(&track_actions(&p, Some((&stack, &page)), track));
                            liked_list.append(&row);
                        }
                        liked_list.set_visible(true);
                        liked_all.set_visible(tracks.len() > LIKED_PREVIEW);
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
                    clear_box(&playlists_row);

                    if is_yandex {
                        let uid = {
                            let client = p.client.lock().unwrap().clone();
                            client.get_uid().await.unwrap_or_else(|_| "user".to_string())
                        };
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
                                                let pl = Playlist { uid: uid.clone(), kind, title: name, track_count: Some(0), og_image: None };
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
                    clear_box(&albums_row);
                    for album in &albums {
                        albums_row.append(&album_tile(&p, &stack, &page, album, TILE_SIZE));
                    }
                    albums_box.set_visible(!albums.is_empty());
                } else {
                    albums_box.set_visible(false);
                }
            });
        })
    };

    {
        let p = player.clone();
        let btn = yt_login_btn.clone();
        let reload = reload_library.clone();
        yt_login_btn.connect_clicked(move |_| {
            let reload = reload.clone();
            super::preferences_dialog::show_ytmusic_auth_dialog(&btn, &p, move || {
                reload();
            });
        });
    }

    let rx = player.subscribe();
    {
        let (player, page, stack, liked_tracks, liked_count, liked_empty_msg, liked_list, liked_all, liked_box, liked_total, reload) = (
            player.clone(),
            page.clone(),
            stack.clone(),
            liked_tracks.clone(),
            liked_count.clone(),
            liked_empty_msg.clone(),
            liked_list.clone(),
            liked_all.clone(),
            liked_box.clone(),
            liked_total.clone(),
            reload_library.clone(),
        );
        glib::spawn_future_local(async move {
            while let Ok(event) = rx.recv().await {
                match event {
                    PlayerUiEvent::ProviderChanged(_) => {
                        reload();
                    }
                    PlayerUiEvent::TrackLiked { track, liked } => {
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

    // Данные подгружаются при каждом показе вкладки
    {
        let reload = reload_library.clone();
        container.connect_map(move |_| {
            reload();
        });
    }

    LibraryViewWidgets {
        container,
        open_liked: open_liked_fn,
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
                            let pl = Playlist { uid: uid.clone(), kind, title: name, track_count: Some(0), og_image: None };
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
