//! Общие виджеты коллекций: обложки, плитки, строки треков и страница альбома/плейлиста.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{
    gdk, glib, Align, Box as GtkBox, Button, Image, Label, ListBox, Orientation, Overflow,
    ScrolledWindow, Stack,
};
use libadwaita::prelude::*;
use libadwaita::ActionRow;

use crate::api::models::{Album, Artist, Playlist, Track};
use crate::player::{PlayerManager, PlayerUiEvent};

thread_local! {
    static COVERS: RefCell<HashMap<String, gdk::Texture>> = RefCell::new(HashMap::new());
}

/// Загружает обложку по URL в `Image` (кэш в памяти; при ошибке остаётся заглушка)
pub fn load_cover(image: &Image, url: Option<String>) {
    let Some(url) = url else { return };
    if let Some(tex) = COVERS.with(|c| c.borrow().get(&url).cloned()) {
        image.set_paintable(Some(&tex));
        return;
    }
    let weak = image.downgrade();
    glib::spawn_future_local(async move {
        let Ok(resp) = reqwest::get(&url).await else { return };
        let Ok(bytes) = resp.bytes().await else { return };
        let Ok(tex) = gdk::Texture::from_bytes(&glib::Bytes::from(&bytes[..])) else { return };
        COVERS.with(|c| c.borrow_mut().insert(url, tex.clone()));
        if let Some(img) = weak.upgrade() {
            img.set_paintable(Some(&tex));
        }
    });
}

/// Квадратная скруглённая обложка заданного размера
pub fn cover_image(size: i32, url: Option<String>) -> Image {
    let img = Image::from_icon_name("audio-x-generic-symbolic");
    img.set_pixel_size(size);
    img.set_halign(Align::Start);
    img.set_valign(Align::Start);
    img.set_overflow(Overflow::Hidden);
    img.add_css_class("cover-art");
    load_cover(&img, url);
    img
}

pub fn section_title(text: &str) -> Label {
    let l = Label::new(Some(text));
    l.set_halign(Align::Start);
    l.add_css_class("section-title");
    l
}

pub fn clear_box(b: &GtkBox) {
    while let Some(child) = b.first_child() {
        b.remove(&child);
    }
}

fn ellipsized(text: &str, css: &str) -> Label {
    let l = Label::new(Some(text));
    l.set_halign(Align::Fill);
    l.set_xalign(0.0);
    l.set_hexpand(true);
    l.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    // ширину задаёт контейнер, длинное название не растягивает плитку
    l.set_width_chars(1);
    l.set_max_width_chars(1);
    l.add_css_class(css);
    l
}

/// Плитка: обложка, название, подпись. Клик вызывает `on_click`.
pub fn tile(title: &str, subtitle: &str, url: Option<String>, size: i32, round: bool, on_click: impl Fn() + 'static) -> Button {
    let col = GtkBox::new(Orientation::Vertical, 4);
    col.set_width_request(size);
    let cover = cover_image(size, url);
    if round {
        cover.add_css_class("round-cover");
    }
    col.append(&cover);
    col.append(&ellipsized(title, "tile-title"));
    col.append(&ellipsized(subtitle, "tile-sub"));

    let btn = Button::new();
    btn.set_child(Some(&col));
    btn.add_css_class("flat");
    btn.add_css_class("cover-tile");
    btn.connect_clicked(move |_| on_click());
    btn
}

/// Строка трека с обложкой и длительностью
pub fn track_row(track: &Track, on_activate: impl Fn() + 'static) -> ActionRow {
    let row = ActionRow::builder()
        .title(glib::markup_escape_text(&track.title))
        .subtitle(glib::markup_escape_text(&track.artists_string()))
        .activatable(true)
        .build();
    row.add_prefix(&cover_image(40, track.cover_url(100)));

    if track.is_explicit() {
        let badge = Label::new(Some("E"));
        badge.add_css_class("explicit-badge");
        badge.set_valign(Align::Center);
        badge.set_tooltip_text(Some("Содержит нецензурную лексику"));
        row.add_suffix(&badge);
    }

    let dur = Label::new(Some(&track.formatted_duration()));
    dur.add_css_class("dim-label");
    row.add_suffix(&dur);

    row.connect_activated(move |_| on_activate());
    row
}

