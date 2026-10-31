use gtk4::prelude::*;
use gtk4::{
    gdk, Adjustment, Align, Box as GtkBox, Button, CenterBox, Image, Label, Orientation, Overflow,
    Picture, Scale,
};

use crate::player::PlayerManager;

pub struct FloatingIslandWidgets {
    pub container: GtkBox,
    pub cover_image: Image,
    pub title_label: Label,
    pub explicit_badge: Label,
    pub artist_label: Label,
    pub like_btn: Button,
    pub dislike_btn: Button,
    pub prev_btn: Button,
    pub play_btn: Button,
    pub play_btn_icon: Image,
    pub next_btn: Button,
    pub seek_scale: Scale,
    pub curr_time_label: Label,
    pub total_time_label: Label,
    pub volume_scale: Scale,
    pub karaoke_btn: Button,
    /// Значок громкости и всплывающий ползунок; окно ползунка добавляется оверлеем окна (`wire_volume_popup`)
    pub volume_btn: Button,
    pub volume_popup: GtkBox,
    /// Выбор Яндекс Станции; класс `cast-active`, когда станция привязана
    pub cast_btn: gtk4::MenuButton,
    pub quality_badge: Label,
    pub repeat_btn: Button,
    /// Цифра «1» поверх значка повтора: видна в режиме «один трек»
    pub repeat_one: Label,
}

/// Обёртка с гибкой шириной: минимум — как у содержимого, натуральная ширина — `nat` px.
/// Невидимая распорка не поднимает минимум, поэтому остров ужимается вместе с окном,
/// а на широком окне держит постоянную дизайн-ширину независимо от длины текста.
fn flex_width(child: &impl IsA<gtk4::Widget>, nat: i32) -> GtkBox {
    let wrap = GtkBox::new(Orientation::Vertical, 0);
    wrap.set_valign(Align::Center);
    wrap.append(child);
    let strut = Picture::for_paintable(&gdk::Paintable::new_empty(nat, 1));
    strut.update_state(&[gtk4::accessible::State::Hidden(true)]);
    wrap.append(&strut);
    wrap
}

/// Применяет стиль расположения плеера:
/// - "dock": полноширинный нижний бар от края до края (Spotify-style)
/// - "floating": парящая капсула с отступами и скруглениями над экраном
pub fn apply_player_layout(island: &FloatingIslandWidgets, layout: &str) {
    if layout == "dock" {
        island.container.add_css_class("dock-player");
        island.container.set_valign(Align::End);
        island.container.set_halign(Align::Fill);
        island.container.set_hexpand(true);
        island.container.set_margin_bottom(0);
        island.container.set_margin_start(0);
        island.container.set_margin_end(0);
    } else {
        island.container.remove_css_class("dock-player");
        island.container.set_valign(Align::End);
        island.container.set_halign(Align::Center);
        island.container.set_hexpand(false);
        island.container.set_margin_bottom(16);
        island.container.set_margin_start(16);
        island.container.set_margin_end(16);
    }
}

