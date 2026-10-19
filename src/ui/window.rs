use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use async_channel::Receiver;
use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Button, Label, Orientation, Overlay, ScrolledWindow, PolicyType,
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

        // Фоновый контейнер с радиальным градиентом
        let background_box = GtkBox::new(Orientation::Vertical, 0);
        background_box.add_css_class("ambient-backdrop");
        background_box.set_vexpand(true);
        background_box.set_hexpand(true);

        // HeaderBar
        let header_bar = HeaderBar::new();
        header_bar.add_css_class("fluid-header");

        // Бренд слева
        let brand_box = GtkBox::new(Orientation::Horizontal, 8);
        brand_box.set_valign(Align::Center);

        let brand_title = Label::new(Some("🌊 Волна"));
        brand_title.add_css_class("heading");
        brand_box.append(&brand_title);

        let brand_badge = Label::new(Some("Wave-First"));
        brand_badge.add_css_class("brand-badge");
        brand_box.append(&brand_badge);

        header_bar.pack_start(&brand_box);

        // ViewStack
        let view_stack = ViewStack::new();
        view_stack.set_vexpand(true);
        view_stack.set_hexpand(true);

        let view_switcher = ViewSwitcher::builder()
            .stack(&view_stack)
            .policy(libadwaita::ViewSwitcherPolicy::Wide)
            .build();
        header_bar.set_title_widget(Some(&view_switcher));

        // Правая часть HeaderBar: Кнопка профиля/авторизации + Настройки
        let right_header_box = GtkBox::new(Orientation::Horizontal, 8);

        let auth_btn = Button::with_label("Войти в Яндекс ID");
        auth_btn.add_css_class("suggested-action");
        right_header_box.append(&auth_btn);

        let settings_btn = Button::from_icon_name("emblem-system-symbolic");
        settings_btn.set_tooltip_text(Some("Настройки Волны и токен"));
        settings_btn.add_css_class("flat");
        settings_btn.add_css_class("circular");
        right_header_box.append(&settings_btn);

        header_bar.pack_end(&right_header_box);
        background_box.append(&header_bar);

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

        background_box.append(&view_stack);
        root_overlay.set_child(Some(&background_box));

        // Нижний плавающий остров плеера
        let island = create_floating_island(&player);

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
        lyrics_container.set_halign(Align::Center);
        lyrics_container.set_margin_top(60);
        lyrics_container.set_margin_bottom(90);

        // Fixed с трансформацией вместо прокрутки: ScrolledWindow сдвигает содержимое только на целые пиксели,
        // и медленная прокрутка синхронизированного текста шла рывками; сдвиг трансформацией дробный
        let lyrics_fixed = gtk4::Fixed::new();
        lyrics_fixed.put(&lyrics_container, 0.0, 0.0);
        lyrics_scrolled.set_child(Some(&lyrics_fixed));
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

        root_overlay.add_overlay(&karaoke_overlay);
        // плеер добавляем после караоке, чтобы он оставался поверх текста песни (оверлеи рисуются в порядке добавления)
        root_overlay.add_overlay(&island.container);
        super::floating_island::wire_volume_popup(&island, &root_overlay);

        let lyrics_data: Rc<RefCell<Vec<LrcLine>>> = Rc::new(RefCell::new(Vec::new()));
        let lyrics_labels: Rc<RefCell<Vec<Label>>> = Rc::new(RefCell::new(Vec::new()));

        // Управление показом караоке: текст плавно проявляется, элементы экрана Волны гаснут, анимированные полосы фона остаются
        let set_karaoke: Rc<dyn Fn(bool)> = {
            let sheet = karaoke_overlay.clone();
            let (content, pill) = (wave_view.content_box.clone(), wave_view.filter_pill.clone());
            let (header, stack) = (header_bar.clone(), view_stack.clone());
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
                    // шапка окна (вкладки, кнопки окна) остаётся видимой и не перекрывается текстом
                    sheet.set_margin_top(header.height());
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

        // Клик по кнопке авторизации
        {
            let p_clone = player.clone();
            let win_weak = window.downgrade();
            auth_btn.connect_clicked(move |_| {
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
            });
        }

        // Открытие настроек
        {
            let p_clone = player.clone();
            let win_weak = window.downgrade();
            let (vs, wave_btn) = (view_stack.clone(), wave_view.menu_button.clone());
            settings_btn.connect_clicked(move |_| {
                if let Some(w) = win_weak.upgrade() {
                    let (vs, wave_btn) = (vs.clone(), wave_btn.clone());
                    show_preferences_dialog(&w, &p_clone, move || {
                        vs.set_visible_child_name("wave");
                        wave_btn.emit_clicked();
                    });
                }
            });
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
                let (fixed, container) = (lyrics_fixed.clone(), lyrics_container.clone());
                let lyrics = lyrics_container.clone();
                let last_frame = std::cell::Cell::new(0_i64);
                let offset = std::cell::Cell::new(0.0_f64);
                let velocity = std::cell::Cell::new(0.0_f64);
                let profile = std::env::var_os("VOLNA_PROFILE").is_some();
                let stats = RefCell::new((0u32, 0u32, 0.0_f64));
                let shift = move |y: f64| {
                    let point = gtk4::graphene::Point::new(0.0, -y as f32);
                    fixed.set_child_transform(&container, Some(&gtk4::gsk::Transform::new().translate(&point)));
                };
                lyrics_scrolled.add_tick_callback(move |_, clock| {
                    let now = clock.frame_time();
                    let dt = if last_frame.get() == 0 { 0.016 } else { ((now - last_frame.get()) as f64 / 1e6).clamp(0.001, 0.1) };
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
                    // строки центрируются по ширине окна: контейнер в Fixed сам не растягивается
                    let width = sc.width();
                    if width > 0 && lyrics.width_request() != width {
                        lyrics.set_size_request(width, -1);
                    }
                    let synced = lyrics.has_css_class("synced");
                    // у синхронизированного текста полосы прокрутки нет: его ведёт сдвиг, а не колесо
                    let policy = if synced { PolicyType::External } else { PolicyType::Automatic };
                    if sc.vscrollbar_policy() != policy {
                        sc.set_policy(PolicyType::Never, policy);
                    }
                    if !synced {
                        // обычный текст листается прокруткой: убираем сдвиг, оставшийся от синхронизированного
                        if offset.get() != 0.0 {
                            offset.set(0.0);
                            velocity.set(0.0);
                            shift(0.0);
                        }
                        return gtk4::glib::ControlFlow::Continue;
                    }
                    // положение строки считаем в координатах содержимого: к границам внутри контейнера
                    // добавляем его верхнее поле (оно и есть полвысоты окна)
                    let bounds = active.borrow().as_ref().and_then(|l| {
                        let c = l.parent()?;
                        l.compute_bounds(&c).map(|b| (b, f64::from(c.margin_top())))
                    });
                    // активной строки нет (новый текст, вступление до первой строки, перемотка в начало):
                    // текст стоит в начале — первая строка по центру. Без этого сдвиг от прошлой песни
                    // оставался, и новый текст висел за экраном до первой строки
                    if bounds.is_none() {
                        if offset.get() != 0.0 || velocity.get() != 0.0 {
                            offset.set(0.0);
                            velocity.set(0.0);
                            shift(0.0);
                        }
                        return gtk4::glib::ControlFlow::Continue;
                    }
                    if let (true, Some((b, top_margin))) = (sc.is_mapped(), bounds) {
                        let vadj = sc.vadjustment();
                        if vadj.value() != 0.0 {
                            vadj.set_value(0.0);
                        }
                        // предел считаем по самому тексту: Fixed учитывает сдвиг дочернего виджета в своём размере,
                        // и upper прокрутки уменьшался вместе со сдвигом (прокрутка застревала на середине песни)
                        let total = f64::from(lyrics.height() + lyrics.margin_top() + lyrics.margin_bottom());
                        let max = (total - vadj.page_size()).max(0.0);
                        let goal = (top_margin + f64::from(b.y() + b.height() / 2.0) - vadj.page_size() / 2.0).clamp(0.0, max);
                        let (cur, vel) = (offset.get(), velocity.get());
                        if (goal - cur).abs() > 0.05 || vel.abs() > 0.5 {
                            // критически задемпфированная пружина: разгон и торможение плавные даже при дальней
                            // перемотке (у экспоненты скорость максимальна в первый же кадр, и длинный прыжок рвётся)
                            const W: f64 = 11.0;
                            // шаги не длиннее 1/120 с: при W·dt > 1 явная схема раскачивается на долгих кадрах
                            let steps = (dt * 120.0).ceil().max(1.0);
                            let h = dt / steps;
                            let (mut cur, mut vel) = (cur, vel);
                            for _ in 0..steps as u32 {
                                vel += (W * W * (goal - cur) - 2.0 * W * vel) * h;
                                cur += vel * h;
                            }
                            velocity.set(vel);
                            offset.set(cur);
                            shift(cur);
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

fn setup_event_listener(
    receiver: Receiver<PlayerUiEvent>,
    window: ApplicationWindow,
    toast_overlay: ToastOverlay,
    wave_view: WaveViewWidgets,
    island: FloatingIslandWidgets,
    auth_btn: Button,
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
) {
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
                    wave_view.update_track_colors(&track);

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
                PlayerUiEvent::DislikeStateChanged(disliked) => {
                    if disliked {
                        island.dislike_btn.add_css_class("disliked");
                    } else {
                        island.dislike_btn.remove_css_class("disliked");
                    }
                }
                PlayerUiEvent::WaveContext(label) => wave_view.set_context(label.as_deref()),
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
                    wave_view.shader_state.borrow_mut().update_from_cover(&path);
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
                            lbl.set_justify(gtk4::Justification::Center);
                            // клик по строке перематывает трек к её началу
                            lbl.set_cursor_from_name(Some("pointer"));
                            let click = gtk4::GestureClick::new();
                            let (p, at) = (player.clone(), line.time_ms);
                            click.connect_released(move |_, _, _, _| p.seek_ms(at));
                            lbl.add_controller(click);
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
                    auth_btn.set_label("Войти в Яндекс ID");
                    auth_btn.remove_css_class("flat");
                    auth_btn.add_css_class("suggested-action");
                    if let Some(w) = win_weak.upgrade() {
                        show_auth_dialog(&w, &player);
                    }
                }
                PlayerUiEvent::UserLoggedIn(login) => {
                    auth_btn.set_label(&format!("👤 {}", login));
                    auth_btn.remove_css_class("suggested-action");
                    auth_btn.add_css_class("flat");
                }
                PlayerUiEvent::UserLoggedOut => {
                    auth_btn.set_label("Войти в Яндекс ID");
                    auth_btn.remove_css_class("flat");
                    auth_btn.add_css_class("suggested-action");

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
                _ => {}
            }
        }
    });
}
