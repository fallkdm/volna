use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Button, Entry, Image, Orientation};
use libadwaita::prelude::*;
use libadwaita::{
    ActionRow, ComboRow, EntryRow, PasswordEntryRow, PreferencesDialog, PreferencesGroup,
    PreferencesPage, SpinRow, SwitchRow,
};

use crate::api::auth::{
    load_clips, load_crossfade, load_discord, load_genius_token, load_lastfm_key, load_nav_style,
    load_player_layout, load_quality, load_theme_accent, load_token, save_crossfade,
    save_discord_client_id, save_discord_rpc, save_genius_token, save_lastfm_key, save_nav_style,
    save_player_layout, save_quality, save_theme_accent, save_token,
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

    // Внешний вид
    let appearance_group = PreferencesGroup::new();
    appearance_group.set_title("Внешний вид");

    // 1. Режим интерфейса
    let nav_style_row = ComboRow::builder()
        .title("Режим интерфейса")
        .subtitle("Расположение навигации: оригинальный верхний бар или левый сайдбар")
        .model(&gtk4::StringList::new(&["Оригинальный (Wave-First)", "Spotify-Style"]))
        .build();
    nav_style_row.set_selected(match load_nav_style().as_str() {
        "spotify" => 1,
        _ => 0,
    });
    {
        let p = player.clone();
        nav_style_row.connect_selected_notify(move |row| {
            let val = match row.selected() {
                1 => "spotify",
                _ => "wave",
            };
            let _ = save_nav_style(val);
            p.emit(crate::player::PlayerUiEvent::NavLayoutChanged(val.to_string()));
        });
    }
    appearance_group.add(&nav_style_row);

    // 2. Стиль плеера
    let layout_row = ComboRow::builder()
        .title("Стиль плеера")
        .subtitle("Форма панели воспроизведения: парящая капсула или прижатая полоса")
        .model(&gtk4::StringList::new(&["Парящий остров", "Прижатый док (Spotify)"]))
        .build();
    layout_row.set_selected(match load_player_layout().as_str() {
        "dock" => 1,
        _ => 0,
    });
    {
        let p = player.clone();
        layout_row.connect_selected_notify(move |row| {
            let val = match row.selected() {
                1 => "dock",
                _ => "floating",
            };
            let _ = save_player_layout(val);
            p.emit(crate::player::PlayerUiEvent::PlayerLayoutChanged(val.to_string()));
        });
    }
    appearance_group.add(&layout_row);

    // 3. Цветовой акцент
    let accent_row = ComboRow::builder()
        .title("Цветовой акцент")
        .subtitle("Цвет подсветки, элементов плеера и линий волны")
        .model(&gtk4::StringList::new(&[
            "Динамический (по обложке)",
            "Spotify Green",
            "Ruby Red",
            "Sunset Orange",
            "Cyber Violet",
            "Deep Ocean",
            "Белый монохром",
        ]))
        .build();
    accent_row.set_selected(match load_theme_accent().as_str() {
        "spotify" => 1,
        "ruby" => 2,
        "sunset" => 3,
        "violet" => 4,
        "ocean" => 5,
        "monochrome" => 6,
        _ => 0,
    });
    {
        let p = player.clone();
        accent_row.connect_selected_notify(move |row| {
            let val = match row.selected() {
                1 => "spotify",
                2 => "ruby",
                3 => "sunset",
                4 => "violet",
                5 => "ocean",
                6 => "monochrome",
                _ => "dynamic",
            };
            let _ = save_theme_accent(val);
            p.emit(crate::player::PlayerUiEvent::ThemeAccentChanged(val.to_string()));
        });
    }
    appearance_group.add(&accent_row);

    page.add(&appearance_group);

    // Воспроизведение: качество звука и плавный переход
    let playback_group = PreferencesGroup::new();
    playback_group.set_title("Воспроизведение");

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
    playback_group.add(&quality_row);

    let (crossfade_enabled, crossfade_dur) = load_crossfade();

    let crossfade_switch = SwitchRow::builder()
        .title("Плавный переход между треками")
        .subtitle("Мягкая смена композиций без резких пауз")
        .active(crossfade_enabled)
        .build();

    let duration_adj = gtk4::Adjustment::new(
        crossfade_dur as f64,
        1.0,
        10.0,
        1.0,
        1.0,
        0.0,
    );

    let duration_spin = SpinRow::builder()
        .title("Длительность перехода")
        .subtitle("Время затухания и нарастания звука в секундах")
        .adjustment(&duration_adj)
        .numeric(true)
        .sensitive(crossfade_enabled)
        .build();

    {
        let p = player.clone();
        let spin_clone = duration_spin.clone();
        crossfade_switch.connect_active_notify(move |row| {
            let active = row.is_active();
            spin_clone.set_sensitive(active);
            let dur = spin_clone.value().round().clamp(1.0, 10.0) as u32;
            let _ = save_crossfade(active, dur);
            p.set_crossfade(active, dur);
            p.emit(crate::player::PlayerUiEvent::CrossfadeChanged { enabled: active, duration: dur });
        });
    }

    {
        let p = player.clone();
        let sw_clone = crossfade_switch.clone();
        duration_spin.connect_value_notify(move |spin| {
            let active = sw_clone.is_active();
            let dur = spin.value().round().clamp(1.0, 10.0) as u32;
            let _ = save_crossfade(active, dur);
            p.set_crossfade(active, dur);
            p.emit(crate::player::PlayerUiEvent::CrossfadeChanged { enabled: active, duration: dur });
        });
    }

    playback_group.add(&crossfade_switch);
    playback_group.add(&duration_spin);
    page.add(&playback_group);

    // Клипы на фоне Волны (по желанию)
    let clips_group = PreferencesGroup::new();
    let clips_row = libadwaita::SwitchRow::builder()
        .title("Клипы на фоне")
        .subtitle("Полные клипы для настоящих музыкальных видео YouTube и короткие зацикленные превью Яндекс Музыки. Арт-треки со статичной обложкой пропускаются")
        .active(load_clips())
        .build();
    {
        let p = player.clone();
        clips_row.connect_active_notify(move |row| {
            p.set_clips_enabled(row.is_active());
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

    // Сервисы: YouTube Music
    let services_group = PreferencesGroup::new();
    services_group.set_title("Сервисы");

    let is_yt_auth = crate::api::auth::load_ytmusic_cookie().is_some()
        || crate::api::auth::load_ytmusic_refresh_token().is_some();
    let yt_status_row = ActionRow::builder()
        .title("YouTube Music")
        .subtitle(if is_yt_auth { "Авторизован в YouTube Music" } else { "Не авторизован (гостевой режим)" })
        .build();

    let yt_btn_box = GtkBox::new(Orientation::Horizontal, 8);

    let yt_login_btn = Button::with_label("Войти");
    yt_login_btn.add_css_class("suggested-action");
    yt_login_btn.add_css_class("pill");
    yt_login_btn.set_valign(gtk4::Align::Center);
    yt_login_btn.set_visible(!is_yt_auth);

    let yt_logout_btn = Button::with_label("Выйти");
    yt_logout_btn.add_css_class("destructive-action");
    yt_logout_btn.add_css_class("pill");
    yt_logout_btn.set_valign(gtk4::Align::Center);
    yt_logout_btn.set_visible(is_yt_auth);

    {
        let p = player.clone();
        let status_row_clone = yt_status_row.clone();
        let in_btn_clone = yt_login_btn.clone();
        let out_btn_clone = yt_logout_btn.clone();
        let parent_widget = parent.upcast_ref::<gtk4::Widget>().clone();

        yt_login_btn.connect_clicked(move |_| {
            let row = status_row_clone.clone();
            let ib = in_btn_clone.clone();
            let ob = out_btn_clone.clone();
            show_ytmusic_auth_dialog(&parent_widget, &p, move || {
                row.set_subtitle("Авторизован в YouTube Music");
                ib.set_visible(false);
                ob.set_visible(true);
            });
        });
    }

    {
        let p = player.clone();
        let status_row_clone = yt_status_row.clone();
        let in_btn_clone = yt_login_btn.clone();
        let out_btn_clone = yt_logout_btn.clone();

        yt_logout_btn.connect_clicked(move |_| {
            let _ = crate::api::auth::clear_ytmusic_cookie();
            let _ = crate::api::auth::clear_ytmusic_refresh_token();
            let guest = std::sync::Arc::new(crate::api::ytmusic::YtMusicClient::new()) as std::sync::Arc<dyn crate::api::provider::MusicProvider>;
            p.providers.register(guest);
            status_row_clone.set_subtitle("Не авторизован (гостевой режим)");
            in_btn_clone.set_visible(true);
            out_btn_clone.set_visible(false);
            p.emit(crate::player::PlayerUiEvent::StatusMessage("Вы вышли из аккаунта YouTube Music".to_string()));
            if p.providers.active_id() == crate::api::provider::ProviderId::YouTubeMusic {
                p.emit(crate::player::PlayerUiEvent::ProviderChanged(crate::api::provider::ProviderId::YouTubeMusic));
            }
        });
    }

    yt_btn_box.append(&yt_login_btn);
    yt_btn_box.append(&yt_logout_btn);
    yt_status_row.add_suffix(&yt_btn_box);
    services_group.add(&yt_status_row);
    page.add(&services_group);

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

/// Отображает диалог входа по Cookie для YouTube Music
pub fn show_ytmusic_auth_dialog(
    parent: &impl IsA<gtk4::Widget>,
    player: &PlayerManager,
    on_success: impl Fn() + 'static,
) {
    let dialog = PreferencesDialog::new();
    dialog.set_title("Вход в YouTube Music");

    let page = PreferencesPage::new();
    let on_success = std::rc::Rc::new(on_success);

    // Вход по коду Google: refresh-токен вместо куки из браузера.
    // Кука живёт до первой ротации токенов Google, refresh-токен — месяцами.
    let device_group = PreferencesGroup::new();
    device_group.set_title("Вход по коду Google (рекомендуется)");
    device_group.set_description(Some(
        "Один раз подтвердите код на google.com/device. Приложение сохранит refresh-токен и будет обновлять доступ само — куки из браузера больше не нужны.",
    ));
    let device_row = ActionRow::builder()
        .title("Войти без куки")
        .subtitle("Код действует несколько минут, вход сохраняется надолго")
        .build();
    let device_btn = Button::with_label("Получить код");
    device_btn.add_css_class("suggested-action");
    device_btn.add_css_class("pill");
    device_btn.set_valign(gtk4::Align::Center);
    device_row.add_suffix(&device_btn);

    // Свой OAuth-клиент Google: встроенный Google отозвал (invalid_client)
    let client_id_row = EntryRow::builder()
        .title("OAuth Client ID")
        .build();
    let client_secret_row = PasswordEntryRow::builder()
        .title("OAuth Client Secret")
        .build();
    if let (Some(id), Some(secret)) = crate::api::auth::load_ytmusic_oauth_client() {
        client_id_row.set_text(&id);
        client_secret_row.set_text(&secret);
    }
    let oauth_hint = gtk4::Label::new(Some(
        "Создайте OAuth-клиент типа «TVs and Limited Input devices» в Google Cloud, включите YouTube Data API v3 и вставьте его id/secret сюда. Это разовая настройка: дальше вход по коду работает без куки и без браузера.",
    ));
    oauth_hint.set_wrap(true);
    oauth_hint.set_xalign(0.0);
    oauth_hint.add_css_class("dim-label");

    let device_status = gtk4::Label::new(None);
    device_status.set_wrap(true);
    device_status.set_xalign(0.0);
    device_status.add_css_class("dim-label");
    device_status.set_visible(false);
    device_group.add(&device_row);
    device_group.add(&client_id_row);
    device_group.add(&client_secret_row);
    device_group.add(&oauth_hint);
    device_group.add(&device_status);

    {
        let player = player.clone();
        let dialog_weak = dialog.downgrade();
        let status = device_status.clone();
        let on_success = on_success.clone();
        let client_id_row = client_id_row.clone();
        let client_secret_row = client_secret_row.clone();
        device_btn.connect_clicked(move |btn| {
            btn.set_sensitive(false);
            // клиент сохраняем до запроса кода: он нужен и для refresh-токена
            let _ = crate::api::auth::save_ytmusic_oauth_client(
                Some(&client_id_row.text()),
                Some(&client_secret_row.text()),
            );
            let player = player.clone();
            let dialog_weak = dialog_weak.clone();
            let status = status.clone();
            let on_success = on_success.clone();
            gtk4::glib::spawn_future_local(async move {
                let http = reqwest::Client::new();
                let code = match crate::api::ytmusic::request_device_code(&http).await {
                    Ok(code) => code,
                    Err(e) => {
                        status.set_text(&format!("Не удалось получить код: {e}"));
                        status.set_visible(true);
                        return;
                    }
                };
                status.set_text(&format!(
                    "Код: {}\nОткройте {} и введите его",
                    code.user_code, code.verification_url
                ));
                status.set_visible(true);
                let _ = gtk4::gio::AppInfo::launch_default_for_uri(
                    &code.verification_url,
                    None::<&gtk4::gio::AppLaunchContext>,
                );

                let interval = code.interval.max(5);
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
                    match crate::api::ytmusic::poll_device_token(&http, &code.device_code).await {
                        Ok(Some(token)) => {
                            let Some(refresh) = token.refresh_token.clone() else {
                                status.set_text("Google не выдал refresh-токен, попробуйте ещё раз");
                                return;
                            };
                            if let Err(e) = crate::api::auth::save_ytmusic_refresh_token(&refresh) {
                                eprintln!("[ytmusic] не удалось сохранить refresh token: {e}");
                            }
                            // протухшая кука больше не нужна
                            let _ = crate::api::auth::clear_ytmusic_cookie();
                            let yt_client =
                                crate::api::ytmusic::YtMusicClient::with_refresh_token(refresh);
                            player.providers.register(
                                std::sync::Arc::new(yt_client)
                                    as std::sync::Arc<dyn crate::api::provider::MusicProvider>,
                            );
                            on_success();
                            player.emit(crate::player::PlayerUiEvent::StatusMessage(
                                "Вход в YouTube Music по коду выполнен".to_string(),
                            ));
                            if player.providers.active_id()
                                == crate::api::provider::ProviderId::YouTubeMusic
                            {
                                player.emit(crate::player::PlayerUiEvent::ProviderChanged(
                                    crate::api::provider::ProviderId::YouTubeMusic,
                                ));
                            }
                            if let Some(d) = dialog_weak.upgrade() {
                                d.close();
                            }
                            return;
                        }
                        Ok(None) => continue,
                        Err(e) => {
                            status.set_text(&format!("Не удалось войти: {e}"));
                            return;
                        }
                    }
                }
            });
        });
    }
    page.add(&device_group);

    let group = PreferencesGroup::new();
    group.set_title("Вход в YouTube Music");
    group.set_description(Some(
        "Войдите в YouTube Music через сессию браузера для доступа к вашим плейлистам, любимым трекам и персональной Волне.",
    ));

    // Шаг 1: Кнопка «Открыть music.youtube.com ↗»
    let step1_row = ActionRow::builder()
        .title("Шаг 1: Откройте YouTube Music в браузере")
        .subtitle("Перейдите на сайт и убедитесь, что вы авторизованы")
        .build();

    let open_browser_btn = Button::with_label("Открыть music.youtube.com ↗");
    open_browser_btn.add_css_class("suggested-action");
    open_browser_btn.add_css_class("pill");
    open_browser_btn.set_valign(gtk4::Align::Center);
    open_browser_btn.connect_clicked(|_| {
        let _ = gtk4::gio::AppInfo::launch_default_for_uri(
            "https://music.youtube.com",
            None::<&gtk4::gio::AppLaunchContext>,
        );
    });
    step1_row.add_suffix(&open_browser_btn);
    group.add(&step1_row);

    // Шаг 2: Подсказка
    let step2_row = ActionRow::builder()
        .title("Шаг 2: Скопируйте Cookie из браузера")
        .subtitle(
            "В браузере откройте инструменты разработчика (F12) → вкладка Network (Сеть) → кликните по любому запросу к YouTube Music и скопируйте заголовок Cookie",
        )
        .build();
    step2_row.set_subtitle_lines(0);
    group.add(&step2_row);

    // Шаг 3: Поле ввода с кнопкой вставки из буфера
    let cookie_row = PasswordEntryRow::builder()
        .title("Шаг 3: Заголовок Cookie")
        .build();

    if let Some(saved_cookie) = crate::api::auth::load_ytmusic_cookie() {
        cookie_row.set_text(&saved_cookie);
    }

    let paste_btn = Button::from_icon_name("edit-paste-symbolic");
    paste_btn.set_tooltip_text(Some("Вставить из буфера обмена"));
    paste_btn.add_css_class("flat");
    paste_btn.set_valign(gtk4::Align::Center);

    let cookie_row_weak = cookie_row.downgrade();
    paste_btn.connect_clicked(move |_| {
        if let Some(display) = gtk4::gdk::Display::default() {
            let clipboard = display.clipboard();
            let row_weak = cookie_row_weak.clone();
            clipboard.read_text_async(None::<&gtk4::gio::Cancellable>, move |res| {
                if let Ok(Some(text)) = res {
                    if let Some(row) = row_weak.upgrade() {
                        row.set_text(&text);
                    }
                }
            });
        }
    });
    cookie_row.add_suffix(&paste_btn);
    group.add(&cookie_row);

    // Кнопка «Войти»
    let action_row = ActionRow::builder()
        .title("Завершить вход")
        .subtitle("Применить куки и подключить YouTube Music")
        .build();

    let login_btn = Button::with_label("Войти");
    login_btn.add_css_class("suggested-action");
    login_btn.add_css_class("pill");
    login_btn.set_valign(gtk4::Align::Center);

    let player_clone = player.clone();
    let dialog_weak = dialog.downgrade();
    let cookie_row_clone = cookie_row.clone();

    login_btn.connect_clicked(move |_| {
        let cookie_text = cookie_row_clone.text().to_string();
        let cookie_trimmed = cookie_text.trim();
        if !cookie_trimmed.is_empty() {
            if let Err(e) = crate::api::auth::save_ytmusic_cookie(cookie_trimmed) {
                eprintln!("[ytmusic] Ошибка сохранения cookie: {e}");
            }
            let yt_client = crate::api::ytmusic::YtMusicClient::with_cookie(cookie_trimmed.to_string());
            player_clone.providers.register(
                std::sync::Arc::new(yt_client) as std::sync::Arc<dyn crate::api::provider::MusicProvider>,
            );
            on_success();
            player_clone.emit(crate::player::PlayerUiEvent::StatusMessage(
                "Успешный вход в YouTube Music! 🔴".to_string(),
            ));
            if player_clone.providers.active_id() == crate::api::provider::ProviderId::YouTubeMusic {
                player_clone.emit(crate::player::PlayerUiEvent::ProviderChanged(
                    crate::api::provider::ProviderId::YouTubeMusic,
                ));
            }
            if let Some(d) = dialog_weak.upgrade() {
                d.close();
            }
        }
    });
    action_row.add_suffix(&login_btn);
    group.add(&action_row);

    page.add(&group);
    dialog.add(&page);
    dialog.present(Some(parent));
}