pub fn create_floating_island(player: &PlayerManager) -> FloatingIslandWidgets {
    // Капсула: верхний ряд (трек | транспорт | функции) + полоса перемотки во всю ширину
    let container = GtkBox::new(Orientation::Vertical, 6);
    container.add_css_class("floating-island-player");
    container.set_valign(Align::End);
    container.set_halign(Align::Center);
    container.set_margin_bottom(16);
    container.set_margin_start(16);
    container.set_margin_end(16);
    container.set_hexpand(false);

    // CenterBox держит транспорт строго по центру острова (под кнопкой Play экрана волны).
    // Натуральная ширина боковых зон ≈ минимальной, иначе CenterBox сдвигает центр;
    // свободное место они забирают через hexpand.
    let top_row = CenterBox::new();

    // ================= Трек: обложка и название =================
    let left_box = GtkBox::new(Orientation::Horizontal, 12);
    left_box.set_hexpand(true);

    // Пустая картинка показывает CSS-заглушку (градиент + нота), обложка трека рисуется поверх
    let cover_image = Image::new();
    cover_image.set_pixel_size(56);
    cover_image.set_overflow(Overflow::Hidden);
    cover_image.set_valign(Align::Center);
    cover_image.add_css_class("circular-cover");
    left_box.append(&cover_image);

    // Метаданные: занимают всё место до транспорта, ширина зависит только от ширины острова, не от текста
    let meta_box = GtkBox::new(Orientation::Vertical, 2);
    meta_box.set_valign(Align::Center);
    meta_box.set_hexpand(true);
    meta_box.set_width_request(40);

    let title_label = Label::new(Some("Волна"));
    title_label.add_css_class("island-title");
    title_label.set_xalign(0.0);
    title_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    title_label.set_width_chars(1);
    title_label.set_max_width_chars(1);
    // Плашка «E» поверх строки названия, сразу за текстом: в обычном ряду длина названия
    // меняла бы естественную ширину острова, а она должна зависеть только от самого острова
    let explicit_badge = super::collection::explicit_badge();
    explicit_badge.set_halign(Align::Start);
    let title_row = gtk4::Overlay::new();
    title_row.set_child(Some(&title_label));
    title_row.add_overlay(&explicit_badge);
    meta_box.append(&title_row);
    {
        let badge = explicit_badge.clone();
        let last = std::cell::Cell::new((String::new(), -1));
        title_label.add_tick_callback(move |label, _| {
            if !badge.is_visible() {
                if label.margin_end() != 0 {
                    label.set_margin_end(0);
                }
                return glib::ControlFlow::Continue;
            }
            // место под плашку справа, текст обрезается многоточием раньше неё
            let reserve = badge.width().max(16) + 8;
            if label.margin_end() != reserve {
                label.set_margin_end(reserve);
            }
            let key = (label.text().to_string(), label.width());
            let prev = last.take();
            if prev != key {
                let text_w = label.create_pango_layout(Some(&key.0)).pixel_size().0;
                badge.set_margin_start(text_w.min(key.1) + 8);
            }
            last.set(key);
            glib::ControlFlow::Continue
        });
    }

    let artist_label = Label::new(Some("Яндекс Музыка"));
    artist_label.add_css_class("island-artist");
    artist_label.set_xalign(0.0);
    artist_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    artist_label.set_width_chars(1);
    artist_label.set_max_width_chars(1);
    meta_box.append(&artist_label);

    left_box.append(&meta_box);

    top_row.set_start_widget(Some(&left_box));

    // ================= Транспорт: ⊘ ⏮ ▶ ⏭ ♡, как в «Волне» Яндекса =================
    let controls_box = GtkBox::new(Orientation::Horizontal, 10);
    controls_box.set_valign(Align::Center);
    // CenterBox не умеет spacing — воздух между зонами задаём полями транспорта
    controls_box.set_margin_start(24);
    controls_box.set_margin_end(24);

    let dislike_btn = Button::from_icon_name("action-unavailable-symbolic");
    dislike_btn.set_tooltip_text(Some("Не нравится"));
    dislike_btn.add_css_class("reaction-btn");
    dislike_btn.set_valign(Align::Center);
    dislike_btn.set_margin_end(6);
    controls_box.append(&dislike_btn);

    let prev_btn = Button::from_icon_name("media-skip-backward-symbolic");
    prev_btn.add_css_class("flat");
    prev_btn.add_css_class("circular");
    prev_btn.add_css_class("island-skip");
    prev_btn.set_valign(Align::Center);
    prev_btn.set_tooltip_text(Some("Предыдущий трек"));
    controls_box.append(&prev_btn);

    let play_btn = Button::new();
    play_btn.add_css_class("circular");
    play_btn.add_css_class("island-play");
    let play_btn_icon = Image::from_icon_name("media-playback-start-symbolic");
    // 16px: в WhiteSur размеры 17–31 подменяются мелкой залитой иконкой с полями
    play_btn_icon.set_pixel_size(16);
    play_btn.set_child(Some(&play_btn_icon));
    play_btn.set_tooltip_text(Some("Пауза / Воспроизведение"));
    controls_box.append(&play_btn);

    let next_btn = Button::from_icon_name("media-skip-forward-symbolic");
    next_btn.add_css_class("flat");
    next_btn.add_css_class("circular");
    next_btn.add_css_class("island-skip");
    next_btn.set_valign(Align::Center);
    next_btn.set_tooltip_text(Some("Следующий трек"));
    controls_box.append(&next_btn);

    let like_btn = Button::from_icon_name("emblem-favorite-symbolic");
    like_btn.set_tooltip_text(Some("Нравится"));
    like_btn.add_css_class("reaction-btn");
    like_btn.set_valign(Align::Center);
    like_btn.set_margin_start(6);
    controls_box.append(&like_btn);

    top_row.set_center_widget(Some(&controls_box));

    // ================= Текст песни, качество, громкость =================
    // Лишнее место делится между зазором перед кнопкой текста и ползунком громкости
    let right_box = GtkBox::new(Orientation::Horizontal, 12);
    right_box.set_hexpand(true);
    right_box.set_valign(Align::Center);

    // иконка лежит в данных приложения (исходники или /usr/share/volna/icons)
    let karaoke_icon = match super::bundled_icon("lyrics-symbolic.svg") {
        Some(path) => {
            let img = Image::from_file(path);
            img.set_pixel_size(18);
            img
        }
        None => {
            let img = Image::from_icon_name("text-x-generic-symbolic");
            img.set_pixel_size(18);
            img
        }
    };
    karaoke_icon.add_css_class("lyrics-btn-icon");

    let karaoke_btn = Button::new();
    karaoke_btn.set_child(Some(&karaoke_icon));
    karaoke_btn.add_css_class("flat");
    karaoke_btn.add_css_class("circular");
    karaoke_btn.add_css_class("karaoke-toggle-btn");
    karaoke_btn.set_valign(Align::Center);
    karaoke_btn.set_hexpand(true);
    karaoke_btn.set_halign(Align::End);
    karaoke_btn.set_tooltip_text(Some("Текст песни"));
    right_box.append(&karaoke_btn);

    let repeat_btn = Button::new();
    let repeat_overlay = gtk4::Overlay::new();
    repeat_overlay.set_child(Some(&Image::from_icon_name("media-playlist-repeat-symbolic")));
    let repeat_one = Label::new(Some("1"));
    repeat_one.add_css_class("repeat-one");
    repeat_one.set_halign(Align::Center);
    repeat_one.set_valign(Align::Center);
    repeat_one.set_visible(false);
    repeat_overlay.add_overlay(&repeat_one);
    repeat_btn.set_child(Some(&repeat_overlay));
    repeat_btn.add_css_class("flat");
    repeat_btn.add_css_class("circular");
    repeat_btn.add_css_class("repeat-btn");
    repeat_btn.set_valign(Align::Center);
    repeat_btn.set_tooltip_text(Some("Повтор: выключен"));
    {
        let p = player.clone();
        repeat_btn.connect_clicked(move |_| p.cycle_repeat());
    }
    right_box.append(&repeat_btn);

    let cast_btn = build_cast_button(player);
    right_box.append(&cast_btn);

    let quality_badge = Label::new(Some("MP3"));
    quality_badge.add_css_class("lossless-badge");
    quality_badge.set_valign(Align::Center);
    right_box.append(&quality_badge);

    // Громкость: только значок, ползунок всплывает над ним при наведении (как в Яндекс Музыке).
    // Всплывающее окно рисуется внутри окна приложения (оверлеем), а не отдельным окном: оно не выходит за края.
    let vol_adj = Adjustment::new(85.0, 0.0, 100.0, 1.0, 10.0, 0.0);
    let volume_scale = Scale::new(Orientation::Vertical, Some(&vol_adj));
    volume_scale.add_css_class("adw-scale");
    volume_scale.add_css_class("volume-vertical");
    volume_scale.set_inverted(true); // вверх — громче
    volume_scale.set_height_request(118);
    volume_scale.set_draw_value(false);

    let vol_icon = Image::from_icon_name("audio-volume-high-symbolic");
    let volume_btn = Button::new();
    volume_btn.set_child(Some(&vol_icon));
    volume_btn.add_css_class("flat");
    volume_btn.add_css_class("circular");
    volume_btn.add_css_class("island-volume-btn");
    volume_btn.set_valign(Align::Center);
    volume_btn.set_halign(Align::End);
    volume_btn.set_hexpand(true);

    let volume_popup = GtkBox::new(Orientation::Vertical, 0);
    volume_popup.add_css_class("volume-popup");
    volume_popup.set_halign(Align::Start);
    volume_popup.set_valign(Align::Start);
    volume_popup.set_visible(false);
    volume_popup.append(&volume_scale);

    // колесо мыши над значком меняет громкость
    {
        let adj = vol_adj.clone();
        let scroll = gtk4::EventControllerScroll::new(gtk4::EventControllerScrollFlags::VERTICAL);
        scroll.connect_scroll(move |_, _, dy| {
            adj.set_value(adj.value() - dy * 5.0);
            gtk4::glib::Propagation::Stop
        });
        volume_btn.add_controller(scroll);
    }
    // значок отражает уровень
    {
        let icon = vol_icon.clone();
        vol_adj.connect_value_changed(move |a| {
            icon.set_icon_name(Some(match a.value() {
                v if v <= 0.5 => "audio-volume-muted-symbolic",
                v if v < 34.0 => "audio-volume-low-symbolic",
                v if v < 67.0 => "audio-volume-medium-symbolic",
                _ => "audio-volume-high-symbolic",
            }));
        });
    }

    right_box.append(&volume_btn);
    top_row.set_end_widget(Some(&right_box));
    container.append(&top_row);

    // ================= Полоса перемотки во всю ширину =================
    // Текущее время — под обложкой, сама полоса начинается ровно от названия трека.
    // Распорка полосы задаёт натуральную ширину всего острова (~900px).
    let progress_box = GtkBox::new(Orientation::Horizontal, 12);

    let curr_time_label = Label::new(Some("00:00"));
    curr_time_label.add_css_class("time-label");
    curr_time_label.set_width_request(56);
    progress_box.append(&curr_time_label);

    let seek_adj = Adjustment::new(0.0, 0.0, 240.0, 1.0, 5.0, 0.0);
    let seek_scale = Scale::new(Orientation::Horizontal, Some(&seek_adj));
    seek_scale.add_css_class("adw-scale");
    seek_scale.add_css_class("island-seek");
    seek_scale.set_draw_value(false);
    let seek_flex = flex_width(&seek_scale, 750);
    seek_flex.set_hexpand(true);
    progress_box.append(&seek_flex);

    let total_time_label = Label::new(Some("00:00"));
    total_time_label.add_css_class("time-label");
    total_time_label.set_xalign(1.0);
    progress_box.append(&total_time_label);

    container.append(&progress_box);

    // Привязка действий плеера к кнопкам
    {
        let p = player.clone();
        play_btn.connect_clicked(move |_| p.toggle_play());
    }
    {
        let p = player.clone();
        next_btn.connect_clicked(move |_| p.next_track());
    }
    {
        let p = player.clone();
        prev_btn.connect_clicked(move |_| p.prev_track());
    }
    {
        let p = player.clone();
        like_btn.connect_clicked(move |_| p.toggle_like());
    }
    {
        let p = player.clone();
        dislike_btn.connect_clicked(move |_| p.toggle_dislike());
    }
    {
        let p = player.clone();
        // change-value приходит на каждое движение мыши при перетаскивании; серия перемоток подряд
        // (каждая сбрасывает конвейер) останавливала воспроизведение, поэтому берём последнее значение раз в 150 мс
        let pending: std::rc::Rc<std::cell::Cell<Option<u64>>> = Default::default();
        let scheduled = std::rc::Rc::new(std::cell::Cell::new(false));
        seek_scale.connect_change_value(move |_, _, new_val| {
            pending.set(Some(new_val.max(0.0) as u64));
            if !scheduled.replace(true) {
                let (p, pending, scheduled) = (p.clone(), pending.clone(), scheduled.clone());
                gtk4::glib::timeout_add_local_once(std::time::Duration::from_millis(150), move || {
                    scheduled.set(false);
                    if let Some(secs) = pending.take() {
                        p.seek(secs);
                    }
                });
            }
            gtk4::glib::Propagation::Proceed
        });
    }
    {
        let p = player.clone();
        volume_scale.connect_value_changed(move |scale| {
            let val = scale.value() / 100.0;
            p.set_volume(val);
        });
    }

    FloatingIslandWidgets {
        container,
        cover_image,
        title_label,
        explicit_badge,
        artist_label,
        like_btn,
        dislike_btn,
        prev_btn,
        play_btn,
        play_btn_icon,
        next_btn,
        seek_scale,
        curr_time_label,
        total_time_label,
        volume_scale,
        volume_btn,
        volume_popup,
        karaoke_btn,
        cast_btn,
        quality_badge,
        repeat_btn,
        repeat_one,
    }
}