/// Строка списка с обложкой и стрелкой справа (исполнитель, альбом, плейлист)
pub fn item_row(title: &str, subtitle: &str, url: Option<String>, round: bool, on_activate: impl Fn() + 'static) -> ActionRow {
    let row = ActionRow::builder()
        .title(glib::markup_escape_text(title))
        .subtitle(glib::markup_escape_text(subtitle))
        .activatable(true)
        .build();
    let cover = cover_image(40, url);
    if round {
        cover.add_css_class("round-cover");
    }
    row.add_prefix(&cover);
    row.add_suffix(&Image::from_icon_name("go-next-symbolic"));
    row.connect_activated(move |_| on_activate());
    row
}

/// Страница содержимого альбома или плейлиста
pub struct DetailPage {
    pub widget: GtkBox,
    cover: Image,
    kind: Label,
    title: Label,
    subtitle: Label,
    list: ListBox,
    tracks: Rc<RefCell<Vec<Track>>>,
    player: PlayerManager,
    wave_btn: Button,
    wave: Rc<RefCell<Option<Wave>>>,
    /// Место под плитки (альбомы артиста) ниже списка треков
    pub extra: GtkBox,
    /// Обработчик ссылок в подписи (`artist:ID`, `album:ID`); задаётся владельцем страницы
    pub link_handler: Rc<RefCell<Option<Rc<dyn Fn(&str)>>>>,
    /// Альбомы, на которые ведут ссылки подписи
    pub link_albums: RefCell<Vec<Album>>,
    /// Справка об исполнителе из Википедии
    about: GtkBox,
    about_for: Rc<RefCell<String>>,
}

/// «Волна» по артисту или треку, запускается кнопкой на странице
pub enum Wave {
    Artist(String, String),
    Track(Track),
}

impl DetailPage {
    pub fn new(player: &PlayerManager, on_back: impl Fn() + 'static) -> Rc<Self> {
        let widget = GtkBox::new(Orientation::Vertical, 12);

        let back = Button::from_icon_name("go-previous-symbolic");
        back.add_css_class("flat");
        back.add_css_class("circular");
        back.set_halign(Align::Start);
        back.set_tooltip_text(Some("Назад"));
        back.connect_clicked(move |_| on_back());
        widget.append(&back);

        let content = GtkBox::new(Orientation::Vertical, 20);

        let cover = cover_image(180, None);
        let kind = Label::new(None);
        kind.set_halign(Align::Start);
        kind.add_css_class("dim-label");
        let title = Label::new(None);
        title.set_halign(Align::Start);
        title.set_xalign(0.0);
        title.set_wrap(true);
        title.add_css_class("detail-title");
        let subtitle = Label::new(None);
        subtitle.set_halign(Align::Start);
        subtitle.set_xalign(0.0);
        subtitle.set_wrap(true);
        subtitle.add_css_class("dim-label");
        let link_handler: Rc<RefCell<Option<Rc<dyn Fn(&str)>>>> = Rc::default();
        {
            let h = link_handler.clone();
            subtitle.connect_activate_link(move |_, uri| {
                let handler = h.borrow().clone();
                if let Some(handler) = handler {
                    handler(uri);
                }
                glib::Propagation::Stop
            });
        }

        let play = Button::with_label("Слушать");
        play.add_css_class("suggested-action");
        play.add_css_class("pill");
        play.set_halign(Align::Start);

        let info = GtkBox::new(Orientation::Vertical, 6);
        info.set_valign(Align::End);
        info.set_hexpand(true);
        info.append(&kind);
        info.append(&title);
        info.append(&subtitle);
        let wave_btn = Button::new();
        wave_btn.add_css_class("pill");
        wave_btn.set_visible(false);
        let actions = GtkBox::new(Orientation::Horizontal, 8);
        actions.append(&play);
        actions.append(&wave_btn);
        info.append(&actions);

        let hero = GtkBox::new(Orientation::Horizontal, 24);
        hero.append(&cover);
        hero.append(&info);
        content.append(&hero);
        let about = GtkBox::new(Orientation::Vertical, 6);
        about.add_css_class("about-card");
        about.set_visible(false);
        content.append(&about);

        let list = ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk4::SelectionMode::None);
        content.append(&list);
        let extra = GtkBox::new(Orientation::Vertical, 10);
        content.append(&extra);

