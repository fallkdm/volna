use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Image, Label, ListBox, Orientation, ScrolledWindow, SearchEntry};
use libadwaita::prelude::*;
use libadwaita::ActionRow;

use crate::api::models::Track;
use crate::player::PlayerManager;

pub struct SearchViewWidgets {
    pub container: GtkBox,
    pub search_entry: SearchEntry,
    pub results_list: ListBox,
    pub status_label: Label,
}

pub fn create_search_view(player: &PlayerManager) -> SearchViewWidgets {
    let container = GtkBox::new(Orientation::Vertical, 16);
    container.set_margin_top(24);
    container.set_margin_bottom(110);
    container.set_margin_start(32);
    container.set_margin_end(32);

    let search_entry = SearchEntry::new();
    search_entry.set_placeholder_text(Some("Поиск треков, артистов, альбомов..."));
    search_entry.set_hexpand(true);
    container.append(&search_entry);

    let status_label = Label::new(Some("Популярные треки"));
    status_label.set_halign(Align::Start);
    status_label.add_css_class("heading");
    container.append(&status_label);

    let scrolled = ScrolledWindow::new();
    scrolled.set_vexpand(true);

    let results_list = ListBox::new();
    results_list.add_css_class("boxed-list");

    // Начальный список популярных треков
    let initial_tracks = [
        Track {
            id: "101".to_string(),
            title: "Midnight City".to_string(),
            artists: vec![crate::api::models::Artist { id: "1".into(), name: "M83".into(), cover: None }],
            albums: vec![crate::api::models::Album { id: "1".into(), title: Some("Hurry Up, We're Dreaming".into()), year: Some(2011), cover_uri: None }],
            duration_ms: Some(243000),
            cover_uri: None,
            og_image: None,
            available: Some(true),
        },
        Track {
            id: "102".to_string(),
            title: "Star Guitar".to_string(),
            artists: vec![crate::api::models::Artist { id: "2".into(), name: "The Chemical Brothers".into(), cover: None }],
            albums: vec![crate::api::models::Album { id: "2".into(), title: Some("Come with Us".into()), year: Some(2002), cover_uri: None }],
            duration_ms: Some(239000),
            cover_uri: None,
            og_image: None,
            available: Some(true),
        },
        Track {
            id: "103".to_string(),
            title: "Resonance".to_string(),
            artists: vec![crate::api::models::Artist { id: "3".into(), name: "HOME".into(), cover: None }],
            albums: vec![crate::api::models::Album { id: "3".into(), title: Some("Odyssey".into()), year: Some(2014), cover_uri: None }],
            duration_ms: Some(212000),
            cover_uri: None,
            og_image: None,
            available: Some(true),
        },
    ];

    for track in initial_tracks {
        let row = create_track_row(&track, player);
        results_list.append(&row);
    }

    scrolled.set_child(Some(&results_list));
    container.append(&scrolled);

    // Обработка поискового запроса
    {
        let player_clone = player.clone();
        let list_weak = results_list.downgrade();
        let status_weak = status_label.downgrade();

        search_entry.connect_activate(move |entry| {
            let query = entry.text().trim().to_string();
            if query.is_empty() {
                return;
            }

            let p = player_clone.clone();
            let lw = list_weak.clone();
            let sw = status_weak.clone();

            if let Some(s) = sw.upgrade() {
                s.set_text(&format!("Поиск: «{}»...", query));
            }

            gtk4::glib::spawn_future_local(async move {
                let client = p.client.lock().unwrap().clone();
                let results = if client.has_token() {
                    client.search(&query).await.unwrap_or_default()
                } else {
                    Vec::new()
                };

                if let Some(list) = lw.upgrade() {
                    while let Some(child) = list.first_child() {
                        list.remove(&child);
                    }

                    if results.is_empty() {
                        if let Some(s) = sw.upgrade() {
                            s.set_text("Ничего не найдено (или требуется авторизация)");
                        }
                    } else {
                        if let Some(s) = sw.upgrade() {
                            s.set_text(&format!("Найдено треков: {}", results.len()));
                        }
                        for track in results {
                            let row = create_track_row(&track, &p);
                            list.append(&row);
                        }
                    }
                }
            });
        });
    }

    SearchViewWidgets {
        container,
        search_entry,
        results_list,
        status_label,
    }
}

fn create_track_row(track: &Track, player: &PlayerManager) -> ActionRow {
    let row = ActionRow::builder()
        .title(&track.title)
        .subtitle(&track.artists_string())
        .activatable(true)
        .build();

    let play_icon = Image::from_icon_name("media-playback-start-symbolic");
    row.add_prefix(&play_icon);

    let dur_label = Label::new(Some(&track.formatted_duration()));
    dur_label.add_css_class("dim-label");
    row.add_suffix(&dur_label);

    let player_clone = player.clone();
    let track_clone = track.clone();
    row.connect_activated(move |_| {
        player_clone.play_single_track(track_clone.clone());
    });

    row
}
