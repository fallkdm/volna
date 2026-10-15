use std::cell::Cell;
use std::rc::Rc;

use gtk4::prelude::*;
use libadwaita::ActionRow;
use gtk4::{
    glib, Align, Box as GtkBox, Button, FlowBox, Label, ListBox, Orientation, ScrolledWindow, SearchEntry,
    SelectionMode, Stack, ToggleButton,
};

use super::collection::{
    album_tile, artist_tile, clear_box, item_row, open_album, open_artist, open_playlist, open_track, playlist_tile, section_title, track_row, DetailPage,
};
use crate::api::history::{clear_history, load_history, push_history, HistoryItem};
use crate::api::models::{Album, SearchResult, Track};
use crate::player::PlayerManager;

pub struct SearchViewWidgets {
    pub container: GtkBox,
    /// Открыть страницу артиста / трека (из плеера); вкладку «Поиск» переключает вызывающий
    pub show_artist: Rc<dyn Fn(String)>,
    pub show_track: Rc<dyn Fn(Track)>,
    pub show_album: Rc<dyn Fn(Album)>,
}

// Порядок совпадает с секциями: Треки, Исполнители, Альбомы, Плейлисты
const FILTERS: [&str; 5] = ["Все", "Треки", "Исполнители", "Альбомы", "Плейлисты"];
const TILE_SIZE: i32 = 150;
/// В режиме «Все» показываем верх выдачи: 6 треков и по одному ряду плиток
const ALL_TRACKS_LIMIT: usize = 6;
const ALL_TILES_LIMIT: usize = 7;

/// Секция выдачи: заголовок и содержимое; показывается, если есть элементы и подходит фильтр
struct Section {
    root: GtkBox,
    has_items: Cell<bool>,
}

impl Section {
    fn new(title: &str, content: &impl IsA<gtk4::Widget>) -> Self {
        let root = GtkBox::new(Orientation::Vertical, 10);
        root.append(&section_title(title));
        root.append(content);
        root.set_visible(false);
        Self { root, has_items: Cell::new(false) }
    }
}

/// Строка истории: действие то же, что у результата поиска, и запись поднимается наверх
fn history_row(p: &PlayerManager, stack: &Stack, page: &Rc<DetailPage>, item: &HistoryItem) -> ActionRow {
    let (p, stack, page, it) = (p.clone(), stack.clone(), page.clone(), item.clone());
    match item {
        HistoryItem::Track(t) => track_row(t, move || {
            push_history(it.clone());
            if let HistoryItem::Track(t) = &it {
                p.play_single_track(t.clone());
            }
        }),
        HistoryItem::Artist(a) => item_row(&a.name, "Исполнитель", a.cover_url(100), true, move || {
            push_history(it.clone());
            if let HistoryItem::Artist(a) = &it {
                open_artist(&p, &stack, &page, a.id.clone());
            }
        }),
        HistoryItem::Album(a) => item_row(a.title.as_deref().unwrap_or(""), &a.artists_string(), a.cover_url(100), false, move || {
            push_history(it.clone());
            if let HistoryItem::Album(a) = &it {
                open_album(&p, &stack, &page, a);
            }
        }),
        HistoryItem::Playlist(pl) => item_row(&pl.title, "Плейлист", pl.cover_url(100), false, move || {
            push_history(it.clone());
            if let HistoryItem::Playlist(pl) = &it {
                open_playlist(&p, &stack, &page, pl);
            }
        }),
    }
}

fn grid() -> FlowBox {
    let g = FlowBox::new();
    g.set_selection_mode(SelectionMode::None);
    g.set_homogeneous(true);
    g.set_min_children_per_line(2);
    g.set_max_children_per_line(8);
    g.set_column_spacing(12);
    g.set_row_spacing(12);
    g.set_valign(Align::Start);
    g
}