/// Кнопка выбора Яндекс Станции: список найденных в сети, подключение и отключение
fn build_cast_button(player: &PlayerManager) -> gtk4::MenuButton {
    let btn = gtk4::MenuButton::new();
    // иконка «устройство с колонкой» лежит в данных приложения, как и иконка текста песни
    match super::bundled_icon("cast-device-symbolic.svg") {
        Some(path) => {
            let img = Image::from_file(path);
            img.set_pixel_size(18);
            btn.set_child(Some(&img));
        }
        None => btn.set_icon_name("audio-speakers-symbolic"),
    }
    btn.add_css_class("flat");
    btn.add_css_class("circular");
    btn.set_valign(Align::Center);
    btn.set_tooltip_text(Some("Яндекс Станция"));

    let pop = gtk4::Popover::new();
    let body = GtkBox::new(Orientation::Vertical, 6);
    body.set_margin_top(10);
    body.set_margin_bottom(10);
    body.set_margin_start(10);
    body.set_margin_end(10);
    let heading = Label::new(Some("Яндекс Станция"));
    heading.add_css_class("section-title");
    heading.set_halign(Align::Start);
    let status = Label::new(None);
    status.add_css_class("dim-label");
    status.set_halign(Align::Start);
    let list = GtkBox::new(Orientation::Vertical, 2);
    let off = Button::with_label("Отключить");
    off.add_css_class("pill");
    off.set_visible(false);
    body.append(&heading);
    body.append(&status);
    body.append(&list);
    body.append(&off);
    pop.set_child(Some(&body));
    btn.set_popover(Some(&pop));

    {
        let (p, pop) = (player.clone(), pop.clone());
        off.connect_clicked(move |_| {
            pop.popdown();
            p.cast_disconnect();
        });
    }
    // при каждом открытии ищем станции заново
    {
        let (p, status, list, off) = (player.clone(), status.clone(), list.clone(), off.clone());
        let pop2 = pop.clone();
        pop.connect_show(move |_| {
            while let Some(c) = list.first_child() {
                list.remove(&c);
            }
            let connected = p.cast_name();
            off.set_visible(connected.is_some());
            status.set_text(&match &connected {
                Some(n) => format!("Играет на «{}»", n),
                None => "Поиск в сети…".to_string(),
            });
            let (p, status, list, pop) = (p.clone(), status.clone(), list.clone(), pop2.clone());
            gtk4::glib::spawn_future_local(async move {
                let client = p.client.lock().unwrap().clone();
                match crate::station::find_stations(&client, std::time::Duration::from_secs(3)).await {
                    Ok(found) if found.is_empty() => {
                        if p.cast_name().is_none() {
                            status.set_text("Станции в сети не найдены");
                        }
                    }
                    Ok(found) => {
                        if p.cast_name().is_none() {
                            status.set_text("Выберите станцию");
                        }
                        for dev in found {
                            let b = Button::with_label(&dev.name);
                            b.add_css_class("flat");
                            let (p, pop) = (p.clone(), pop.clone());
                            b.connect_clicked(move |_| {
                                pop.popdown();
                                p.cast_connect(dev.clone());
                            });
                            list.append(&b);
                        }
                    }
                    Err(e) => status.set_text(&format!("Не удалось найти станции: {}", e)),
                }
            });
        });
    }
    btn
}

