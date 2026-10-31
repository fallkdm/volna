use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use async_channel::Receiver;
use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Button, Label, Orientation, Overlay, Overflow, ScrolledWindow, PolicyType,
};
use libadwaita::prelude::*;
use libadwaita::{
    Application, ApplicationWindow, HeaderBar, Toast, ToastOverlay, ViewStack, ViewSwitcher,
};

use crate::api::models::{parse_lrc, LrcLine};
use crate::player::{PlayerManager, PlayerUiEvent, RepeatMode};
use super::auth_dialog::show_auth_dialog;
use super::floating_island::{create_floating_island, FloatingIslandWidgets};
use super::library_view::create_library_view;
use super::preferences_dialog::show_preferences_dialog;
use super::search_view::create_search_view;
use super::wave_view::{create_wave_view, WaveViewWidgets};

pub struct VolnaWindow {
    pub window: ApplicationWindow,
}

impl VolnaWindow {
    pub fn new(
        app: &Application,
        player: PlayerManager,
        event_receiver: Receiver<PlayerUiEvent>,
    ) -> Self {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("Волна")
            .default_width(960)
            .default_height(680)
            .build();

        window.add_css_class("volna-main-window");

        let toast_overlay = ToastOverlay::new();

        // Главный GtkOverlay
        let root_overlay = Overlay::new();

        // HeaderBar
        let header_bar = HeaderBar::new();
        header_bar.add_css_class("fluid-header");

        // Бренд слева для Wave-First режима
        let header_brand_box = GtkBox::new(Orientation::Horizontal, 8);
        header_brand_box.set_valign(Align::Center);

        let brand_title = Label::new(Some("🌊 Волна"));
        brand_title.add_css_class("heading");
        header_brand_box.append(&brand_title);

        let brand_badge = Label::new(Some("Wave-First"));
        brand_badge.add_css_class("brand-badge");
        header_brand_box.append(&brand_badge);

        header_bar.pack_start(&header_brand_box);

        // ViewStack
        let view_stack = ViewStack::new();
        view_stack.set_vexpand(true);
        view_stack.set_hexpand(true);

        let view_switcher = ViewSwitcher::builder()
            .stack(&view_stack)
            .policy(libadwaita::ViewSwitcherPolicy::Wide)
            .build();
        header_bar.set_title_widget(Some(&view_switcher));

        // Правая часть HeaderBar для Wave-First режима: Кнопка профиля/авторизации + Настройки
        let header_right_box = GtkBox::new(Orientation::Horizontal, 8);

        let header_auth_btn = Button::with_label("Войти в Яндекс ID");
        header_auth_btn.add_css_class("suggested-action");
        header_right_box.append(&header_auth_btn);

        let header_settings_btn = Button::from_icon_name("emblem-system-symbolic");
        header_settings_btn.set_tooltip_text(Some("Настройки Волны и токен"));
        header_settings_btn.add_css_class("flat");
        header_settings_btn.add_css_class("circular");
        header_right_box.append(&header_settings_btn);

        header_bar.pack_end(&header_right_box);

        let is_playing_state = Rc::new(RefCell::new(false));
        let accent_color = Rc::new(RefCell::new([1.0_f64, 0.42_f64, 0.29_f64])); // #ff6b4a

        // Экран 1: Волна
        let wave_view = create_wave_view(&player, is_playing_state.clone(), accent_color);
        let wave_page = view_stack.add_titled(&wave_view.container, Some("wave"), "Волна");
        wave_page.set_icon_name(Some("audio-volume-high-symbolic"));

        // Экран 2: Поиск
        let search_view = create_search_view(&player);
        let search_page = view_stack.add_titled(&search_view.container, Some("search"), "Поиск");
        search_page.set_icon_name(Some("system-search-symbolic"));

        // Экран 3: Медиатека
        let library_view = create_library_view(&player);
        let lib_page = view_stack.add_titled(&library_view.container, Some("library"), "Медиатека");
        lib_page.set_icon_name(Some("folder-music-symbolic"));

        let nav_style = crate::api::auth::load_nav_style();
        let player_layout = crate::api::auth::load_player_layout();
        let theme_accent = crate::api::auth::load_theme_accent();
        let theme_accent_state = Rc::new(RefCell::new(theme_accent.clone()));

        apply_accent_classes(&window, &theme_accent);

        let dynamic_provider = gtk4::CssProvider::new();
        if let Some(display) = gtk4::gdk::Display::default() {
            gtk4::style_context_add_provider_for_display(
                &display,
                &dynamic_provider,
                gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
        }

        // ================= Монолитный левый сайдбар =================
        const SIDEBAR_EXPANDED_WIDTH: i32 = 240;
        const SIDEBAR_COLLAPSED_WIDTH: i32 = 68;
        let is_collapsed = Rc::new(RefCell::new(crate::api::auth::load_sidebar_collapsed()));
        let initial_collapsed = *is_collapsed.borrow();

        let is_spotify_nav = nav_style == "spotify";
        let is_spotify_nav_state = Rc::new(RefCell::new(is_spotify_nav));

        // Внешняя обёртка: на каждом кадре анимируется ТОЛЬКО её ширина (клипп).
        // Внутренности сайдбара всегда имеют постоянный размер и не перекладываются —
        // иначе разметка кнопок и подписей (Pango, ellipsize) на каждом кадре дёргает анимацию.
        let sidebar = GtkBox::new(Orientation::Vertical, 0);
        sidebar.add_css_class("sidebar");
        sidebar.add_css_class("spotify-sidebar");
        let initial_width = if initial_collapsed { SIDEBAR_COLLAPSED_WIDTH } else { SIDEBAR_EXPANDED_WIDTH };
        sidebar.set_width_request(initial_width);
        sidebar.set_halign(Align::Start);
        sidebar.set_valign(Align::Fill);
        sidebar.set_vexpand(true);
        sidebar.set_hexpand(false);
        sidebar.set_overflow(Overflow::Hidden);
        if initial_collapsed {
            sidebar.add_css_class("collapsed");
        }

        // 1. Верхняя часть: Логотип «🌊 Волна» слева + кнопка сворачивания справа
        // Вынесена из sidebar_panel напрямую в sidebar, чтобы ширина шапки следовала
        // за шириной сайдбара (68px <-> 240px), и кнопка collapse_btn была строго видна
        // и отцентрирована в свернутом состоянии.
        let header_box = GtkBox::new(Orientation::Horizontal, 0);
        header_box.add_css_class("sidebar-header-box");
        header_box.set_valign(Align::Center);
        header_box.set_hexpand(true);
        header_box.set_halign(Align::Fill);

        let brand_box = GtkBox::new(Orientation::Horizontal, 8);
        brand_box.set_valign(Align::Center);
        brand_box.set_halign(Align::Start);
        brand_box.set_hexpand(true);
        brand_box.set_margin_start(4);

        let brand_title = Label::new(Some("🌊 Волна"));
        brand_title.add_css_class("sidebar-brand-title");
        brand_title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        brand_title.set_width_chars(1);
        brand_box.append(&brand_title);

        // Кнопка сворачивания панели: 44x40 со строго центрированной иконкой 20px
        let collapse_btn = Button::new();
        collapse_btn.add_css_class("flat");
        collapse_btn.add_css_class("sidebar-collapse-btn");
        collapse_btn.set_size_request(44, 40);
        collapse_btn.set_focusable(false);
        collapse_btn.set_tooltip_text(Some(if initial_collapsed { "Развернуть панель" } else { "Свернуть панель" }));
        collapse_btn.set_valign(Align::Center);

        if initial_collapsed {
            brand_box.set_visible(false);
            brand_box.set_opacity(0.0);
            collapse_btn.set_halign(Align::Center);
            collapse_btn.set_hexpand(true);
        } else {
            brand_box.set_visible(true);
            brand_box.set_opacity(1.0);
            collapse_btn.set_halign(Align::End);
            collapse_btn.set_hexpand(false);
        }

        let collapse_icon = gtk4::Image::from_icon_name("view-sidebar-symbolic");
        collapse_icon.set_pixel_size(20);
        collapse_icon.set_halign(Align::Center);
        collapse_icon.set_valign(Align::Center);
        collapse_btn.set_child(Some(&collapse_icon));

        header_box.append(&brand_box);
        header_box.append(&collapse_btn);

        sidebar.append(&header_box);

        let sidebar_panel = GtkBox::new(Orientation::Vertical, 0);
        sidebar_panel.add_css_class("sidebar-panel");
        sidebar_panel.set_size_request(SIDEBAR_EXPANDED_WIDTH - 25, -1);
        sidebar_panel.set_vexpand(true);
        sidebar_panel.set_halign(Align::Start);
        sidebar_panel.set_valign(Align::Fill);

        let sidebar_clip = Overlay::new();
        sidebar_clip.set_vexpand(true);
        sidebar_clip.set_hexpand(true);
        sidebar_clip.set_halign(Align::Fill);
        sidebar_clip.set_valign(Align::Fill);
        sidebar_clip.set_overflow(Overflow::Hidden);

        // Пустой основной слой отвечает только за высоту; постоянная панель лежит поверх него.
        let sidebar_sizer = GtkBox::new(Orientation::Vertical, 0);
        sidebar_sizer.set_vexpand(true);
        sidebar_clip.set_child(Some(&sidebar_sizer));
        sidebar_clip.add_overlay(&sidebar_panel);
        sidebar_clip.set_measure_overlay(&sidebar_panel, false);
        sidebar.append(&sidebar_clip);

        // 2. Средняя часть: Навигационные кнопки (чистый стиль Spotify, без дублирующих эмодзи)
        let sidebar_nav_box = GtkBox::new(Orientation::Vertical, 2);
        sidebar_nav_box.add_css_class("sidebar-nav-container");
        sidebar_nav_box.set_vexpand(true);
        sidebar_nav_box.set_hexpand(true);
        sidebar_nav_box.set_overflow(Overflow::Hidden);

        let nav_wave = create_sidebar_nav_btn("radio-symbolic", "Волна", initial_collapsed);
        let nav_search = create_sidebar_nav_btn("edit-find-symbolic", "Поиск", initial_collapsed);
        let nav_liked = create_sidebar_nav_btn("emblem-favorite-symbolic", "Любимые треки", initial_collapsed);

        let btn_wave = nav_wave.btn.clone();
        let btn_search = nav_search.btn.clone();
        let btn_liked = nav_liked.btn.clone();

        sidebar_nav_box.append(&btn_wave);
        sidebar_nav_box.append(&btn_search);
        sidebar_nav_box.append(&btn_liked);
        sidebar_panel.append(&sidebar_nav_box);

        // 3. Нижняя часть: Профиль пользователя и кнопка настроек (всегда вертикальный стек)
        let sidebar_sep = gtk4::Separator::new(Orientation::Horizontal);
        sidebar_sep.add_css_class("sidebar-separator");
        sidebar_panel.append(&sidebar_sep);

        let sidebar_footer = GtkBox::new(Orientation::Vertical, 4);
        sidebar_footer.add_css_class("sidebar-footer");
        sidebar_footer.set_valign(Align::End);
        sidebar_footer.set_margin_bottom(8);
        sidebar_footer.set_margin_top(4);
        sidebar_footer.set_overflow(Overflow::Hidden);

        // Кнопка профиля (с оверлеем для нулевого влияния на ширину при анимации)
        let auth_btn = Button::new();
        auth_btn.add_css_class("flat");
        auth_btn.add_css_class("sidebar-profile-btn");
        auth_btn.set_focusable(false);
        auth_btn.set_halign(Align::Fill);
        auth_btn.set_hexpand(true);
        auth_btn.set_overflow(Overflow::Hidden);
        auth_btn.set_tooltip_text(Some("Войти в Яндекс ID"));

        let auth_overlay = Overlay::new();
        auth_overlay.set_hexpand(true);
        auth_overlay.set_halign(Align::Fill);
        auth_overlay.set_overflow(Overflow::Hidden);

        let auth_avatar_box = GtkBox::new(Orientation::Horizontal, 0);
        auth_avatar_box.set_size_request(44, 40);
        auth_avatar_box.set_halign(Align::Start);
        auth_avatar_box.set_valign(Align::Center);

        let auth_avatar = gtk4::Image::from_icon_name("avatar-default-symbolic");
        auth_avatar.set_pixel_size(24);
        auth_avatar.set_halign(Align::Center);
        auth_avatar.set_valign(Align::Center);
        auth_avatar.set_hexpand(true);
        auth_avatar.add_css_class("sidebar-avatar-icon");
        auth_avatar_box.append(&auth_avatar);

        auth_overlay.set_child(Some(&auth_avatar_box));

        let auth_name_box = GtkBox::new(Orientation::Horizontal, 0);
        auth_name_box.set_valign(Align::Center);
        auth_name_box.set_halign(Align::Start);
        auth_name_box.set_margin_start(44);
        auth_name_box.set_overflow(Overflow::Hidden);

        let auth_name_label = Label::new(Some("Войти в Яндекс ID"));
        auth_name_label.set_halign(Align::Start);
        auth_name_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        auth_name_label.set_width_chars(1);
        auth_name_label.add_css_class("sidebar-profile-name");
        auth_name_box.append(&auth_name_label);

        auth_overlay.add_overlay(&auth_name_box);
        auth_overlay.set_measure_overlay(&auth_name_box, false);

        if initial_collapsed {
            auth_name_box.set_visible(false);
            auth_name_box.set_opacity(0.0);
        }

        auth_btn.set_child(Some(&auth_overlay));

        // Кнопка настроек (с оверлеем)
        let settings_btn = Button::new();
        settings_btn.add_css_class("flat");
        settings_btn.add_css_class("sidebar-settings-btn");
        settings_btn.set_focusable(false);
        settings_btn.set_halign(Align::Fill);
        settings_btn.set_hexpand(true);
        settings_btn.set_overflow(Overflow::Hidden);
        settings_btn.set_tooltip_text(Some("Настройки Волны и токен"));

        let settings_overlay = Overlay::new();
        settings_overlay.set_hexpand(true);
        settings_overlay.set_halign(Align::Fill);
        settings_overlay.set_overflow(Overflow::Hidden);

        let settings_icon_box = GtkBox::new(Orientation::Horizontal, 0);
        settings_icon_box.set_size_request(44, 40);
        settings_icon_box.set_halign(Align::Start);
        settings_icon_box.set_valign(Align::Center);

        let settings_icon = gtk4::Image::from_icon_name("emblem-system-symbolic");
        settings_icon.set_pixel_size(20);
        settings_icon.set_halign(Align::Center);
        settings_icon.set_valign(Align::Center);
        settings_icon.set_hexpand(true);
        settings_icon.add_css_class("sidebar-nav-icon");
        settings_icon_box.append(&settings_icon);

        settings_overlay.set_child(Some(&settings_icon_box));

        let settings_label_box = GtkBox::new(Orientation::Horizontal, 0);
        settings_label_box.set_valign(Align::Center);
        settings_label_box.set_halign(Align::Start);
        settings_label_box.set_margin_start(44);
        settings_label_box.set_overflow(Overflow::Hidden);

        let settings_label = Label::new(Some("Настройки"));
        settings_label.set_halign(Align::Start);
        settings_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        settings_label.set_width_chars(1);
        settings_label.add_css_class("sidebar-nav-label");
        settings_label_box.append(&settings_label);

        settings_overlay.add_overlay(&settings_label_box);
        settings_overlay.set_measure_overlay(&settings_label_box, false);

        if initial_collapsed {
            settings_label_box.set_visible(false);
            settings_label_box.set_opacity(0.0);
        }

        settings_btn.set_child(Some(&settings_overlay));

        sidebar_footer.append(&auth_btn);
        sidebar_footer.append(&settings_btn);
        sidebar_panel.append(&sidebar_footer);

        // Плавная 60 FPS анимация сворачивания / разворачивания (TimedAnimation 250мс)
        {
            let is_collapsed = is_collapsed.clone();
            let sidebar = sidebar.clone();
            let c_btn = collapse_btn.clone();
            let brand_box = brand_box.clone();
            let auth_name_box = auth_name_box.clone();
            let settings_label_box = settings_label_box.clone();
            let nav_items = vec![nav_wave.clone(), nav_search.clone(), nav_liked.clone()];
            let anim_holder: Rc<RefCell<Option<libadwaita::TimedAnimation>>> = Rc::default();

            collapse_btn.connect_clicked(move |_| {
                if let Some(a) = anim_holder.borrow_mut().take() {
                    a.pause();
                }
                let currently_collapsed = *is_collapsed.borrow();
                let next_collapsed = !currently_collapsed;
                *is_collapsed.borrow_mut() = next_collapsed;
                let _ = crate::api::auth::save_sidebar_collapsed(next_collapsed);

                let from_w = if currently_collapsed { SIDEBAR_COLLAPSED_WIDTH as f64 } else { SIDEBAR_EXPANDED_WIDTH as f64 };
                let to_w = if next_collapsed { SIDEBAR_COLLAPSED_WIDTH as f64 } else { SIDEBAR_EXPANDED_WIDTH as f64 };

                c_btn.set_tooltip_text(Some(if next_collapsed { "Развернуть панель" } else { "Свернуть панель" }));

                if next_collapsed {
                    // При сворачивании СРАЗУ скрываем текст и центрируем кнопку:
                    // при сжатии сайдбара остаются только аккуратные иконки,
                    // и ни одна буква не торчит наружу лесенкой поверх волны.
                    brand_box.set_visible(false);
                    brand_box.set_opacity(0.0);
                    auth_name_box.set_visible(false);
                    auth_name_box.set_opacity(0.0);
                    settings_label_box.set_visible(false);
                    settings_label_box.set_opacity(0.0);
                    for item in &nav_items {
                        item.label_box.set_visible(false);
                        item.label_box.set_opacity(0.0);
                    }
                    c_btn.set_halign(Align::Center);
                    c_btn.set_hexpand(true);
                } else {
                    brand_box.set_visible(true);
                    brand_box.set_opacity(0.0);
                    auth_name_box.set_visible(true);
                    auth_name_box.set_opacity(0.0);
                    settings_label_box.set_visible(true);
                    settings_label_box.set_opacity(0.0);
                    for item in &nav_items {
                        item.label_box.set_visible(true);
                        item.label_box.set_opacity(0.0);
                    }
                    c_btn.set_halign(Align::End);
                    c_btn.set_hexpand(false);
                }

                let s = sidebar.clone();
                let bb = brand_box.clone();
                let ab = auth_name_box.clone();
                let sb = settings_label_box.clone();
                let items = nav_items.clone();

                let target = libadwaita::CallbackAnimationTarget::new(move |val| {
                    let w = val.round() as i32;
                    s.set_width_request(w);
                    let t = ((val - SIDEBAR_COLLAPSED_WIDTH as f64) / (SIDEBAR_EXPANDED_WIDTH as f64 - SIDEBAR_COLLAPSED_WIDTH as f64)).clamp(0.0, 1.0);
                    if !next_collapsed {
                        bb.set_opacity(t);
                        ab.set_opacity(t);
                        sb.set_opacity(t);
                        for item in &items {
                            item.label_box.set_opacity(t);
                        }
                    }
                });
                let anim = libadwaita::TimedAnimation::new(&sidebar, from_w, to_w, 180, target);
                anim.set_easing(libadwaita::Easing::EaseOutCubic);

                let s_done = sidebar.clone();
                let bb_done = brand_box.clone();
                let ab_done = auth_name_box.clone();
                let sb_done = settings_label_box.clone();
                let items_done = nav_items.clone();
                let c_btn_done = c_btn.clone();

                anim.connect_done(move |_| {
                    if next_collapsed {
                        s_done.set_width_request(SIDEBAR_COLLAPSED_WIDTH);
                        s_done.add_css_class("collapsed");
                        bb_done.set_visible(false);
                        bb_done.set_opacity(0.0);
                        ab_done.set_visible(false);
                        ab_done.set_opacity(0.0);
                        sb_done.set_visible(false);
                        sb_done.set_opacity(0.0);
                        for item in &items_done {
                            item.label_box.set_visible(false);
                            item.label_box.set_opacity(0.0);
                        }
                        c_btn_done.set_halign(Align::Center);
                        c_btn_done.set_hexpand(true);
                    } else {
                        s_done.set_width_request(SIDEBAR_EXPANDED_WIDTH);
                        s_done.remove_css_class("collapsed");
                        bb_done.set_visible(true);
                        bb_done.set_opacity(1.0);
                        bb_done.set_hexpand(true);
                        ab_done.set_visible(true);
                        ab_done.set_opacity(1.0);
                        sb_done.set_visible(true);
                        sb_done.set_opacity(1.0);
                        for item in &items_done {
                            item.label_box.set_visible(true);
                            item.label_box.set_opacity(1.0);
                        }
                        c_btn_done.set_halign(Align::End);
                        c_btn_done.set_hexpand(false);
                    }
                });
                anim.play();
                *anim_holder.borrow_mut() = Some(anim);
            });
        }

        sidebar.set_visible(is_spotify_nav);
        view_switcher.set_visible(!is_spotify_nav);
        header_brand_box.set_visible(!is_spotify_nav);
        header_right_box.set_visible(!is_spotify_nav);

        let (bw, bs, b_lk) = (btn_wave.clone(), btn_search.clone(), btn_liked.clone());
        let vs_ref = view_stack.clone();
        let update_active_nav = Rc::new(move || {
            bw.remove_css_class("active");
            bs.remove_css_class("active");
            b_lk.remove_css_class("active");
            match vs_ref.visible_child_name().as_deref() {
                Some("wave") => bw.add_css_class("active"),
                Some("search") => bs.add_css_class("active"),
                Some("library") => b_lk.add_css_class("active"),
                _ => {}
            }
        });
        update_active_nav();

        {
            let u = update_active_nav.clone();
            view_stack.connect_visible_child_notify(move |_| u());
        }

        {
            let vs = view_stack.clone();
            btn_wave.connect_clicked(move |_| vs.set_visible_child_name("wave"));
        }
        {
            let vs = view_stack.clone();
            btn_search.connect_clicked(move |_| vs.set_visible_child_name("search"));
        }
        {
            let vs = view_stack.clone();
            let open_liked = library_view.open_liked.clone();
            let u = update_active_nav.clone();
            btn_liked.connect_clicked(move |_| {
                vs.set_visible_child_name("library");
                open_liked();
                u();
            });
        }

        let content_overlay = Overlay::new();
        content_overlay.set_vexpand(true);
        content_overlay.set_hexpand(true);
        content_overlay.set_child(Some(&view_stack));

        let main_content_box = GtkBox::new(Orientation::Vertical, 0);
        main_content_box.set_vexpand(true);
        main_content_box.set_hexpand(true);
        // Отступ снизу нужен только под прижатый док; парящий остров — оверлей, контент идёт под него
        main_content_box.set_margin_bottom(if player_layout == "dock" { 92 } else { 0 });
        main_content_box.append(&header_bar);
        main_content_box.append(&content_overlay);

        let app_body_overlay = Overlay::new();
        app_body_overlay.set_vexpand(true);
        app_body_overlay.set_hexpand(true);
        app_body_overlay.set_child(Some(&main_content_box));
        app_body_overlay.add_overlay(&sidebar);
        app_body_overlay.set_measure_overlay(&sidebar, false);

        // Нижний плеер (парящий остров или прижатый док)
        let island = create_floating_island(&player);
        super::floating_island::apply_player_layout(&island, &player_layout);
        if player_layout == "dock" {
            sidebar.set_margin_bottom(92);
        } else {
            sidebar.set_margin_bottom(0);
        }

        root_overlay.set_child(Some(&app_body_overlay));
        root_overlay.add_overlay(&island.container);
        root_overlay.set_measure_overlay(&island.container, false);

        if let Some((core, deep, ridge)) = get_accent_palette(&theme_accent) {
            wave_view.shader_state.borrow_mut().set_target_colors(core, deep, ridge);
        }

        // Закрытие текста песни: задаётся ниже, когда создан экран караоке; переходы по клику из плеера закрывают его,
        // иначе страница открывалась бы под текстом
        let close_karaoke: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::default();

        // Клик по названию трека или артисту (в плеере и на экране волны) открывает его страницу
        let open_from_player = |label: &gtk4::Label, artist: bool| {
            let click = gtk4::GestureClick::new();
            let (p, vs) = (player.clone(), view_stack.clone());
            let (show_artist, show_track) = (search_view.show_artist.clone(), search_view.show_track.clone());
            let close = close_karaoke.clone();
            click.connect_released(move |_, _, _, _| {
                let Some(track) = p.state.lock().unwrap().current_track.clone() else { return };
                let f = close.borrow().clone();
                if let Some(f) = f {
                    f();
                }
                if artist {
                    // shortcut: у трека с несколькими артистами открывается первый, для выбора нужен popover
                    let Some(a) = track.artists.first() else { return };
                    vs.set_visible_child_name("search");
                    show_artist(a.id.clone());
                } else {
                    vs.set_visible_child_name("search");
                    show_track(track);
                }
            });
            label.add_controller(click);
            label.set_cursor_from_name(Some("pointer"));
        };
        open_from_player(&island.title_label, false);
        open_from_player(&island.artist_label, true);
        open_from_player(&wave_view.title_label, false);
        open_from_player(&wave_view.artist_label, true);

        // Клик по плашке источника («Альбом: …», «Плейлист: …», «По треку: …») открывает этот источник
        {
            let click = gtk4::GestureClick::new();
            let (vs, target) = (view_stack.clone(), wave_view.source_target.clone());
            let (show_artist, show_track, show_album, show_playlist) = (
                search_view.show_artist.clone(),
                search_view.show_track.clone(),
                search_view.show_album.clone(),
                search_view.show_playlist.clone(),
            );
            let close = close_karaoke.clone();
            click.connect_released(move |_, _, _, _| {
                let Some(target) = target.borrow().clone() else { return };
                let f = close.borrow().clone();
                if let Some(f) = f {
                    f();
                }
                use crate::player::SourceTarget::*;
                match target {
                    Library => vs.set_visible_child_name("library"),
                    Album(a) => {
                        vs.set_visible_child_name("search");
                        show_album(a);
                    }
                    Playlist(pl) => {
                        vs.set_visible_child_name("search");
                        show_playlist(pl);
                    }
                    Artist(id) => {
                        vs.set_visible_child_name("search");
                        show_artist(id);
                    }
                    Track(t) => {
                        vs.set_visible_child_name("search");
                        show_track(t);
                    }
                }
            });
            wave_view.pill_label.add_controller(click);
            wave_view.pill_label.set_cursor_from_name(Some("pointer"));
        }

        // Клик по обложке в плеере открывает альбом текущего трека
        {
            let click = gtk4::GestureClick::new();
            let (p, vs, show_album) = (player.clone(), view_stack.clone(), search_view.show_album.clone());
            let close = close_karaoke.clone();
            click.connect_released(move |_, _, _, _| {
                let Some(track) = p.state.lock().unwrap().current_track.clone() else { return };
                let f = close.borrow().clone();
                if let Some(f) = f {
                    f();
                }
                let Some(mut album) = track.albums.first().cloned() else { return };
                if album.artists.is_empty() {
                    album.artists = track.artists.clone();
                }
                vs.set_visible_child_name("search");
                show_album(album);
            });
            island.cover_image.add_controller(click);
            island.cover_image.set_cursor_from_name(Some("pointer"));
        }

        // ================= Оверлей текста песни (Karaoke Overlay) =================
        let karaoke_overlay = Overlay::new();
        karaoke_overlay.add_css_class("karaoke-sheet");
        karaoke_overlay.set_visible(false);
        karaoke_overlay.set_vexpand(true);
        karaoke_overlay.set_hexpand(true);

        // 1. Скроллируемый контейнер текста песни (базовый слой)
        let lyrics_scrolled = ScrolledWindow::new();
        lyrics_scrolled.add_css_class("karaoke-lyrics-scrolled");
        lyrics_scrolled.set_policy(PolicyType::Never, PolicyType::Automatic);
        lyrics_scrolled.set_vexpand(true);
        lyrics_scrolled.set_hexpand(true);

        let lyrics_container = GtkBox::new(Orientation::Vertical, 14);
        lyrics_container.set_valign(Align::Start);
        lyrics_container.set_halign(Align::Fill);
        lyrics_container.set_hexpand(true);
        lyrics_container.set_margin_top(60);
        lyrics_container.set_margin_bottom(90);

        lyrics_scrolled.set_child(Some(&lyrics_container));
        // Синхронизированный текст: поля сверху и снизу в полвысоты окна, чтобы любая строка (в том числе
        // первая и последняя) могла встать строго по центру; размер окна меняется — поля пересчитываются
        {
            let (container, scrolled) = (lyrics_container.clone(), lyrics_scrolled.downgrade());
            lyrics_scrolled.vadjustment().connect_page_size_notify(move |_| {
                if let Some(scrolled) = scrolled.upgrade() {
                    apply_lyrics_margins(&container, &scrolled);
                }
            });
        }
        karaoke_overlay.set_child(Some(&lyrics_scrolled));

        // 2. Абсолютно зафиксированная шапка (слой поверх текста, Align::Start по вертикали)
        let k_header = GtkBox::new(Orientation::Horizontal, 16);
        k_header.set_valign(Align::Start);
        k_header.set_hexpand(true);
        k_header.set_margin_top(20);
        k_header.set_margin_start(24);
        k_header.set_margin_end(24);

        let k_track_info = GtkBox::new(Orientation::Vertical, 2);
        k_track_info.set_valign(Align::Center);
        k_track_info.set_halign(Align::Start);

        let k_track_title = Label::new(Some("Волна"));
        k_track_title.add_css_class("heading");
        k_track_title.set_halign(Align::Start);
        k_track_title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        k_track_title.set_max_width_chars(28);

        let k_track_artist = Label::new(Some("Яндекс Музыка"));
        k_track_artist.add_css_class("dim-label");
        k_track_artist.set_halign(Align::Start);
        k_track_artist.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        k_track_artist.set_max_width_chars(28);

        let k_explicit = super::collection::explicit_badge();
        k_explicit.set_margin_start(8);
        let k_title_row = GtkBox::new(Orientation::Horizontal, 0);
        k_title_row.append(&k_track_title);
        k_title_row.append(&k_explicit);
        k_track_info.append(&k_title_row);
        k_track_info.append(&k_track_artist);
        k_header.append(&k_track_info);

        let k_close_btn = Button::from_icon_name("window-close-symbolic");
        k_close_btn.set_halign(Align::End);
        k_close_btn.set_valign(Align::Center);
        k_close_btn.set_hexpand(true);
        k_close_btn.add_css_class("circular");
        k_close_btn.add_css_class("karaoke-close-btn");
        k_close_btn.set_tooltip_text(Some("Закрыть текст песни"));
        k_header.append(&k_close_btn);
        karaoke_overlay.add_overlay(&k_header);

        // 3. Крупная неоновая цифра обратного отсчета (парит в верхней трети, не дергает верстку)
        let countdown_label = Label::new(Some("3"));
        countdown_label.add_css_class("karaoke-countdown-digit");
        countdown_label.set_halign(Align::Center);
        countdown_label.set_valign(Align::Start);
        countdown_label.set_margin_top(90);
        countdown_label.set_visible(false);
        karaoke_overlay.add_overlay(&countdown_label);

        content_overlay.add_overlay(&karaoke_overlay);
        super::floating_island::wire_volume_popup(&island, &root_overlay);

        let lyrics_data: Rc<RefCell<Vec<LrcLine>>> = Rc::new(RefCell::new(Vec::new()));
        let lyrics_labels: Rc<RefCell<Vec<Label>>> = Rc::new(RefCell::new(Vec::new()));

        // Управление показом караоке: текст плавно проявляется, элементы экрана Волны гаснут, анимированные полосы фона остаются
        let set_karaoke: Rc<dyn Fn(bool)> = {
            let sheet = karaoke_overlay.clone();
            let (content, pill) = (wave_view.content_box.clone(), wave_view.filter_pill.clone());
            let stack = view_stack.clone();
            // одна анимация на всех: новая сменяет текущую с того места, где та остановилась
            let current: Rc<RefCell<Option<libadwaita::TimedAnimation>>> = Rc::default();
            let want_open = Rc::new(std::cell::Cell::new(false));
            Rc::new(move |open| {
                if !open && !sheet.is_visible() {
                    return;
                }
                want_open.set(open);
                if let Some(prev) = current.borrow_mut().take() {
                    prev.pause();
                }
                if open {
                    sheet.set_margin_top(0);
                    // полупрозрачная подложка рассчитана на фон Волны; поверх Поиска и Медиатеки — плотная
                    if stack.visible_child_name().as_deref() == Some("wave") {
                        sheet.remove_css_class("solid");
                    } else {
                        sheet.add_css_class("solid");
                    }
                    if !sheet.is_visible() {
                        sheet.set_opacity(0.0);
                        sheet.set_visible(true);
                    }
                }
                let to = if open { 1.0 } else { 0.0 };
                let (t_sheet, t_content, t_pill) = (sheet.clone(), content.clone(), pill.clone());
                let target = libadwaita::CallbackAnimationTarget::new(move |v| {
                    t_sheet.set_opacity(v);
                    t_content.set_opacity(1.0 - v);
                    t_pill.set_opacity(1.0 - v);
                });
                let animation = libadwaita::TimedAnimation::new(&sheet, sheet.opacity(), to, 350, target);
                if !open {
                    let (sheet_done, want) = (sheet.clone(), want_open.clone());
                    animation.connect_done(move |_| {
                        if !want.get() {
                            sheet_done.set_visible(false);
                        }
                    });
                }
                animation.play();
                *current.borrow_mut() = Some(animation);
            })
        };
        // переход на другую вкладку закрывает текст песни, иначе вкладка менялась бы под ним
        {
            let (set, sheet) = (set_karaoke.clone(), karaoke_overlay.clone());
            view_stack.connect_visible_child_notify(move |_| {
                if sheet.is_visible() {
                    set(false);
                }
            });
        }
        {
            let set = set_karaoke.clone();
            *close_karaoke.borrow_mut() = Some(Rc::new(move || set(false)));
        }
        {
            // кнопка «Т» работает как переключатель: повторное нажатие закрывает текст
            let (set, sheet) = (set_karaoke.clone(), karaoke_overlay.clone());
            island.karaoke_btn.connect_clicked(move |_| set(!(sheet.is_visible() && sheet.opacity() > 0.5)));
        }
        {
            let set = set_karaoke.clone();
            k_close_btn.connect_clicked(move |_| set(false));
        }

        toast_overlay.set_child(Some(&root_overlay));
        window.set_content(Some(&toast_overlay));

        // демо для скриншотов: сразу открыть текст песни
        if std::env::var_os("VOLNA_DEMO_LYRICS").is_some() {
            let btn = island.karaoke_btn.clone();
            gtk4::glib::timeout_add_local_once(Duration::from_millis(1500), move || btn.emit_clicked());
        }

        // Клик по кнопке авторизации (сайдбар и верхний header_bar)
        let handle_auth_click = {
            let p_clone = player.clone();
            let win_weak = window.downgrade();
            Rc::new(move || {
                if let Some(w) = win_weak.upgrade() {
                    let is_logged_in = p_clone.client.lock().unwrap().has_token();
                    if is_logged_in {
                        // Диалог с предложением выйти
                        let alert = libadwaita::AlertDialog::new(
                            Some("Управление аккаунтом"),
                            Some("Вы авторизованы в Яндекс Музыке."),
                        );
                        alert.add_response("cancel", "Отмена");
                        alert.add_response("logout", "Выйти из аккаунта");
                        alert.set_response_appearance("logout", libadwaita::ResponseAppearance::Destructive);

                        let p_inner = p_clone.clone();
                        alert.choose(Some(&w), None::<&gio::Cancellable>, move |response| {
                            if response == "logout" {
                                p_inner.logout();
                            }
                        });
                    } else {
                        show_auth_dialog(&w, &p_clone);
                    }
                }
            })
        };

        {
            let on_auth = handle_auth_click.clone();
            auth_btn.connect_clicked(move |_| on_auth());
        }
        {
            let on_auth = handle_auth_click;
            header_auth_btn.connect_clicked(move |_| on_auth());
        }

        // Открытие настроек (сайдбар и верхний header_bar)
        let handle_settings_click = {
            let p_clone = player.clone();
            let win_weak = window.downgrade();
            let vs = view_stack.clone();
            let wave_btn = wave_view.menu_button.clone();
            Rc::new(move || {
                if let Some(w) = win_weak.upgrade() {
                    let (vs, wave_btn) = (vs.clone(), wave_btn.clone());
                    show_preferences_dialog(&w, &p_clone, move || {
                        vs.set_visible_child_name("wave");
                        wave_btn.emit_clicked();
                    });
                }
            })
        };

        {
            let on_settings = handle_settings_click.clone();
            settings_btn.connect_clicked(move |_| on_settings());
        }
        {
            let on_settings = handle_settings_click;
            header_settings_btn.connect_clicked(move |_| on_settings());
        }

        // После переключения трека позиция ещё какое-то время старая (новый поток не начал играть),
        // а текст нового трека уже пришёл: без этого флага подсвечивалась строка из середины
        let awaiting_start = Rc::new(std::cell::Cell::new(false));

        // Таймер тика прогресса и синхронизации караоке
        {
            let awaiting = awaiting_start.clone();
            let profile_buf = std::env::var_os("VOLNA_PROFILE").is_some();
            let player_clone = player.clone();
            let island_seek = island.seek_scale.clone();
            let island_curr_time = island.curr_time_label.clone();
            let island_total_time = island.total_time_label.clone();
            let is_playing_ref = is_playing_state.clone();
            let l_data = lyrics_data.clone();
            let l_labels = lyrics_labels.clone();
            let cnt_label = countdown_label.clone();
            let sc_clone = lyrics_scrolled.clone();

            // Активная строка: цель прокрутки считается по её текущему положению на каждом кадре,
            // поэтому прокрутка следует за плавным ростом строки, а не за её конечным размером
            let active_label: Rc<RefCell<Option<Label>>> = Rc::default();
            let last_sec = Rc::new(std::cell::Cell::new(u64::MAX));
            let last_active = Rc::new(std::cell::Cell::new(usize::MAX));

            {
                let (sc, active) = (lyrics_scrolled.clone(), active_label.clone());
                let container = lyrics_container.clone();
                let last_frame = std::cell::Cell::new(0_i64);
                let velocity = std::cell::Cell::new(0.0_f64);
                let profile = std::env::var_os("VOLNA_PROFILE").is_some();
                let stats = RefCell::new((0u32, 0u32, 0.0_f64));

                lyrics_scrolled.add_tick_callback(move |_, clock| {
                    let now = clock.frame_time();
                    let dt = if last_frame.get() == 0 {
                        0.016
                    } else {
                        ((now - last_frame.get()) as f64 / 1e6).clamp(0.001, 0.1)
                    };
                    last_frame.set(now);

                    if profile && sc.is_mapped() {
                        let mut st = stats.borrow_mut();
                        st.0 += 1;
                        if dt > 0.025 { st.1 += 1; }
                        st.2 = st.2.max(dt);
                        if st.0 >= 120 {
                            eprintln!("[lyrics] fps≈{:.0}, пропусков >25мс: {}, худший кадр {:.0} мс", clock.fps(), st.1, st.2 * 1000.0);
                            *st = (0, 0, 0.0);
                        }
                    }

                    let synced = container.has_css_class("synced");
                    let policy = if synced { PolicyType::External } else { PolicyType::Automatic };
                    if sc.vscrollbar_policy() != policy {
                        sc.set_policy(PolicyType::Never, policy);
                    }

                    if !synced {
                        return gtk4::glib::ControlFlow::Continue;
                    }

                    let vadj = sc.vadjustment();

                    // Активная строка: положение относительно container
                    let bounds = active.borrow().as_ref().and_then(|l| {
                        l.compute_bounds(&container)
                    });

                    if bounds.is_none() {
                        if vadj.value() != 0.0 {
                            vadj.set_value(0.0);
                        }
                        velocity.set(0.0);
                        return gtk4::glib::ControlFlow::Continue;
                    }

                    if let (true, Some(b)) = (sc.is_mapped(), bounds) {
                        let top_margin = f64::from(container.margin_top());
                        let max = (vadj.upper() - vadj.page_size()).max(0.0);
                        let goal = (top_margin + f64::from(b.y() + b.height() / 2.0) - vadj.page_size() / 2.0).clamp(0.0, max);
                        let mut cur = vadj.value();
                        let mut vel = velocity.get();

                        if (goal - cur).abs() > 0.05 || vel.abs() > 0.5 {
                            // критически задемпфированная пружина: плавный ход без рывков
                            const W: f64 = 11.0;
                            let steps = (dt * 120.0).ceil().max(1.0);
                            let h = dt / steps;
                            for _ in 0..steps as u32 {
                                vel += (W * W * (goal - cur) - 2.0 * W * vel) * h;
                                cur += vel * h;
                            }
                            velocity.set(vel);
                            vadj.set_value(cur);
                        }
                    }

                    gtk4::glib::ControlFlow::Continue
                });
            }

            // 50 мс: подсветка строки привязана к позиции с точностью до миллисекунд
            gtk4::glib::timeout_add_local(Duration::from_millis(50), move || {
                if *is_playing_ref.borrow() {
                    if let Some(pos_ms) = player_clone.position_ms() {
                        if awaiting.get() {
                            if pos_ms > 1500 {
                                return gtk4::glib::ControlFlow::Continue;
                            }
                            awaiting.set(false);
                        }
                        let pos = pos_ms / 1000;
                        // полоса прогресса и время обновляются раз в секунду
                        if last_sec.get() != pos {
                            last_sec.set(pos);
                            let dur = player_clone.duration_seconds().unwrap_or(240);
                            island_seek.set_range(0.0, dur as f64);
                            island_seek.set_value(pos as f64);
                            // полоска прогрузки, как в видеоплеерах: докуда поток уже скачан
                            if let Some(f) = player_clone.audio.buffered_fraction() {
                                island_seek.set_show_fill_level(true);
                                island_seek.set_restrict_to_fill_level(false);
                                island_seek.set_fill_level(dur as f64 * f);
                                if profile_buf {
                                    eprintln!("[buffer] {:.0}% загружено", f * 100.0);
                                }
                            }
                            island_curr_time.set_text(&format_seconds(pos));
                            island_total_time.set_text(&format_seconds(dur));
                        }

                        // Синхронизация караоке и индикатор обратного отсчета 3..2..1
                        let lines = l_data.borrow();
                        let labels = l_labels.borrow();
                        if !lines.is_empty() && lines.len() == labels.len() {
                            // Проверяем обратный отсчет ИСКЛЮЧИТЕЛЬНО перед первой строкой в начале песни
                            let next_line_info = match lines.iter().position(|l| l.time_ms > pos_ms) {
                                Some(0) => Some(lines[0].time_ms),
                                _ => None,
                            };

                            let mut counting_down = false;
                            if let Some(target_time) = next_line_info {
                                if target_time > pos_ms {
                                    let diff_ms = target_time - pos_ms;
                                    if diff_ms <= 3000 && diff_ms > 0 {
                                        counting_down = true;
                                        let digit_str = if diff_ms > 2000 {
                                            "3"
                                        } else if diff_ms > 1000 {
                                            "2"
                                        } else {
                                            "1"
                                        };
                                        let newly_shown = !cnt_label.is_visible();
                                        if newly_shown {
                                            cnt_label.set_visible(true);
                                        }
                                        if newly_shown || cnt_label.text() != digit_str {
                                            cnt_label.set_text(digit_str);
                                            animate_countdown_digit(&cnt_label);
                                        }
                                        if sc_clone.opacity() != 0.0 {
                                            sc_clone.set_opacity(0.0);
                                        }
                                    }
                                }
                            }

                            if !counting_down {
                                if cnt_label.is_visible() {
                                    cnt_label.set_visible(false);
                                }
                                if sc_clone.opacity() != 1.0 {
                                    sc_clone.set_opacity(1.0);
                                }
                            }

                            let active_idx = lines.iter().rposition(|l| l.time_ms <= pos_ms);
                            if let Some(idx) = active_idx {
                                // CSS и цель прокрутки меняем только при смене строки, а не каждые 50 мс
                                if last_active.get() != idx || labels.get(idx).map_or(false, |l| !l.has_css_class("active")) {
                                    last_active.set(idx);
                                    for (i, lbl) in labels.iter().enumerate() {
                                        if i == idx {
                                            lbl.add_css_class("active");
                                        } else {
                                            lbl.remove_css_class("active");
                                        }
                                    }
                                    *active_label.borrow_mut() = labels.get(idx).cloned();
                                }
                            } else if last_active.get() != usize::MAX {
                                // перемотали к вступлению: подсветку снимаем, текст возвращается к первой строке
                                last_active.set(usize::MAX);
                                for lbl in labels.iter() {
                                    lbl.remove_css_class("active");
                                }
                                *active_label.borrow_mut() = None;
                            }
                        } else {
                            last_active.set(usize::MAX);
                            if cnt_label.is_visible() {
                                cnt_label.set_visible(false);
                            }
                            if sc_clone.opacity() != 1.0 {
                                sc_clone.set_opacity(1.0);
                            }
                        }
                    }
                }
                gtk4::glib::ControlFlow::Continue
            });
        }

        // Обработка асинхронных событий
        setup_event_listener(
            event_receiver,
            window.clone(),
            toast_overlay,
            wave_view,
            island,
            auth_btn,
            auth_name_label,
            header_auth_btn,
            player.clone(),
            is_playing_state,
            lyrics_container,
            lyrics_data,
            lyrics_labels,
            countdown_label,
            lyrics_scrolled,
            k_track_title,
            k_explicit,
            k_track_artist,
            awaiting_start,
            sidebar,
            view_switcher,
            header_brand_box,
            header_right_box,
            theme_accent_state,
            dynamic_provider,
            is_spotify_nav_state,
            main_content_box.clone(),
        );

        Self { window }
    }

    pub fn present(&self) {
        self.window.present();
    }
}

/// Появление цифры отсчёта: плавное проявление, лёгкий «наплыв» сверху вниз и затухание к концу секунды.
/// Меняются только прозрачность и верхний отступ одной метки, поэтому это дёшево.
fn animate_countdown_digit(label: &Label) {
    const BASE_MARGIN: i32 = 90;
    let target_label = label.clone();
    let target = libadwaita::CallbackAnimationTarget::new(move |v| {
        let opacity = if v < 0.25 {
            v / 0.25
        } else if v > 0.75 {
            (1.0 - v) / 0.25
        } else {
            1.0
        };
        // ease-out: цифра быстро «садится» на место и дальше стоит
        let t = (v / 0.4).min(1.0);
        let slide = (1.0 - (1.0 - (1.0 - t) * (1.0 - t))) * 36.0;
        target_label.set_opacity(opacity.clamp(0.0, 1.0));
        target_label.set_margin_top(BASE_MARGIN - 36 + (36.0 - slide) as i32);
    });
    let animation = libadwaita::TimedAnimation::new(label, 0.0, 1.0, 900, target);
    animation.set_easing(libadwaita::Easing::Linear);
    animation.play();
}

/// Поля текста песни: у синхронизированного — полвысоты окна сверху и снизу (любая строка может встать по центру),
/// у обычного статического текста — небольшие отступы, чтобы он начинался сверху
fn apply_lyrics_margins(container: &GtkBox, scrolled: &ScrolledWindow) {
    let (top, bottom) = if container.has_css_class("synced") {
        let half = (scrolled.vadjustment().page_size() / 2.0) as i32;
        (half, half)
    } else {
        (60, 90)
    };
    container.set_margin_top(top);
    container.set_margin_bottom(bottom);
}

fn format_seconds(secs: u64) -> String {
    let m = secs / 60;
    let s = secs % 60;
    format!("{:02}:{:02}", m, s)
}

fn get_accent_palette(accent: &str) -> Option<([f64; 3], [f64; 3], [f64; 3])> {
    match accent {
        "spotify" => Some((
            [0.114, 0.725, 0.329],
            [0.03, 0.22, 0.08],
            [0.35, 0.95, 0.55],
        )),
        "ruby" => Some((
            [0.882, 0.114, 0.282],
            [0.26, 0.03, 0.08],
            [1.0, 0.40, 0.55],
        )),
        "sunset" => Some((
            [0.95, 0.42, 0.02],
            [0.36, 0.10, 0.00],
            [1.0, 0.84, 0.42],
        )),
        "violet" => Some((
            [0.659, 0.333, 0.969],
            [0.20, 0.08, 0.32],
            [0.85, 0.60, 1.0],
        )),
        "ocean" => Some((
            [0.231, 0.510, 0.965],
            [0.05, 0.15, 0.32],
            [0.55, 0.78, 1.0],
        )),
        "monochrome" => Some((
            [0.90, 0.90, 0.90],
            [0.18, 0.18, 0.18],
            [1.0, 1.0, 1.0],
        )),
        _ => None,
    }
}

fn apply_accent_classes(widget: &impl IsA<gtk4::Widget>, accent: &str) {
    for a in &["accent-dynamic", "accent-spotify", "accent-ruby", "accent-sunset", "accent-violet", "accent-ocean", "accent-monochrome"] {
        widget.remove_css_class(a);
    }
    widget.add_css_class(&format!("accent-{}", accent));
}

fn update_dynamic_accent_css(provider: &gtk4::CssProvider, r: f64, g: f64, b: f64) {
    let (r, g, b) = (
        (r * 255.0).round().clamp(0.0, 255.0) as u8,
        (g * 255.0).round().clamp(0.0, 255.0) as u8,
        (b * 255.0).round().clamp(0.0, 255.0) as u8,
    );
    let css = format!(
        ".accent-dynamic .island-play {{ background-color: rgb({r}, {g}, {b}); background-image: linear-gradient(180deg, rgba(255,255,255,0.2) 0%, rgba(0,0,0,0.1) 100%); box-shadow: 0 6px 18px rgba({r}, {g}, {b}, 0.45); }}\n\
         .accent-dynamic scale.adw-scale highlight {{ background-color: rgb({r}, {g}, {b}); background-image: none; }}\n\
         .accent-dynamic scale.volume-vertical highlight {{ background-color: rgb({r}, {g}, {b}); background-image: none; }}\n\
         .accent-dynamic .sidebar-nav-btn.active {{ background-color: rgba({r}, {g}, {b}, 0.16); color: #ffffff; border: none; }}\n\
         .accent-dynamic .sidebar-nav-btn.active .sidebar-nav-icon {{ color: rgb({r}, {g}, {b}); }}\n\
         .accent-dynamic .brand-badge {{ background-color: rgba({r}, {g}, {b}, 0.22); color: rgb({r}, {g}, {b}); border-color: rgba({r}, {g}, {b}, 0.45); }}\n\
         .accent-dynamic .rotor-filter-btn.selected {{ background: rgba({r}, {g}, {b}, 0.25); color: #ffffff; }}\n\
         .accent-dynamic .liked-heart {{ color: rgb({r}, {g}, {b}); }}\n"
    );
    provider.load_from_string(&css);
}

#[derive(Clone)]
struct SidebarNavItem {
    btn: Button,
    #[allow(dead_code)]
    overlay: Overlay,
    #[allow(dead_code)]
    icon_box: GtkBox,
    #[allow(dead_code)]
    icon: gtk4::Image,
    #[allow(dead_code)]
    label: Label,
    label_box: GtkBox,
}

fn create_sidebar_nav_btn(icon_name: &str, label_text: &str, is_collapsed: bool) -> SidebarNavItem {
    let btn = Button::new();
    btn.add_css_class("flat");
    btn.add_css_class("sidebar-nav-btn");
    btn.set_focusable(false);
    btn.set_halign(Align::Fill);
    btn.set_hexpand(true);
    btn.set_overflow(Overflow::Hidden);
    btn.set_tooltip_text(Some(label_text));

    let overlay = Overlay::new();
    overlay.set_hexpand(true);
    overlay.set_halign(Align::Fill);
    overlay.set_overflow(Overflow::Hidden);

    let icon_box = GtkBox::new(Orientation::Horizontal, 0);
    icon_box.set_size_request(44, 40);
    icon_box.set_halign(Align::Start);
    icon_box.set_valign(Align::Center);

    let icon = gtk4::Image::from_icon_name(icon_name);
    icon.set_pixel_size(20);
    icon.set_halign(Align::Center);
    icon.set_valign(Align::Center);
    icon.set_hexpand(true);
    icon.add_css_class("sidebar-nav-icon");
    if icon_name == "emblem-favorite-symbolic" {
        icon.add_css_class("liked-heart");
    }
    icon_box.append(&icon);

    overlay.set_child(Some(&icon_box));

    let label_box = GtkBox::new(Orientation::Horizontal, 0);
    label_box.set_valign(Align::Center);
    label_box.set_halign(Align::Start);
    label_box.set_margin_start(44);
    label_box.set_overflow(Overflow::Hidden);

    let label = Label::new(Some(label_text));
    label.set_halign(Align::Start);
    label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    label.set_width_chars(1);
    label.add_css_class("sidebar-nav-label");
    label_box.append(&label);

    overlay.add_overlay(&label_box);
    overlay.set_measure_overlay(&label_box, false);

    if is_collapsed {
        label_box.set_visible(false);
        label_box.set_opacity(0.0);
    }

    btn.set_child(Some(&overlay));

    SidebarNavItem { btn, overlay, icon_box, icon, label, label_box }
}

fn setup_event_listener(
    receiver: Receiver<PlayerUiEvent>,
    window: ApplicationWindow,
    toast_overlay: ToastOverlay,
    wave_view: WaveViewWidgets,
    island: FloatingIslandWidgets,
    auth_btn: Button,
    auth_name_label: Label,
    header_auth_btn: Button,
    player: PlayerManager,
    is_playing_state: Rc<RefCell<bool>>,
    lyrics_container: GtkBox,
    lyrics_data: Rc<RefCell<Vec<LrcLine>>>,
    lyrics_labels: Rc<RefCell<Vec<Label>>>,
    countdown_label: Label,
    lyrics_scrolled: ScrolledWindow,
    k_track_title: Label,
    k_explicit: Label,
    k_track_artist: Label,
    awaiting_start: Rc<std::cell::Cell<bool>>,
    sidebar: GtkBox,
    view_switcher: ViewSwitcher,
    header_brand_box: GtkBox,
    header_right_box: GtkBox,
    theme_accent_state: Rc<RefCell<String>>,
    dynamic_provider: gtk4::CssProvider,
    is_spotify_nav_state: Rc<RefCell<bool>>,
    main_content_box: GtkBox,
) {
    if player.client.lock().unwrap().has_token() {
        auth_name_label.set_text("Аккаунт");
        auth_btn.add_css_class("logged-in");
        header_auth_btn.set_label("Аккаунт");
        header_auth_btn.remove_css_class("suggested-action");
        header_auth_btn.add_css_class("flat");
        header_auth_btn.add_css_class("logged-in");
    }
    let win_weak = window.downgrade();
    gtk4::glib::spawn_future_local(async move {
        while let Ok(event) = receiver.recv().await {
            match event {
                PlayerUiEvent::TrackStarted { track, .. } => {
                    awaiting_start.set(std::env::var_os("VOLNA_DEMO_POS_MS").is_none());
                    // клип прошлого трека гаснет; клип нового придёт отдельным событием, если он есть и включён
                    wave_view.clip.stop();
                    let artists = track.artists_string();
                    let album = track.albums.first().and_then(|a| a.title.as_ref()).map(|s| s.as_str()).unwrap_or("");
                    let subtitle = if album.is_empty() {
                        artists.clone()
                    } else {
                        format!("{} • {}", artists, album)
                    };

                    wave_view.title_label.set_markup(&track.title_markup());
                    wave_view.explicit_badge.set_visible(track.is_explicit());
                    wave_view.artist_label.set_text(&subtitle);

                    let accent = theme_accent_state.borrow().clone();
                    if accent == "dynamic" {
                        wave_view.update_track_colors(&track);
                        let c = track.core_rgb();
                        update_dynamic_accent_css(&dynamic_provider, c[0] as f64, c[1] as f64, c[2] as f64);
                    } else if let Some((core, deep, ridge)) = get_accent_palette(&accent) {
                        wave_view.shader_state.borrow_mut().set_target_colors(core, deep, ridge);
                    }

                    island.title_label.set_markup(&track.title_markup());
                    island.explicit_badge.set_visible(track.is_explicit());
                    island.artist_label.set_text(&artists);

                    k_track_title.set_markup(&track.title_markup());
                    k_explicit.set_visible(track.is_explicit());
                    k_track_artist.set_text(&artists);

                    // Сбрасываем текст песни и обратный отсчет для нового трека
                    while let Some(child) = lyrics_container.first_child() {
                        lyrics_container.remove(&child);
                    }
                    *lyrics_data.borrow_mut() = Vec::new();
                    *lyrics_labels.borrow_mut() = Vec::new();
                    countdown_label.set_visible(false);
                    lyrics_scrolled.set_opacity(1.0);

                    if let Some(dur_ms) = track.duration_ms {
                        let total_secs = dur_ms / 1000;
                        island.total_time_label.set_text(&format_seconds(total_secs));
                        island.seek_scale.set_range(0.0, total_secs as f64);
                    }
                }
                PlayerUiEvent::PlaybackToggled(is_playing) => {
                    *is_playing_state.borrow_mut() = is_playing;
                    let icon_name = if is_playing {
                        "media-playback-pause-symbolic"
                    } else {
                        "media-playback-start-symbolic"
                    };

                    wave_view.play_btn_icon.set_icon_name(Some(icon_name));
                    island.play_btn_icon.set_icon_name(Some(icon_name));
                    wave_view.drawing_area.queue_draw();
                }
                PlayerUiEvent::Progress { current_sec, total_sec } => {
                    island.curr_time_label.set_text(&format_seconds(current_sec));
                    island.total_time_label.set_text(&format_seconds(total_sec));
                    island.seek_scale.set_value(current_sec as f64);
                }
                PlayerUiEvent::LikeStateChanged(liked) => {
                    if liked {
                        island.like_btn.add_css_class("liked");
                    } else {
                        island.like_btn.remove_css_class("liked");
                    }
                }
                PlayerUiEvent::TrackLiked { .. } => {}
                PlayerUiEvent::DislikeStateChanged(disliked) => {
                    if disliked {
                        island.dislike_btn.add_css_class("disliked");
                    } else {
                        island.dislike_btn.remove_css_class("disliked");
                    }
                }
                PlayerUiEvent::WaveContext(label) => {
                    wave_view.set_context(label.as_deref());
                    *wave_view.source_target.borrow_mut() = None; // цель для станции придёт следом (StationTarget)
                }
                PlayerUiEvent::SourceContext(source) => {
                    wave_view.set_source(source.as_ref().map(|s| s.label.as_str()));
                    *wave_view.source_target.borrow_mut() = source.and_then(|s| s.target);
                }
                PlayerUiEvent::StationTarget(target) => *wave_view.source_target.borrow_mut() = target,
                PlayerUiEvent::RepeatChanged(mode) => {
                    let tip = match mode {
                        RepeatMode::Off => "Повтор: выключен",
                        RepeatMode::One => "Повтор: один трек",
                        RepeatMode::All => "Повтор: вся очередь",
                    };
                    island.repeat_one.set_visible(mode == RepeatMode::One);
                    island.repeat_btn.set_tooltip_text(Some(tip));
                    if mode == RepeatMode::Off {
                        island.repeat_btn.remove_css_class("active");
                    } else {
                        island.repeat_btn.add_css_class("active");
                    }
                }
                PlayerUiEvent::CastChanged(name) => {
                    match &name {
                        Some(n) => {
                            island.cast_btn.add_css_class("cast-active");
                            island.cast_btn.set_tooltip_text(Some(&format!("Играет на «{}»", n)));
                        }
                        None => {
                            island.cast_btn.remove_css_class("cast-active");
                            island.cast_btn.set_tooltip_text(Some("Яндекс Станция"));
                        }
                    }
                }
                PlayerUiEvent::StreamQuality(label) => {
                    island.quality_badge.set_text(&label);
                }
                PlayerUiEvent::CoverUpdated(path) => {
                    island.cover_image.set_from_file(Some(&path));
                    let accent = theme_accent_state.borrow().clone();
                    if accent == "dynamic" {
                        wave_view.shader_state.borrow_mut().update_from_cover(&path);
                        let c = wave_view.shader_state.borrow().target_core;
                        update_dynamic_accent_css(&dynamic_provider, c[0], c[1], c[2]);
                    }
                }
                PlayerUiEvent::ClipUpdated(path) => match path {
                    Some(p) => wave_view.clip.play(&p),
                    None => wave_view.clip.stop(),
                },
                PlayerUiEvent::LyricsUpdated(lrc_text) => {
                    while let Some(child) = lyrics_container.first_child() {
                        lyrics_container.remove(&child);
                    }

                    let parsed = parse_lrc(&lrc_text);
                    if parsed.is_empty() {
                        lyrics_container.remove_css_class("synced");
                    } else {
                        lyrics_container.add_css_class("synced");
                    }
                    apply_lyrics_margins(&lyrics_container, &lyrics_scrolled);
                    lyrics_scrolled.vadjustment().set_value(0.0);
                    if !parsed.is_empty() {
                        let mut new_labels = Vec::new();
                        for line in &parsed {
                            let lbl = Label::new(Some(&line.text));
                            lbl.add_css_class("karaoke-line");
                            lbl.set_wrap(true);
                            lbl.set_halign(Align::Fill);
                            lbl.set_hexpand(true);
                            lbl.set_justify(gtk4::Justification::Center);
                            // клик по строке перематывает трек к её началу
                            lbl.set_cursor_from_name(Some("pointer"));
                            let click = gtk4::GestureClick::new();
                            let (p, at) = (player.clone(), line.time_ms);
                            click.connect_released(move |_, _, _, _| p.seek_ms(at));
                            lyrics_container.append(&lbl);
                            new_labels.push(lbl);
                        }

                        *lyrics_data.borrow_mut() = parsed;
                        *lyrics_labels.borrow_mut() = new_labels;
                    } else {
                        countdown_label.set_visible(false);
                        lyrics_scrolled.set_opacity(1.0);
                        *lyrics_data.borrow_mut() = Vec::new();
                        *lyrics_labels.borrow_mut() = Vec::new();

                        let trimmed = lrc_text.trim();
                        if !trimmed.is_empty() {
                            for line in lrc_text.lines() {
                                let fixed_line = crate::api::models::fix_mojibake(line.trim());
                                let line_trimmed = fixed_line.as_str();
                                if line_trimmed.is_empty() {
                                    let spacer = GtkBox::new(Orientation::Vertical, 0);
                                    spacer.set_size_request(-1, 10);
                                    lyrics_container.append(&spacer);
                                } else {
                                    let lbl = Label::new(Some(line_trimmed));
                                    lbl.add_css_class("plain-lyrics-line");
                                    lbl.set_wrap(true);
                                    lbl.set_halign(Align::Fill);
                                    lbl.set_hexpand(true);
                                    lbl.set_justify(gtk4::Justification::Center);
                                    lyrics_container.append(&lbl);
                                }
                            }
                        } else {
                            let empty_box = GtkBox::new(Orientation::Vertical, 12);
                            empty_box.set_valign(Align::Center);
                            empty_box.set_halign(Align::Center);
                            empty_box.set_margin_top(60);

                            let empty_icon = gtk4::Image::from_icon_name("audio-x-generic-symbolic");
                            empty_icon.set_pixel_size(48);
                            empty_icon.add_css_class("dim-label");
                            empty_box.append(&empty_icon);

                            let empty_label = Label::new(Some("У этого трека текст песни пока не добавлен"));
                            empty_label.add_css_class("heading");
                            empty_label.add_css_class("dim-label");
                            empty_box.append(&empty_label);

                            lyrics_container.append(&empty_box);
                        }
                    }
                }
                PlayerUiEvent::AuthRequired => {
                    auth_name_label.set_text("Войти в Яндекс ID");
                    auth_btn.set_tooltip_text(Some("Войти в Яндекс ID"));
                    auth_btn.remove_css_class("logged-in");

                    header_auth_btn.set_label("Войти в Яндекс ID");
                    header_auth_btn.remove_css_class("flat");
                    header_auth_btn.remove_css_class("logged-in");
                    header_auth_btn.add_css_class("suggested-action");

                    if let Some(w) = win_weak.upgrade() {
                        show_auth_dialog(&w, &player);
                    }
                }
                PlayerUiEvent::UserLoggedIn(login) => {
                    auth_name_label.set_text(&login);
                    auth_btn.set_tooltip_text(Some(&format!("Вы вошли как {}", login)));
                    auth_btn.add_css_class("logged-in");

                    header_auth_btn.set_label(&format!("👤 {}", login));
                    header_auth_btn.remove_css_class("suggested-action");
                    header_auth_btn.add_css_class("flat");
                    header_auth_btn.add_css_class("logged-in");
                }
                PlayerUiEvent::UserLoggedOut => {
                    auth_name_label.set_text("Войти в Яндекс ID");
                    auth_btn.set_tooltip_text(Some("Войти в Яндекс ID"));
                    auth_btn.remove_css_class("logged-in");

                    header_auth_btn.set_label("Войти в Яндекс ID");
                    header_auth_btn.remove_css_class("flat");
                    header_auth_btn.remove_css_class("logged-in");
                    header_auth_btn.add_css_class("suggested-action");

                    wave_view.title_label.set_text("Волна");
                    wave_view.explicit_badge.set_visible(false);
                    island.explicit_badge.set_visible(false);
                    wave_view.artist_label.set_text("Войдите в Яндекс ID для запуска потока");
                    island.title_label.set_text("Волна");
                    island.artist_label.set_text("Яндекс Музыка");
                    island.cover_image.clear(); // пустая картинка = CSS-заглушка обложки
                }
                PlayerUiEvent::StatusMessage(msg) => {
                    let toast = Toast::new(&msg);
                    toast.set_use_markup(false);
                    toast_overlay.add_toast(toast);
                }
                PlayerUiEvent::Error(err) => {
                    // текст ошибки может содержать «<host>» и подобное: разметку отключаем, иначе тост не показывается
                    let toast = Toast::new(&format!("Ошибка: {}", err));
                    toast.set_use_markup(false);
                    toast_overlay.add_toast(toast);
                }
                PlayerUiEvent::CrossfadeChanged { .. } => {}
                PlayerUiEvent::NavLayoutChanged(style) => {
                    let is_spotify = style == "spotify";
                    *is_spotify_nav_state.borrow_mut() = is_spotify;
                    sidebar.set_visible(is_spotify);
                    view_switcher.set_visible(!is_spotify);
                    header_brand_box.set_visible(!is_spotify);
                    header_right_box.set_visible(!is_spotify);
                }
                PlayerUiEvent::PlayerLayoutChanged(layout) => {
                    super::floating_island::apply_player_layout(&island, &layout);
                    let dock_margin = if layout == "dock" { 92 } else { 0 };
                    sidebar.set_margin_bottom(dock_margin);
                    main_content_box.set_margin_bottom(dock_margin);
                }
                PlayerUiEvent::ThemeAccentChanged(accent) => {
                    *theme_accent_state.borrow_mut() = accent.clone();
                    if let Some(w) = win_weak.upgrade() {
                        apply_accent_classes(&w, &accent);
                    }
                    if let Some((core, deep, ridge)) = get_accent_palette(&accent) {
                        dynamic_provider.load_from_string("");
                        wave_view.shader_state.borrow_mut().set_target_colors(core, deep, ridge);
                    } else {
                        let t = player.state.lock().unwrap().current_track.clone();
                        if let Some(t) = t {
                            wave_view.update_track_colors(&t);
                            let c = t.core_rgb();
                            update_dynamic_accent_css(&dynamic_provider, c[0] as f64, c[1] as f64, c[2] as f64);
                        } else {
                            let core = [0.95, 0.42, 0.02];
                            let deep = [0.36, 0.10, 0.00];
                            let ridge = [1.0, 0.84, 0.42];
                            wave_view.shader_state.borrow_mut().set_target_colors(core, deep, ridge);
                            update_dynamic_accent_css(&dynamic_provider, core[0], core[1], core[2]);
                        }
                    }
                }
                _ => {}
            }
        }
    });
}