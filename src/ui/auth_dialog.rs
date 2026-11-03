use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Entry, Label, Orientation, Spinner};
use libadwaita::prelude::*;
use libadwaita::{ActionRow, PreferencesDialog, PreferencesGroup, PreferencesPage};

use crate::api::auth::{
    OAUTH_AUTHORIZE_URL, extract_token_from_input, request_device_code, save_token,
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
    token_entry.set_placeholder_text(Some(
        "y0_AgAAAA... или https://music.yandex.ru/#access_token=...",
    ));
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
        "Вы также можете подтвердить вход по коду устройства на ya.ru/device",
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
                        let login = acc
                            .account
                            .as_ref()
                            .and_then(|a| a.login.clone())
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
                        p.emit(crate::player::PlayerUiEvent::StatusMessage(format!(
                            "Добро пожаловать, {}! Запускаем Волну 🌊",
                            login
                        )));

                        // Сразу запускаем реальную Волну
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

struct VkLoginFlow {
    username: String,
    password: zeroize::Zeroizing<String>,
    validation_sid: Option<String>,
    captcha_sid: Option<String>,
}

/// Direct Auth VK X с переходами «пароль → 2FA/captcha» внутри одного диалога.
pub fn show_vk_auth_dialog(parent: &impl IsA<gtk4::Widget>, player: &PlayerManager) {
    let dialog = PreferencesDialog::new();
    dialog.set_title("Вход во ВКонтакте");

    let page = PreferencesPage::new();
    page.set_title("VK Музыка");
    page.set_icon_name(Some("avatar-default-symbolic"));

    let group = PreferencesGroup::new();
    group.set_title("Аккаунт ВКонтакте");
    group.set_description(Some(
        "Используется вход VK X. Пароль не сохраняется; токен хранится в системном ключевом хранилище.",
    ));

    let login_entry = Entry::new();
    login_entry.set_width_chars(28);
    login_entry.set_hexpand(true);
    login_entry.set_placeholder_text(Some("Телефон или email"));
    let login_row = ActionRow::builder().title("Логин").build();
    login_row.add_suffix(&login_entry);
    group.add(&login_row);

    let password_entry = gtk4::PasswordEntry::new();
    password_entry.set_width_chars(28);
    password_entry.set_hexpand(true);
    password_entry.set_property("placeholder-text", "Пароль");
    let password_row = ActionRow::builder().title("Пароль").build();
    password_row.add_suffix(&password_entry);
    group.add(&password_row);

    let code_entry = Entry::new();
    code_entry.set_width_chars(18);
    code_entry.set_hexpand(true);
    code_entry.set_placeholder_text(Some("Код из SMS или приложения"));
    let code_row = ActionRow::builder()
        .title("Двухфакторная проверка")
        .subtitle("Введите одноразовый код")
        .build();
    code_row.add_suffix(&code_entry);
    code_row.set_visible(false);
    group.add(&code_row);

    let captcha_image = gtk4::Image::new();
    captcha_image.set_size_request(180, 72);
    let captcha_row = ActionRow::builder().title("Капча").build();
    captcha_row.add_suffix(&captcha_image);
    captcha_row.set_visible(false);
    group.add(&captcha_row);

    let captcha_entry = Entry::new();
    captcha_entry.set_width_chars(18);
    captcha_entry.set_hexpand(true);
    captcha_entry.set_placeholder_text(Some("Символы с картинки"));
    let captcha_input_row = ActionRow::builder().title("Проверочный код").build();
    captcha_input_row.add_suffix(&captcha_entry);
    captcha_input_row.set_visible(false);
    group.add(&captcha_input_row);

    page.add(&group);
    dialog.add(&page);

    let status_label = Label::new(None);
    status_label.set_wrap(true);
    status_label.set_halign(Align::Center);
    status_label.set_margin_top(8);
    status_label.set_visible(false);
    status_label.add_css_class("error");
    status_label.set_hexpand(true);

    let submit_btn = Button::with_label("Войти");
    submit_btn.add_css_class("suggested-action");
    let spinner = Spinner::new();
    spinner.set_visible(false);
    let action_box = GtkBox::new(Orientation::Horizontal, 8);
    action_box.set_halign(Align::End);
    action_box.append(&status_label);
    action_box.append(&spinner);
    action_box.append(&submit_btn);
    let action_row = ActionRow::new();
    action_row.add_suffix(&action_box);
    group.add(&action_row);

    let flow: std::rc::Rc<std::cell::RefCell<Option<VkLoginFlow>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));

    submit_btn.connect_clicked({
        let player = player.clone();
        let dialog = dialog.downgrade();
        let status = status_label.downgrade();
        let login_entry = login_entry.clone();
        let password_entry = password_entry.clone();
        let code_entry = code_entry.clone();
        let code_row = code_row.clone();
        let captcha_entry = captcha_entry.clone();
        let captcha_row = captcha_row.clone();
        let captcha_input_row = captcha_input_row.clone();
        let captcha_image = captcha_image.clone();
        let submit_btn = submit_btn.clone();
        move |_| {
            let login = login_entry.text().trim().to_owned();
            let password = password_entry.text().to_string();
            if flow.borrow().is_none() {
                if login.is_empty() || password.is_empty() {
                    if let Some(status) = status.upgrade() {
                        status.set_text("Введите логин и пароль");
                        status.set_visible(true);
                    }
                    return;
                }
                *flow.borrow_mut() = Some(VkLoginFlow {
                    username: login,
                    password: zeroize::Zeroizing::new(password),
                    validation_sid: None,
                    captcha_sid: None,
                });
                password_entry.set_text("");
            }

            let (credentials, challenge) = {
                let flow = flow.borrow();
                let Some(state) = flow.as_ref() else { return };
                (
                    crate::api::vk::auth::VkCredentials {
                        username: state.username.clone(),
                        password: zeroize::Zeroizing::new(state.password.as_str().to_owned()),
                    },
                    crate::api::vk::auth::VkAuthChallengeAnswer {
                        code: Some(code_entry.text().to_string())
                            .filter(|value| !value.trim().is_empty()),
                        validation_sid: state.validation_sid.clone(),
                        captcha_sid: state.captcha_sid.clone(),
                        captcha_key: Some(captcha_entry.text().to_string())
                            .filter(|value| !value.trim().is_empty()),
                    },
                )
            };

            submit_btn.set_sensitive(false);
            spinner.set_visible(true);
            spinner.start();
            if let Some(status) = status.upgrade() {
                status.set_visible(false);
            }

            gtk4::glib::spawn_future_local({
                let player = player.clone();
                let dialog = dialog.clone();
                let status = status.clone();
                let flow = flow.clone();
                let submit_btn = submit_btn.clone();
                let spinner = spinner.clone();
                let code_row = code_row.clone();
                let captcha_row = captcha_row.clone();
                let captcha_input_row = captcha_input_row.clone();
                let captcha_entry = captcha_entry.clone();
                let captcha_image = captcha_image.clone();
                async move {
                    let http = match reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(20))
                        .build()
                    {
                        Ok(http) => http,
                        Err(error) => {
                            if let Some(status) = status.upgrade() {
                                status
                                    .set_text(&format!("Не удалось создать HTTP-клиент: {error}"));
                                status.set_visible(true);
                            }
                            submit_btn.set_sensitive(true);
                            spinner.stop();
                            spinner.set_visible(false);
                            return;
                        }
                    };
                    let outcome =
                        crate::api::vk::auth::direct_auth(&http, &credentials, &challenge).await;
                    submit_btn.set_sensitive(true);
                    spinner.stop();
                    spinner.set_visible(false);

                    match outcome {
                        Ok(crate::api::vk::auth::VkAuthOutcome::Success(success)) => {
                            if let Err(error) =
                                crate::api::vk::auth::save_vk_token(&success.access_token)
                            {
                                if let Some(status) = status.upgrade() {
                                    status
                                        .set_text(&format!("Не удалось сохранить токен: {error}"));
                                    status.set_visible(true);
                                }
                                return;
                            }
                            if let Err(error) = crate::api::auth::save_vk_user_id(success.user_id) {
                                if let Some(status) = status.upgrade() {
                                    status
                                        .set_text(&format!("Не удалось сохранить ID VK: {error}"));
                                    status.set_visible(true);
                                }
                                return;
                            }
                            match crate::api::vk::provider::VkProvider::new(
                                success.access_token,
                                success.user_id,
                            ) {
                                Ok(provider) => {
                                    player.register_vk_provider(provider);
                                    player.switch_provider(crate::api::provider::ProviderId::Vk);
                                    *flow.borrow_mut() = None;
                                    player.emit(crate::player::PlayerUiEvent::StatusMessage(
                                        "ВКонтакте подключён".to_string(),
                                    ));
                                    if let Some(dialog) = dialog.upgrade() {
                                        dialog.close();
                                    }
                                }
                                Err(error) => {
                                    if let Some(status) = status.upgrade() {
                                        status.set_text(&error.to_string());
                                        status.set_visible(true);
                                    }
                                }
                            }
                        }
                        Ok(crate::api::vk::auth::VkAuthOutcome::TwoFactorRequired(required)) => {
                            if let Some(state) = flow.borrow_mut().as_mut() {
                                state.validation_sid = required.validation_sid;
                            }
                            code_row.set_visible(true);
                            submit_btn.set_label("Подтвердить");
                            if let Some(status) = status.upgrade() {
                                status.remove_css_class("error");
                                status.set_text(
                                    required
                                        .error_description
                                        .as_deref()
                                        .unwrap_or("Введите код двухфакторной проверки"),
                                );
                                status.set_visible(true);
                            }
                        }
                        Ok(crate::api::vk::auth::VkAuthOutcome::CaptchaRequired(required)) => {
                            if let Some(state) = flow.borrow_mut().as_mut() {
                                state.captcha_sid = Some(required.captcha_sid);
                            }
                            captcha_row.set_visible(true);
                            captcha_input_row.set_visible(true);
                            captcha_entry.set_text("");
                            submit_btn.set_label("Продолжить");
                            match http.get(&required.captcha_img).send().await {
                                Ok(response) if response.status().is_success() => {
                                    if let Ok(bytes) = response.bytes().await {
                                        let bytes = gtk4::glib::Bytes::from(bytes.as_ref());
                                        if let Ok(texture) = gtk4::gdk::Texture::from_bytes(&bytes)
                                        {
                                            captcha_image.set_paintable(Some(&texture));
                                        }
                                    }
                                }
                                _ => {
                                    if let Some(status) = status.upgrade() {
                                        status.set_text("Не удалось загрузить картинку капчи");
                                        status.set_visible(true);
                                    }
                                }
                            }
                        }
                        Ok(crate::api::vk::auth::VkAuthOutcome::Error(error)) => {
                            if let Some(status) = status.upgrade() {
                                status.add_css_class("error");
                                status.set_text(
                                    error
                                        .error_description
                                        .as_deref()
                                        .unwrap_or("ВКонтакте отклонил вход"),
                                );
                                status.set_visible(true);
                            }
                        }
                        Err(error) => {
                            if let Some(status) = status.upgrade() {
                                status.add_css_class("error");
                                status.set_text(&error.to_string());
                                status.set_visible(true);
                            }
                        }
                    }
                }
            });
        }
    });

    dialog.present(Some(parent));
}

