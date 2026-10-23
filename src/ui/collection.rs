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
use crate::player::{PlayerManager, PlayerUiEvent, PlaySource, SourceTarget};

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
    // подписи внутри растягиваются (hexpand), но сама плитка — нет: иначе несколько плиток расползались на всю ширину
    btn.set_hexpand(false);
    btn.connect_clicked(move |_| on_click());
    btn
}

/// Скруглённая плашка «E» для explicit-треков; по умолчанию скрыта
pub fn explicit_badge() -> Label {
    let badge = Label::new(Some("E"));
    badge.add_css_class("explicit-badge");
    badge.set_valign(Align::Center);
    badge.set_tooltip_text(Some("Содержит нецензурную лексику"));
    badge.set_visible(false);
    badge
}

/// Строка трека с обложкой и длительностью
pub fn track_row(track: &Track, on_activate: impl Fn() + 'static) -> ActionRow {
    let row = ActionRow::builder()
        .title(track.title_markup())
        .subtitle(glib::markup_escape_text(&track.artists_string()))
        .activatable(true)
        .build();
    row.add_prefix(&cover_image(40, track.cover_url(100)));

    if track.is_explicit() {
        let badge = explicit_badge();
        badge.set_visible(true);
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
    /// Плашка «E» у названия на странице explicit-трека
    pub title_badge: Label,
    subtitle: Label,
    list: ListBox,
    tracks: Rc<RefCell<Vec<Track>>>,
    /// Подпись источника очереди для плашки на экране Волны (по типу страницы)
    source: Rc<RefCell<Option<PlaySource>>>,
    /// Видео в шапке страницы артиста: кадры из GStreamer рисуются в `Picture`
    /// Фото артиста (адреса 1000×1000) для просмотра по клику на аватарку
    photos: Rc<RefCell<Vec<String>>>,
    /// Размытое свечение того же видео на всю ширину шапки
    ambient_pic: gtk4::Picture,
    video: super::wave_clip::ClipController,
    video_shared: std::sync::Arc<super::wave_clip::ClipShared>,
    /// Номер показа видео: таймер кадров и запоздавшие загрузки устаревшего видео отбрасываются
    video_gen: std::cell::Cell<u32>,
    /// Плашка «N слушателей в месяц» у артиста
    listeners: GtkBox,
    listeners_label: Label,
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
    back: Button,
    /// Стек экрана и сама страница: для переходов «к альбому/исполнителю» из меню треков
    nav: NavCell,
    /// Плейлист, открытый на странице, если он свой: (uid, kind)
    owner_target: Rc<RefCell<Option<(String, String)>>>,
    /// Поиск треков для добавления и заглушка пустого плейлиста (только у своих)
    owner_box: GtkBox,
    empty: GtkBox,
    desc_btn: Button,
    cover_btn: Button,
    offline_btn: Button,
    cover_handler: std::cell::Cell<Option<glib::SignalHandlerId>>,
    /// Сердечко плейлиста («Нравится»)
    heart_btn: Button,
    heart_handler: std::cell::Cell<Option<glib::SignalHandlerId>>,
    desc_handler: std::cell::Cell<Option<glib::SignalHandlerId>>,
    /// Меню «⋯» своего плейлиста (удалить, приватность); у остальных страниц скрыто
    owner_btn: gtk4::MenuButton,
    /// Вызывается после удаления плейлиста, чтобы владелец страницы обновил свой список
    on_removed: RefCell<Option<Rc<dyn Fn()>>>,
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
        let heart_btn = Button::with_label(heart_glyph(false));
        heart_btn.add_css_class("pill");
        heart_btn.set_tooltip_text(Some("Мне нравится"));
        heart_btn.set_visible(false);
        let owner_btn = gtk4::MenuButton::new();
        owner_btn.set_icon_name("view-more-symbolic");
        owner_btn.add_css_class("pill");
        owner_btn.set_visible(false);

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
        let title_badge = explicit_badge();
        title_badge.set_margin_start(12);
        title_badge.add_css_class("hero");
        let title_row = GtkBox::new(Orientation::Horizontal, 0);
        title_row.append(&title);
        title_row.append(&title_badge);
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
        info.append(&title_row);
        let listeners = GtkBox::new(Orientation::Horizontal, 6);
        listeners.add_css_class("listeners-pill");
        listeners.set_halign(Align::Start);
        listeners.set_visible(false);
        listeners.append(&Image::from_icon_name("avatar-default-symbolic"));
        let listeners_label = Label::new(None);
        listeners.append(&listeners_label);
        info.append(&listeners);
        info.append(&subtitle);
        let desc_btn = Button::new();
        desc_btn.add_css_class("flat");
        desc_btn.add_css_class("dim-label");
        desc_btn.add_css_class("playlist-desc");
        desc_btn.set_halign(Align::Start);
        desc_btn.set_visible(false);
        info.append(&desc_btn);
        let offline_btn = Button::with_label("Загрузить");
        offline_btn.add_css_class("pill");
        offline_btn.set_tooltip_text(Some("Сохранить треки в приложении для прослушивания без интернета"));
        offline_btn.set_visible(false);
        let wave_btn = Button::new();
        wave_btn.add_css_class("pill");
        wave_btn.set_visible(false);
        let actions = GtkBox::new(Orientation::Horizontal, 8);
        actions.append(&play);
        actions.append(&offline_btn);
        actions.append(&wave_btn);
        actions.append(&heart_btn);
        actions.append(&owner_btn);
        info.append(&actions);

        let cover_btn = Button::with_label("Добавить обложку");
        cover_btn.add_css_class("cover-upload");
        cover_btn.set_halign(Align::Center);
        cover_btn.set_valign(Align::End);
        cover_btn.set_margin_bottom(12);
        cover_btn.set_visible(false);
        let cover_overlay = gtk4::Overlay::new();
        cover_overlay.add_css_class("cover-overlay");
        cover_overlay.set_child(Some(&cover));
        cover_overlay.add_overlay(&cover_btn);
        let hero = GtkBox::new(Orientation::Horizontal, 24);
        hero.append(&cover_overlay);
        // клик по аватарке артиста открывает все его фото
        let photos: Rc<RefCell<Vec<String>>> = Rc::default();
        {
            let (photos, title, anchor) = (photos.clone(), title.clone(), cover.clone());
            let click = gtk4::GestureClick::new();
            click.connect_released(move |_, _, _, _| {
                let urls = photos.borrow().clone();
                if !urls.is_empty() {
                    show_photo_gallery(&anchor, &title.text(), &urls);
                }
            });
            cover.add_controller(click);
        }
        hero.append(&info);
        // видео артиста лежит под всей страницей (оверлей вокруг прокрутки), края у него растворяются в фоне
        // видео артиста играет размытым фоном позади шапки на всю ширину страницы
        let ambient_pic = gtk4::Picture::new();
        ambient_pic.set_content_fit(gtk4::ContentFit::Fill); // кадр уже обрезан под пропорции в `ambient_frame`
        ambient_pic.set_can_shrink(true);
        ambient_pic.set_can_target(false);
        ambient_pic.add_css_class("artist-ambient");
        ambient_pic.set_visible(false);
        content.append(&hero);
        let video_shared = std::sync::Arc::new(super::wave_clip::ClipShared::default());
        let video = super::wave_clip::ClipController::new(video_shared.clone());
        let about = GtkBox::new(Orientation::Vertical, 6);
        about.add_css_class("about-card");
        about.set_visible(false);
        content.append(&about);

        let list = ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk4::SelectionMode::None);

        let tracks: Rc<RefCell<Vec<Track>>> = Rc::default();
        let source: Rc<RefCell<Option<PlaySource>>> = Rc::default();
        let owner_target: Rc<RefCell<Option<(String, String)>>> = Rc::default();
        let nav: NavCell = Rc::default();
        let owner_box = GtkBox::new(Orientation::Vertical, 12);
        owner_box.set_visible(false);
        let search = gtk4::SearchEntry::new();
        search.set_placeholder_text(Some("Поиск трека"));
        let results = ListBox::new();
        results.add_css_class("boxed-list");
        results.set_selection_mode(gtk4::SelectionMode::None);
        results.set_visible(false);
        let empty = GtkBox::new(Orientation::Vertical, 6);
        empty.set_halign(Align::Center);
        empty.set_margin_top(24);
        empty.set_margin_bottom(24);
        let empty_title = Label::new(Some("Добавьте треки в плейлист"));
        empty_title.add_css_class("section-title");
        let empty_sub = Label::new(Some("Треки можно найти через поиск"));
        empty_sub.add_css_class("dim-label");
        empty.append(&empty_title);
        empty.append(&empty_sub);
        owner_box.append(&search);
        owner_box.append(&results);
        owner_box.append(&empty);
        content.append(&owner_box);
        content.append(&list);

        {
            let seq = Rc::new(std::cell::Cell::new(0u32));
            let (p, tracks, target, list, empty, results, nav) =
                (player.clone(), tracks.clone(), owner_target.clone(), list.clone(), empty.clone(), results.clone(), nav.clone());
            search.connect_search_changed(move |e| {
                let q = e.text().trim().to_string();
                seq.set(seq.get() + 1);
                let my = seq.get();
                results.remove_all();
                results.set_visible(false);
                if q.is_empty() {
                    return;
                }
                let (p, tracks, target, list, empty, results, seq, nav) =
                    (p.clone(), tracks.clone(), target.clone(), list.clone(), empty.clone(), results.clone(), seq.clone(), nav.clone());
                glib::spawn_future_local(async move {
                    let client = p.client.lock().unwrap().clone();
                    let Ok(found) = client.search(&q).await else { return };
                    if seq.get() != my {
                        return;
                    }
                    for t in found.tracks.map(|s| s.results).unwrap_or_default().into_iter().take(8) {
                        let row = ActionRow::builder()
                            .title(t.title_markup())
                            .subtitle(glib::markup_escape_text(&t.artists_string()))
                            .build();
                        row.add_prefix(&cover_image(40, t.cover_url(100)));
                        let add = Button::from_icon_name("list-add-symbolic");
                        add.add_css_class("flat");
                        add.add_css_class("circular");
                        add.set_valign(Align::Center);
                        add.set_tooltip_text(Some("Добавить в плейлист"));
                        let (p, tracks, target, list, empty, t, nav) =
                            (p.clone(), tracks.clone(), target.clone(), list.clone(), empty.clone(), t.clone(), nav.clone());
                        add.connect_clicked(move |b| {
                            let Some((uid, kind)) = target.borrow().clone() else { return };
                            b.set_sensitive(false);
                            let (p, tracks, list, empty, t, b, nav) = (p.clone(), tracks.clone(), list.clone(), empty.clone(), t.clone(), b.clone(), nav.clone());
                            glib::spawn_future_local(async move {
                                let client = p.client.lock().unwrap().clone();
                                match client.add_track_to_playlist(&uid, &kind, &t).await {
                                    Ok(()) => {
                                        b.set_icon_name("object-select-symbolic");
                                        insert_track_row(&p, &nav, &tracks, &list, &empty, &t);
                                    }
                                    Err(e) => {
                                        b.set_sensitive(true);
                                        p.emit(PlayerUiEvent::Error(format!("Не удалось добавить трек: {}", e)));
                                    }
                                }
                            });
                        });
                        row.add_suffix(&add);
                        results.append(&row);
                    }
                    results.set_visible(results.first_child().is_some());
                });
            });
        }
        let extra = GtkBox::new(Orientation::Vertical, 10);
        content.append(&extra);

        let scrolled = ScrolledWindow::new();
        scrolled.set_vexpand(true);
        content.set_margin_bottom(130);
        content.set_margin_end(14);
        scrolled.set_child(Some(&content));
        let page_overlay = gtk4::Overlay::new();
        page_overlay.set_vexpand(true);
        page_overlay.set_child(Some(&gtk4::Box::new(Orientation::Vertical, 0)));
        page_overlay.add_overlay(&ambient_pic);
        page_overlay.add_overlay(&scrolled);
        // видео занимает верх страницы на всю ширину и уезжает вверх вместе с прокруткой
        {
            let (bg, adj) = (ambient_pic.clone(), scrolled.vadjustment());
            page_overlay.connect_get_child_position(move |ov, child| {
                if child != bg.upcast_ref::<gtk4::Widget>() {
                    return None;
                }
                Some(gdk::Rectangle::new(0, -(adj.value() as i32) - AMBIENT_LIFT, ov.width(), AMBIENT_HEIGHT))
            });
            let ov = page_overlay.clone();
            scrolled.vadjustment().connect_value_changed(move |_| ov.queue_allocate());
        }
        widget.append(&page_overlay);

        {
            let (p, t, src) = (player.clone(), tracks.clone(), source.clone());
            play.connect_clicked(move |_| p.set_queue_and_play_from(t.borrow().clone(), 0, src.borrow().clone()));
        }

        {
            let (p, t, busy) = (player.clone(), tracks.clone(), Rc::new(std::cell::Cell::new(false)));
            offline_btn.connect_clicked(move |b| {
                if busy.get() {
                    return;
                }
                let list = t.borrow().clone();
                let missing: Vec<Track> = list.iter().filter(|t| crate::cache::offline_audio_path(&t.id).is_none()).cloned().collect();
                if missing.is_empty() {
                    for t in &list {
                        crate::cache::remove_offline(&t.id);
                    }
                    b.set_label("Загрузить");
                    return;
                }
                busy.set(true);
                let (p, b, busy) = (p.clone(), b.clone(), busy.clone());
                glib::spawn_future_local(async move {
                    let client = p.client.lock().unwrap().clone();
                    let n = missing.len();
                    for (i, t) in missing.iter().enumerate() {
                        b.set_label(&format!("Загрузка {}/{}", i + 1, n));
                        if let Err(e) = client.save_track_offline(t).await {
                            p.emit(PlayerUiEvent::Error(format!("Не удалось сохранить «{}»: {}", t.title, e)));
                            break;
                        }
                    }
                    b.set_label(offline_label(&list));
                    busy.set(false);
                });
            });
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

        Rc::new(Self { widget, cover, kind, title, title_badge, subtitle, list, tracks, source: source.clone(), photos: photos.clone(), ambient_pic, video, video_shared, video_gen: Default::default(), listeners: listeners.clone(), listeners_label: listeners_label.clone(), player: player.clone(), wave_btn, wave, extra, link_handler, link_albums: RefCell::default(), about, about_for: Rc::default(), back, nav, owner_target, owner_box, empty, desc_btn, cover_btn, offline_btn, cover_handler: Default::default(), heart_btn, heart_handler: Default::default(), desc_handler: Default::default(), owner_btn, on_removed: RefCell::default() })
    }

    /// Кнопка-описание у своего плейлиста: клик открывает редактирование
    fn set_description(self: &Rc<Self>, uid: &str, kind: &str, text: &str) {
        self.desc_btn.set_label(if text.is_empty() { "Добавить описание…" } else { text });
        self.desc_btn.set_visible(true);
        // обработчик навешивается заново для каждого плейлиста
        if let Some(old) = self.desc_handler.take() {
            self.desc_btn.disconnect(old);
        }
        let (page, uid, kind, text) = (self.clone(), uid.to_string(), kind.to_string(), text.to_string());
        let id = self.desc_btn.connect_clicked(move |b| {
            let dialog = libadwaita::AlertDialog::new(Some("Описание плейлиста"), None);
            let entry = gtk4::Entry::new();
            entry.set_text(&text);
            entry.set_activates_default(true);
            dialog.set_extra_child(Some(&entry));
            dialog.add_response("cancel", "Отмена");
            dialog.add_response("save", "Сохранить");
            dialog.set_response_appearance("save", libadwaita::ResponseAppearance::Suggested);
            dialog.set_default_response(Some("save"));
            let (page, uid, kind) = (page.clone(), uid.clone(), kind.clone());
            dialog.choose(Some(b), None::<&gtk4::gio::Cancellable>, move |response| {
                if response != "save" {
                    return;
                }
                let new_text = entry.text().trim().to_string();
                glib::spawn_future_local(async move {
                    let client = page.player.client.lock().unwrap().clone();
                    match client.set_playlist_description(&uid, &kind, &new_text).await {
                        Ok(()) => page.set_description(&uid, &kind, &new_text),
                        Err(e) => page.player.emit(PlayerUiEvent::Error(format!("Не удалось сохранить описание: {}", e))),
                    }
                });
            });
        });
        self.desc_handler.set(Some(id));
    }

    /// Сердечко плейлиста: состояние берётся из «Нравится» пользователя
    pub fn set_playlist_context(self: &Rc<Self>, me: String, uid: String, kind: String, liked: bool) {
        let key = format!("{}:{}", uid, kind);
        self.heart_btn.set_label(heart_glyph(liked));
        self.heart_btn.set_visible(true);
        if liked { self.heart_btn.add_css_class("liked-heart"); } else { self.heart_btn.remove_css_class("liked-heart"); }
        if let Some(old) = self.heart_handler.take() {
            self.heart_btn.disconnect(old);
        }
        let (p, liked_now) = (self.player.clone(), Rc::new(std::cell::Cell::new(liked)));
        let id = self.heart_btn.connect_clicked(move |b| {
            let like = !liked_now.get();
            liked_now.set(like);
            b.set_label(heart_glyph(like));
            if like { b.add_css_class("liked-heart"); } else { b.remove_css_class("liked-heart"); }
            let (me, key) = (me.clone(), key.clone());
            run_api(&p, None, move |c, _| async move { c.set_playlist_like(&me, &key, like).await });
        });
        self.heart_handler.set(Some(id));
    }

    /// «Добавить обложку»: выбор картинки и загрузка
    fn set_cover_upload(self: &Rc<Self>, uid: &str, kind: &str) {
        self.cover_btn.set_visible(true);
        if let Some(old) = self.cover_handler.take() {
            self.cover_btn.disconnect(old);
        }
        let (page, uid, kind) = (self.clone(), uid.to_string(), kind.to_string());
        let id = self.cover_btn.connect_clicked(move |b| {
            let (page, uid, kind, b) = (page.clone(), uid.clone(), kind.clone(), b.clone());
            glib::spawn_future_local(async move {
                let filter = gtk4::FileFilter::new();
                filter.add_mime_type("image/png");
                filter.add_mime_type("image/jpeg");
                filter.set_name(Some("Изображения"));
                let filters = gtk4::gio::ListStore::new::<gtk4::FileFilter>();
                filters.append(&filter);
                let dialog = gtk4::FileDialog::builder().title("Обложка плейлиста").filters(&filters).build();
                let window = b.root().and_downcast::<gtk4::Window>();
                let Ok(file) = dialog.open_future(window.as_ref()).await else { return };
                let Some(path) = file.path() else { return };
                let Ok(data) = std::fs::read(&path) else {
                    page.player.emit(PlayerUiEvent::Error("Не удалось прочитать файл".into()));
                    return;
                };
                if data.len() > 10 * 1024 * 1024 {
                    page.player.emit(PlayerUiEvent::Error("Файл больше 10 МБ".into()));
                    return;
                }
                let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "cover.jpg".into());
                let mime = if name.to_lowercase().ends_with(".png") { "image/png" } else { "image/jpeg" };
                let client = page.player.client.lock().unwrap().clone();
                match client.upload_playlist_cover(&uid, &kind, &name, mime, data).await {
                    Ok(url) => {
                        let url = url.map(|u| crate::api::models::cover_url(&u, 400));
                        load_cover(&page.cover, url);
                        page.player.emit(PlayerUiEvent::StatusMessage("Обложка загружена".into()));
                    }
                    Err(e) => page.player.emit(PlayerUiEvent::Error(format!("Не удалось загрузить обложку: {}", e))),
                }
            });
        });
        self.cover_handler.set(Some(id));
    }

    /// Стек, в котором лежит страница; нужен меню треков для переходов к альбому и исполнителю
    pub fn set_nav(self: &Rc<Self>, stack: &Stack) {
        *self.nav.borrow_mut() = Some((stack.clone(), Rc::downgrade(self)));
        // клики по исполнителю и альбому в подписи страницы (`artist:ID`, `album:ID`); нужны на любом экране со страницами
        let (p, stack, weak) = (self.player.clone(), stack.clone(), Rc::downgrade(self));
        *self.link_handler.borrow_mut() = Some(Rc::new(move |uri: &str| {
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

    pub fn set_on_removed(&self, f: impl Fn() + 'static) {
        *self.on_removed.borrow_mut() = Some(Rc::new(f));
    }

    /// Меню своего плейлиста: переключатель приватности и удаление (с подтверждением)
    pub fn set_owner_menu(self: &Rc<Self>, uid: String, kind: String, meta: crate::api::client::PlaylistMeta) {
        let (private, description) = (meta.private, meta.description.clone());
        let date = meta.modified.get(..10).unwrap_or("");
        if !date.is_empty() && !meta.owner.is_empty() {
            let when = if date == chrono::Local::now().format("%Y-%m-%d").to_string() {
                "сегодня".to_string()
            } else {
                format!("{}.{}.{}", &date[8..10], &date[5..7], &date[..4])
            };
            self.subtitle.set_markup(&glib::markup_escape_text(&format!("{} обновил плейлист {}", meta.owner, when)));
        }
        *self.owner_target.borrow_mut() = Some((uid.clone(), kind.clone()));
        self.owner_box.set_visible(true);
        self.empty.set_visible(self.tracks.borrow().is_empty());
        self.set_description(&uid, &kind, &description);
        self.set_cover_upload(&uid, &kind);

        let menu = GtkBox::new(Orientation::Vertical, 0);
        let wave = menu_item("media-playlist-consecutive-symbolic", "Моя волна по плейлисту");
        let share = menu_item("edit-copy-symbolic", "Скопировать ссылку");

        let delete = menu_item("user-trash-symbolic", "Удалить плейлист");
        let privacy = GtkBox::new(Orientation::Horizontal, 12);
        privacy.set_margin_start(12);
        privacy.set_margin_end(12);
        privacy.set_margin_top(6);
        privacy.set_margin_bottom(6);
        let label = Label::new(Some("Приватный плейлист"));
        label.set_hexpand(true);
        label.set_xalign(0.0);
        let switch = gtk4::Switch::new();
        switch.set_active(private);
        privacy.append(&label);
        privacy.append(&switch);
        let to_other = menu_item("list-add-symbolic", "Добавить в другой плейлист");
        let rename = menu_item("document-edit-symbolic", "Переименовать");
        menu.append(&to_other);
        menu.append(&rename);
        menu.append(&wave);
        menu.append(&share);
        menu.append(&delete);
        menu.append(&privacy);

        let pop = gtk4::Popover::new();
        let views = Stack::new();
        let others = GtkBox::new(Orientation::Vertical, 0);
        views.add_named(&menu, Some("main"));
        views.add_named(&others, Some("others"));
        pop.set_child(Some(&views));
        pop.connect_closed({
            let views = views.clone();
            move |_| views.set_visible_child_name("main")
        });
        self.owner_btn.set_popover(Some(&pop));
        self.owner_btn.set_visible(true);

        {
            let (page, views, others, pop, uid, kind) = (self.clone(), views.clone(), others.clone(), pop.clone(), uid.clone(), kind.clone());
            to_other.connect_clicked(move |_| {
                clear_box(&others);
                let back = menu_item("go-previous-symbolic", "Назад");
                let v = views.clone();
                back.connect_clicked(move |_| v.set_visible_child_name("main"));
                others.append(&back);
                views.set_visible_child_name("others");
                let (page, others, pop, uid, kind) = (page.clone(), others.clone(), pop.clone(), uid.clone(), kind.clone());
                glib::spawn_future_local(async move {
                    let client = page.player.client.lock().unwrap().clone();
                    let Ok(lists) = client.get_user_playlists(&uid).await else { return };
                    for pl in lists.into_iter().filter(|l| l.kind != kind) {
                        let b = menu_item("view-list-symbolic", &pl.title);
                        let (page, pop, uid, target) = (page.clone(), pop.clone(), uid.clone(), pl.kind.clone());
                        b.connect_clicked(move |_| {
                            pop.popdown();
                            let tracks = page.tracks.borrow().clone();
                            let (uid, target) = (uid.clone(), target.clone());
                            run_api(&page.player, Some("Треки добавлены в плейлист"), move |c, _| async move {
                                c.add_tracks_to_playlist(&uid, &target, &tracks).await
                            });
                        });
                        others.append(&b);
                    }
                });
            });
        }

        {
            let (p, uid, kind) = (self.player.clone(), uid.clone(), kind.clone());
            switch.connect_active_notify(move |sw| {
                let (uid, kind, private) = (uid.clone(), kind.clone(), sw.is_active());
                run_api(&p, None, move |c, _| async move { c.set_playlist_private(&uid, &kind, private).await });
            });
        }
        {
            let (page, pop, uid, kind) = (self.clone(), pop.clone(), uid.clone(), kind.clone());
            rename.connect_clicked(move |b| {
                pop.popdown();
                let dialog = libadwaita::AlertDialog::new(Some("Название плейлиста"), None);
                let entry = gtk4::Entry::new();
                entry.set_text(&page.title.text());
                entry.set_activates_default(true);
                dialog.set_extra_child(Some(&entry));
                dialog.add_response("cancel", "Отмена");
                dialog.add_response("save", "Сохранить");
                dialog.set_response_appearance("save", libadwaita::ResponseAppearance::Suggested);
                dialog.set_default_response(Some("save"));
                let (page, uid, kind) = (page.clone(), uid.clone(), kind.clone());
                dialog.choose(Some(b), None::<&gtk4::gio::Cancellable>, move |response| {
                    let name = entry.text().trim().to_string();
                    if response != "save" || name.is_empty() {
                        return;
                    }
                    glib::spawn_future_local(async move {
                        let client = page.player.client.lock().unwrap().clone();
                        match client.rename_playlist(&uid, &kind, &name).await {
                            Ok(()) => page.title.set_text(&name),
                            Err(e) => page.player.emit(PlayerUiEvent::Error(format!("Не удалось переименовать: {}", e))),
                        }
                    });
                });
            });
        }
        {
            let (page, pop, uid, kind) = (self.clone(), pop.clone(), uid.clone(), kind.clone());
            wave.connect_clicked(move |_| {
                pop.popdown();
                page.player.play_playlist_wave(&uid, &kind, &page.title.text());
            });
        }
        {
            let (p, pop, uid, kind) = (self.player.clone(), pop.clone(), uid.clone(), kind.clone());
            share.connect_clicked(move |_| {
                pop.popdown();
                if let Some(d) = gdk::Display::default() {
                    d.clipboard().set_text(&format!("https://music.yandex.ru/users/{}/playlists/{}", uid, kind));
                    p.emit(PlayerUiEvent::StatusMessage("Ссылка скопирована".into()));
                }
            });
        }
        let page = self.clone();
        delete.connect_clicked(move |b| {
            pop.popdown();
            let dialog = libadwaita::AlertDialog::new(Some("Удалить плейлист?"), Some("Это действие нельзя отменить."));
            dialog.add_response("cancel", "Отмена");
            dialog.add_response("delete", "Удалить");
            dialog.set_response_appearance("delete", libadwaita::ResponseAppearance::Destructive);
            let (page, uid, kind) = (page.clone(), uid.clone(), kind.clone());
            dialog.choose(Some(b), None::<&gtk4::gio::Cancellable>, move |response| {
                if response != "delete" {
                    return;
                }
                glib::spawn_future_local(async move {
                    let client = page.player.client.lock().unwrap().clone();
                    match client.delete_playlist(&uid, &kind).await {
                        Ok(()) => {
                            page.back.emit_clicked();
                            let cb = page.on_removed.borrow().clone();
                            if let Some(cb) = cb {
                                cb();
                            }
                        }
                        Err(e) => page.player.emit(PlayerUiEvent::Error(format!("Не удалось удалить плейлист: {}", e))),
                    }
                });
            });
        });
    }

    /// Включает зацикленное видео в шапке (беззвучно); кадры забирает таймер, пока страница на этом видео
    pub fn play_background_video(self: &Rc<Self>, path: &str) {
        let token = self.video_gen.get() + 1;
        self.video_gen.set(token);
        self.ambient_pic.set_visible(true);
        self.video.play(path);
        let weak = Rc::downgrade(self);
        gtk4::glib::timeout_add_local(std::time::Duration::from_millis(40), move || {
            let Some(page) = weak.upgrade() else { return glib::ControlFlow::Break };
            if page.video_gen.get() != token {
                return glib::ControlFlow::Break;
            }
            if let Some(f) = page.video_shared.take_fresh() {
                let ratio = page.ambient_pic.width().max(1) as f32 / AMBIENT_HEIGHT as f32;
                let (aw, ah, glow) = ambient_frame(&f, ratio);
                let glow = gdk::MemoryTexture::new(aw, ah, gdk::MemoryFormat::R8g8b8a8Premultiplied, &glib::Bytes::from_owned(glow), (aw * 4) as usize);
                page.ambient_pic.set_paintable(Some(&glow));
            }
            glib::ControlFlow::Continue
        });
    }

    /// Останавливает видео шапки и возвращает обычную обложку
    pub fn stop_background_video(&self) {
        self.video_gen.set(self.video_gen.get() + 1);
        self.video.stop();
        self.ambient_pic.set_paintable(None::<&gdk::Paintable>);
        self.ambient_pic.set_visible(false);
    }

    /// Номер показа видео: по нему загрузка проверяет, что страница ещё та же
    pub fn video_token(&self) -> u32 {
        self.video_gen.get()
    }

    /// Фото артиста для просмотра по клику на аватарку (если их больше одного)
    pub fn set_photos(&self, uris: &[String]) {
        let urls: Vec<String> = uris.iter().map(|u| crate::api::models::cover_url(u, 1000)).collect();
        if urls.len() > 1 {
            self.cover.set_cursor_from_name(Some("pointer"));
            self.cover.set_tooltip_text(Some("Все фото"));
        }
        *self.photos.borrow_mut() = if urls.len() > 1 { urls } else { Vec::new() };
    }

    /// Страница, куда ведёт клик по плашке источника (ставится после `show`)
    pub fn set_source_target(&self, target: SourceTarget) {
        if let Some(src) = self.source.borrow_mut().as_mut() {
            src.target = Some(target);
        }
    }

    /// Плашка «N слушателей в месяц» на странице артиста
    pub fn set_listeners(&self, count: u64) {
        self.listeners_label.set_text(&format!("{} слушателей в месяц", group_digits(count)));
        self.listeners.set_visible(true);
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
        self.stop_background_video();
        self.photos.borrow_mut().clear();
        self.cover.set_cursor(None::<&gdk::Cursor>);
        *self.source.borrow_mut() = source_label(kind, title).map(|label| {
            // у «Мне нравится» и «Загруженных» страницы нет, клик ведёт на вкладку Медиатека
            PlaySource::new(label, (kind == "Коллекция").then_some(SourceTarget::Library))
        });
        self.listeners.set_visible(false);
        self.title_badge.set_visible(false);
        self.subtitle.set_markup(&glib::markup_escape_text(subtitle));
        self.link_albums.borrow_mut().clear();
        clear_box(&self.about);
        self.about.set_visible(false);
        self.about_for.borrow_mut().clear();
        self.cover.set_icon_name(Some("audio-x-generic-symbolic"));
        load_cover(&self.cover, cover_url);
        self.set_round_cover(false);
        self.wave_btn.set_visible(false);
        self.owner_btn.set_visible(false);
        self.owner_box.set_visible(false);
        self.heart_btn.set_visible(false);
        self.desc_btn.set_visible(false);
        self.cover_btn.set_visible(false);
        *self.owner_target.borrow_mut() = None;
        *self.wave.borrow_mut() = None;
        clear_box(&self.extra);

        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        for track in tracks.iter() {
            let (p, t, id, src) = (self.player.clone(), self.tracks.clone(), track.id.clone(), self.source.clone());
            // позицию ищем по id: добавление в плейлист сдвигает индексы
            let row = track_row(track, move || {
                let i = t.borrow().iter().position(|x| x.id == id).unwrap_or(0);
                p.set_queue_and_play_from(t.borrow().clone(), i, src.borrow().clone())
            });
            row.add_suffix(&row_actions(&self.player, &self.nav, track));
            self.list.append(&row);
        }
        self.empty.set_visible(tracks.is_empty());
        self.offline_btn.set_label(offline_label(&tracks));
        self.offline_btn.set_visible(!tracks.is_empty());
        *self.tracks.borrow_mut() = tracks;
    }

    /// Реактивно добавляет или удаляет трек из «Мне нравится» или обновляет сердечко в открытых списках
    pub fn handle_track_liked(&self, track: &Track, liked: bool) {
        if self.title.text() == "Мне нравится" {
            if liked {
                if self.tracks.borrow().iter().any(|t| t.id == track.id) {
                    return;
                }
                insert_track_row(&self.player, &self.nav, &self.tracks, &self.list, &self.empty, track);
                let count = self.tracks.borrow().len();
                self.subtitle.set_markup(&glib::markup_escape_text(&format!("{} треков", count)));
                self.offline_btn.set_visible(true);
            } else {
                let pos = self.tracks.borrow().iter().position(|t| t.id == track.id);
                if let Some(idx) = pos {
                    self.tracks.borrow_mut().remove(idx);
                    if let Some(row) = self.list.row_at_index(idx as i32) {
                        self.list.remove(&row);
                    }
                    let count = self.tracks.borrow().len();
                    self.subtitle.set_markup(&glib::markup_escape_text(&format!("{} треков", count)));
                    if count == 0 {
                        self.empty.set_visible(true);
                        self.offline_btn.set_visible(false);
                    }
                }
            }
        } else {
            // В открытых страницах альбомов, артистов или плейлистов обновляем сердечко на соответствующей строке
            if let Some(idx) = self.tracks.borrow().iter().position(|t| t.id == track.id) {
                if let Some(row) = self.list.row_at_index(idx as i32) {
                    update_heart_in_widget(row.upcast_ref(), liked);
                }
            }
        }
    }
}

/// Добавляет трек в начало списка страницы (как делает сервер при добавлении в плейлист)
fn insert_track_row(p: &PlayerManager, nav: &NavCell, tracks: &Rc<RefCell<Vec<Track>>>, list: &ListBox, empty: &GtkBox, track: &Track) {
    tracks.borrow_mut().insert(0, track.clone());
    empty.set_visible(false);
    let (p2, t) = (p.clone(), tracks.clone());
    // индексы строк сдвинулись, поэтому позицию ищем по id
    let id = track.id.clone();
    let source = nav.borrow().as_ref().and_then(|(_, w)| w.upgrade()).map(|pg| pg.source.clone()).unwrap_or_default();
    let row = track_row(track, move || {
        let i = t.borrow().iter().position(|x| x.id == id).unwrap_or(0);
        p2.set_queue_and_play_from(t.borrow().clone(), i, source.borrow().clone())
    });
    row.add_suffix(&row_actions(p, nav, track));
    list.insert(&row, 0);
}

type NavCell = Rc<RefCell<Option<(Stack, std::rc::Weak<DetailPage>)>>>;

/// Сердечко и «⋯» для строки на странице: переходы работают, если владелец задал стек (`set_nav`)
fn row_actions(p: &PlayerManager, nav: &NavCell, track: &Track) -> GtkBox {
    let target = nav.borrow().as_ref().and_then(|(s, w)| Some((s.clone(), w.upgrade()?)));
    track_actions(p, target.as_ref().map(|(s, pg)| (s, pg)), track)
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
        ensure_liked(&p).await;
        match result {
            Ok(tracks) => {
                page.show(kind, &title, &subtitle, cover_url, tracks);
                match (&source, &album) {
                    (Source::Album(_), Some(al)) => page.set_source_target(SourceTarget::Album(al.clone())),
                    (Source::Playlist(uid, k), _) => page.set_source_target(SourceTarget::Playlist(Playlist {
                        uid: uid.clone(),
                        kind: k.clone(),
                        title: title.clone(),
                        ..Default::default()
                    })),
                    _ => {}
                }
                if let Source::Playlist(uid, k) = &source {
                    if let Ok(me) = client.get_uid().await {
                        let liked = client.get_liked_playlist_keys(&me).await.unwrap_or_default().contains(&format!("{}:{}", uid, k));
                        page.set_playlist_context(me.clone(), uid.clone(), k.clone(), liked);
                        // меню «⋯» только у своих плейлистов
                        if me == *uid {
                            if let Ok(meta) = client.get_playlist_meta(uid, k).await {
                                page.set_owner_menu(uid.clone(), k.clone(), meta);
                            }
                        }
                    }
                }
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
                ensure_liked(&p).await;
                let a = &brief.artist;
                page.show("Исполнитель", &a.name, "", a.cover_url(400), brief.popular_tracks);
                page.set_round_cover(true);
                page.set_source_target(SourceTarget::Artist(artist_id.clone()));
                let photos: Vec<String> = brief.all_covers.iter().filter_map(|c| c.uri.clone()).collect();
                page.set_photos(&photos);
                if let Some(n) = brief.stats.as_ref().and_then(|s| s.last_month_listeners) {
                    page.set_listeners(n);
                }
                // видео шапки: только https и хосты Яндекса, ссылка приходит из ответа сервера
                if let Some(url) = brief.background_video_url.clone().filter(|u| trusted_video_url(u)) {
                    let (page, token) = (page.clone(), page.video_token());
                    glib::spawn_future_local(async move {
                        if let Some(path) = crate::cache::fetch_clip_cached(&url).await {
                            // пока качалось, страницу могли сменить
                            if page.video_token() == token {
                                page.play_background_video(&path.to_string_lossy());
                            }
                        }
                    });
                }
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
        ensure_liked(&p).await;
        let album = track.albums.first().and_then(|a| a.title.clone());
        let sub = match album {
            Some(a) => format!("{} · {}", track.artists_string(), a),
            None => track.artists_string(),
        };
        page.show("Трек", &track.title, &sub, track.cover_url(400), tracks);
        page.title_badge.set_visible(track.is_explicit());
        page.set_source_target(SourceTarget::Track(track.clone()));
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

/// Загружает «Мне нравится» через менеджер плеера один раз за запуск
pub async fn ensure_liked(player: &PlayerManager) {
    player.ensure_liked_loaded().await;
}

/// Сердечко текстом: иконки тем оформления разные, а у части из них вместо сердца звезда
fn heart_glyph(liked: bool) -> &'static str {
    if liked { "♥" } else { "♡" }
}

/// Находит кнопку сердечка внутри строки списка и обновляет её состояние
pub fn update_heart_in_widget(widget: &gtk4::Widget, liked: bool) {
    if let Some(btn) = find_heart_button(widget) {
        btn.set_label(heart_glyph(liked));
        if liked {
            btn.add_css_class("liked-heart");
        } else {
            btn.remove_css_class("liked-heart");
        }
    }
}

fn find_heart_button(widget: &gtk4::Widget) -> Option<Button> {
    if let Ok(btn) = widget.clone().downcast::<Button>() {
        if btn.has_css_class("track-heart-btn") {
            return Some(btn);
        }
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        if let Some(btn) = find_heart_button(&c) {
            return Some(btn);
        }
        child = c.next_sibling();
    }
    None
}

/// Запускает запрос в фоне и показывает ошибку в строке состояния
fn run_api<F, Fut>(player: &PlayerManager, ok_msg: Option<&'static str>, f: F)
where
    F: FnOnce(crate::api::client::YandexMusicClient, String) -> Fut + 'static,
    Fut: std::future::Future<Output = Result<(), crate::api::client::ApiError>> + 'static,
{
    let p = player.clone();
    glib::spawn_future_local(async move {
        let client = p.client.lock().unwrap().clone();
        let res = match client.get_uid().await {
            Ok(uid) => f(client, uid).await,
            Err(e) => Err(e),
        };
        match res {
            Ok(()) => {
                if let Some(m) = ok_msg {
                    p.emit(PlayerUiEvent::StatusMessage(m.into()));
                }
            }
            Err(e) => p.emit(PlayerUiEvent::Error(format!("Не удалось выполнить действие: {}", e))),
        }
    });
}

fn menu_item(icon: &str, text: &str) -> Button {
    let row = GtkBox::new(Orientation::Horizontal, 12);
    row.append(&Image::from_icon_name(icon));
    let l = Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_hexpand(true);
    row.append(&l);
    let b = Button::new();
    b.set_child(Some(&row));
    b.add_css_class("flat");
    b
}

/// Сердечко и меню «⋯» справа от трека: волна по треку, дизлайк, плейлист, ссылка, переходы
pub fn track_actions(player: &PlayerManager, nav: Option<(&Stack, &Rc<DetailPage>)>, track: &Track) -> GtkBox {
    let bx = GtkBox::new(Orientation::Horizontal, 2);
    bx.set_valign(Align::Center);

    let is_liked = player.is_track_liked(&track.id);
    let heart = Button::with_label(heart_glyph(is_liked));
    heart.add_css_class("flat");
    heart.add_css_class("circular");
    heart.add_css_class("track-heart-btn");
    heart.set_tooltip_text(Some("Мне нравится"));
    if is_liked {
        heart.add_css_class("liked-heart");
    }
    {
        let (p, track) = (player.clone(), track.clone());
        heart.connect_clicked(move |b| {
            let like = !p.is_track_liked(&track.id);
            b.set_label(heart_glyph(like));
            if like {
                b.add_css_class("liked-heart");
            } else {
                b.remove_css_class("liked-heart");
            }
            p.set_track_liked(&track, like);
        });
    }
    bx.append(&heart);

    let menu = gtk4::MenuButton::new();
    menu.set_icon_name("view-more-symbolic");
    menu.add_css_class("flat");
    menu.add_css_class("circular");
    let pop = gtk4::Popover::new();
    let views = Stack::new();
    let main = GtkBox::new(Orientation::Vertical, 0);
    let plists = GtkBox::new(Orientation::Vertical, 0);
    views.add_named(&main, Some("main"));
    views.add_named(&plists, Some("playlists"));
    pop.set_child(Some(&views));
    pop.connect_closed({
        let views = views.clone();
        move |_| views.set_visible_child_name("main")
    });
    menu.set_popover(Some(&pop));

    let add = |icon: &str, text: &str, f: Box<dyn Fn()>| {
        let b = menu_item(icon, text);
        let pop = pop.clone();
        b.connect_clicked(move |_| {
            pop.popdown();
            f();
        });
        main.append(&b);
    };

    {
        let (p, t) = (player.clone(), track.clone());
        add("media-playlist-consecutive-symbolic", "Моя волна по треку", Box::new(move || p.play_single_track(t.clone())));
    }
    {
        let (p, id) = (player.clone(), track.id.clone());
        add("dialog-error-symbolic", "Не нравится", Box::new(move || {
            let id = id.clone();
            run_api(&p, Some("Трек скрыт из рекомендаций"), move |c, uid| async move { c.dislike_track(&uid, &id).await });
        }));
    }

    // «Добавить в плейлист» открывает вторую страницу с плейлистами пользователя
    let to_pl = menu_item("list-add-symbolic", "Добавить в плейлист");
    {
        let (p, t, views, plists) = (player.clone(), track.clone(), views.clone(), plists.clone());
        let pop = pop.clone();
        to_pl.connect_clicked(move |_| {
            clear_box(&plists);
            let back = menu_item("go-previous-symbolic", "Назад");
            let v = views.clone();
            back.connect_clicked(move |_| v.set_visible_child_name("main"));
            plists.append(&back);
            let create = menu_item("list-add-symbolic", "Создать плейлист");
            {
                let (p, t, pop) = (p.clone(), t.clone(), pop.clone());
                create.connect_clicked(move |b| {
                    pop.popdown();
                    let (p, t) = (p.clone(), t.clone());
                    ask_playlist_name(b, move |name| {
                        run_api(&p, Some("Плейлист создан, трек добавлен"), move |c, uid| async move {
                            let kind = c.create_playlist(&uid, &name).await?;
                            c.add_track_to_playlist(&uid, &kind, &t).await
                        });
                    });
                });
            }
            plists.append(&create);
            views.set_visible_child_name("playlists");
            let (p, t, plists, pop) = (p.clone(), t.clone(), plists.clone(), pop.clone());
            glib::spawn_future_local(async move {
                let client = p.client.lock().unwrap().clone();
                let lists = match client.get_uid().await {
                    Ok(uid) => client.get_user_playlists(&uid).await,
                    Err(e) => Err(e),
                };
                match lists {
                    Ok(lists) => {
                        for pl in lists {
                            let b = menu_item("view-list-symbolic", &pl.title);
                            let (p, t, pop, kind) = (p.clone(), t.clone(), pop.clone(), pl.kind.clone());
                            b.connect_clicked(move |_| {
                                pop.popdown();
                                let t = t.clone();
                                let kind = kind.clone();
                                run_api(&p, Some("Добавлено в плейлист"), move |c, uid| async move {
                                    c.add_track_to_playlist(&uid, &kind, &t).await
                                });
                            });
                            plists.append(&b);
                        }
                    }
                    Err(e) => p.emit(PlayerUiEvent::Error(format!("Не удалось загрузить плейлисты: {}", e))),
                }
            });
        });
    }
    main.append(&to_pl);

    {
        let (p, t) = (player.clone(), track.clone());
        add("edit-copy-symbolic", "Скопировать ссылку", Box::new(move || {
            let url = match t.albums.first() {
                Some(a) => format!("https://music.yandex.ru/album/{}/track/{}", a.id, t.id),
                None => format!("https://music.yandex.ru/track/{}", t.id),
            };
            if let Some(d) = gdk::Display::default() {
                d.clipboard().set_text(&url);
                p.emit(PlayerUiEvent::StatusMessage("Ссылка скопирована".into()));
            }
        }));
    }
    {
        let (p, t) = (player.clone(), track.clone());
        add("folder-download-symbolic", "Скачать", Box::new(move || {
            p.emit(PlayerUiEvent::StatusMessage("Скачивание…".into()));
            let (p, t) = (p.clone(), t.clone());
            glib::spawn_future_local(async move {
                let client = p.client.lock().unwrap().clone();
                match client.download_track(&t).await {
                    Ok(path) => p.emit(PlayerUiEvent::StatusMessage(format!("Сохранено: {}", path.display()))),
                    Err(e) => p.emit(PlayerUiEvent::Error(format!("Не удалось скачать: {}", e))),
                }
            });
        }));
    }
    {
        let (p, t) = (player.clone(), track.clone());
        add("folder-download-symbolic", "Сохранить офлайн (в приложении)", Box::new(move || {
            if crate::cache::offline_audio_path(&t.id).is_some() {
                crate::cache::remove_offline(&t.id);
                p.emit(PlayerUiEvent::StatusMessage("Удалено из офлайна".into()));
                return;
            }
            p.emit(PlayerUiEvent::StatusMessage("Загрузка для офлайна…".into()));
            let (p, t) = (p.clone(), t.clone());
            glib::spawn_future_local(async move {
                let client = p.client.lock().unwrap().clone();
                match client.save_track_offline(&t).await {
                    Ok(()) => p.emit(PlayerUiEvent::StatusMessage("Трек сохранён для прослушивания без интернета".into())),
                    Err(e) => p.emit(PlayerUiEvent::Error(format!("Не удалось сохранить: {}", e))),
                }
            });
        }));
    }
    {
        let (p, t) = (player.clone(), track.clone());
        add("video-x-generic-symbolic", "Трейлер", Box::new(move || {
            let (p, t) = (p.clone(), t.clone());
            glib::spawn_future_local(async move {
                let client = p.client.lock().unwrap().clone();
                match client.get_clip_url_for_track(&t).await {
                    // клип открывается в системном проигрывателе
                    Some(url) => {
                        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
                    }
                    None => p.emit(PlayerUiEvent::StatusMessage("У трека нет трейлера".into())),
                }
            });
        }));
    }
    {
        let (p, t) = (player.clone(), track.clone());
        add("code-symbolic", "Скопировать HTML-код", Box::new(move || {
            let album = t.albums.first().map(|a| a.id.clone()).unwrap_or_default();
            let src = if album.is_empty() {
                format!("https://music.yandex.ru/iframe/track/{}", t.id)
            } else {
                format!("https://music.yandex.ru/iframe/track/{}/{}", t.id, album)
            };
            if let Some(d) = gdk::Display::default() {
                d.clipboard().set_text(&format!(
                    "<iframe frameborder=\"0\" style=\"border:none;width:100%;height:100%;\" width=\"100%\" height=\"100%\" src=\"{}\"></iframe>",
                    src
                ));
                p.emit(PlayerUiEvent::StatusMessage("HTML-код скопирован".into()));
            }
        }));
    }
    {
        let (p, t, anchor) = (player.clone(), track.clone(), menu.clone());
        add("dialog-information-symbolic", "О треке", Box::new(move || show_track_credits(&p, &anchor, &t)));
    }
    // переходы нужны экрану со стеком страниц; без него (например, добавление в плейлист на лету) их нет
    if let (Some((stack, page)), Some(album)) = (nav, track.albums.first().cloned()) {
        let (p, stack, page) = (player.clone(), stack.clone(), page.clone());
        add("media-optical-symbolic", "Перейти к альбому", Box::new(move || open_album(&p, &stack, &page, &album)));
    }
    if let (Some((stack, page)), Some(artist)) = (nav, track.artists.first()) {
        let (p, stack, page, id) = (player.clone(), stack.clone(), page.clone(), artist.id.clone());
        add("avatar-default-symbolic", "Перейти к исполнителю", Box::new(move || open_artist(&p, &stack, &page, id.clone())));
    }

    bx.append(&menu);
    bx
}

/// Спрашивает название нового плейлиста; пустое название игнорируется
pub fn ask_playlist_name(anchor: &impl IsA<gtk4::Widget>, on_ok: impl FnOnce(String) + 'static) {
    let dialog = libadwaita::AlertDialog::new(Some("Новый плейлист"), None);
    let entry = gtk4::Entry::new();
    entry.set_placeholder_text(Some("Название"));
    entry.set_activates_default(true);
    dialog.set_extra_child(Some(&entry));
    dialog.add_response("cancel", "Отмена");
    dialog.add_response("create", "Создать");
    dialog.set_response_appearance("create", libadwaita::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("create"));
    dialog.choose(Some(anchor), None::<&gtk4::gio::Cancellable>, move |response| {
        let name = entry.text().trim().to_string();
        if response == "create" && !name.is_empty() {
            on_ok(name);
        }
    });
}

/// Плитка «Новый плейлист» с плюсом вместо обложки
pub fn new_playlist_tile(size: i32, on_click: impl Fn() + 'static) -> Button {
    let plus = Label::new(Some("+"));
    plus.add_css_class("new-playlist-plus");
    let cover = GtkBox::new(Orientation::Vertical, 0);
    cover.add_css_class("new-playlist-cover");
    cover.set_size_request(size, size);
    cover.append(&plus);
    plus.set_vexpand(true);
    plus.set_hexpand(true);
    let col = GtkBox::new(Orientation::Vertical, 4);
    col.set_width_request(size);
    col.append(&cover);
    col.append(&ellipsized("Новый плейлист", "tile-title"));
    let btn = Button::new();
    btn.set_child(Some(&col));
    btn.add_css_class("flat");
    btn.add_css_class("cover-tile");
    btn.set_hexpand(false);
    btn.connect_clicked(move |_| on_click());
    btn
}

/// Окно «О треке»: исполнитель, авторы музыки и текста, лейбл, источник фонограммы
fn show_track_credits(player: &PlayerManager, anchor: &impl IsA<gtk4::Widget>, track: &Track) {
    let content = GtkBox::new(Orientation::Vertical, 14);
    content.set_margin_top(12);
    content.set_margin_bottom(24);
    content.set_margin_start(24);
    content.set_margin_end(24);
    let status = Label::new(Some("Загрузка…"));
    status.add_css_class("dim-label");
    content.append(&status);

    let scrolled = ScrolledWindow::new();
    scrolled.set_child(Some(&content));
    let dialog = libadwaita::Dialog::new();
    dialog.set_title(&track.title);
    dialog.set_content_width(480);
    dialog.set_content_height(460);
    let toolbar = libadwaita::ToolbarView::new();
    toolbar.add_top_bar(&libadwaita::HeaderBar::new());
    toolbar.set_content(Some(&scrolled));
    dialog.set_child(Some(&toolbar));
    dialog.present(Some(anchor));

    let (p, id) = (player.clone(), track.id.clone());
    glib::spawn_future_local(async move {
        let client = p.client.lock().unwrap().clone();
        let credits = client.get_track_credits(&id).await;
        content.remove(&status);
        match credits {
            Ok(list) if !list.is_empty() => {
                for (title, value) in list {
                    let head = Label::new(Some(&title));
                    head.set_halign(Align::Start);
                    head.add_css_class("section-title");
                    let text = Label::new(Some(&value));
                    text.set_halign(Align::Start);
                    text.set_xalign(0.0);
                    text.set_wrap(true);
                    text.set_selectable(true);
                    text.add_css_class("dim-label");
                    content.append(&head);
                    content.append(&text);
                }
            }
            Ok(_) => {
                let l = Label::new(Some("Сведений о треке нет"));
                l.add_css_class("dim-label");
                content.append(&l);
            }
            Err(e) => {
                let l = Label::new(Some(&format!("Не удалось загрузить: {}", e)));
                l.set_wrap(true);
                content.append(&l);
            }
        }
    });
}

/// Подпись кнопки загрузки: «Удалить загрузки», если все треки уже сохранены
fn offline_label(tracks: &[Track]) -> &'static str {
    if !tracks.is_empty() && tracks.iter().all(|t| crate::cache::offline_audio_path(&t.id).is_some()) {
        "Удалить загрузки"
    } else {
        "Загрузить"
    }
}

/// Подпись источника очереди для плашки на экране Волны по типу страницы
fn source_label(kind: &str, title: &str) -> Option<String> {
    match kind {
        "Плейлист" => Some(format!("Плейлист: {title}")),
        "Альбом" => Some(format!("Альбом: {title}")),
        "Исполнитель" => Some(format!("Популярное: {title}")),
        "Трек" => Some(format!("Похожие на «{title}»")),
        "Коллекция" => Some(title.to_string()),
        _ => None,
    }
}

/// 5510287 → «5 510 287» (неразрывные пробелы)
fn group_digits(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push('\u{a0}');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod source_tests {
    use super::*;

    #[test]
    fn labels_and_digits() {
        assert_eq!(source_label("Плейлист", "Попа").as_deref(), Some("Плейлист: Попа"));
        assert_eq!(source_label("Исполнитель", "Три дня дождя").as_deref(), Some("Популярное: Три дня дождя"));
        assert_eq!(source_label("Что-то", "x"), None);
        assert_eq!(group_digits(5_510_287), "5\u{a0}510\u{a0}287");
        assert_eq!(group_digits(999), "999");
    }
}

/// Видео шапки принимается только по https с хоста Яндекса
fn trusted_video_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let host = rest.split('/').next().unwrap_or("");
    host.ends_with(".yandex.net") || host.ends_with(".yandex.ru")
}

#[cfg(test)]
mod video_url_tests {
    use super::*;

    #[test]
    fn only_yandex_https_hosts_are_trusted() {
        assert!(trusted_video_url("https://runtime.strm.yandex.ru/videos/a/b.mp4"));
        assert!(trusted_video_url("https://video-preview.s3.yandex.net/x.mp4"));
        assert!(!trusted_video_url("http://runtime.strm.yandex.ru/x.mp4"));
        assert!(!trusted_video_url("https://evil.example.com/yandex.ru/x.mp4"));
        assert!(!trusted_video_url("https://yandex.ru.evil.com/x.mp4"));
    }
}

/// Высота видео за шапкой артиста, px
const AMBIENT_HEIGHT: i32 = 580;
/// На сколько видео поднято выше верха страницы: верх кадра обычно тёмный пустой, его уводим за край
const AMBIENT_LIFT: i32 = 40;

/// Кадр для свечения за шапкой: вырезка под пропорции области, уменьшенная до 64 px по ширине.
/// Растягивание маленькой картинки и CSS-размытие дают мягкое цветное пятно; к низу и бокам оно
/// становится прозрачным (премультиплицированный RGBA) и затемняется, чтобы текст шапки читался.
fn ambient_frame(f: &super::wave_clip::ClipFrame, ratio: f32) -> (i32, i32, Vec<u8>) {
    const W: usize = 112;
    let (sw, sh) = (f.width.max(1) as usize, f.height.max(1) as usize);
    let (cw, ch) = if sw as f32 / sh as f32 > ratio { ((sh as f32 * ratio) as usize, sh) } else { (sw, (sw as f32 / ratio) as usize) };
    let (cw, ch) = (cw.clamp(1, sw), ch.clamp(1, sh));
    let (x0, y0) = ((sw - cw) / 2, ((sh - ch) as f32 * 0.3) as usize);
    let h = ((W as f32 / ratio.max(0.1)) as usize).clamp(4, 112);
    let smooth = |t: f32| {
        let t = t.clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let mut out = Vec::with_capacity(W * h * 4);
    for oy in 0..h {
        // прозрачность: сверху плотно, нижние 55% растворяются; по бокам короткое растворение
        let fy = smooth((h - 1 - oy) as f32 / (h - 1).max(1) as f32 / 0.62);
        for ox in 0..W {
            let fx = smooth(ox as f32 / W as f32 / 0.12) * smooth((W - 1 - ox) as f32 / W as f32 / 0.12);
            // среднее по блоку исходного кадра
            let (bx0, bx1) = (x0 + ox * cw / W, (x0 + (ox + 1) * cw / W).max(x0 + ox * cw / W + 1));
            let (by0, by1) = (y0 + oy * ch / h, (y0 + (oy + 1) * ch / h).max(y0 + oy * ch / h + 1));
            let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
            for y in (by0..by1.min(sh)).step_by(2) {
                for x in (bx0..bx1.min(sw)).step_by(2) {
                    let i = (y * sw + x) * 4;
                    r += f.data[i] as u32;
                    g += f.data[i + 1] as u32;
                    b += f.data[i + 2] as u32;
                    n += 1;
                }
            }
            let n = n.max(1) as f32;
            let a = fx * fy;
            let m = a * 0.88; // лёгкое затемнение: текст шапки читается, видео остаётся заметным
            out.extend_from_slice(&[(r as f32 / n * m) as u8, (g as f32 / n * m) as u8, (b as f32 / n * m) as u8, (a * 255.0) as u8]);
        }
    }
    (W as i32, h as i32, out)
}

#[cfg(test)]
mod ambient_tests {
    use super::*;
    use crate::ui::wave_clip::ClipFrame;

    #[test]
    fn background_is_small_and_fades_to_bottom_and_sides() {
        let f = ClipFrame { width: 640, height: 860, data: vec![200; 640 * 860 * 4] };
        let (w, h, d) = ambient_frame(&f, 4.0);
        assert_eq!((w, h), (112, 28));
        let alpha = |x: usize, y: usize| d[(y * w as usize + x) * 4 + 3];
        assert!(alpha(56, 0) > 240, "верх в центре плотный");
        assert!(alpha(56, 27) < 5, "низ прозрачен");
        assert!(alpha(0, 0) < 5 && alpha(111, 0) < 5, "края по бокам прозрачны");
        assert!(d[56 * 4] < 200, "кадр слегка затемнён");
    }
}

/// Загружает картинку в `Picture` (тот же кэш в памяти, что у обложек)
fn load_picture(pic: &gtk4::Picture, url: String) {
    if let Some(tex) = COVERS.with(|c| c.borrow().get(&url).cloned()) {
        pic.set_paintable(Some(&tex));
        return;
    }
    let weak = pic.downgrade();
    glib::spawn_future_local(async move {
        let Ok(resp) = reqwest::get(&url).await else { return };
        let Ok(bytes) = resp.bytes().await else { return };
        let Ok(tex) = gdk::Texture::from_bytes(&glib::Bytes::from(&bytes[..])) else { return };
        COVERS.with(|c| c.borrow_mut().insert(url, tex.clone()));
        if let Some(p) = weak.upgrade() {
            p.set_paintable(Some(&tex));
        }
    });
}

/// Просмотр фото артиста: листается стрелками на экране, клавишами и жестом, внизу точки
fn show_photo_gallery(anchor: &impl IsA<gtk4::Widget>, title: &str, urls: &[String]) {
    let carousel = libadwaita::Carousel::new();
    carousel.set_hexpand(true);
    carousel.set_vexpand(true);
    let mut pages = Vec::new();
    for url in urls {
        let pic = gtk4::Picture::new();
        pic.set_content_fit(gtk4::ContentFit::Contain);
        pic.set_can_shrink(true);
        pic.set_margin_top(8);
        pic.set_margin_bottom(8);
        load_picture(&pic, url.clone());
        carousel.append(&pic);
        pages.push(pic);
    }
    let dots = libadwaita::CarouselIndicatorDots::new();
    dots.set_carousel(Some(&carousel));
    dots.set_margin_bottom(12);

    // стрелки поверх картинки по бокам
    let arrow = |icon: &str, align: Align| {
        let b = Button::from_icon_name(icon);
        b.add_css_class("circular");
        b.add_css_class("osd");
        b.set_halign(align);
        b.set_valign(Align::Center);
        b.set_margin_start(12);
        b.set_margin_end(12);
        b
    };
    let (prev, next) = (arrow("go-previous-symbolic", Align::Start), arrow("go-next-symbolic", Align::End));
    let overlay = gtk4::Overlay::new();
    overlay.set_child(Some(&carousel));
    overlay.add_overlay(&prev);
    overlay.add_overlay(&next);

    let go = {
        let (carousel, pages) = (carousel.clone(), pages.clone());
        move |step: i32| {
            let n = pages.len() as i32;
            let cur = carousel.position().round() as i32;
            let target = (cur + step).rem_euclid(n) as usize;
            carousel.scroll_to(&pages[target], true);
        }
    };
    let go = Rc::new(go);
    {
        let go = go.clone();
        prev.connect_clicked(move |_| go(-1));
    }
    {
        let go = go.clone();
        next.connect_clicked(move |_| go(1));
    }
    let keys = gtk4::EventControllerKey::new();
    {
        let go = go.clone();
        keys.connect_key_pressed(move |_, key, _, _| match key {
            gdk::Key::Left => {
                go(-1);
                glib::Propagation::Stop
            }
            gdk::Key::Right => {
                go(1);
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
    }

    let content = GtkBox::new(Orientation::Vertical, 0);
    content.append(&overlay);
    content.append(&dots);
    let dialog = libadwaita::Dialog::new();
    dialog.set_title(title);
    dialog.set_content_width(720);
    dialog.set_content_height(760);
    dialog.add_controller(keys);
    // заголовок с крестиком: без него закрыть можно только Esc
    let toolbar = libadwaita::ToolbarView::new();
    toolbar.add_top_bar(&libadwaita::HeaderBar::new());
    toolbar.set_content(Some(&content));
    dialog.set_child(Some(&toolbar));
    dialog.present(Some(anchor));
}
