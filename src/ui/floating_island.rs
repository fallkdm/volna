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

    // иконка лежит в исходниках: ищем по пути сборки, затем относительно текущей папки
    let built_icon = concat!(env!("CARGO_MANIFEST_DIR"), "/data/icons/lyrics-symbolic.svg");
    let karaoke_icon = if std::path::Path::new(built_icon).exists() {
        let img = Image::from_file(built_icon);
        img.set_pixel_size(18);
        img
    } else if std::path::Path::new("data/icons/lyrics-symbolic.svg").exists() {
        let img = Image::from_file("data/icons/lyrics-symbolic.svg");
        img.set_pixel_size(18);
        img
    } else {
        let img = Image::from_icon_name("text-x-generic-symbolic");
        img.set_pixel_size(18);
        img
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

    let quality_badge = Label::new(Some("MP3"));
    quality_badge.add_css_class("lossless-badge");
    quality_badge.set_valign(Align::Center);
    right_box.append(&quality_badge);

    // Громкость
    let volume_box = GtkBox::new(Orientation::Horizontal, 8);
    volume_box.set_valign(Align::Center);
    let vol_icon = Image::from_icon_name("audio-volume-high-symbolic");
    vol_icon.add_css_class("island-volume-icon");
    volume_box.append(&vol_icon);

    let vol_adj = Adjustment::new(85.0, 0.0, 100.0, 1.0, 10.0, 0.0);
    let volume_scale = Scale::new(Orientation::Horizontal, Some(&vol_adj));
    volume_scale.add_css_class("adw-scale");
    volume_scale.set_width_request(32);
    volume_scale.set_hexpand(true);
    volume_scale.set_draw_value(false);
    volume_scale.set_tooltip_text(Some("Громкость"));
    volume_box.set_hexpand(true);
    volume_box.append(&volume_scale);

    right_box.append(&volume_box);
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
        karaoke_btn,
        quality_badge,
        repeat_btn,
        repeat_one,
    }
}