        let scrolled = ScrolledWindow::new();
        scrolled.set_vexpand(true);
        scrolled.set_child(Some(&content));
        widget.append(&scrolled);

        let tracks: Rc<RefCell<Vec<Track>>> = Rc::default();
        {
            let (p, t) = (player.clone(), tracks.clone());
            play.connect_clicked(move |_| p.set_queue_and_play(t.borrow().clone(), 0));
        }

        let wave: Rc<RefCell<Option<Wave>>> = Rc::default();
        {
            let (p, w) = (player.clone(), wave.clone());
            wave_btn.connect_clicked(move |_| match &*w.borrow() {
                Some(Wave::Artist(id, name)) => p.play_artist_wave(id, name),
                Some(Wave::Track(t)) => p.play_single_track(t.clone()),
                None => {}
            });
        }

        Rc::new(Self { widget, cover, kind, title, subtitle, list, tracks, player: player.clone(), wave_btn, wave, extra, link_handler, link_albums: RefCell::default(), about, about_for: Rc::default() })
    }

    /// Показывает кнопку «Волна по …»
    pub fn set_wave(&self, wave: Wave) {
        self.wave_btn.set_label(match wave {
            Wave::Artist(..) => "Волна по артисту",
            Wave::Track(_) => "Волна по треку",
        });
        self.wave_btn.set_visible(true);
        *self.wave.borrow_mut() = Some(wave);
    }

    /// Подпись со ссылками: исполнители ведут на страницу артиста, альбом на страницу альбома
    pub fn set_subtitle_links(&self, artists: &[Artist], album: Option<&Album>, year: Option<u32>) {
        let parts: Vec<String> = artists
            .iter()
            .map(|a| format!("<a href=\"artist:{}\">{}</a>", a.id, glib::markup_escape_text(&a.name)))
            .collect();
        let mut text = parts.join(", ");
        if let Some(al) = album {
            if !text.is_empty() {
                text.push_str(" · ");
            }
            text.push_str(&format!("<a href=\"album:{}\">{}</a>", al.id, glib::markup_escape_text(al.title.as_deref().unwrap_or(""))));
            *self.link_albums.borrow_mut() = vec![al.clone()];
        } else if let Some(y) = year {
            if !text.is_empty() {
                text.push_str(" · ");
            }
            text.push_str(&y.to_string());
        }
        self.subtitle.set_markup(&text);
    }

    /// Подгружает краткую справку об исполнителе (если страница в Википедии найдена)
    pub fn load_about(&self, artist: String) {
        let name = artist.clone();
        self.load_info(artist, "Об исполнителе", async move {
            // Википедия, затем Last.fm, затем Genius
            if let Some(s) = crate::api::wiki::artist_summary(&name).await {
                return Some(s);
            }
            if let Some(s) = crate::api::lastfm::artist_summary(&name).await {
                return Some(s);
            }
            crate::api::genius::artist_summary(&name).await
        });
    }

    /// Справка об альбоме: Википедия, затем Last.fm (в Genius у альбомов нет надёжного описания)
    pub fn load_album_about(&self, title: String, artist: String) {
        let key = format!("{artist}\u{0}{title}");
        self.load_info(key, "Об альбоме", async move {
            if let Some(s) = crate::api::wiki::album_summary(&title, &artist).await {
                return Some(s);
            }
            crate::api::lastfm::album_summary(&title, &artist).await
        });
    }

    /// Показывает «Загрузка…», ищет справку и заменяет её карточкой с текстом, источником и кнопкой перевода
    fn load_info(
        &self,
        key: String,
        heading: &'static str,
        lookup: impl std::future::Future<Output = Option<crate::api::wiki::ArtistSummary>> + 'static,
    ) {
        *self.about_for.borrow_mut() = key.clone();
        let (about, about_for) = (self.about.clone(), self.about_for.clone());

        // пока справка ищется, показываем «Загрузка…»
        clear_box(&about);
        let loading = GtkBox::new(Orientation::Horizontal, 10);
        let spinner = gtk4::Spinner::new();
        spinner.start();
        let loading_text = Label::new(Some("Загрузка…"));
        loading_text.add_css_class("dim-label");
        loading.append(&spinner);
        loading.append(&loading_text);
        about.append(&loading);
        about.set_visible(true);

        glib::spawn_future_local(async move {
            // сеть могла зависнуть: дольше 15 с не ждём
            let found = tokio::time::timeout(std::time::Duration::from_secs(15), lookup).await.ok().flatten();
            // пользователь мог уйти на другую страницу, пока шёл запрос
            if *about_for.borrow() != key {
                return;
            }
            let Some(summary) = found else {
                // ничего не нашли или соединение оборвалось: «Загрузка…» убираем
                clear_box(&about);
                about.set_visible(false);
                return;
            };
            clear_box(&about);
            let head = Label::new(Some(heading));
            head.set_halign(Align::Start);
            head.add_css_class("section-title");
            let text = Label::new(Some(&summary.extract));
            text.set_halign(Align::Start);
            text.set_xalign(0.0);
            text.set_wrap(true);
            text.set_selectable(true);
            let source = Label::new(None);
            source.set_halign(Align::Start);
            source.add_css_class("dim-label");
            let license = if summary.license.is_empty() { String::new() } else { format!(" · {}", summary.license) };
            source.set_markup(&format!(
                "<a href=\"{}\">{}</a>{}",
                glib::markup_escape_text(&summary.url),
                summary.source,
                license
            ));
            about.append(&head);
            about.append(&text);
            about.append(&source);

            // английский текст: предлагаем перевод по кнопке (результат запоминается, повторный клик возвращает оригинал)
            if crate::api::translate::is_english(&summary.extract) {
                let original = summary.extract.clone();
                let translated: Rc<RefCell<Option<String>>> = Rc::default();
                let btn = Button::with_label("Перевести на русский");
                btn.add_css_class("pill");
                btn.set_halign(Align::Start);
                btn.set_tooltip_text(Some("Машинный перевод: MyMemory"));
                let showing_translation = Rc::new(std::cell::Cell::new(false));
                btn.connect_clicked(move |btn| {
                    if showing_translation.get() {
                        text.set_text(&original);
                        btn.set_label("Перевести на русский");
                        showing_translation.set(false);
                        return;
                    }
                    if let Some(t) = translated.borrow().clone() {
                        text.set_text(&t);
                        btn.set_label("Показать оригинал");
                        showing_translation.set(true);
                        return;
                    }
                    btn.set_sensitive(false);
                    btn.set_label("Перевод…");
                    let (btn, text, original, translated, showing) =
                        (btn.clone(), text.clone(), original.clone(), translated.clone(), showing_translation.clone());
                    glib::spawn_future_local(async move {
                        let result = crate::api::translate::to_russian(&original).await;
                        btn.set_sensitive(true);
                        match result {
                            Some(t) => {
                                text.set_text(&t);
                                *translated.borrow_mut() = Some(t);
                                btn.set_label("Показать оригинал");
                                showing.set(true);
                            }
                            None => btn.set_label("Не удалось перевести, повторить"),
                        }
                    });
                });
                about.append(&btn);
            }
            about.set_visible(true);
        });
    }

    pub fn set_round_cover(&self, round: bool) {
        if round {
            self.cover.add_css_class("round-cover");
        } else {
            self.cover.remove_css_class("round-cover");
        }
    }

    pub fn show(&self, kind: &str, title: &str, subtitle: &str, cover_url: Option<String>, tracks: Vec<Track>) {
        self.kind.set_text(kind);
        self.title.set_text(title);
        self.subtitle.set_markup(&glib::markup_escape_text(subtitle));
        self.link_albums.borrow_mut().clear();
        clear_box(&self.about);
        self.about.set_visible(false);
        self.about_for.borrow_mut().clear();
        self.cover.set_icon_name(Some("audio-x-generic-symbolic"));
        load_cover(&self.cover, cover_url);
        self.set_round_cover(false);
        self.wave_btn.set_visible(false);
        *self.wave.borrow_mut() = None;
        clear_box(&self.extra);

        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        for (i, track) in tracks.iter().enumerate() {
            let (p, t) = (self.player.clone(), self.tracks.clone());
            self.list.append(&track_row(track, move || p.set_queue_and_play(t.borrow().clone(), i)));
        }
        *self.tracks.borrow_mut() = tracks;
    }
}

