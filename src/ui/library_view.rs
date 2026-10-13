use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Image, Label, ListBox, Orientation, ScrolledWindow};
use libadwaita::prelude::*;
use libadwaita::ActionRow;

use crate::player::PlayerManager;

pub struct LibraryViewWidgets {
    pub container: GtkBox,
    pub library_list: ListBox,
}

pub fn create_library_view(player: &PlayerManager) -> LibraryViewWidgets {
    let container = GtkBox::new(Orientation::Vertical, 16);
    container.set_margin_top(24);
    container.set_margin_bottom(110);
    container.set_margin_start(32);
    container.set_margin_end(32);

    let heading = Label::new(Some("Ваша медиатека"));
    heading.set_halign(Align::Start);
    heading.add_css_class("heading");
    container.append(&heading);

    let scrolled = ScrolledWindow::new();
    scrolled.set_vexpand(true);

    let library_list = ListBox::new();
    library_list.add_css_class("boxed-list");

    // Карточка 1: Мне нравится (с реальной загрузкой любимых треков)
    let favorites_row = ActionRow::builder()
        .title("Мне нравится")
        .subtitle("Любимые треки профиля (нажмите для воспроизведения)")
        .activatable(true)
        .build();

    let fav_icon = Image::from_icon_name("emblem-favorite-symbolic");
    fav_icon.set_pixel_size(24);
    favorites_row.add_prefix(&fav_icon);

    let arrow1 = Image::from_icon_name("media-playback-start-symbolic");
    favorites_row.add_suffix(&arrow1);

    {
        let p = player.clone();
        favorites_row.connect_activated(move |_| {
            let p_clone = p.clone();
            p.emit(crate::player::PlayerUiEvent::StatusMessage(
                "Загружаем треки «Мне нравится»...".into(),
            ));

            tokio::spawn(async move {
                let client = p_clone.client.lock().unwrap().clone();
                if !client.has_token() {
                    p_clone.emit(crate::player::PlayerUiEvent::StatusMessage(
                        "Укажите токен в настройках ⚙ для доступа к коллекции".into(),
                    ));
                    return;
                }

                match client.get_account_status().await {
                    Ok(acc) => {
                        let uid = acc.account.map(|a| a.uid).unwrap_or_default();
                        if uid.is_empty() {
                            p_clone.emit(crate::player::PlayerUiEvent::Error(
                                "Не удалось определить UID пользователя".into(),
                            ));
                            return;
                        }

                        match client.get_liked_tracks(&uid).await {
                            Ok(tracks) => {
                                if tracks.is_empty() {
                                    p_clone.emit(crate::player::PlayerUiEvent::StatusMessage(
                                        "Список «Мне нравится» пуст".into(),
                                    ));
                                } else {
                                    p_clone.emit(crate::player::PlayerUiEvent::StatusMessage(
                                        format!("Загружено {} любимых треков ❤️", tracks.len()),
                                    ));
                                    p_clone.set_queue_and_play(tracks, 0);
                                }
                            }
                            Err(e) => {
                                p_clone.emit(crate::player::PlayerUiEvent::Error(
                                    format!("Ошибка получения треков: {}", e),
                                ));
                            }
                        }
                    }
                    Err(e) => {
                        p_clone.emit(crate::player::PlayerUiEvent::Error(
                            format!("Ошибка аккаунта: {}", e),
                        ));
                    }
                }
            });
        });
    }

    library_list.append(&favorites_row);

    // Другие подборки
    let other_items = [
        ("Плейлист дня", "Персональная ежедневная подборка", "media-playlist-repeat-symbolic"),
        ("Премьера", "Свежие релизы интересных артистов", "starred-symbolic"),
        ("Дежавю", "Треки, которые вы давно не слушали", "view-refresh-symbolic"),
        ("Тайник", "Забытые любимые треки", "folder-music-symbolic"),
    ];

    for (title, desc, icon_name) in other_items {
        let row = ActionRow::builder()
            .title(title)
            .subtitle(desc)
            .activatable(true)
            .build();

        let icon = Image::from_icon_name(icon_name);
        icon.set_pixel_size(24);
        row.add_prefix(&icon);

        let arrow = Image::from_icon_name("go-next-symbolic");
        row.add_suffix(&arrow);

        let p = player.clone();
        let t_name = title.to_string();
        row.connect_activated(move |_| {
            p.emit(crate::player::PlayerUiEvent::StatusMessage(
                format!("Подборка: {}", t_name),
            ));
        });

        library_list.append(&row);
    }

    scrolled.set_child(Some(&library_list));
    container.append(&scrolled);

    LibraryViewWidgets {
        container,
        library_list,
    }
}
