//! Общие виджеты коллекций: обложки, плитки, строки треков и страница альбома/плейлиста.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{
    gdk, glib, Align, Box as GtkBox, Button, Image, Label, ListBox, Orientation, Overflow,
    ScrolledWindow, Stack,
};
use libadwaita::prelude::*;
use libadwaita::ActionRow;

use crate::api::models::{Album, Artist, Playlist, Track, TrackSource};
use crate::api::provider::ProviderId;
use crate::player::{PlayerManager, PlayerUiEvent, PlaySource, SourceTarget};

/// Кэш текстур обложек с ограничением по памяти.
///
/// Раньше сюда складывалась каждая увиденная обложка навсегда: после часа
/// поиска и альбомов процесс разрастался до сотен мегабайт. Теперь это LRU
/// с бюджетом 16 МБ; видимые картинки держат собственные ссылки и не ломаются.
const COVER_CACHE_BUDGET: usize = 16 * 1024 * 1024;

#[derive(Default)]
struct CoverCache {
    map: HashMap<String, gdk::Texture>,
    order: VecDeque<String>,
    bytes: usize,
}

impl CoverCache {
    fn texture_bytes(tex: &gdk::Texture) -> usize {
        (tex.width() as usize)
            .saturating_mul(tex.height() as usize)
            .saturating_mul(4)
    }

    fn get(&mut self, key: &str) -> Option<gdk::Texture> {
        let tex = self.map.get(key)?.clone();
        if let Some(pos) = self.order.iter().position(|item| item == key) {
            self.order.remove(pos);
        }
        self.order.push_back(key.to_owned());
        Some(tex)
    }

    fn insert(&mut self, key: String, tex: gdk::Texture) {
        let size = Self::texture_bytes(&tex);
        // Одиночная гигантская обложка не должна вытеснять весь кэш.
        if size > COVER_CACHE_BUDGET / 2 {
            return;
        }
        if let Some(old) = self.map.remove(&key) {
            self.bytes = self.bytes.saturating_sub(Self::texture_bytes(&old));
        }
        if let Some(pos) = self.order.iter().position(|item| *item == key) {
            self.order.remove(pos);
        }
        self.bytes = self.bytes.saturating_add(size);
        self.map.insert(key.clone(), tex);
        self.order.push_back(key);
        while self.bytes > COVER_CACHE_BUDGET {
            let Some(old_key) = self.order.pop_front() else {
                break;
            };
            if let Some(old) = self.map.remove(&old_key) {
                self.bytes = self.bytes.saturating_sub(Self::texture_bytes(&old));
            }
        }
    }
}

thread_local! {
    static COVERS: RefCell<CoverCache> = RefCell::new(CoverCache::default());
}

/// Сколько сейчас занимают текстуры обложек в памяти и каков лимит (байты).
pub fn cover_cache_usage() -> (usize, usize) {
    COVERS.with(|cache| {
        let cache = cache.borrow();
        (cache.bytes, COVER_CACHE_BUDGET)
    })
}

/// Полностью очищает кэш текстур обложек; возвращает освобождённые байты.
/// Видимые картинки продолжают работать: у них свои ссылки на текстуры.
pub fn clear_cover_memory_cache() -> usize {
    COVERS.with(|cache| {
        let mut cache = cache.borrow_mut();
        let bytes = cache.bytes;
        cache.map.clear();
        cache.order.clear();
        cache.bytes = 0;
        bytes
    })
}

fn shared_http_client() -> &'static reqwest::Client {
    static HTTP_CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36")
            .build()
            .unwrap_or_default()
    })
}

#[derive(Clone)]
struct BannerRgba {
    width: i32,
    height: i32,
    rgba: Vec<u8>,
}

