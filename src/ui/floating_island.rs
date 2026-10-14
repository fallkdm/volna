use gtk4::prelude::*;
use gtk4::{
    Align, Box as GtkBox, Button, Image, Label, Orientation, Scale, Adjustment,
};

use crate::player::PlayerManager;

pub struct FloatingIslandWidgets {
    pub container: GtkBox,
    pub cover_image: Image,
    pub title_label: Label,
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
}

pub fn create_floating_island(player: &PlayerManager) -> FloatingIslandWidgets {
    let container = GtkBox::new(Orientation::Horizontal, 16);
    container.add_css_class("floating-island-player");
    container.set_valign(Align::End);
    container.set_halign(Align::Center);
    container.set_margin_bottom(22);
    container.set_hexpand(false);

    // ================= Левая секция =================
    let left_box = GtkBox::new(Orientation::Horizontal, 10);
    left_box.set_valign(Align::Center);

    // Круглая обложка
    let cover_image = Image::from_icon_name("audio-x-generic-symbolic");
    cover_image.set_pixel_size(44);
    cover_image.add_css_class("circular-cover");
    left_box.append(&cover_image);

    // Метаданные
    let meta_box = GtkBox::new(Orientation::Vertical, 2);
    meta_box.set_valign(Align::Center);

    let title_label = Label::new(Some("Моя волна"));
    title_label.add_css_class("island-title");
    title_label.set_halign(Align::Start);
    title_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    title_label.set_max_width_chars(16);
    meta_box.append(&title_label);

    let artist_label = Label::new(Some("Яндекс Музыка"));
    artist_label.add_css_class("island-artist");
    artist_label.set_halign(Align::Start);
    artist_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    artist_label.set_max_width_chars(16);
    meta_box.append(&artist_label);

    left_box.append(&meta_box);

    // Реакции (Лайк / Дизлайк)
    let reactions_box = GtkBox::new(Orientation::Horizontal, 2);
    reactions_box.set_valign(Align::Center);

    let dislike_btn = Button::from_icon_name("starred-symbolic");
    dislike_btn.set_tooltip_text(Some("Не нравится (Дизлайк)"));
    dislike_btn.add_css_class("reaction-btn");

    let like_btn = Button::from_icon_name("emblem-favorite-symbolic");
    like_btn.set_tooltip_text(Some("Нравится (Лайк)"));
    like_btn.add_css_class("reaction-btn");

    reactions_box.append(&dislike_btn);
    reactions_box.append(&like_btn);
    left_box.append(&reactions_box);

    container.append(&left_box);

    // ================= Центральная секция =================
    let center_box = GtkBox::new(Orientation::Vertical, 4);
    center_box.set_valign(Align::Center);
    center_box.set_halign(Align::Center);

    // Транспортные кнопки (Prev, Play, Next)
    let controls_box = GtkBox::new(Orientation::Horizontal, 14);
    controls_box.set_halign(Align::Center);

    let prev_btn = Button::from_icon_name("media-skip-backward-symbolic");
    prev_btn.add_css_class("flat");
    prev_btn.add_css_class("circular");
    prev_btn.set_tooltip_text(Some("Предыдущий трек"));
    controls_box.append(&prev_btn);

    let play_btn = Button::new();
    play_btn.add_css_class("circular");
    play_btn.add_css_class("suggested-action");
    let play_btn_icon = Image::from_icon_name("media-playback-start-symbolic");
    play_btn_icon.set_pixel_size(18);
    play_btn.set_child(Some(&play_btn_icon));
    play_btn.set_tooltip_text(Some("Пауза / Воспроизведение"));
    controls_box.append(&play_btn);

    let next_btn = Button::from_icon_name("media-skip-forward-symbolic");
    next_btn.add_css_class("flat");
    next_btn.add_css_class("circular");
    next_btn.set_tooltip_text(Some("Следующий трек"));
    controls_box.append(&next_btn);

    center_box.append(&controls_box);

    // Прогресс воспроизведения
    let progress_box = GtkBox::new(Orientation::Horizontal, 8);
    progress_box.set_valign(Align::Center);

    let curr_time_label = Label::new(Some("00:00"));
    curr_time_label.add_css_class("time-label");
    progress_box.append(&curr_time_label);

    let seek_adj = Adjustment::new(0.0, 0.0, 240.0, 1.0, 5.0, 0.0);
    let seek_scale = Scale::new(Orientation::Horizontal, Some(&seek_adj));
    seek_scale.add_css_class("adw-scale");
    seek_scale.set_width_request(240);
    seek_scale.set_draw_value(false);
    progress_box.append(&seek_scale);

    let total_time_label = Label::new(Some("00:00"));
    total_time_label.add_css_class("time-label");
    progress_box.append(&total_time_label);

    center_box.append(&progress_box);
    container.append(&center_box);

    // ================= Правая секция =================
    let right_box = GtkBox::new(Orientation::Horizontal, 10);
    right_box.set_valign(Align::Center);

    let karaoke_icon = if std::path::Path::new("/home/philk/volna/data/icons/lyrics-symbolic.svg").exists() {
        let img = Image::from_file("/home/philk/volna/data/icons/lyrics-symbolic.svg");
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
    karaoke_btn.set_tooltip_text(Some("Текст песни"));
    right_box.append(&karaoke_btn);

    let lossless_badge = Label::new(Some("LOSSLESS"));
    lossless_badge.add_css_class("lossless-badge");
    right_box.append(&lossless_badge);

    // Громкость
    let volume_box = GtkBox::new(Orientation::Horizontal, 4);
    volume_box.set_valign(Align::Center);
    let vol_icon = Image::from_icon_name("audio-volume-high-symbolic");
    volume_box.append(&vol_icon);

    let vol_adj = Adjustment::new(85.0, 0.0, 100.0, 1.0, 10.0, 0.0);
    let volume_scale = Scale::new(Orientation::Horizontal, Some(&vol_adj));
    volume_scale.add_css_class("adw-scale");
    volume_scale.set_width_request(65);
    volume_scale.set_draw_value(false);
    volume_box.append(&volume_scale);

    right_box.append(&volume_box);
    container.append(&right_box);

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
        seek_scale.connect_change_value(move |_, _, new_val| {
            p.seek(new_val as u64);
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
    }
}