/// Что открыть на странице
pub enum Source {
    Album(String),
    Playlist(String, String),
}

/// Загружает треки и показывает страницу `detail` в `stack`
pub fn open(
    player: &PlayerManager,
    stack: &Stack,
    page: &Rc<DetailPage>,
    source: Source,
    kind: &'static str,
    title: String,
    subtitle: String,
    cover_url: Option<String>,
    album: Option<Album>,
) {
    let (p, stack, page) = (player.clone(), stack.clone(), page.clone());
    glib::spawn_future_local(async move {
        let client = p.client.lock().unwrap().clone();
        let result = match &source {
            Source::Album(id) => client.get_album_tracks(id).await,
            Source::Playlist(uid, k) => client.get_playlist_tracks(uid, k).await,
        };
        match result {
            Ok(tracks) => {
                page.show(kind, &title, &subtitle, cover_url, tracks);
                if let Some(al) = &album {
                    page.set_subtitle_links(&al.artists, None, al.year);
                    if let Some(artist) = al.artists.first() {
                        page.load_album_about(title.clone(), artist.name.clone());
                    }
                }
                stack.set_visible_child_name("detail");
            }
            Err(e) => p.emit(PlayerUiEvent::Error(format!("Не удалось загрузить «{}»: {}", title, e))),
        }
    });
}

