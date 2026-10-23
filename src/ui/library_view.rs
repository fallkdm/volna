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

    // Мне нравится: превью и переход к полному списку
    let liked_title = Button::with_label("Мне нравится");
    liked_title.add_css_class("flat");
    liked_title.add_css_class("section-title");
    liked_title.set_halign(Align::Start);
    let liked_count = Label::new(None);
    liked_count.set_halign(Align::Start);
    liked_count.add_css_class("dim-label");
    let liked_list = ListBox::new();
    liked_list.add_css_class("boxed-list");
    liked_list.set_selection_mode(SelectionMode::None);
    let liked_all = Button::with_label("Показать все");
    liked_all.add_css_class("pill");
    liked_all.set_halign(Align::Start);

    let liked_box = GtkBox::new(Orientation::Vertical, 8);
    liked_box.append(&liked_title);
    liked_box.append(&liked_count);
    liked_box.append(&liked_list);
    liked_box.append(&liked_all);
    liked_box.set_visible(false);
    main.append(&liked_box);

    let liked_tracks: Rc<RefCell<Vec<crate::api::models::Track>>> = Rc::default();
    for open_liked in [liked_title.clone(), liked_all.clone()] {
        let (page, stack, tracks) = (page.clone(), stack.clone(), liked_tracks.clone());
        open_liked.connect_clicked(move |_| {
            let tracks = tracks.borrow().clone();
            let sub = format!("{} треков", tracks.len());
            page.show("Коллекция", "Мне нравится", &sub, None, tracks);
            stack.set_visible_child_name("detail");
        });
    }

    let rx = player.subscribe();
    {
        let (player, page, stack, liked_tracks, liked_count, liked_list, liked_box) = (
            player.clone(),
            page.clone(),
            stack.clone(),
            liked_tracks.clone(),
            liked_count.clone(),
            liked_list.clone(),
            liked_box.clone(),
        );
        glib::spawn_future_local(async move {
            while let Ok(event) = rx.recv().await {
                if let PlayerUiEvent::TrackLiked { track, liked } = event {
                    page.handle_track_liked(&track, liked);

                    if liked {
                        if !liked_tracks.borrow().iter().any(|t| t.id == track.id) {
                            liked_tracks.borrow_mut().insert(0, track.clone());
                        }
                    } else {
                        liked_tracks.borrow_mut().retain(|t| t.id != track.id);
                    }

                    let count = liked_tracks.borrow().len();
                    liked_count.set_text(&format!("{} треков", count));

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
                    liked_box.set_visible(!liked_tracks.borrow().is_empty());
                }
            }
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

    // Данные подгружаются при каждом показе вкладки
    {
        let player = player.clone();
        let (stack, page) = (stack.clone(), page.clone());
        container.connect_map(move |_| {
            let (p, stack, page, status) = (player.clone(), stack.clone(), page.clone(), status.clone());
            let (liked_box, liked_count, liked_list, liked_tracks) =
                (liked_box.clone(), liked_count.clone(), liked_list.clone(), liked_tracks.clone());
            let (playlists_box, playlists_row) = (playlists_box.clone(), playlists_row.clone());
            let (albums_box, albums_row) = (albums_box.clone(), albums_row.clone());
            let (offline_box, offline_list) = (offline_box.clone(), offline_list.clone());

            // офлайн-треки показываются сразу и без сети
            offline_list.remove_all();
            let saved = crate::cache::list_offline();
            for (i, track) in saved.iter().enumerate() {
                let (p, queue) = (p.clone(), saved.clone());
                let row = track_row(track, { let (p, queue) = (p.clone(), queue.clone()); move || p.set_queue_and_play_from(queue.clone(), i, Some(PlaySource::new("Загруженные", Some(SourceTarget::Library)))) });
                row.add_suffix(&track_actions(&p, Some((&stack, &page)), track));
                offline_list.append(&row);
            }
            offline_box.set_visible(!saved.is_empty());

            glib::spawn_future_local(async move {
                let client = p.client.lock().unwrap().clone();
                if !client.has_token() {
                    status.set_text("Укажите токен в настройках ⚙ для доступа к коллекции");
                    status.set_visible(true);
                    return;
                }
                let uid = match client.get_uid().await {
                    Ok(uid) => uid,
                    Err(e) => {
                        status.set_text(&format!("Ошибка аккаунта: {}", e));
                        status.set_visible(true);
                        return;
                    }
                };
                status.set_text("");
                status.set_visible(false);

                ensure_liked(&p).await;
                if let Ok(tracks) = client.get_liked_tracks(&uid).await {
                    liked_count.set_text(&format!("{} треков", tracks.len()));
                    liked_list.remove_all();
                    for (i, track) in tracks.iter().take(LIKED_PREVIEW).enumerate() {
                        let (p, queue) = (p.clone(), tracks.clone());
                        let row = track_row(track, { let (p, queue) = (p.clone(), queue.clone()); move || p.set_queue_and_play_from(queue.clone(), i, Some(PlaySource::new("Мне нравится", Some(SourceTarget::Library)))) });
                        row.add_suffix(&track_actions(&p, Some((&stack, &page)), track));
                        liked_list.append(&row);
                    }
                    liked_box.set_visible(!tracks.is_empty());
                    *liked_tracks.borrow_mut() = tracks;
                }

                {
                    let (p, stack2, page2, row, uid) = (p.clone(), stack.clone(), page.clone(), playlists_row.clone(), uid.clone());
                    page.set_on_removed(move || {
                        let (p, stack, page, row, uid) = (p.clone(), stack2.clone(), page2.clone(), row.clone(), uid.clone());
                        glib::spawn_future_local(async move { fill_playlists(&p, &stack, &page, &row, &uid).await });
                    });
                }
                fill_playlists(&p, &stack, &page, &playlists_row, &uid).await;
                playlists_box.set_visible(true);

                if let Ok(albums) = client.get_liked_albums(&uid).await {
                    clear_box(&albums_row);
                    for album in &albums {
                        albums_row.append(&album_tile(&p, &stack, &page, album, TILE_SIZE));
                    }
                    albums_box.set_visible(!albums.is_empty());
                }
            });
        });
    }

    LibraryViewWidgets { container }
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