pub fn create_search_view(player: &PlayerManager) -> SearchViewWidgets {
    let container = GtkBox::new(Orientation::Vertical, 0);
    container.set_margin_top(24);
    // отступ под плавающий плеер даёт само содержимое прокрутки: оно уезжает под плеер и докручивается выше него
    container.set_margin_start(32);
    container.set_margin_end(32);

    let stack = Stack::new();
    stack.set_transition_type(gtk4::StackTransitionType::Crossfade);
    container.append(&stack);

    let main = GtkBox::new(Orientation::Vertical, 14);
    stack.add_named(&main, Some("main"));

    let page = {
        let stack = stack.clone();
        DetailPage::new(player, move || stack.set_visible_child_name("main"))
    };
    stack.add_named(&page.widget, Some("detail"));

    let search_entry = SearchEntry::new();
    search_entry.set_placeholder_text(Some("Поиск треков, альбомов, плейлистов..."));
    search_entry.set_hexpand(true);
    main.append(&search_entry);

    // Фильтры-«чипы»
    let chips = GtkBox::new(Orientation::Horizontal, 8);
    let filter = Rc::new(Cell::new(0usize));
    let mut first: Option<ToggleButton> = None;
    let mut buttons = Vec::new();
    for name in FILTERS {
        let b = ToggleButton::with_label(name);
        b.add_css_class("pill");
        match &first {
            Some(f) => b.set_group(Some(f)),
            None => {
                b.set_active(true);
                first = Some(b.clone());
            }
        }
        chips.append(&b);
        buttons.push(b);
    }
    main.append(&chips);

    let status_label = Label::new(Some("Введите запрос"));
    status_label.set_halign(Align::Start);
    status_label.add_css_class("dim-label");
    main.append(&status_label);

    // История поиска: открытые результаты, показывается, пока поле пустое
    let history_box = GtkBox::new(Orientation::Vertical, 12);
    main.append(&history_box);
    let rebuild_history: Rc<dyn Fn()> = {
        let (hb, p, stack, page) = (history_box.clone(), player.clone(), stack.clone(), page.clone());
        Rc::new(move || {
            clear_box(&hb);
            let items = load_history();
            hb.set_visible(!items.is_empty());
            if items.is_empty() {
                return;
            }
            hb.append(&section_title("История поиска"));
            let columns = GtkBox::new(Orientation::Horizontal, 16);
            columns.set_homogeneous(true);
            let lists = [ListBox::new(), ListBox::new()];
            for l in &lists {
                l.add_css_class("boxed-list");
                l.set_selection_mode(SelectionMode::None);
                l.set_valign(Align::Start);
                columns.append(l);
            }
            for (i, item) in items.iter().enumerate() {
                lists[i % 2].append(&history_row(&p, &stack, &page, item));
            }
            hb.append(&columns);

            let clear = Button::with_label("Очистить историю");
            clear.add_css_class("pill");
            clear.set_halign(Align::Center);
            let hb2 = hb.clone();
            clear.connect_clicked(move |_| {
                clear_history();
                clear_box(&hb2);
                hb2.set_visible(false);
            });
            hb.append(&clear);
        })
    };
    rebuild_history();

    // Секции результатов
    let tracks_list = ListBox::new();
    tracks_list.add_css_class("boxed-list");
    tracks_list.set_selection_mode(SelectionMode::None);
    let artists_grid = grid();
    let albums_grid = grid();
    let playlists_grid = grid();
    let sections = Rc::new([
        Section::new("Треки", &tracks_list),
        Section::new("Исполнители", &artists_grid),
        Section::new("Альбомы", &albums_grid),
        Section::new("Плейлисты", &playlists_grid),
    ]);

    let results = GtkBox::new(Orientation::Vertical, 24);
    for s in sections.iter() {
        results.append(&s.root);
    }
    let scrolled = ScrolledWindow::new();
    scrolled.set_vexpand(true);
    results.set_margin_bottom(130);
    results.set_margin_end(14);
    scrolled.set_child(Some(&results));
    main.append(&scrolled);

    // Фильтр: 0 — всё, 1..=3 — одна секция
    let apply_filter = {
        let (filter, sections) = (filter.clone(), sections.clone());
        let (tracks_list, artists_grid, albums_grid, playlists_grid) =
            (tracks_list.clone(), artists_grid.clone(), albums_grid.clone(), playlists_grid.clone());
        Rc::new(move || {
            for (i, s) in sections.iter().enumerate() {
                let wanted = filter.get() == 0 || filter.get() == i + 1;
                s.root.set_visible(wanted && s.has_items.get());
            }
            // «Все»: только верх выдачи по каждому разделу, полные списки в своих фильтрах
            let limit = if filter.get() == 0 { Some(ALL_TRACKS_LIMIT) } else { None };
            let tile_limit = if filter.get() == 0 { Some(ALL_TILES_LIMIT) } else { None };
            let mut i = 0;
            while let Some(row) = tracks_list.row_at_index(i) {
                row.set_visible(limit.map_or(true, |n| (i as usize) < n));
                i += 1;
            }
            for grid in [&artists_grid, &albums_grid, &playlists_grid] {
                let mut i = 0;
                while let Some(child) = grid.child_at_index(i) {
                    child.set_visible(tile_limit.map_or(true, |n| (i as usize) < n));
                    i += 1;
                }
            }
        })
    };
    for (i, b) in buttons.iter().enumerate() {
        let (filter, apply) = (filter.clone(), apply_filter.clone());
        b.connect_toggled(move |b| {
            if b.is_active() {
                filter.set(i);
                apply();
            }
        });
    }

    // Обработка поискового запроса
    {
        let player = player.clone();
        let (stack, page) = (stack.clone(), page.clone());
        let (history_box, rebuild_history) = (history_box.clone(), rebuild_history.clone());
        let seq = Rc::new(Cell::new(0u64));
        let do_search = Rc::new(move |query: String| {
            // номер запроса: ответ на устаревший запрос (пока печатали) отбрасывается
            let my_seq = seq.get() + 1;
            seq.set(my_seq);
            let seq = seq.clone();
            if query.is_empty() {
                status_label.set_text("Введите запрос");
                rebuild_history();
                tracks_list.remove_all();
                artists_grid.remove_all();
                albums_grid.remove_all();
                playlists_grid.remove_all();
                for s in sections.iter() {
                    s.has_items.set(false);
                }
                apply_filter();
                return;
            }
            history_box.set_visible(false);
            status_label.set_text(&format!("Поиск: «{}»...", query));

            let (p, stack, page, sections, apply) =
                (player.clone(), stack.clone(), page.clone(), sections.clone(), apply_filter.clone());
            let (status, tracks_list, artists_grid, albums_grid, playlists_grid) = (
                status_label.clone(),
                tracks_list.clone(),
                artists_grid.clone(),
                albums_grid.clone(),
                playlists_grid.clone(),
            );

            glib::spawn_future_local(async move {
                let client = p.client.lock().unwrap().clone();
                if !client.has_token() {
                    status.set_text("Укажите токен в настройках ⚙ для поиска");
                    return;
                }
                let result: SearchResult = match client.search(&query).await {
                    Ok(r) => r,
                    Err(e) => {
                        if seq.get() == my_seq {
                            status.set_text(&format!("Ошибка поиска: {}", e));
                        }
                        return;
                    }
                };
                if seq.get() != my_seq {
                    return;
                }

                tracks_list.remove_all();
                artists_grid.remove_all();
                albums_grid.remove_all();
                playlists_grid.remove_all();

                let tracks = result.tracks.map(|s| s.results).unwrap_or_default();
                let artists = result.artists.map(|s| s.results).unwrap_or_default();
                let albums = result.albums.map(|s| s.results).unwrap_or_default();
                let playlists = result.playlists.map(|s| s.results).unwrap_or_default();

                // Клик по треку: он играет, дальше «Волна по треку»
                for track in &tracks {
                    let (p, t) = (p.clone(), track.clone());
                    tracks_list.append(&track_row(track, move || {
                        push_history(HistoryItem::Track(t.clone()));
                        p.play_single_track(t.clone());
                    }));
                }
                for artist in &artists {
                    let tile = artist_tile(&p, &stack, &page, artist, TILE_SIZE);
                    let a = artist.clone();
                    tile.connect_clicked(move |_| push_history(HistoryItem::Artist(a.clone())));
                    artists_grid.append(&tile);
                }
                for album in &albums {
                    let tile = album_tile(&p, &stack, &page, album, TILE_SIZE);
                    let a = album.clone();
                    tile.connect_clicked(move |_| push_history(HistoryItem::Album(a.clone())));
                    albums_grid.append(&tile);
                }
                for pl in &playlists {
                    let tile = playlist_tile(&p, &stack, &page, pl, TILE_SIZE);
                    let pl = pl.clone();
                    tile.connect_clicked(move |_| push_history(HistoryItem::Playlist(pl.clone())));
                    playlists_grid.append(&tile);
                }

                sections[0].has_items.set(!tracks.is_empty());
                sections[1].has_items.set(!artists.is_empty());
                sections[2].has_items.set(!albums.is_empty());
                sections[3].has_items.set(!playlists.is_empty());
                apply();

                if tracks.is_empty() && artists.is_empty() && albums.is_empty() && playlists.is_empty() {
                    status.set_text("Ничего не найдено");
                } else {
                    status.set_text(&format!(
                        "Найдено: треков {}, исполнителей {}, альбомов {}, плейлистов {}",
                        tracks.len(),
                        artists.len(),
                        albums.len(),
                        playlists.len()
                    ));

                }
            });
        });

        // Поиск по мере ввода (SearchEntry сам выдерживает паузу) и сразу по Enter
        let ds = do_search.clone();
        search_entry.connect_search_changed(move |e| ds(e.text().trim().to_string()));
        search_entry.connect_activate(move |e| do_search(e.text().trim().to_string()));
    }

    {
        let (p, stack, weak) = (player.clone(), stack.clone(), Rc::downgrade(&page));
        *page.link_handler.borrow_mut() = Some(Rc::new(move |uri: &str| {
            let Some(page) = weak.upgrade() else { return };
            if let Some(id) = uri.strip_prefix("artist:") {
                open_artist(&p, &stack, &page, id.to_string());
            } else if let Some(id) = uri.strip_prefix("album:") {
                let album = page.link_albums.borrow().iter().find(|a| a.id == id).cloned();
                if let Some(album) = album {
                    open_album(&p, &stack, &page, &album);
                }
            }
        }));
    }

    let show_artist: Rc<dyn Fn(String)> = {
        let (p, stack, page) = (player.clone(), stack.clone(), page.clone());
        Rc::new(move |id| open_artist(&p, &stack, &page, id))
    };
    let show_track: Rc<dyn Fn(Track)> = {
        let (p, stack, page) = (player.clone(), stack.clone(), page.clone());
        Rc::new(move |t| open_track(&p, &stack, &page, t))
    };

    let show_album: Rc<dyn Fn(Album)> = {
        let (p, stack, page) = (player.clone(), stack.clone(), page.clone());
        Rc::new(move |a| open_album(&p, &stack, &page, &a))
    };

    SearchViewWidgets { container, show_artist, show_track, show_album }
}
