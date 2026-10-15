use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{
    glib, Align, Box as GtkBox, Button, Label, ListBox, Orientation, ScrolledWindow,
    SelectionMode, Stack,
};

use super::collection::{album_tile, clear_box, playlist_tile, section_title, tile_row, track_row, DetailPage};
use crate::player::PlayerManager;

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

                if let Ok(tracks) = client.get_liked_tracks(&uid).await {
                    liked_count.set_text(&format!("{} треков", tracks.len()));
                    liked_list.remove_all();
                    for (i, track) in tracks.iter().take(LIKED_PREVIEW).enumerate() {
                        let (p, queue) = (p.clone(), tracks.clone());
                        liked_list.append(&track_row(track, move || p.set_queue_and_play(queue.clone(), i)));
                    }
                    liked_box.set_visible(!tracks.is_empty());
                    *liked_tracks.borrow_mut() = tracks;
                }

                if let Ok(playlists) = client.get_user_playlists(&uid).await {
                    clear_box(&playlists_row);
                    for pl in &playlists {
                        playlists_row.append(&playlist_tile(&p, &stack, &page, pl, TILE_SIZE));
                    }
                    playlists_box.set_visible(!playlists.is_empty());
                }

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