/// Captcha для уже авторизованного `audio.*`-вызова. Ответ пробуждает исходный запрос.
pub fn show_vk_api_captcha_dialog(
    parent: &impl IsA<gtk4::Widget>,
    image_url: Option<String>,
    respond: async_channel::Sender<String>,
) {
    let dialog = PreferencesDialog::new();
    dialog.set_title("Проверка ВКонтакте");
    let page = PreferencesPage::new();
    let group = PreferencesGroup::new();
    group.set_title("Введите капчу");
    group.set_description(Some(
        "VK запросил подтверждение перед продолжением запроса.",
    ));

    let image = gtk4::Image::new();
    image.set_size_request(180, 72);
    let image_row = ActionRow::new();
    image_row.add_suffix(&image);
    group.add(&image_row);

    let entry = Entry::new();
    entry.set_hexpand(true);
    entry.set_placeholder_text(Some("Символы с картинки"));
    let entry_row = ActionRow::builder().title("Проверочный код").build();
    entry_row.add_suffix(&entry);
    group.add(&entry_row);

    let submit = Button::with_label("Продолжить");
    submit.add_css_class("suggested-action");
    let submit_row = ActionRow::new();
    submit_row.add_suffix(&submit);
    group.add(&submit_row);
    page.add(&group);
    dialog.add(&page);

    if let Some(url) = image_url {
        let image = image.clone();
        gtk4::glib::spawn_future_local(async move {
            let Ok(response) = reqwest::Client::new().get(url).send().await else {
                return;
            };
            if !response.status().is_success() {
                return;
            }
            if let Ok(bytes) = response.bytes().await {
                let bytes = gtk4::glib::Bytes::from(bytes.as_ref());
                if let Ok(texture) = gtk4::gdk::Texture::from_bytes(&bytes) {
                    image.set_paintable(Some(&texture));
                }
            }
        });
    }

    submit.connect_clicked({
        let dialog = dialog.clone();
        let entry = entry.clone();
        move |_| {
            let answer = entry.text().trim().to_owned();
            if answer.is_empty() {
                return;
            }
            let _ = respond.try_send(answer);
            dialog.close();
        }
    });
    entry.connect_activate({
        let submit = submit.clone();
        move |_| submit.emit_clicked()
    });

    dialog.present(Some(parent));
}