/// Загружает обложку по URL в `Image` (кэш в памяти; при ошибке остаётся заглушка)
pub fn load_cover(image: &Image, url: Option<String>) {
    let Some(url) = url else { return };
    if let Some(tex) = COVERS.with(|c| c.borrow_mut().get(&url)) {
        image.set_paintable(Some(&tex));
        return;
    }
    let client = shared_http_client().clone();
    let weak = image.downgrade();
    glib::spawn_future_local(async move {
        let Ok(resp) = client.get(&url).send().await else { return };
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

/// Загружает обложку трека. Если у трека обложка отсутствует или ведет на i.ytimg.com,
/// сначала отображается быстрый превью (если есть), а параллельно в фоне вызывается
/// `resolve_studio_cover`. При нахождении студийной обложки текстура плавно заменяется.
pub fn load_track_cover(image: &Image, track: &Track) {
    let preview_url = track.cover_url(100);
    load_cover(image, preview_url.clone());

    let is_ytimg = preview_url.as_deref().map_or(false, |u| u.contains("ytimg.com"));
    let needs_studio = preview_url.is_none() || is_ytimg;

    if needs_studio {
        let artist = track.artists_string();
        let title = track.title.clone();
        let weak = image.downgrade();
        glib::spawn_future_local(async move {
            if let Some(studio_url) = crate::cache::resolve_studio_cover(&artist, &title).await {
                if let Some(img) = weak.upgrade() {
                    load_cover(&img, Some(studio_url));
                }
            }
        });
    }
}

/// Квадратная скруглённая обложка трека с автоматическим фоновым резолвом студийной обложки
pub fn track_cover_image(size: i32, track: &Track) -> Image {
    let img = Image::from_icon_name("audio-x-generic-symbolic");
    img.set_pixel_size(size);
    img.set_halign(Align::Start);
    img.set_valign(Align::Start);
    img.set_overflow(Overflow::Hidden);
    img.add_css_class("cover-art");
    load_track_cover(&img, track);
    img
}

/// Загружает аватар артиста в `Image`. Сначала отображает `fallback_url` (если есть).
/// Параллельно в фоне через `cover_resolver::resolve_artist_avatar` ищет студийный квадратный
/// портрет 1:1 в Deezer/iTunes и при нахождении плавно обновляет текстуру.
pub fn load_artist_avatar(image: &Image, artist_name: &str, fallback_url: Option<String>) {
    load_cover(image, fallback_url);

    let trimmed = artist_name.trim();
    if trimmed.is_empty() {
        return;
    }

    let name = trimmed.to_string();
    let weak = image.downgrade();
    glib::spawn_future_local(async move {
        if let Some(avatar_url) = crate::cache::cover_resolver::resolve_artist_avatar(&name).await {
            if let Some(img) = weak.upgrade() {
                load_cover(&img, Some(avatar_url));
            }
        }
    });
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
    row.add_prefix(&track_cover_image(40, track));

    if track.is_explicit() {
        let badge = explicit_badge();
        badge.set_visible(true);
        row.add_suffix(&badge);
    }

    let dur_str = track.formatted_duration();
    if !dur_str.is_empty() {
        let dur = Label::new(Some(&dur_str));
        dur.add_css_class("dim-label");
        row.add_suffix(&dur);
    }

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
    let cover = cover_image(40, url.clone());
    if round {
        cover.add_css_class("round-cover");
        load_artist_avatar(&cover, title, url);
    }
    row.add_prefix(&cover);
    row.add_suffix(&Image::from_icon_name("go-next-symbolic"));
    row.connect_activated(move |_| on_activate());
    row
}

/// Строка исполнителя со скруглённой аватаркой и студийным разрешением 1:1 через Deezer/iTunes
pub fn artist_row(artist_name: &str, subtitle: &str, fallback_url: Option<String>, on_activate: impl Fn() + 'static) -> ActionRow {
    let row = ActionRow::builder()
        .title(glib::markup_escape_text(artist_name))
        .subtitle(glib::markup_escape_text(subtitle))
        .activatable(true)
        .build();
    let cover = cover_image(40, fallback_url.clone());
    cover.add_css_class("round-cover");
    load_artist_avatar(&cover, artist_name, fallback_url);
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
    /// Верхний чёткий слой фонового баннера YouTube Music (внутри `Overflow::Hidden` + scrim)
    banner_wrap: gtk4::Overlay,
    banner_pic: gtk4::Picture,
    banner_src: Rc<RefCell<Option<BannerRgba>>>,
    banner_rendered_w: Rc<std::cell::Cell<i32>>,
    banner_resize_pending: Rc<std::cell::Cell<bool>>,
    video: super::wave_clip::ClipController,
    video_shared: std::sync::Arc<super::wave_clip::ClipShared>,
    /// Номер показа видео/баннера: таймер кадров и запоздавшие загрузки устаревшего фона отбрасываются
    video_gen: Rc<std::cell::Cell<u32>>,
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
    /// Артисты, на которых ведут ссылки подписи
    pub link_artists: RefCell<Vec<(Artist, crate::api::models::TrackSource)>>,
    /// Справка об исполнителе из Википедии
    about: GtkBox,
    about_for: Rc<RefCell<String>>,
    back: Button,
    scrolled: ScrolledWindow,
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
    Artist(String, String, crate::api::models::TrackSource),
    Track(Track),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenersMetric {
    Monthly,
    LastFm,
}

impl ListenersMetric {
    pub fn format_label(&self, count: u64) -> String {
        format_listeners_label(count, *self)
    }
}

pub fn format_listeners_label(count: u64, metric: ListenersMetric) -> String {
    match metric {
        ListenersMetric::Monthly => format!("{} слушателей в месяц", group_digits(count)),
        ListenersMetric::LastFm => format!("{} слушателей на Last.fm", group_digits(count)),
    }
}

impl DetailPage {
    pub fn new(player: &PlayerManager, on_back: impl Fn() + 'static) -> Rc<Self> {
        let widget = GtkBox::new(Orientation::Vertical, 0);
        widget.set_hexpand(true);
        widget.set_vexpand(true);

        let back = Button::from_icon_name("go-previous-symbolic");
        back.add_css_class("flat");
        back.add_css_class("circular");
        back.add_css_class("detail-back-btn");
        back.set_halign(Align::Start);
        back.set_valign(Align::Start);
        back.set_margin_top(24);
        back.set_margin_start(32);
        back.set_tooltip_text(Some("Назад"));
        back.connect_clicked(move |_| on_back());
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

        let video_gen: Rc<std::cell::Cell<u32>> = Default::default();
        let banner_src: Rc<RefCell<Option<BannerRgba>>> = Default::default();
        let banner_rendered_w: Rc<std::cell::Cell<i32>> = Default::default();
        let banner_resize_pending: Rc<std::cell::Cell<bool>> = Default::default();

        let banner_pic = gtk4::Picture::new();
        banner_pic.set_content_fit(gtk4::ContentFit::Fill);
        banner_pic.set_can_shrink(true);
        banner_pic.set_can_target(false);
        banner_pic.add_css_class("artist-yt-banner");

        let banner_wrap = gtk4::Overlay::new();
        banner_wrap.set_can_target(false);
        banner_wrap.set_overflow(Overflow::Hidden);
        banner_wrap.set_visible(false);
        banner_wrap.set_child(Some(&GtkBox::new(Orientation::Vertical, 0)));
        banner_wrap.add_overlay(&banner_pic);
        {
            let (pic_ref, src_ref, rendered_w, pending, gen_ref, wrap_weak) = (
                banner_pic.clone(),
                banner_src.clone(),
                banner_rendered_w.clone(),
                banner_resize_pending.clone(),
                video_gen.clone(),
                banner_wrap.downgrade(),
            );
            banner_wrap.connect_get_child_position(move |ov, child| {
                if child != pic_ref.upcast_ref::<gtk4::Widget>() {
                    return None;
                }
                let paintable = pic_ref.paintable()?;
                let (sw, sh) = (paintable.intrinsic_width(), paintable.intrinsic_height());
                let (tw, th) = (ov.width(), ov.height());
                if tw > 16
                    && (tw - rendered_w.get()).abs() > 32
                    && !pending.get()
                    && src_ref.borrow().is_some()
                {
                    pending.set(true);
                    let token = gen_ref.get();
                    let (pic, src, rw, pend, gref, wweak) = (
                        pic_ref.clone(),
                        src_ref.clone(),
                        rendered_w.clone(),
                        pending.clone(),
                        gen_ref.clone(),
                        wrap_weak.clone(),
                    );
                    glib::idle_add_local_once(move || {
                        pend.set(false);
                        if gref.get() != token {
                            return;
                        }
                        let Some(wrap) = wweak.upgrade() else { return };
                        let cur_w = wrap.width();
                        if cur_w <= 16 {
                            return;
                        }
                        if let Some(banner) = src.borrow().as_ref() {
                            let crop = banner_cover_rect(
                                banner.width,
                                banner.height,
                                cur_w,
                                BANNER_HEIGHT,
                                BANNER_FOCUS_Y,
                            );
                            let faded = banner_fade_rgba(
                                &banner.rgba,
                                banner.width as usize,
                                banner.height as usize,
                                crop,
                                cur_w,
                                BANNER_HEIGHT,
                            );
                            let tex = gdk::MemoryTexture::new(
                                banner.width,
                                banner.height,
                                gdk::MemoryFormat::R8g8b8a8Premultiplied,
                                &glib::Bytes::from_owned(faded),
                                (banner.width * 4) as usize,
                            );
                            rw.set(cur_w);
                            pic.set_paintable(Some(&tex));
                        }
                    });
                }
                Some(banner_cover_rect(sw, sh, tw, th, BANNER_FOCUS_Y))
            });
        }
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
                        row.add_prefix(&track_cover_image(40, &t));
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
        scrolled.set_hexpand(true);
        scrolled.set_vexpand(true);
        scrolled.set_margin_top(68);
        scrolled.set_margin_start(32);
        scrolled.set_margin_end(32);
        content.set_margin_bottom(130);
        content.set_margin_end(14);
        scrolled.set_child(Some(&content));
        let page_overlay = gtk4::Overlay::new();
        page_overlay.set_hexpand(true);
        page_overlay.set_vexpand(true);
        page_overlay.set_child(Some(&gtk4::Box::new(Orientation::Vertical, 0)));
        page_overlay.add_overlay(&ambient_pic);
        page_overlay.add_overlay(&banner_wrap);
        page_overlay.add_overlay(&scrolled);
        page_overlay.add_overlay(&back);
        page_overlay.set_measure_overlay(&back, false);
        // видео и баннер занимают верх страницы на всю ширину и уезжают вверх вместе с прокруткой
        {
            let (bg, banner, adj) = (ambient_pic.clone(), banner_wrap.clone(), scrolled.vadjustment());
            page_overlay.connect_get_child_position(move |ov, child| {
                if child == bg.upcast_ref::<gtk4::Widget>() {
                    return Some(gdk::Rectangle::new(0, -(adj.value() as i32) - AMBIENT_LIFT, ov.width(), AMBIENT_HEIGHT));
                }
                if child == banner.upcast_ref::<gtk4::Widget>() {
                    return Some(gdk::Rectangle::new(0, -(adj.value() as i32), ov.width(), BANNER_HEIGHT));
                }
                None
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
                Some(Wave::Artist(id, name, source)) => p.play_artist_wave(id, name, *source),
                Some(Wave::Track(t)) => p.play_single_track(t.clone()),
                None => {}
            });
        }

        Rc::new(Self { widget, cover, kind, title, title_badge, subtitle, list, tracks, source: source.clone(), photos: photos.clone(), ambient_pic, banner_wrap, banner_pic, banner_src, banner_rendered_w, banner_resize_pending, video, video_shared, video_gen, listeners: listeners.clone(), listeners_label: listeners_label.clone(), player: player.clone(), wave_btn, wave, extra, link_handler, link_albums: RefCell::default(), link_artists: RefCell::default(), about, about_for: Rc::default(), back, scrolled, nav, owner_target, owner_box, empty, desc_btn, cover_btn, offline_btn, cover_handler: Default::default(), heart_btn, heart_handler: Default::default(), desc_handler: Default::default(), owner_btn, on_removed: RefCell::default() })
    }

    /// Синхронизирует боковые отступы содержимого и кнопки «Назад», сохраняя фон на всю ширину
    pub fn set_side_margins(&self, gutter: i32) {
        self.back.set_margin_start(gutter);
        self.scrolled.set_margin_start(gutter);
        self.scrolled.set_margin_end(gutter);
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
            if let Some(idx_str) = uri.strip_prefix("artist:") {
                if let Ok(idx) = idx_str.parse::<usize>() {
                    let pair_opt = page.link_artists.borrow().get(idx).cloned();
                    if let Some((artist, source)) = pair_opt {
                        open_artist(&p, &stack, &page, artist, source);
                    }
                }
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
        let wave = menu_item("media-playlist-consecutive-symbolic", "Волна по плейлисту");
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

    fn clear_yt_banner(&self) {
        self.banner_resize_pending.set(false);
        self.banner_rendered_w.set(0);
        *self.banner_src.borrow_mut() = None;
        self.banner_pic.set_paintable(None::<&gdk::Paintable>);
        self.banner_wrap.set_visible(false);
    }

    /// Включает зацикленное видео в шапке (беззвучно); кадры забирает таймер, пока страница на этом видео
    pub fn play_background_video(self: &Rc<Self>, path: &str) {
        self.clear_yt_banner();
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

    /// Загружает фоновый баннер артиста YouTube Music в верхний слой (`banner_pic`) с 4-сторонним затуханием
    pub fn load_background_banner(self: &Rc<Self>, url: String) {
        self.video.stop();
        self.ambient_pic.set_paintable(None::<&gdk::Paintable>);
        self.ambient_pic.set_visible(false);
        let token = self.video_gen.get() + 1;
        self.video_gen.set(token);
        let page = self.clone();
        glib::spawn_future_local(async move {
            let cache_key = format!("yt-banner-raw:{url}");
            let tex = if let Some(cached) = COVERS.with(|c| c.borrow_mut().get(&cache_key)) {
                cached
            } else {
                let Ok(resp) = shared_http_client().get(&url).send().await else { return };
                let Ok(bytes) = resp.bytes().await else { return };
                let Ok(tex) = gdk::Texture::from_bytes(&glib::Bytes::from(&bytes[..])) else { return };
                COVERS.with(|c| c.borrow_mut().insert(cache_key, tex.clone()));
                tex
            };

            if page.video_token() != token {
                return;
            }

            let (w, h) = (tex.width(), tex.height());
            if w <= 0 || h <= 0 {
                return;
            }

            let mut downloader = gdk::TextureDownloader::new(&tex);
            downloader.set_format(gdk::MemoryFormat::R8g8b8a8);
            let (raw_bytes, stride) = downloader.download_bytes();
            let row_bytes = (w as usize) * 4;
            let rgba = if stride == row_bytes && raw_bytes.len() >= row_bytes * (h as usize) {
                raw_bytes[..row_bytes * (h as usize)].to_vec()
            } else {
                let mut flat = Vec::with_capacity(row_bytes * (h as usize));
                for y in 0..(h as usize) {
                    let start = y * stride;
                    let end = start + row_bytes;
                    if end > raw_bytes.len() {
                        return;
                    }
                    flat.extend_from_slice(&raw_bytes[start..end]);
                }
                flat
            };

            let target_w = page.banner_wrap.width().max(page.widget.width()).max(900);
            let crop = banner_cover_rect(w, h, target_w, BANNER_HEIGHT, BANNER_FOCUS_Y);
            let faded = banner_fade_rgba(&rgba, w as usize, h as usize, crop, target_w, BANNER_HEIGHT);
            let banner_tex = gdk::MemoryTexture::new(
                w,
                h,
                gdk::MemoryFormat::R8g8b8a8Premultiplied,
                &glib::Bytes::from_owned(faded),
                (w * 4) as usize,
            );

            *page.banner_src.borrow_mut() = Some(BannerRgba { width: w, height: h, rgba });
            page.banner_rendered_w.set(target_w);
            page.ambient_pic.set_paintable(None::<&gdk::Paintable>);
            page.ambient_pic.set_visible(false);
            page.banner_pic.set_paintable(Some(&banner_tex));
            page.banner_wrap.set_visible(true);
            page.banner_wrap.queue_allocate();
        });
    }

    /// Останавливает видео/баннер шапки и возвращает обычную обложку
    pub fn stop_background_video(&self) {
        self.video_gen.set(self.video_gen.get() + 1);
        self.video.stop();
        self.ambient_pic.set_paintable(None::<&gdk::Paintable>);
        self.ambient_pic.set_visible(false);
        self.clear_yt_banner();
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

    /// Плашка количества слушателей на странице артиста
    pub fn set_listeners(&self, count: u64, metric: ListenersMetric) {
        self.listeners_label.set_text(&format_listeners_label(count, metric));
        self.listeners.set_visible(true);
    }

    /// Показывает кнопку «Волна по …»
    pub fn set_wave(&self, wave: Wave) {
        let label = match &wave {
            Wave::Artist(_, name, _) => format!("Волна по {}", name),
            Wave::Track(_) => "Волна по треку".to_string(),
        };
        self.wave_btn.set_label(&label);
        self.wave_btn.set_visible(true);
        *self.wave.borrow_mut() = Some(wave);
    }

    /// Подпись со ссылками: исполнители ведут на страницу артиста, альбом на страницу альбома
    pub fn set_subtitle_links(&self, artists: &[Artist], album: Option<&Album>, year: Option<u32>, source: crate::api::models::TrackSource) {
        *self.link_artists.borrow_mut() = artists.iter().map(|a| (a.clone(), source)).collect();
        for a in artists {
            self.player.register_artist(source, &a.id, &a.name);
        }
        let parts: Vec<String> = artists
            .iter()
            .enumerate()
            .map(|(idx, a)| format!("<a href=\"artist:{}\">{}</a>", idx, glib::markup_escape_text(&a.name)))
            .collect();
        let mut text = parts.join(", ");
        if let Some(al) = album {
            if !text.is_empty() {
                text.push_str(" · ");
            }
            let raw_title = al.title.as_deref().unwrap_or("");
            let display_title = if raw_title.trim().is_empty() || raw_title.trim().eq_ignore_ascii_case("unnamed") {
                "Альбом"
            } else {
                raw_title
            };
            text.push_str(&format!("<a href=\"album:{}\">{}</a>", al.id, glib::markup_escape_text(display_title)));
            *self.link_albums.borrow_mut() = vec![al.clone()];
        } else if let Some(y) = year {
            if !text.is_empty() {
                text.push_str(" · ");
            }
            text.push_str(&y.to_string());
        }
        self.subtitle.set_markup(&text);
    }

    /// Подгружает краткую справку об исполнителе (Википедия, Last.fm, Genius, Discogs)
    pub fn load_about(&self, artist: String) {
        let name = artist.clone();
        self.load_info(artist, "Об исполнителе", async move {
            // Википедия, затем Last.fm, затем Genius, затем Discogs
            if let Some(s) = crate::api::wiki::artist_summary(&name).await {
                return Some(s);
            }
            if let Some(s) = crate::api::lastfm::artist_summary(&name).await {
                return Some(s);
            }
            if let Some(s) = crate::api::genius::artist_summary(&name).await {
                return Some(s);
            }
            crate::api::discogs::artist_summary(&name).await
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
        load_cover(&self.cover, cover_url.clone());
        if cover_url.as_deref().map_or(false, |u| u.contains("ytimg.com")) {
            let weak_cover = self.cover.downgrade();
            let a_str = subtitle.to_string();
            let t_str = title.to_string();
            glib::spawn_future_local(async move {
                if let Some(studio_url) = crate::cache::resolve_studio_cover(&a_str, &t_str).await {
                    if let Some(img) = weak_cover.upgrade() {
                        load_cover(&img, Some(studio_url));
                    }
                }
            });
        }
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

        self.set_tracks(tracks);
    }

    /// Обновляет список треков на странице
    pub fn set_tracks(&self, tracks: Vec<Track>) {
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
        self.offline_btn.set_visible(
            !tracks.is_empty() && !tracks.iter().any(|track| track.source == TrackSource::Vk),
        );
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
                self.offline_btn.set_visible(track.source != TrackSource::Vk);
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
        let is_yt = match &source {
            Source::Album(id) => id.starts_with("MPRE") || p.providers.active_id() == ProviderId::YouTubeMusic,
            Source::Playlist(uid, _) => {
                uid.starts_with("VL")
                    || uid.starts_with("PL")
                    || uid.starts_with("RD")
                    || p.providers.active_id() == ProviderId::YouTubeMusic
            }
        };
        let is_vk = match &source {
            Source::Album(id) => id.starts_with("vk:"),
            Source::Playlist(uid, kind) => uid.starts_with("vk:") || kind.starts_with("vk:"),
        };
        let session = p.providers.session();

        let result: Result<Vec<Track>, String> = match &source {
            Source::Album(id) => {
                if is_vk {
                    match p.providers.get(ProviderId::Vk) {
                        Some(provider) => provider
                            .get_album_tracks(id)
                            .await
                            .map_err(|e| e.to_string()),
                        None => Err("Требуется вход во ВКонтакте".to_owned()),
                    }
                } else if is_yt {
                    p.yt_client.get_album_tracks(id).await.map_err(|e| e.to_string())
                } else {
                    let client = p.client.lock().unwrap().clone();
                    client.get_album_tracks(id).await.map_err(|e| e.to_string())
                }
            }
            Source::Playlist(uid, k) => {
                if is_vk {
                    let playlist = Playlist {
                        uid: uid.clone(),
                        kind: k.clone(),
                        title: title.clone(),
                        ..Default::default()
                    };
                    match p.providers.get(ProviderId::Vk) {
                        Some(provider) => provider
                            .get_playlist_tracks(&playlist)
                            .await
                            .map_err(|e| e.to_string()),
                        None => Err("Требуется вход во ВКонтакте".to_owned()),
                    }
                } else if is_yt {
                    p.yt_client.get_album_tracks(uid).await.map_err(|e| e.to_string())
                } else {
                    let client = p.client.lock().unwrap().clone();
                    client.get_playlist_tracks(uid, k).await.map_err(|e| e.to_string())
                }
            }
        };

        if !is_yt && !is_vk {
            ensure_liked(&p).await;
        }
        if !p.providers.is_current(session.id, session.generation) {
            return;
        }
        match result {
            Ok(tracks) => {
                let cover_url = cover_url.or_else(|| tracks.first().and_then(|t| t.cover_url(400)));
                let display_title = if title.trim().is_empty() || title.trim().eq_ignore_ascii_case("unnamed") {
                    "Альбом".to_string()
                } else {
                    title.clone()
                };
                page.show(kind, &display_title, &subtitle, cover_url, tracks);
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
                    if !is_yt && !is_vk {
                        let client = p.client.lock().unwrap().clone();
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
                }
                if let Some(al) = &album {
                    let track_source = if is_vk {
                        TrackSource::Vk
                    } else if is_yt {
                        TrackSource::YouTubeMusic
                    } else {
                        TrackSource::Yandex
                    };
                    page.set_subtitle_links(&al.artists, None, al.year, track_source);
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
    let title = album
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty() && !t.eq_ignore_ascii_case("unnamed"))
        .unwrap_or("Альбом")
        .to_string();
    open(
        player,
        stack,
        page,
        Source::Album(album.id.clone()),
        "Альбом",
        title,
        album_subtitle(album),
        album.cover_url(400),
        Some(album.clone()),
    );
}

/// Плитка альбома: клик открывает страницу альбома
pub fn album_tile(player: &PlayerManager, stack: &Stack, page: &Rc<DetailPage>, album: &Album, size: i32) -> Button {
    let raw_title = album.title.as_deref().unwrap_or("").trim();
    let title = if raw_title.is_empty() || raw_title.eq_ignore_ascii_case("unnamed") {
        "Альбом"
    } else {
        raw_title
    };
    let date_lbl = album.date_label();
    let sub = if !date_lbl.is_empty() {
        format!("{} • {}", album.type_badge(), date_lbl)
    } else {
        album_subtitle(album)
    };
    let (p, stack, page) = (player.clone(), stack.clone(), page.clone());
    let al = album.clone();
    tile(title, &sub, album.cover_url(300), size, false, move || open_album(&p, &stack, &page, &al))
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
pub fn artist_tile(player: &PlayerManager, stack: &Stack, page: &Rc<DetailPage>, artist: &Artist, source: TrackSource, size: i32) -> Button {
    let col = GtkBox::new(Orientation::Vertical, 4);
    col.set_width_request(size);
    let cover = cover_image(size, artist.cover_url(300));
    cover.add_css_class("round-cover");
    load_artist_avatar(&cover, &artist.name, artist.cover_url(300));
    col.append(&cover);
    col.append(&ellipsized(&artist.name, "tile-title"));
    col.append(&ellipsized("Исполнитель", "tile-sub"));

    let btn = Button::new();
    btn.set_child(Some(&col));
    btn.add_css_class("flat");
    btn.add_css_class("cover-tile");
    let (p, stack, page, artist) = (player.clone(), stack.clone(), page.clone(), artist.clone());
    btn.connect_clicked(move |_| {
        open_artist(&p, &stack, &page, artist.clone(), source);
    });
    btn
}

/// Страница артиста: популярные треки, альбомы и «Волна по артисту»
pub fn open_artist(
    player: &PlayerManager,
    stack: &Stack,
    page: &Rc<DetailPage>,
    artist: Artist,
    source: crate::api::models::TrackSource,
) {
    let (p, stack, page) = (player.clone(), stack.clone(), page.clone());
    glib::spawn_future_local(async move {
        p.register_artist(source, &artist.id, &artist.name);

        let brief_result: Result<crate::api::models::ArtistBrief, String> = match source {
            TrackSource::YouTubeMusic => {
                let clean_target = artist.id.strip_prefix("yt:").unwrap_or(&artist.id);
                let target = if clean_target.starts_with("UC") {
                    clean_target
                } else if !artist.name.trim().is_empty() {
                    &artist.name
                } else {
                    clean_target
                };
                p.yt_client.get_artist(target).await.map_err(|e| e.to_string())
            }
            TrackSource::Vk => {
                if let Some(vk) = p.providers.get(ProviderId::Vk) {
                    vk.get_artist_brief(&artist).await.map_err(|e| e.to_string())
                } else {
                    Err("Требуется вход во ВКонтакте".to_string())
                }
            }
            TrackSource::Yandex => {
                let clean_id = artist.id.strip_prefix("ya:").unwrap_or(&artist.id);
                let client = p.client.lock().unwrap().clone();
                match client.get_artist_brief(clean_id).await {
                    Ok(mut brief) => {
                        // Загружаем полную дискографию (все альбомы и синглы) через direct-albums
                        if let Ok(direct_albums) = client.get_artist_direct_albums(clean_id).await {
                            if !direct_albums.is_empty() {
                                let mut albums = direct_albums;
                                for a in &mut albums {
                                    a.ensure_month_parsed();
                                }
                                brief.albums = albums;
                            }
                        }
                        Ok(brief)
                    }
                    Err(e) => Err(e.to_string()),
                }
            }
        };

        match brief_result {
            Ok(brief) => {
                if source == TrackSource::Yandex {
                    ensure_liked(&p).await;
                }
                let a = &brief.artist;
                page.show("Исполнитель", &a.name, "", None, brief.popular_tracks);
                page.set_round_cover(true);
                load_artist_avatar(&page.cover, &a.name, None);
                let real_artist_id = match source {
                    TrackSource::Vk => {
                        if a.id.starts_with("vk:") {
                            a.id.clone()
                        } else if !a.name.trim().is_empty() {
                            format!("vk:name:{}", glib::Uri::escape_string(&a.name, None, true))
                        } else {
                            format!("vk:{}", a.id)
                        }
                    }
                    TrackSource::YouTubeMusic => {
                        if a.id.starts_with("yt:") {
                            a.id.clone()
                        } else {
                            format!("yt:{}", a.id)
                        }
                    }
                    TrackSource::Yandex => {
                        if a.id.starts_with("ya:") {
                            a.id.clone()
                        } else {
                            format!("ya:{}", a.id)
                        }
                    }
                };
                p.register_artist(source, &a.id, &a.name);
                p.register_artist(source, &real_artist_id, &a.name);
                page.set_source_target(SourceTarget::Artist(real_artist_id.clone()));
                let photos: Vec<String> = brief
                    .all_covers
                    .iter()
                    .filter_map(|c| c.uri.clone())
                    .map(|uri| crate::api::models::reachable_image_url(&uri))
                    .collect();
                page.set_photos(&photos);

                // Если студийный аватар найден, добавляем его первой фотографией в галерею
                let weak_page = Rc::downgrade(&page);
                let artist_name = a.name.clone();
                glib::spawn_future_local(async move {
                    if let Some(avatar_url) = crate::cache::cover_resolver::resolve_artist_avatar(&artist_name).await {
                        if let Some(pg) = weak_page.upgrade() {
                            let mut current = pg.photos.borrow().clone();
                            if !current.contains(&avatar_url) {
                                current.insert(0, avatar_url);
                                pg.set_photos(&current);
                            }
                        }
                    }
                });

                if let Some(n) = brief.stats.as_ref().and_then(|s| s.last_month_listeners) {
                    page.set_listeners(n, ListenersMetric::Monthly);
                } else {
                    page.listeners.set_visible(false);
                    let weak = Rc::downgrade(&page);
                    let a_name = a.name.clone();
                    glib::spawn_future_local(async move {
                        if let Ok(Some(count)) = crate::cache::lastfm::get_artist_listeners(&a_name).await {
                            if let Some(pg) = weak.upgrade() {
                                pg.set_listeners(count, ListenersMetric::LastFm);
                            }
                        }
                    });
                }

                // видео шапки (Яндекс) или фоновый баннер (только YouTube Music)
                if source == TrackSource::YouTubeMusic {
                    if let Some(url) = brief
                        .background_image_url
                        .as_deref()
                        .map(crate::api::models::reachable_image_url)
                        .filter(|u| u.starts_with("https://"))
                    {
                        page.load_background_banner(url);
                    }
                } else if let Some(url) = brief.background_video_url.clone().filter(|u| trusted_video_url(u)) {
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

                if source == TrackSource::Vk {
                    page.wave_btn.set_visible(false);
                } else {
                    page.set_wave(Wave::Artist(real_artist_id.clone(), a.name.clone(), source));
                }

                if !brief.albums.is_empty() {
                    let mut by_year: std::collections::BTreeMap<u32, Vec<Album>> =
                        std::collections::BTreeMap::new();
                    let mut without_year: Vec<Album> = Vec::new();

                    for mut album in brief.albums {
                        album.ensure_month_parsed();
                        if let Some(year) = album.year {
                            by_year.entry(year).or_default().push(album);
                        } else {
                            without_year.push(album);
                        }
                    }

                    for (year, mut albums_in_year) in by_year.into_iter().rev() {
                        albums_in_year.sort_by(|a, b| {
                            b.month.cmp(&a.month)
                                .then_with(|| b.release_date.cmp(&a.release_date))
                        });
                        let title = format!("{} — {} рел.", year, albums_in_year.len());
                        page.extra.append(&section_title(&title));
                        let (row, inner) = tile_row();
                        for album in &albums_in_year {
                            inner.append(&album_tile(&p, &stack, &page, album, 160));
                        }
                        page.extra.append(&row);
                    }

                    if !without_year.is_empty() {
                        page.extra.append(&section_title("Другие релизы"));
                        let (row, inner) = tile_row();
                        for album in &without_year {
                            inner.append(&album_tile(&p, &stack, &page, album, 160));
                        }
                        page.extra.append(&row);
                    }
                }

                if !brief.playlists.is_empty() {
                    page.extra.append(&section_title("Плейлисты"));
                    let (row, inner) = tile_row();
                    for pl in &brief.playlists {
                        inner.append(&playlist_tile(&p, &stack, &page, pl, 160));
                    }
                    page.extra.append(&row);
                }

                stack.set_visible_child_name("detail");
            }
            Err(e) => {
                p.emit(PlayerUiEvent::Error(format!("Не удалось загрузить артиста: {}", e)));
            }
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
        page.set_subtitle_links(&track.artists, track.albums.first(), None, track.source);
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

    // Колесо мыши прокручивает ряд по горизонтали: у горизонтального
    // ScrolledWindow без этого вертикальный скролл игнорируется вообще.
    let hadj = scrolled.hadjustment();
    let anim_holder: Rc<RefCell<Option<libadwaita::TimedAnimation>>> = Rc::default();
    let target_value = Rc::new(std::cell::Cell::new(hadj.value()));
    let wheel = gtk4::EventControllerScroll::new(gtk4::EventControllerScrollFlags::VERTICAL);
    {
        let scrolled_for_anim = scrolled.clone();
        wheel.connect_scroll(move |_, _dx, dy| {
            let page = hadj.page_size();
            // шаг поменьше и с анимацией: колесо прокручивает ряд мягко, а не рывком
            let step = if page > 1.0 { page * 0.16 } else { 64.0 };
            let max = (hadj.upper() - page).max(hadj.lower());
            let current = hadj.value();
            let mut base = target_value.get();
            // если значение увели далеко (протяжка скроллбара) — считаем от факта
            if (current - base).abs() > step {
                base = current;
            }
            let next = (base + dy * step).clamp(hadj.lower(), max);
            target_value.set(next);
            if (next - current).abs() < 0.5 {
                if (dy < 0.0 && current <= hadj.lower() + 0.5)
                    || (dy > 0.0 && current >= max - 0.5)
                {
                    // край ряда: отдаём событие странице
                    return gtk4::glib::Propagation::Proceed;
                }
                return gtk4::glib::Propagation::Stop;
            }
            if let Some(previous) = anim_holder.borrow_mut().take() {
                previous.pause();
            }
            let from = current;
            let adjustment = hadj.clone();
            let target = libadwaita::CallbackAnimationTarget::new(move |v| {
                adjustment.set_value(from + (next - from) * v);
            });
            let animation =
                libadwaita::TimedAnimation::new(&scrolled_for_anim, 0.0, 1.0, 170, target);
            animation.set_easing(libadwaita::Easing::EaseOutCubic);
            let holder = anim_holder.clone();
            animation.connect_done(move |_| {
                *holder.borrow_mut() = None;
            });
            animation.play();
            *anim_holder.borrow_mut() = Some(animation);
            gtk4::glib::Propagation::Stop
        });
    }
    scrolled.add_controller(wheel);
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
        b
    };

    {
        let (p, t) = (player.clone(), track.clone());
        let wave = add(
            "media-playlist-consecutive-symbolic",
            "Волна по треку",
            Box::new(move || p.play_single_track(t.clone())),
        );
        // VK Mix не принимает ID отдельного трека как seed.
        wave.set_visible(track.source != TrackSource::Vk);
    }
    {
        let (p, t) = (player.clone(), track.clone());
        let is_yt = t.source == TrackSource::YouTubeMusic
            || t.id.len() == 11
            || p.providers.active_id() == ProviderId::YouTubeMusic;
        let is_vk = t.source == TrackSource::Vk;
        add(
            "dialog-error-symbolic",
            "Не нравится",
            Box::new(move || {
                if is_vk {
                    let (p, t) = (p.clone(), t.clone());
                    glib::spawn_future_local(async move {
                        let result = match p.providers.get(ProviderId::Vk) {
                            Some(provider) => provider.set_track_dislike(&t.id).await,
                            None => Err("Требуется вход во ВКонтакте".into()),
                        };
                        match result {
                            Ok(()) => {
                                p.emit(PlayerUiEvent::StatusMessage(
                                    "Трек скрыт из рекомендаций 👎".into(),
                                ));
                                let is_current = p
                                    .state
                                    .lock()
                                    .unwrap()
                                    .current_track
                                    .as_ref()
                                    .map(|cur| cur.id == t.id)
                                    .unwrap_or(false);
                                if is_current {
                                    p.next_track();
                                }
                            }
                            Err(error) => p.emit(PlayerUiEvent::Error(format!(
                                "Не удалось скрыть трек VK: {error}"
                            ))),
                        }
                    });
                } else if is_yt {
                    let (p, t) = (p.clone(), t.clone());
                    glib::spawn_future_local(async move {
                        if let Some(yt) = p.providers.get(ProviderId::YouTubeMusic) {
                            let _ = yt.set_track_like(&t.id, false).await;
                        }
                        let _ = p.yt_client.dislike_track(&t.id).await;
                        p.emit(PlayerUiEvent::StatusMessage(
                            "Трек скрыт из рекомендаций 👎".into(),
                        ));
                        let is_current = p
                            .state
                            .lock()
                            .unwrap()
                            .current_track
                            .as_ref()
                            .map(|cur| cur.id == t.id)
                            .unwrap_or(false);
                        if is_current {
                            p.next_track();
                        }
                    });
                } else {
                    let id = t.id.clone();
                    run_api(&p, Some("Трек скрыт из рекомендаций 👎"), move |c, uid| async move {
                        c.dislike_track(&uid, &id).await
                    });
                }
            }),
        );
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
            let is_yt = t.source == TrackSource::YouTubeMusic || p.providers.active_id() == ProviderId::YouTubeMusic;
            {
                let (p, t, pop) = (p.clone(), t.clone(), pop.clone());
                create.connect_clicked(move |b| {
                    pop.popdown();
                    if is_yt {
                        let yt = p.yt_client.clone();
                        let p_clone = p.clone();
                        let t_clone = t.clone();
                        ask_playlist_name(b, move |name| {
                            glib::spawn_future_local(async move {
                                match yt.create_playlist(&name, Some(&t_clone.id)).await {
                                    Ok(_) => p_clone.emit(PlayerUiEvent::StatusMessage("Плейлист создан, трек добавлен".into())),
                                    Err(e) => p_clone.emit(PlayerUiEvent::Error(format!("Ошибка создания плейлиста: {}", e))),
                                }
                            });
                        });
                    } else {
                        let (p, t) = (p.clone(), t.clone());
                        ask_playlist_name(b, move |name| {
                            run_api(&p, Some("Плейлист создан, трек добавлен"), move |c, uid| async move {
                                let kind = c.create_playlist(&uid, &name).await?;
                                c.add_track_to_playlist(&uid, &kind, &t).await
                            });
                        });
                    }
                });
            }
            plists.append(&create);
            views.set_visible_child_name("playlists");
            let (p, t, plists, pop) = (p.clone(), t.clone(), plists.clone(), pop.clone());
            glib::spawn_future_local(async move {
                if is_yt {
                    let lists = if let Some(yt_prov) = p.providers.get(ProviderId::YouTubeMusic) {
                        yt_prov.get_playlists().await
                    } else {
                        p.providers.current().get_playlists().await
                    };
                    match lists {
                        Ok(lists) => {
                            for pl in lists {
                                let b = menu_item("view-list-symbolic", &pl.title);
                                let yt = p.yt_client.clone();
                                let p_clone = p.clone();
                                let pl_id = pl.id().to_string();
                                let track_id = t.id.clone();
                                let pop = pop.clone();
                                b.connect_clicked(move |_| {
                                    pop.popdown();
                                    let (yt, p_clone, pl_id, track_id) = (yt.clone(), p_clone.clone(), pl_id.clone(), track_id.clone());
                                    glib::spawn_future_local(async move {
                                        match yt.add_track_to_playlist(&pl_id, &track_id).await {
                                            Ok(()) => p_clone.emit(PlayerUiEvent::StatusMessage("Добавлено в плейлист".into())),
                                            Err(e) => p_clone.emit(PlayerUiEvent::Error(format!("Не удалось добавить: {}", e))),
                                        }
                                    });
                                });
                                plists.append(&b);
                            }
                        }
                        Err(e) => p.emit(PlayerUiEvent::Error(format!("Не удалось загрузить плейлисты: {}", e))),
                    }
                } else {
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
                }
            });
        });
    }
    to_pl.set_visible(track.source != TrackSource::Vk);
    main.append(&to_pl);

    {
        let (p, t) = (player.clone(), track.clone());
        add("edit-copy-symbolic", "Скопировать ссылку", Box::new(move || {
            let url = if t.source == TrackSource::Vk {
                format!(
                    "https://vk.com/audio{}",
                    t.id.strip_prefix("vk:").unwrap_or(&t.id)
                )
            } else if t.source == TrackSource::YouTubeMusic || t.id.len() == 11 {
                format!("https://music.youtube.com/watch?v={}", t.id)
            } else {
                match t.albums.first() {
                    Some(a) => format!("https://music.yandex.ru/album/{}/track/{}", a.id, t.id),
                    None => format!("https://music.yandex.ru/track/{}", t.id),
                }
            };
            if let Some(d) = gdk::Display::default() {
                d.clipboard().set_text(&url);
                p.emit(PlayerUiEvent::StatusMessage("Ссылка скопирована".into()));
            }
        }));
    }
    {
        let (p, t) = (player.clone(), track.clone());
        let download = add("folder-download-symbolic", "Скачать", Box::new(move || {
            p.emit(PlayerUiEvent::StatusMessage("Скачивание…".into()));
            let (p, t) = (p.clone(), t.clone());
            glib::spawn_future_local(async move {
                if t.source == TrackSource::YouTubeMusic || t.id.len() == 11 {
                    match p.yt_client.download_track_file(&t).await {
                        Ok(path) => p.emit(PlayerUiEvent::StatusMessage(format!("Сохранено: {}", path.display()))),
                        Err(e) => p.emit(PlayerUiEvent::Error(format!("Не удалось скачать: {}", e))),
                    }
                } else {
                    let client = p.client.lock().unwrap().clone();
                    match client.download_track(&t).await {
                        Ok(path) => p.emit(PlayerUiEvent::StatusMessage(format!("Сохранено: {}", path.display()))),
                        Err(e) => p.emit(PlayerUiEvent::Error(format!("Не удалось скачать: {}", e))),
                    }
                }
            });
        }));
        download.set_visible(track.source != TrackSource::Vk);
    }
    {
        let (p, t) = (player.clone(), track.clone());
        let offline = add("folder-download-symbolic", "Сохранить офлайн (в приложении)", Box::new(move || {
            if crate::cache::offline_audio_path(&t.id).is_some() {
                crate::cache::remove_offline(&t.id);
                p.emit(PlayerUiEvent::StatusMessage("Удалено из офлайна".into()));
                return;
            }
            p.emit(PlayerUiEvent::StatusMessage("Загрузка для офлайна…".into()));
            let (p, t) = (p.clone(), t.clone());
            glib::spawn_future_local(async move {
                if t.source == TrackSource::YouTubeMusic || t.id.len() == 11 {
                    match p.yt_client.save_track_offline(&t).await {
                        Ok(()) => p.emit(PlayerUiEvent::StatusMessage("Трек сохранён для прослушивания без интернета".into())),
                        Err(e) => p.emit(PlayerUiEvent::Error(format!("Не удалось сохранить: {}", e))),
                    }
                } else {
                    let client = p.client.lock().unwrap().clone();
                    match client.save_track_offline(&t).await {
                        Ok(()) => p.emit(PlayerUiEvent::StatusMessage("Трек сохранён для прослушивания без интернета".into())),
                        Err(e) => p.emit(PlayerUiEvent::Error(format!("Не удалось сохранить: {}", e))),
                    }
                }
            });
        }));
        offline.set_visible(track.source != TrackSource::Vk);
    }
    {
        let (p, t) = (player.clone(), track.clone());
        let trailer = add("video-x-generic-symbolic", "Трейлер", Box::new(move || {
            if t.source == TrackSource::YouTubeMusic || t.id.len() == 11 {
                let url = format!("https://www.youtube.com/watch?v={}", t.id);
                let _ = std::process::Command::new("xdg-open").arg(url).spawn();
            } else {
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
            }
        }));
        trailer.set_visible(track.source != TrackSource::Vk);
    }
    {
        let (p, t) = (player.clone(), track.clone());
        let html = add("code-symbolic", "Скопировать HTML-код", Box::new(move || {
            if t.source == TrackSource::YouTubeMusic || t.id.len() == 11 {
                let html = format!(
                    r#"<iframe width="560" height="315" src="https://www.youtube-nocookie.com/embed/{}" title="{}" frameborder="0" allow="accelerometer; autoplay; clipboard-write; encrypted-media; gyroscope; picture-in-picture; web-share" referrerpolicy="strict-origin-when-cross-origin" allowfullscreen></iframe>"#,
                    t.id, t.title
                );
                if let Some(d) = gdk::Display::default() {
                    d.clipboard().set_text(&html);
                    p.emit(PlayerUiEvent::StatusMessage("HTML-код YouTube скопирован".into()));
                }
            } else {
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
            }
        }));
        html.set_visible(track.source != TrackSource::Vk);
    }
    {
        let (p, t, anchor) = (player.clone(), track.clone(), menu.clone());
        add("dialog-information-symbolic", "О треке", Box::new(move || show_track_credits(&p, &anchor, &t)));
    }
    // переходы нужны экрану со стеком страниц; без него (например, добавление в плейлист на лету) их нет
    if let (Some((stack, page)), Some(album)) = (nav, track.albums.first().cloned()) {
        let (p, stack, page) = (player.clone(), stack.clone(), page.clone());
        let album_nav = add("media-optical-symbolic", "Перейти к альбому", Box::new(move || open_album(&p, &stack, &page, &album)));
        album_nav.set_visible(track.source != TrackSource::Vk);
    }
    if let Some((stack, page)) = nav {
        if track.artists.len() <= 1 {
            if let Some(artist) = track.artists.first() {
                let (p, stack, page, artist, source) = (player.clone(), stack.clone(), page.clone(), artist.clone(), track.source);
                add("avatar-default-symbolic", "Перейти к исполнителю", Box::new(move || open_artist(&p, &stack, &page, artist.clone(), source)));
            }
        } else {
            for a in &track.artists {
                let (p, stack, page, artist, source) = (player.clone(), stack.clone(), page.clone(), a.clone(), track.source);
                let label = format!("Перейти к {}", artist.name);
                add("avatar-default-symbolic", &label, Box::new(move || open_artist(&p, &stack, &page, artist.clone(), source)));
            }
        }
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

    if track.source == TrackSource::YouTubeMusic
        || track.source == TrackSource::Vk
        || track.id.len() == 11
    {
        content.remove(&status);
        let duration = track.formatted_duration();
        let album = track
            .albums
            .first()
            .and_then(|a| a.title.as_deref())
            .unwrap_or("Сингл");
        let (service, link) = match track.source {
            TrackSource::YouTubeMusic => (
                "YouTube Music",
                format!("https://music.youtube.com/watch?v={}", track.id),
            ),
            TrackSource::Vk => (
                "ВКонтакте",
                format!(
                    "https://vk.com/audio{}",
                    track.id.strip_prefix("vk:").unwrap_or(&track.id)
                ),
            ),
            TrackSource::Yandex => unreachable!(),
        };
        let items: [(&str, String); 6] = [
            ("Исполнитель", track.artists_string()),
            ("Альбом", album.to_string()),
            (
                "Длительность",
                if duration.is_empty() {
                    "—".to_string()
                } else {
                    duration
                },
            ),
            ("Сервис", service.to_string()),
            ("ID", track.id.clone()),
            ("Ссылка", link),
        ];
        for (title, value) in items {
            let head = Label::new(Some(title));
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
        return;
    }

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
pub fn group_digits(n: u64) -> String {
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
const AMBIENT_HEIGHT: i32 = 650;
/// На сколько видео поднято выше верха страницы: верх кадра обычно тёмный пустой, его уводим за край
const AMBIENT_LIFT: i32 = 40;
/// Высота чёткого верхнего слоя баннера YouTube Music, px (начинается от самого верха под HeaderBar)
const BANNER_HEIGHT: i32 = 490;
/// Верхне-смещённый Y-фокус (~22%), чтобы не обрезать головы артистов
const BANNER_FOCUS_Y: f32 = 0.22;

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Прямоугольник заполнения (`cover`) для баннера с заданным Y-фокусом (`0.0` = верх, `0.5` = центр)
fn banner_cover_rect(src_w: i32, src_h: i32, target_w: i32, target_h: i32, focus_y: f32) -> gdk::Rectangle {
    if src_w <= 0 || src_h <= 0 || target_w <= 0 || target_h <= 0 {
        return gdk::Rectangle::new(0, 0, target_w.max(1), target_h.max(1));
    }
    let src_ratio = src_w as f32 / src_h as f32;
    let target_ratio = target_w as f32 / target_h as f32;
    if src_ratio < target_ratio {
        let w = target_w;
        let h = ((target_w as f32 / src_ratio).round() as i32).max(target_h);
        let y = -(((h - target_h) as f32 * focus_y.clamp(0.0, 1.0)).round() as i32);
        gdk::Rectangle::new(0, y, w, h)
    } else {
        let h = target_h;
        let w = ((target_h as f32 * src_ratio).round() as i32).max(target_w);
        let x = -((w - target_w) / 2);
        gdk::Rectangle::new(x, 0, w, h)
    }
}

/// Подготавливает премультиплицированный RGBA для верхнего чёткого слоя баннера
/// с 4-сторонним плавным затуханием в фон `#141414` и мягким затемнением зоны текста/кнопок.
fn banner_fade_rgba(
    rgba: &[u8],
    sw: usize,
    sh: usize,
    crop: gdk::Rectangle,
    target_w: i32,
    target_h: i32,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(sw * sh * 4);
    if rgba.len() < sw * sh * 4
        || sw == 0
        || sh == 0
        || target_w <= 0
        || target_h <= 0
        || crop.width() <= 0
        || crop.height() <= 0
    {
        return out;
    }
    let cx = crop.x() as f32;
    let cy = crop.y() as f32;
    let cw = crop.width() as f32;
    let ch = crop.height() as f32;
    let tw = target_w as f32;
    let th = target_h as f32;
    let fade_bottom_start = th * 0.50;
    let fade_bottom_end = th * 0.95;
    let fade_bottom_span = (fade_bottom_end - fade_bottom_start).max(1.0);
    for sy in 0..sh {
        let screen_y = cy + (sy as f32 + 0.5) / sh as f32 * ch;
        let fy_top = smoothstep(screen_y / 14.0);
        let fy_bottom = smoothstep((fade_bottom_end - screen_y) / fade_bottom_span);
        let fy = fy_top * fy_bottom;
        if fy <= 0.001 {
            out.resize(out.len() + sw * 4, 0);
            continue;
        }
        let v = (screen_y / th).clamp(0.0, 1.0);
        let bottom_t = smoothstep(((v - 0.44) / 0.40).clamp(0.0, 1.0));
        let dim_y = 1.0 - 0.20 * bottom_t;
        let row_off = sy * sw * 4;
        for sx in 0..sw {
            let screen_x = cx + (sx as f32 + 0.5) / sw as f32 * cw;
            let fx_left = smoothstep(screen_x / 24.0);
            let fx_right = smoothstep((tw - screen_x) / 44.0);
            let fx = fx_left * fx_right;
            let i = row_off + sx * 4;
            let a = (rgba[i + 3] as f32 / 255.0) * fx * fy;
            if a <= 0.001 {
                out.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                let u = (screen_x / tw).clamp(0.0, 1.0);
                let left_t = smoothstep(1.0 - (u / 0.58).clamp(0.0, 1.0));
                let dim_x = 0.56 + 0.32 * (1.0 - left_t);
                let dim = dim_x * dim_y;
                let m = a * dim;
                out.extend_from_slice(&[
                    (rgba[i] as f32 * m) as u8,
                    (rgba[i + 1] as f32 * m) as u8,
                    (rgba[i + 2] as f32 * m) as u8,
                    (a * 255.0) as u8,
                ]);
            }
        }
    }
    out
}

/// Кадр для свечения за шапкой: вырезка под пропорции области, уменьшенная до 112 px по ширине.
/// Растягивание маленькой картинки и CSS-размытие дают мягкое цветное пятно; к низу и бокам оно
/// становится прозрачным (премультиплицированный RGBA) и затемняется, чтобы текст шапки читался.
fn ambient_rgba(rgba: &[u8], sw: usize, sh: usize, ratio: f32, y_focus: f32) -> (i32, i32, Vec<u8>) {
    const W: usize = 112;
    let (sw, sh) = (sw.max(1), sh.max(1));
    if rgba.len() < sw * sh * 4 {
        return (1, 1, vec![0, 0, 0, 0]);
    }
    let (cw, ch) = if sw as f32 / sh as f32 > ratio {
        ((sh as f32 * ratio) as usize, sh)
    } else {
        (sw, (sw as f32 / ratio) as usize)
    };
    let (cw, ch) = (cw.clamp(1, sw), ch.clamp(1, sh));
    let (x0, y0) = ((sw - cw) / 2, ((sh - ch) as f32 * y_focus.clamp(0.0, 1.0)) as usize);
    let h = ((W as f32 / ratio.max(0.1)) as usize).clamp(4, 112);
    let step_x = (cw / W / 3).max(2);
    let step_y = (ch / h / 3).max(2);
    let mut out = Vec::with_capacity(W * h * 4);
    for oy in 0..h {
        // прозрачность: сверху плотно, нижние 55% растворяются; по бокам короткое растворение
        let fy = smoothstep((h - 1 - oy) as f32 / (h - 1).max(1) as f32 / 0.62);
        for ox in 0..W {
            let fx = smoothstep(ox as f32 / W as f32 / 0.12)
                * smoothstep((W - 1 - ox) as f32 / W as f32 / 0.12);
            // среднее по блоку исходного кадра
            let (bx0, bx1) = (x0 + ox * cw / W, (x0 + (ox + 1) * cw / W).max(x0 + ox * cw / W + 1));
            let (by0, by1) = (y0 + oy * ch / h, (y0 + (oy + 1) * ch / h).max(y0 + oy * ch / h + 1));
            let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
            for y in (by0..by1.min(sh)).step_by(step_y) {
                for x in (bx0..bx1.min(sw)).step_by(step_x) {
                    let i = (y * sw + x) * 4;
                    r += rgba[i] as u32;
                    g += rgba[i + 1] as u32;
                    b += rgba[i + 2] as u32;
                    n += 1;
                }
            }
            let n = n.max(1) as f32;
            let a = fx * fy;
            let m = a * 0.88; // лёгкое затемнение: текст шапки читается, фон остаётся заметным
            out.extend_from_slice(&[
                (r as f32 / n * m) as u8,
                (g as f32 / n * m) as u8,
                (b as f32 / n * m) as u8,
                (a * 255.0) as u8,
            ]);
        }
    }
    (W as i32, h as i32, out)
}

fn ambient_frame(f: &super::wave_clip::ClipFrame, ratio: f32) -> (i32, i32, Vec<u8>) {
    ambient_rgba(&f.data, f.width.max(1) as usize, f.height.max(1) as usize, ratio, 0.3)
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

    #[test]
    fn banner_cover_rect_uses_top_weighted_focus_y() {
        // Баннер 1080x450 (2.4:1) в широкой области 1200x340 (3.53:1): масштабируется до 1200x500,
        // лишние 160 px распределяются с y-фокусом 0.22 => y = -35, а не -80 (центр)
        let r = banner_cover_rect(1080, 450, 1200, 340, BANNER_FOCUS_Y);
        assert_eq!(r.x(), 0);
        assert_eq!(r.width(), 1200);
        assert_eq!(r.height(), 500);
        assert_eq!(r.y(), -35);

        // В узкой области 600x340 (1.76:1 < 2.4:1) баннер центрируется по X: 816x340, x = -108, y = 0
        let r_narrow = banner_cover_rect(1080, 450, 600, 340, BANNER_FOCUS_Y);
        assert_eq!(r_narrow.y(), 0);
        assert_eq!(r_narrow.height(), 340);
        assert_eq!(r_narrow.width(), 816);
        assert_eq!(r_narrow.x(), -108);
    }

    #[test]
    fn banner_fade_rgba_fades_all_four_edges_and_dims_readability_zone() {
        let (w, h) = (1200, BANNER_HEIGHT as usize);
        let rgba = vec![255u8; w * h * 4];
        let crop = gdk::Rectangle::new(0, 0, w as i32, BANNER_HEIGHT);
        let faded = banner_fade_rgba(&rgba, w, h, crop, w as i32, BANNER_HEIGHT);
        let px = |x: usize, y: usize| -> [u8; 4] {
            let i = (y * w + x) * 4;
            [faded[i], faded[i + 1], faded[i + 2], faded[i + 3]]
        };

        assert!(px(600, 0)[3] < 5, "верхний край у y=0 плавно растворяется");
        assert_eq!(px(600, 24)[3], 255, "в верхней полосе под кнопкой Назад (y=24) баннер уже полностью непрозрачен");
        assert_eq!(px(600, h - 1), [0, 0, 0, 0], "нижний край полностью прозрачен без остатков цвета");
        assert!(px(0, 140)[3] < 5, "левый край плавно затухает");
        assert!(px(w - 1, 140)[3] < 5, "правый край у скроллбара плавно затухает");

        let face = px(760, 150);
        assert_eq!(face[3], 255, "верхне-центральная зона лиц полностью непрозрачна");
        assert!(face[0] >= 210, "зона лиц остаётся яркой: {}", face[0]);

        let left_header = px(120, 150);
        assert_eq!(left_header[3], 255, "левая зона заголовка непрозрачна");
        assert!(left_header[0] < face[0], "левая зона затемнена для читаемости текста");

        let lower_left = px(240, 320);
        assert!(lower_left[3] > 0 && lower_left[3] < 255, "нижняя зона плавно затухает");
        let lower_brightness = lower_left[0] as f32 / lower_left[3] as f32;
        let face_brightness = face[0] as f32 / face[3] as f32;
        assert!(lower_brightness < face_brightness, "нижняя зона темнее зоны лиц");
    }

    #[test]
    fn banner_fade_rgba_zeroes_cropped_out_pixels_outside_widget() {
        let (w, h) = (1200, BANNER_HEIGHT as usize);
        let rgba = [200u8, 200, 200, 255].repeat(w * h);
        let target_w = 600;
        let crop_narrow = banner_cover_rect(w as i32, h as i32, target_w, BANNER_HEIGHT, BANNER_FOCUS_Y);
        assert_eq!(crop_narrow.x(), -300);
        let faded_narrow = banner_fade_rgba(&rgba, w, h, crop_narrow, target_w, BANNER_HEIGHT);
        let px_narrow = |x: usize, y: usize| -> [u8; 4] {
            let i = (y * w + x) * 4;
            [faded_narrow[i], faded_narrow[i + 1], faded_narrow[i + 2], faded_narrow[i + 3]]
        };
        assert_eq!(px_narrow(10, 150), [0, 0, 0, 0], "обрезанный левый край строго прозрачен");
        assert_eq!(px_narrow(1190, 150), [0, 0, 0, 0], "обрезанный правый край строго прозрачен");
        assert!(px_narrow(300, 150)[3] < 5, "левая видимая граница плавно затухает");
        assert!(px_narrow(899, 150)[3] < 5, "правая видимая граница плавно затухает");
        assert_eq!(px_narrow(600, 150)[3], 255, "центр видимой области непрозрачен");

        let (sw, sh) = (1080usize, 450usize);
        let rgba_tall = [200u8, 200, 200, 255].repeat(sw * sh);
        let crop_wide = banner_cover_rect(sw as i32, sh as i32, 1200, BANNER_HEIGHT, BANNER_FOCUS_Y);
        assert!(crop_wide.y() < 0);
        let faded_wide = banner_fade_rgba(&rgba_tall, sw, sh, crop_wide, 1200, BANNER_HEIGHT);
        let px_wide = |x: usize, y: usize| -> [u8; 4] {
            let i = (y * sw + x) * 4;
            [faded_wide[i], faded_wide[i + 1], faded_wide[i + 2], faded_wide[i + 3]]
        };
        assert_eq!(px_wide(540, 0), [0, 0, 0, 0], "обрезанный верх строго прозрачен");
        assert_eq!(px_wide(540, sh - 1), [0, 0, 0, 0], "низ за 95% BANNER_HEIGHT строго прозрачен");
    }
}

/// Загружает картинку в `Picture` (тот же кэш в памяти, что у обложек)
fn load_picture(pic: &gtk4::Picture, url: String) {
    if let Some(tex) = COVERS.with(|c| c.borrow_mut().get(&url)) {
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