fn album_subtitle(album: &Album) -> String {
    match album.year {
        Some(y) if !album.artists.is_empty() => format!("{} · {}", album.artists_string(), y),
        Some(y) => y.to_string(),
        None => album.artists_string(),
    }
}

/// Открывает страницу альбома (например, по клику на обложку в плеере)
pub fn open_album(player: &PlayerManager, stack: &Stack, page: &Rc<DetailPage>, album: &Album) {
    open(
        player,
        stack,
        page,
        Source::Album(album.id.clone()),
        "Альбом",
        album.title.clone().unwrap_or_default(),
        album_subtitle(album),
        album.cover_url(400),
        Some(album.clone()),
    );
}

/// Плитка альбома: клик открывает страницу альбома
pub fn album_tile(player: &PlayerManager, stack: &Stack, page: &Rc<DetailPage>, album: &Album, size: i32) -> Button {
    let title = album.title.clone().unwrap_or_default();
    let sub = album_subtitle(album);
    let (p, stack, page) = (player.clone(), stack.clone(), page.clone());
    let al = album.clone();
    tile(&title, &sub, album.cover_url(300), size, false, move || open_album(&p, &stack, &page, &al))
}

fn playlist_subtitle(pl: &Playlist) -> String {
    pl.track_count.map(|n| format!("{} треков", n)).unwrap_or_default()
}

