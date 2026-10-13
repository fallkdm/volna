use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Entry, Label, Orientation, Spinner};
use libadwaita::prelude::*;
use libadwaita::{ActionRow, PreferencesDialog, PreferencesGroup, PreferencesPage};

use crate::api::auth::{
    extract_token_from_input, request_device_code, save_token, OAUTH_AUTHORIZE_URL,
};
use crate::api::client::YandexMusicClient;
use crate::player::PlayerManager;

/// Отображает диалог входа в аккаунт Яндекс Музыки
pub fn show_auth_dialog(parent: &impl IsA<gtk4::Widget>, player: &PlayerManager) {
    let dialog = PreferencesDialog::new();
    dialog.set_title("Вход в Яндекс Музыку");

    let page = PreferencesPage::new();
    page.set_title("Авторизация");
    page.set_icon_name(Some("avatar-default-symbolic"));

    // ================= Группа 1: Быстрый вход по ссылке OAuth =================
    let link_group = PreferencesGroup::new();
    link_group.set_title("Быстрый вход через браузер");
    link_group.set_description(Some(
        "Нажмите кнопку ниже, подтвердите вход в Яндекс ID, затем скопируйте полученную адресную строку."
    ));

    let step1_row = ActionRow::builder()
        .title("1. Открыть страницу входа")
        .subtitle("Откроется официальная страница Яндекс ID в браузере")
        .build();

    let open_browser_btn = Button::with_label("Получить токен ↗");
    open_browser_btn.add_css_class("suggested-action");

    open_browser_btn.connect_clicked(|_| {
        let _ = gio::AppInfo::launch_default_for_uri(
            OAUTH_AUTHORIZE_URL,
            None::<&gio::AppLaunchContext>,
        );
    });

    step1_row.add_suffix(&open_browser_btn);
    link_group.add(&step1_row);

    let step2_row = ActionRow::builder()
        .title("2. Вставить токен или адресную строку")
        .subtitle("Вставьте токен или полный URL из адресной строки браузера после входа")
        .build();

    let input_box = GtkBox::new(Orientation::Horizontal, 8);
    let token_entry = Entry::new();
    token_entry.set_width_chars(28);
    token_entry.set_placeholder_text(Some("y0_AgAAAA... или https://music.yandex.ru/#access_token=..."));
    token_entry.set_hexpand(true);

    let paste_btn = Button::from_icon_name("edit-paste-symbolic");
    paste_btn.set_tooltip_text(Some("Вставить из буфера обмена"));

    let submit_btn = Button::with_label("Войти");
    submit_btn.add_css_class("suggested-action");

    let spinner = Spinner::new();
    spinner.set_visible(false);

    input_box.append(&token_entry);
    input_box.append(&paste_btn);
    input_box.append(&spinner);
    input_box.append(&submit_btn);
    step2_row.add_suffix(&input_box);
    link_group.add(&step2_row);

    page.add(&link_group);

    // ================= Группа 2: Вход по коду устройства (ya.ru/device) =================
    let device_group = PreferencesGroup::new();
    device_group.set_title("Альтернатива: Вход по коду (ya.ru/device)");
    device_group.set_description(Some(
        "Вы также можете подтвердить вход по коду устройства на ya.ru/device"
    ));

    let code_row = ActionRow::builder()
        .title("Код устройства")
        .subtitle("Загрузка одноразового кода...")
        .build();

    let open_device_btn = Button::with_label("Открыть ya.ru/device ↗");
    open_device_btn.connect_clicked(|_| {
        let _ = gio::AppInfo::launch_default_for_uri(
            "https://ya.ru/device",
            None::<&gio::AppLaunchContext>,
        );
    });
    code_row.add_suffix(&open_device_btn);
    device_group.add(&code_row);

    page.add(&device_group);

    // Статус и ошибки
    let status_label = Label::new(None);
    status_label.set_halign(Align::Center);
    status_label.set_margin_top(8);
    status_label.add_css_class("error");
    status_label.set_visible(false);

    dialog.add(&page);

    // Вставка из буфера обмена
    {
        let entry_clone = token_entry.clone();
        let display = gtk4::gdk::Display::default();
        paste_btn.connect_clicked(move |_| {
            if let Some(d) = display.as_ref() {
                let clipboard = d.clipboard();
                let ec = entry_clone.clone();
                clipboard.read_text_async(None::<&gio::Cancellable>, move |res| {
                    if let Ok(Some(text)) = res {
                        ec.set_text(&text);
                    }
                });
            }
        });
    }

    // Асинхронный запрос кода устройства при открытии диалога
    {
        let row_weak = code_row.downgrade();
        gtk4::glib::spawn_future_local(async move {
            if let Ok(device_code) = request_device_code().await {
                if let Some(r) = row_weak.upgrade() {
                    r.set_title(&format!("Код: {}", device_code.user_code));
                    r.set_subtitle(&format!(
                        "Введите этот код на {} для подтверждения",
                        device_code.verification_url
                    ));
                }
            }
        });
    }

    // Обработчик кнопки «Войти»
    {
        let player_clone = player.clone();
        let dialog_weak = dialog.downgrade();
        let entry_clone = token_entry.clone();
        let status_weak = status_label.downgrade();
        let spinner_clone = spinner.clone();
        let submit_clone = submit_btn.clone();

        submit_btn.connect_clicked(move |_| {
            let raw_text = entry_clone.text().to_string();
            let clean_token = extract_token_from_input(&raw_text);

            if clean_token.is_empty() {
                if let Some(s) = status_weak.upgrade() {
                    s.set_text("Пожалуйста, вставьте токен или адресную строку");
                    s.set_visible(true);
                }
                return;
            }

            submit_clone.set_sensitive(false);
            spinner_clone.set_visible(true);
            spinner_clone.start();

            let p = player_clone.clone();
            let d_weak = dialog_weak.clone();
            let s_weak = status_weak.clone();
            let sp_clone = spinner_clone.clone();
            let btn_clone = submit_clone.clone();

            gtk4::glib::spawn_future_local(async move {
                let test_client = YandexMusicClient::new(Some(clean_token.clone()));
                match test_client.get_account_status().await {
                    Ok(acc) => {
                        let login = acc.account.as_ref().and_then(|a| a.login.clone())
                            .or_else(|| acc.account.as_ref().and_then(|a| a.full_name.clone()))
                            .unwrap_or_else(|| "Яндекс Пользователь".to_string());

                        // Сохраняем проверенный токен
                        let _ = save_token(&clean_token);
                        if let Ok(mut c) = p.client.lock() {
                            c.set_token(Some(clean_token));
                        }
                        if let Ok(mut state) = p.state.lock() {
                            state.user_login = Some(login.clone());
                        }

                        p.emit(crate::player::PlayerUiEvent::UserLoggedIn(login.clone()));
                        p.emit(crate::player::PlayerUiEvent::StatusMessage(
                            format!("Добро пожаловать, {}! Запускаем Мою волну 🌊", login)
                        ));

                        // Сразу запускаем реальную Мою волну
                        p.fetch_wave_tracks("onyourwave");

                        if let Some(d) = d_weak.upgrade() {
                            d.close();
                        }
                    }
                    Err(e) => {
                        btn_clone.set_sensitive(true);
                        sp_clone.stop();
                        sp_clone.set_visible(false);
                        if let Some(s) = s_weak.upgrade() {
                            s.set_text(&format!("Неверный токен или ошибка сети: {}", e));
                            s.set_visible(true);
                        }
                    }
                }
            });
        });
    }

    dialog.present(Some(parent));
}
