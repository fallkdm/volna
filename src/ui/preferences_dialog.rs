use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Button, Entry, Image, Orientation};
use libadwaita::prelude::*;
use libadwaita::{ActionRow, ComboRow, PasswordEntryRow, PreferencesDialog, PreferencesGroup, PreferencesPage};

use crate::api::auth::{
    load_clips, load_discord, save_clips, save_discord_client_id, save_discord_rpc, load_genius_token, load_lastfm_key, load_quality, load_token, save_genius_token, save_lastfm_key, save_quality, save_token,
};
use crate::cache::{clear_clips_cache, clear_covers_cache, clips_cache_size, covers_cache_size};
use crate::player::PlayerManager;

fn size_text(bytes: u64) -> String {
    format!("{:.1} МБ", bytes as f64 / 1_048_576.0)
}

/// Строка в стиле оригинала: название, подпись и стрелка справа
fn nav_row(title: &str, subtitle: &str) -> ActionRow {
    let row = ActionRow::builder().title(title).subtitle(subtitle).activatable(true).build();
    row.add_suffix(&Image::from_icon_name("go-next-symbolic"));
    row
}

/// Окно настроек. `open_wave_settings` открывает панель настройки волны на экране «Волна».
pub fn show_preferences_dialog(
    parent: &impl IsA<gtk4::Widget>,
    player: &PlayerManager,
    open_wave_settings: impl Fn() + 'static,
) {
    let dialog = PreferencesDialog::new();
    dialog.set_title("Настройки");

    let page = PreferencesPage::new();

    // Волна: настроение, характер и язык настраиваются на самом экране волны
    let wave_group = PreferencesGroup::new();
    let wave_row = nav_row("Настроить волну", "Настроение, характер и язык композиций");
    {
        let d_weak = dialog.downgrade();
        wave_row.connect_activated(move |_| {
            if let Some(d) = d_weak.upgrade() {
                d.close();
            }
            open_wave_settings();
        });
    }
    wave_group.add(&wave_row);
    page.add(&wave_group);

    // Качество звука: действует со следующего трека
    let quality_group = PreferencesGroup::new();
    let quality_row = ComboRow::builder()
        .title("Качество звука")
        .subtitle("Яндекс отдаёт сторонним клиентам только MP3 (сейчас до 192 кбит/с); lossless недоступен. Применится со следующего трека")
        .model(&gtk4::StringList::new(&["Превосходное", "Оптимальное", "Экономичное"]))
        .build();
    quality_row.set_selected(match load_quality().as_str() {
        "balanced" => 1,
        "economy" => 2,
        _ => 0,
    });
    quality_row.connect_selected_notify(|row| {
        let _ = save_quality(match row.selected() {
            1 => "balanced",
            2 => "economy",
            _ => "best",
        });
    });
    quality_group.add(&quality_row);
    page.add(&quality_group);

    // Клипы на фоне Волны (по желанию)
    let clips_group = PreferencesGroup::new();
    let clips_row = libadwaita::SwitchRow::builder()
        .title("Клипы на фоне")
        .subtitle("Если у трека есть клип в Яндексе, его короткое превью (4–10 с) зациклено и полупрозрачно играет на фоне Волны. Скачивается несколько МБ на трек. Применится со следующего трека")
        .active(load_clips())
        .build();
    {
        let p = player.clone();
        clips_row.connect_active_notify(move |row| {
            let _ = save_clips(row.is_active());
            if !row.is_active() {
                p.emit(crate::player::PlayerUiEvent::ClipUpdated(None));
            }
        });
    }
    clips_group.add(&clips_row);
    page.add(&clips_group);

    // Discord: что играет сейчас — в статусе профиля
    let discord_group = PreferencesGroup::new();
    discord_group.set_title("Discord");
    discord_group.set_description(Some(
        "Показывает в профиле Discord название трека, исполнителя, обложку и время. Discord должен быть запущен.",
    ));
    let (discord_on, discord_id) = load_discord();
    let has_builtin = crate::discord::DEFAULT_APP_ID.is_some();
    let discord_row = libadwaita::SwitchRow::builder().title("Статус в Discord").active(discord_on).build();
    if !has_builtin && discord_id.is_none() {
        discord_row.set_subtitle("Нужен Application ID: впишите свой ниже");
    }
    let discord_id_row = libadwaita::EntryRow::builder()
        .title(if has_builtin { "Свой Application ID (необязательно)" } else { "Application ID (discord.com/developers)" })
        .show_apply_button(true)
        .build();
    if let Some(id) = discord_id {
        discord_id_row.set_text(&id);
    }
    {
        let p = player.clone();
        discord_row.connect_active_notify(move |row| {
            let _ = save_discord_rpc(row.is_active());
            p.emit(crate::player::PlayerUiEvent::DiscordSettingsChanged);
        });
        let p = player.clone();
        discord_id_row.connect_apply(move |row| {
            let _ = save_discord_client_id(&row.text());
            p.emit(crate::player::PlayerUiEvent::DiscordSettingsChanged);
        });
    }
    discord_group.add(&discord_row);
    // свой ID нужен только пока у приложения нет встроенного; потом это скрытая «расширенная» настройка
    discord_group.add(&discord_id_row);
    page.add(&discord_group);

    // Память
    let memory_group = PreferencesGroup::new();
    let clear_row = nav_row("Очистить память", &format!("Обложки и клипы: {}", size_text(covers_cache_size() + clips_cache_size())));
    {
        let p = player.clone();
        clear_row.connect_activated(move |row| {
            let freed = clear_covers_cache() + clear_clips_cache();
            row.set_subtitle("Обложки и клипы: 0.0 МБ");
            p.emit(crate::player::PlayerUiEvent::StatusMessage(format!("Освобождено {}", size_text(freed))));
        });
    }
    memory_group.add(&clear_row);
    page.add(&memory_group);

    // Авторизация в Яндекс Музыке
    let auth_group = PreferencesGroup::new();
    auth_group.set_title("Авторизация");

    let token_row = ActionRow::builder()
        .title("OAuth токен")
        .subtitle("Для персональной Волны и высокого битрейта. Хранится в ~/.config/volna/config.json")
        .build();

    let entry_box = GtkBox::new(Orientation::Horizontal, 8);
    let token_entry = Entry::new();
    token_entry.set_width_chars(24);
    token_entry.set_visibility(false);
    token_entry.set_placeholder_text(Some("y0_AgAAAA..."));
    token_entry.set_valign(gtk4::Align::Center);

    if let Some(saved) = load_token() {
        token_entry.set_text(&saved);
    }

    let save_btn = Button::with_label("Сохранить");
    save_btn.add_css_class("suggested-action");
    save_btn.add_css_class("pill");
    save_btn.set_valign(gtk4::Align::Center);

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
                "Токен сохранен! Загружаем персональную Волну...".into(),
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

    // Genius: справка об исполнителях, которых нет в Википедии
    let genius_group = PreferencesGroup::new();
    genius_group.set_title("Справка об исполнителях");
    genius_group.set_description(Some(
        "Дополнительные источники после Википедии: ключ Last.fm API (last.fm/api/account/create) и токен Genius (genius.com/api-clients). Хранятся в ~/.config/volna/config.json",
    ));
    let genius_row = PasswordEntryRow::builder().title("Токен Genius").show_apply_button(true).build();
    if let Some(saved) = load_genius_token() {
        genius_row.set_text(&saved);
    }
    genius_row.connect_apply(|row| {
        let _ = save_genius_token(&row.text());
    });
    genius_group.add(&genius_row);

    let lastfm_row = PasswordEntryRow::builder().title("Ключ Last.fm API").show_apply_button(true).build();
    if let Some(saved) = load_lastfm_key() {
        lastfm_row.set_text(&saved);
    }
    lastfm_row.connect_apply(|row| {
        let _ = save_lastfm_key(&row.text());
    });
    genius_group.add(&lastfm_row);
    page.add(&genius_group);

    // О приложении
    let about_group = PreferencesGroup::new();
    about_group.set_title("О приложении");
    about_group.set_description(Some(
        "Неофициальный клиент. Не связан с ООО «Яндекс» и не одобрен им; автор не претендует на права Яндекса. \
         «Яндекс», «Яндекс Музыка» и «Моя волна» — товарные знаки их правообладателей. \
         Приложение не хранит и не распространяет музыку: воспроизведение идёт через ваш личный аккаунт. \
         Распространяется по лицензии GNU GPL v3.0, без каких-либо гарантий.",
    ));
    let about_row = ActionRow::builder()
        .title("Версия")
        .subtitle(format!("Волна {}", env!("CARGO_PKG_VERSION")))
        .build();
    about_group.add(&about_row);
    page.add(&about_group);

    dialog.add(&page);
    dialog.present(Some(parent));
}
