use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Label, ListBox, Orientation, ScrolledWindow, SearchEntry};
use libadwaita::prelude::*;
use libadwaita::ActionRow;

use crate::player::PlayerManager;

pub struct SearchViewWidgets {
    pub container: GtkBox,
    pub search_entry: SearchEntry,
    pub results_list: ListBox,
}

pub fn create_search_view(_player: &PlayerManager) -> SearchViewWidgets {
    let container = GtkBox::new(Orientation::Vertical, 16);
    container.set_margin_top(24);
    container.set_margin_bottom(110);
    container.set_margin_start(32);
    container.set_margin_end(32);

    let search_entry = SearchEntry::new();
    search_entry.set_placeholder_text(Some("Треки, артисты, альбомы..."));
    search_entry.set_hexpand(true);
    container.append(&search_entry);

    let results_label = Label::new(Some("Популярное сейчас"));
    results_label.set_halign(Align::Start);
    results_label.add_css_class("heading");
    container.append(&results_label);

    let scrolled = ScrolledWindow::new();
    scrolled.set_vexpand(true);

    let results_list = ListBox::new();
    results_list.add_css_class("boxed-list");

    let sample_items = [
        ("Midnight City", "M83", "04:03"),
        ("Star Guitar", "The Chemical Brothers", "03:59"),
        ("Resonance", "HOME", "03:32"),
        ("Intro", "The xx", "02:08"),
        ("Nightcall", "Kavinsky", "04:19"),
        ("Get Lucky", "Daft Punk feat. Pharrell Williams", "04:08"),
    ];

    for (title, artist, duration) in sample_items {
        let row = ActionRow::builder()
            .title(title)
            .subtitle(artist)
            .build();
        
        let dur_label = Label::new(Some(duration));
        dur_label.add_css_class("dim-label");
        row.add_suffix(&dur_label);
        
        results_list.append(&row);
    }

    scrolled.set_child(Some(&results_list));
    container.append(&scrolled);

    SearchViewWidgets {
        container,
        search_entry,
        results_list,
    }
}