/// Размер всплывающего ползунка громкости (ширина × высота), px
const VOLUME_POPUP_SIZE: (i32, i32) = (30, 132);

/// Подключает всплывающий ползунок громкости: показ над значком при наведении, внутри `overlay`.
/// Положение задаёт сам оверлей по сигналу `get-child-position`: по центру над значком, не выходя за края.
pub fn wire_volume_popup(island: &FloatingIslandWidgets, overlay: &gtk4::Overlay) {
    let popup = &island.volume_popup;
    popup.set_size_request(VOLUME_POPUP_SIZE.0, VOLUME_POPUP_SIZE.1);
    overlay.add_overlay(popup);
    {
        let (btn, popup) = (island.volume_btn.clone(), popup.clone());
        overlay.connect_get_child_position(move |ov, child| {
            if child != popup.upcast_ref::<gtk4::Widget>() {
                return None;
            }
            let b = btn.compute_bounds(ov)?;
            let (w, h) = VOLUME_POPUP_SIZE;
            let x = (b.x() + b.width() / 2.0 - w as f32 / 2.0).clamp(0.0, (ov.width() - w).max(0) as f32);
            let y = (b.y() - h as f32 - 6.0).max(0.0);
            Some(gtk4::gdk::Rectangle::new(x as i32, y as i32, w, h))
        });
    }

    // курсор над значком / над ползунком; ползунок закрывается, когда его нет ни там, ни там
    let over = std::rc::Rc::new([std::cell::Cell::new(false), std::cell::Cell::new(false)]);
    for (i, widget) in [island.volume_btn.clone().upcast::<gtk4::Widget>(), popup.clone().upcast::<gtk4::Widget>()].into_iter().enumerate() {
        let motion = gtk4::EventControllerMotion::new();
        {
            let (over, popup, overlay) = (over.clone(), popup.clone(), overlay.clone());
            motion.connect_enter(move |_, _, _| {
                over[i].set(true);
                if !popup.is_visible() {
                    popup.set_visible(true);
                    overlay.queue_resize();
                }
            });
        }
        {
            let (over, popup) = (over.clone(), popup.clone());
            motion.connect_leave(move |_| {
                over[i].set(false);
                let (over, popup) = (over.clone(), popup.clone());
                // небольшая задержка: курсор переходит со значка на ползунок
                gtk4::glib::timeout_add_local_once(std::time::Duration::from_millis(300), move || {
                    if !over[0].get() && !over[1].get() {
                        popup.set_visible(false);
                    }
                });
            });
        }
        widget.add_controller(motion);
    }
}
