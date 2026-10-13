use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Image, Label, ListBox, Orientation, ScrolledWindow};
use libadwaita::prelude::*;
use libadwaita::ActionRow;

use crate::player::PlayerManager;

pub struct LibraryViewWidgets {
    pub container: GtkBox,
    pub library_list: ListBox,
}

pub fn create_library_view(_player: &PlayerManager) -> LibraryViewWidgets {
    let container = GtkBox::new(Orientation::Vertical, 16);
    container.set_margin_top(24);
    container.set_margin_bottom(110);
    container.set_margin_start(32);
    container.set_margin_end(32);

    let heading = Label::new(Some("Ваша коллекция"));
    heading.set_halign(Align::Start);
    heading.add_css_class("heading");
    container.append(&heading);

    let scrolled = ScrolledWindow::new();
    scrolled.set_vexpand(true);

    let library_list = ListBox::new();
    library_list.add_css_class("boxed-list");

    let items = [
        ("Мне нравится", "148 треков", "emblem-favorite-symbolic"),
        ("Плейлист дня", "Обновляется ежедневно", "media-playlist-repeat-symbolic"),
        ("Премьера", "Новинки ваших артистов", "starred-symbolic"),
        ("Дежавю", "Треки, которые вы давно не слушали", "view-refresh-symbolic"),
        ("Тайник", "Забытые любимые треки", "folder-music-symbolic"),
    ];

    for (title, desc, icon_name) in items {
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

        library_list.append(&row);
    }

    scrolled.set_child(Some(&library_list));
    container.append(&scrolled);

    LibraryViewWidgets {
        container,
        library_list,
    }
}
