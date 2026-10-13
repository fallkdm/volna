use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Button, Entry, Orientation};
use libadwaita::prelude::*;
use libadwaita::{ActionRow, PreferencesDialog, PreferencesGroup, PreferencesPage};

use crate::api::auth::{load_token, save_token};
use crate::player::PlayerManager;

pub fn show_preferences_dialog(parent: &impl IsA<gtk4::Widget>, player: &PlayerManager) {
    let dialog = PreferencesDialog::new();
    dialog.set_title("Настройки Моей волны");

    let page = PreferencesPage::new();
    page.set_title("Параметры потока");
    page.set_icon_name(Some("audio-volume-high-symbolic"));

    // Группа 1: Настроение
    let mood_group = PreferencesGroup::new();
    mood_group.set_title("Настроение");
    mood_group.set_description(Some("Выберите эмоциональную окраску рекомендаций"));

    let moods = [
        ("⚡ Энергичное", "energetic"),
        ("☕ Спокойное", "calm"),
        ("🎉 Весёлое", "fun"),
        ("🌧 Грустное", "sad"),
    ];

    for (name, tag) in moods {
        let row = ActionRow::builder()
            .title(name)
            .activatable(true)
            .build();

        let player_clone = player.clone();
        let dialog_weak = dialog.downgrade();
        row.connect_activated(move |_| {
            player_clone.fetch_wave_tracks(tag);
            if let Some(d) = dialog_weak.upgrade() {
                d.close();
            }
        });
        mood_group.add(&row);
    }
    page.add(&mood_group);

    // Группа 2: Характер
    let char_group = PreferencesGroup::new();
    char_group.set_title("Характер рекомендаций");

    let characters = [
        ("❤️ Любимое", "favorite"),
        ("🧭 Незнакомое", "discover"),
        ("🔥 Популярное", "popular"),
    ];

    for (name, tag) in characters {
        let row = ActionRow::builder()
            .title(name)
            .activatable(true)
            .build();

        let player_clone = player.clone();
        let dialog_weak = dialog.downgrade();
        row.connect_activated(move |_| {
            player_clone.fetch_wave_tracks(tag);
            if let Some(d) = dialog_weak.upgrade() {
                d.close();
            }
        });
        char_group.add(&row);
    }
    page.add(&char_group);

    // Группа 3: Язык
    let lang_group = PreferencesGroup::new();
    lang_group.set_title("Язык композиций");

    let languages = [
        ("🌐 Любой", "any"),
        ("🇷🇺 Русский", "russian"),
        ("🌍 Иностранный", "foreign"),
        ("🎹 Без слов", "instrumental"),
    ];

    for (name, tag) in languages {
        let row = ActionRow::builder()
            .title(name)
            .activatable(true)
            .build();

        let player_clone = player.clone();
        let dialog_weak = dialog.downgrade();
        row.connect_activated(move |_| {
            player_clone.fetch_wave_tracks(tag);
            if let Some(d) = dialog_weak.upgrade() {
                d.close();
            }
        });
        lang_group.add(&row);
    }
    page.add(&lang_group);

    // Группа 4: Авторизация в Яндекс Музыке
    let auth_group = PreferencesGroup::new();
    auth_group.set_title("Авторизация Яндекс Музыки");
    auth_group.set_description(Some("OAuth токен для доступа к персональной Волне и полным трекам"));

    let token_row = ActionRow::builder()
        .title("OAuth токен")
        .subtitle("Сохраняется в ~/.config/volna/config.json")
        .build();

    let entry_box = GtkBox::new(Orientation::Horizontal, 8);
    let token_entry = Entry::new();
    token_entry.set_width_chars(24);
    token_entry.set_visibility(false);
    token_entry.set_placeholder_text(Some("y0_AgAAAA..."));

    if let Some(saved) = load_token() {
        token_entry.set_text(&saved);
    }

    let save_btn = Button::with_label("Сохранить");
    save_btn.add_css_class("suggested-action");

    let player_clone = player.clone();
    let entry_clone = token_entry.clone();
    let dialog_weak = dialog.downgrade();
    save_btn.connect_clicked(move |_| {
        let new_token = entry_clone.text().to_string();
        if !new_token.is_empty() {
            let _ = save_token(&new_token);
            if let Ok(mut client) = player_clone.client.lock() {
                client.set_token(Some(new_token));
            }
            player_clone.emit(crate::player::PlayerUiEvent::StatusMessage(
                "Токен успешно сохранен! Загружаем персональную Волну...".into(),
            ));
            player_clone.fetch_wave_tracks("onyourwave");
        }
        if let Some(d) = dialog_weak.upgrade() {
            d.close();
        }
    });

    entry_box.append(&token_entry);
    entry_box.append(&save_btn);
    token_row.add_suffix(&entry_box);
    auth_group.add(&token_row);
    page.add(&auth_group);

    dialog.add(&page);
    dialog.present(Some(parent));
}