/// Открывает страницу плейлиста
pub fn open_playlist(player: &PlayerManager, stack: &Stack, page: &Rc<DetailPage>, pl: &Playlist) {
    open(
        player,
        stack,
        page,
        Source::Playlist(pl.uid.clone(), pl.kind.clone()),
        "Плейлист",
        pl.title.clone(),
        playlist_subtitle(pl),
        pl.cover_url(400),
        None,
    );
}

/// Плитка плейлиста: клик открывает страницу плейлиста
pub fn playlist_tile(player: &PlayerManager, stack: &Stack, page: &Rc<DetailPage>, pl: &Playlist, size: i32) -> Button {
    let (p, stack, page, playlist) = (player.clone(), stack.clone(), page.clone(), pl.clone());
    tile(&pl.title, &playlist_subtitle(pl), pl.cover_url(300), size, false, move || open_playlist(&p, &stack, &page, &playlist))
}

/// Плитка артиста: клик открывает страницу артиста
pub fn artist_tile(player: &PlayerManager, stack: &Stack, page: &Rc<DetailPage>, artist: &Artist, size: i32) -> Button {
    let (p, stack, page, id) = (player.clone(), stack.clone(), page.clone(), artist.id.clone());
    tile(&artist.name, "Исполнитель", artist.cover_url(300), size, true, move || {
        open_artist(&p, &stack, &page, id.clone())
    })
}

/// Страница артиста: популярные треки, альбомы и «Волна по артисту»
pub fn open_artist(player: &PlayerManager, stack: &Stack, page: &Rc<DetailPage>, artist_id: String) {
    let (p, stack, page) = (player.clone(), stack.clone(), page.clone());
    glib::spawn_future_local(async move {
        let client = p.client.lock().unwrap().clone();
        match client.get_artist_brief(&artist_id).await {
            Ok(brief) => {
                let a = &brief.artist;
                page.show("Исполнитель", &a.name, "", a.cover_url(400), brief.popular_tracks);
                page.set_round_cover(true);
                page.load_about(a.name.clone());
                page.set_wave(Wave::Artist(artist_id, a.name.clone()));
                if !brief.albums.is_empty() {
                    page.extra.append(&section_title("Альбомы"));
                    let (row, inner) = tile_row();
                    for album in &brief.albums {
                        inner.append(&album_tile(&p, &stack, &page, album, 160));
                    }
                    page.extra.append(&row);
                }
                stack.set_visible_child_name("detail");
            }
            Err(e) => p.emit(PlayerUiEvent::Error(format!("Не удалось загрузить артиста: {}", e))),
        }
    });
}

/// Страница трека: сам трек, похожие и «Волна по треку»
pub fn open_track(player: &PlayerManager, stack: &Stack, page: &Rc<DetailPage>, track: Track) {
    let (p, stack, page) = (player.clone(), stack.clone(), page.clone());
    glib::spawn_future_local(async move {
        let client = p.client.lock().unwrap().clone();
        let mut tracks = vec![track.clone()];
        if let Ok(similar) = client.get_similar_tracks(&track.id).await {
            tracks.extend(similar);
        }
        let album = track.albums.first().and_then(|a| a.title.clone());
        let sub = match album {
            Some(a) => format!("{} · {}", track.artists_string(), a),
            None => track.artists_string(),
        };
        page.show("Трек", &track.title, &sub, track.cover_url(400), tracks);
        page.set_subtitle_links(&track.artists, track.albums.first(), None);
        if let Some(a) = track.artists.first() {
            page.load_about(a.name.clone());
        }
        page.set_wave(Wave::Track(track));
        stack.set_visible_child_name("detail");
    });
}

/// Горизонтальная лента плиток
pub fn tile_row() -> (ScrolledWindow, GtkBox) {
    let row = GtkBox::new(Orientation::Horizontal, 14);
    let scrolled = ScrolledWindow::new();
    scrolled.set_policy(gtk4::PolicyType::Automatic, gtk4::PolicyType::Never);
    scrolled.set_child(Some(&row));
    (scrolled, row)
}
