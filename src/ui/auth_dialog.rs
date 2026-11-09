use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Entry, Image, Label, Orientation, Spinner};
use libadwaita::prelude::*;
use libadwaita::{ActionRow, PreferencesDialog, PreferencesGroup, PreferencesPage};

use crate::api::auth::{
    extract_token_from_input, request_device_code, save_token, OAUTH_AUTHORIZE_URL,
};
use crate::api::client::YandexMusicClient;
use crate::player::PlayerManager;
use crate::ui::preferences_dialog::apply_dialog_theme;

/// Отображает диалог входа в аккаунт Яндекс Музыки
pub fn show_auth_dialog(parent: &impl IsA<gtk4::Widget>, player: &PlayerManager) {
    let dialog = PreferencesDialog::new();
    dialog.set_title("Вход в Яндекс Музыку");
    apply_dialog_theme(&dialog);

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
                            format!("Добро пожаловать, {}! Запускаем Волну 🌊", login)
                        ));

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

struct VkDirectAuthState {
    username: String,
    password: zeroize::Zeroizing<String>,
    validation_sid: Option<String>,
    captcha_sid: Option<String>,
}

/// Кнопка «копировать диагностику»: текст ошибки с экрана + ответы VK.
fn trace_copy_button(status: &Label) -> Button {
    let button = Button::from_icon_name("edit-copy-symbolic");
    button.add_css_class("flat");
    button.set_valign(gtk4::Align::Center);
    button.set_tooltip_text(Some("Копировать текст ошибки и ответы VK"));
    let status = status.clone();
    button.connect_clicked(move |_| {
        let mut text = String::new();
        let visible = status.text().to_string();
        if !visible.trim().is_empty() {
            text.push_str("[UI] ");
            text.push_str(visible.trim());
            text.push('\n');
        }
        let trace = crate::api::vk::auth::diagnostics();
        if trace.trim().is_empty() {
            text.push_str("VK: ответов пока нет — сделайте попытку входа и нажмите копирование снова");
        } else {
            text.push_str(&trace);
        }
        eprintln!("[vk] диагностика скопирована, {} символов", text.len());
        if let Some(display) = gtk4::gdk::Display::default() {
            display.clipboard().set_text(&text);
        }
    });
    button
}

fn apply_vk_auth_success(
    success: crate::api::vk::models::VkAuthSuccessDto,
    player: &PlayerManager,
    dialog: Option<&PreferencesDialog>,
    status_label: &Label,
) -> bool {
    if let Err(error) = crate::api::vk::auth::save_vk_token(&success.access_token) {
        status_label.set_text(&format!("Не удалось сохранить токен: {error}"));
        status_label.set_visible(true);
        return false;
    }
    if let Err(error) = crate::api::auth::save_vk_user_id(success.user_id) {
        status_label.set_text(&format!("Не удалось сохранить ID VK: {error}"));
        status_label.set_visible(true);
        return false;
    }

    match crate::api::vk::provider::VkProvider::new(success.access_token, success.user_id) {
        Ok(provider) => {
            player.register_vk_provider(provider);
            player.switch_provider(crate::api::provider::ProviderId::Vk);
            player.emit(crate::player::PlayerUiEvent::StatusMessage(
                "ВКонтакте подключён".to_string(),
            ));
            if let Some(dlg) = dialog {
                dlg.close();
            }
            true
        }
        Err(error) => {
            status_label.set_text(&error.to_string());
            status_label.set_visible(true);
            false
        }
    }
}

/// Прямая авторизация ВКонтакте (Direct Auth VK X / VK Android).
pub fn show_vk_auth_dialog(parent: &impl IsA<gtk4::Widget>, player: &PlayerManager) {
    let dialog = PreferencesDialog::new();
    dialog.set_title("Вход во ВКонтакте");
    apply_dialog_theme(&dialog);

    let page = PreferencesPage::new();
    page.set_title("VK Музыка");
    page.set_icon_name(Some("avatar-default-symbolic"));

    // --- Экран ввода логина и пароля ---
    let login_group = PreferencesGroup::new();
    login_group.set_title("Войти во ВКонтакте");
    login_group.set_description(Some("Используя ваш аккаунт VK ID или номер телефона"));

    let login_entry = Entry::new();
    login_entry.set_width_chars(28);
    login_entry.set_hexpand(true);
    login_entry.set_placeholder_text(Some("Номер телефона или почта"));
    let login_row = ActionRow::builder().title("Логин").build();
    login_row.add_suffix(&login_entry);
    login_group.add(&login_row);

    // Код с почты (VK ID): появляется только если сервер попросил email-подтверждение
    let otp_status = Label::new(None);
    otp_status.set_selectable(true);
    otp_status.set_can_focus(true);
    otp_status.set_wrap(true);
    otp_status.set_xalign(0.0);
    otp_status.add_css_class("dim-label");
    otp_status.set_visible(false);
    login_group.add(&otp_status);

    let otp_code_entry = Entry::new();
    otp_code_entry.set_width_chars(8);
    otp_code_entry.set_max_length(6);
    otp_code_entry.set_placeholder_text(Some("123456"));
    let otp_confirm_btn = Button::with_label("Подтвердить");
    otp_confirm_btn.add_css_class("suggested-action");
    otp_confirm_btn.set_valign(gtk4::Align::Center);
    let otp_row = ActionRow::builder().title("Код из письма").build();
    otp_row.add_suffix(&otp_code_entry);
    otp_row.add_suffix(&otp_confirm_btn);
    otp_row.add_suffix(&trace_copy_button(&otp_status));
    otp_row.set_visible(false);
    login_group.add(&otp_row);

    // Капча VK: показывается, когда сервер просит подтвердить, что мы не робот
    let otp_captcha_image = Image::new();
    otp_captcha_image.set_pixel_size(208);
    let otp_captcha_entry = Entry::new();
    otp_captcha_entry.set_width_chars(10);
    otp_captcha_entry.set_placeholder_text(Some("текст с картинки"));
    let otp_captcha_row = ActionRow::builder().title("Код с картинки").build();
    otp_captcha_row.add_prefix(&otp_captcha_image);
    otp_captcha_row.add_suffix(&otp_captcha_entry);
    otp_captcha_row.set_visible(false);
    login_group.add(&otp_captcha_row);
    page.add(&login_group);

    let password_group = PreferencesGroup::new();
    password_group.set_title("Пароль");

    let password_entry = gtk4::PasswordEntry::new();
    password_entry.set_width_chars(28);
    password_entry.set_hexpand(true);
    password_entry.set_property("placeholder-text", "Пароль");
    password_entry.set_show_peek_icon(true);
    let password_row = ActionRow::builder().title("Пароль").build();
    password_row.add_prefix(&Image::from_icon_name("dialog-password-symbolic"));
    password_row.add_suffix(&password_entry);
    password_group.add(&password_row);

    let security_row = ActionRow::builder()
        .title("Ваш аккаунт в безопасности")
        .subtitle("Волна отправляет данные напрямую на серверы VK и сохраняет полученный токен в защищённом системном хранилище.")
        .build();
    security_row.add_prefix(&Image::from_icon_name("channel-secure-symbolic"));
    password_group.add(&security_row);

    let login_status = Label::new(None);
    login_status.set_selectable(true);
    login_status.set_can_focus(true);
    login_status.set_wrap(true);
    login_status.set_halign(Align::Center);
    login_status.set_visible(false);
    login_status.add_css_class("error");

    let login_spinner = Spinner::new();
    login_spinner.set_visible(false);

    let to_browser_btn = Button::with_label("Войти через браузер");
    to_browser_btn.add_css_class("flat");

    let submit_login_btn = Button::with_label("Войти");
    submit_login_btn.add_css_class("suggested-action");

    // Без пароля и звонков: VK присылает код на привязанную почту
    let email_code_btn = Button::with_label("Код на почту");
    email_code_btn.add_css_class("flat");

    let password_btn_box = GtkBox::new(Orientation::Horizontal, 8);
    password_btn_box.set_halign(Align::End);
    password_btn_box.append(&login_status);
    password_btn_box.append(&login_spinner);
    // Диагностика: копирует последние ответы VK, чтобы не перепечатывать ошибки
    let copy_trace_btn = trace_copy_button(&login_status);

    password_btn_box.append(&to_browser_btn);
    password_btn_box.append(&copy_trace_btn);
    password_btn_box.append(&email_code_btn);
    password_btn_box.append(&submit_login_btn);

    let password_action_row = ActionRow::new();
    password_action_row.add_suffix(&password_btn_box);
    password_group.add(&password_action_row);
    page.add(&password_group);

    // --- Шаг двухфакторной аутентификации (2FA / Код подтверждения) ---
    let code_group = PreferencesGroup::new();
    code_group.set_title("Подтверждение входа");
    code_group.set_visible(false);

    let code_entry = Entry::new();
    code_entry.set_width_chars(20);
    code_entry.set_hexpand(true);
    code_entry.set_placeholder_text(Some("Код подтверждения"));
    let code_row = ActionRow::builder().title("Код подтверждения").build();
    code_row.add_prefix(&Image::from_icon_name("mail-unread-symbolic"));
    code_row.add_suffix(&code_entry);
    code_group.add(&code_row);

    let code_status = Label::new(None);
    code_status.set_selectable(true);
    code_status.set_can_focus(true);
    code_status.set_wrap(true);
    code_status.set_halign(Align::Center);
    code_status.set_visible(false);
    code_status.add_css_class("error");
    code_row.add_suffix(&trace_copy_button(&code_status));

    let back_to_login_from_code_btn = Button::with_label("← Назад");
    let resend_email_btn = Button::with_label("Отправить код на почту");
    resend_email_btn.add_css_class("flat");
    resend_email_btn.set_visible(false);
    let submit_code_btn = Button::with_label("Подтвердить");
    submit_code_btn.add_css_class("suggested-action");

    let code_spinner = Spinner::new();
    code_spinner.set_visible(false);

    let code_btn_box = GtkBox::new(Orientation::Horizontal, 8);
    code_btn_box.set_halign(Align::End);
    code_btn_box.append(&code_status);
    code_btn_box.append(&code_spinner);
    code_btn_box.append(&back_to_login_from_code_btn);
    code_btn_box.append(&resend_email_btn);
    code_btn_box.append(&submit_code_btn);

    let code_action_row = ActionRow::new();
    code_action_row.add_suffix(&code_btn_box);
    code_group.add(&code_action_row);
    page.add(&code_group);

    // --- Шаг капчи ---
    let captcha_group = PreferencesGroup::new();
    captcha_group.set_title("Введите проверочный код");
    captcha_group.set_description(Some("Символы с картинки для подтверждения безопасности"));
    captcha_group.set_visible(false);

    let captcha_image = gtk4::Image::new();
    captcha_image.set_size_request(180, 72);
    let captcha_img_row = ActionRow::builder().title("Картинка").build();
    captcha_img_row.add_suffix(&captcha_image);
    captcha_group.add(&captcha_img_row);

    let captcha_entry = Entry::new();
    captcha_entry.set_width_chars(18);
    captcha_entry.set_hexpand(true);
    captcha_entry.set_placeholder_text(Some("Символы с картинки"));
    let captcha_input_row = ActionRow::builder().title("Код с картинки").build();
    captcha_input_row.add_suffix(&captcha_entry);
    captcha_group.add(&captcha_input_row);

    let captcha_status = Label::new(None);
    captcha_status.set_selectable(true);
    captcha_status.set_can_focus(true);
    captcha_status.set_wrap(true);
    captcha_status.set_halign(Align::Center);
    captcha_status.set_visible(false);
    captcha_status.add_css_class("error");

    let back_to_login_from_captcha_btn = Button::with_label("← Назад");
    let submit_captcha_btn = Button::with_label("Подтвердить");
    submit_captcha_btn.add_css_class("suggested-action");

    let captcha_spinner = Spinner::new();
    captcha_spinner.set_visible(false);

    let captcha_btn_box = GtkBox::new(Orientation::Horizontal, 8);
    captcha_btn_box.set_halign(Align::End);
    captcha_btn_box.append(&captcha_status);
    captcha_btn_box.append(&captcha_spinner);
    captcha_btn_box.append(&back_to_login_from_captcha_btn);
    captcha_btn_box.append(&submit_captcha_btn);

    let captcha_action_row = ActionRow::new();
    captcha_action_row.add_suffix(&captcha_btn_box);
    captcha_group.add(&captcha_action_row);
    page.add(&captcha_group);

    // --- Шаг резервного входа через браузер (VK ID) ---
    let browser_group = PreferencesGroup::new();
    browser_group.set_title("Вход через браузер (VK ID)");
    browser_group.set_description(Some(
        "Нажмите кнопку «1. Открыть страницу входа VK», авторизуйтесь на официальном сайте VK ID в браузере, затем скопируйте адресную строку браузера (или токен) и вставьте её в поле ниже.",
    ));
    browser_group.set_visible(false);

    let open_browser_btn = Button::with_label("1. Открыть страницу входа VK");
    open_browser_btn.add_css_class("suggested-action");
    open_browser_btn.connect_clicked(|_| {
        let _ = gtk4::gio::AppInfo::launch_default_for_uri(
            "https://oauth.vk.com/authorize?client_id=2685278&scope=audio,offline&redirect_uri=https://oauth.vk.com/blank.html&display=page&response_type=token",
            None::<&gtk4::gio::AppLaunchContext>,
        );
    });

    let open_browser_row = ActionRow::builder()
        .title("Официальная страница авторизации")
        .subtitle("Откроется ваш браузер по умолчанию с официальной защищённой формой VK ID")
        .build();
    open_browser_row.add_prefix(&Image::from_icon_name("web-browser-symbolic"));
    open_browser_row.add_suffix(&open_browser_btn);
    browser_group.add(&open_browser_row);

    let token_entry = Entry::new();
    token_entry.set_hexpand(true);
    token_entry.set_placeholder_text(Some("Вставьте ссылку из браузера (blank.html#access_token=...) или токен"));
    let token_row = ActionRow::builder().title("Ссылка или токен").build();
    token_row.add_prefix(&Image::from_icon_name("insert-link-symbolic"));
    token_row.add_suffix(&token_entry);
    browser_group.add(&token_row);

    let browser_status = Label::new(None);
    browser_status.set_selectable(true);
    browser_status.set_can_focus(true);
    browser_status.set_wrap(true);
    browser_status.set_halign(Align::Center);
    browser_status.set_visible(false);
    browser_status.add_css_class("error");

    let browser_spinner = Spinner::new();
    browser_spinner.set_visible(false);

    let back_from_browser_btn = Button::with_label("← Назад");
    let submit_token_btn = Button::with_label("2. Подключить  →");
    submit_token_btn.add_css_class("suggested-action");

    let browser_btn_box = GtkBox::new(Orientation::Horizontal, 8);
    browser_btn_box.set_halign(Align::End);
    browser_btn_box.append(&browser_status);
    browser_btn_box.append(&browser_spinner);
    browser_btn_box.append(&back_from_browser_btn);
    browser_btn_box.append(&submit_token_btn);

    let browser_action_row = ActionRow::new();
    browser_action_row.add_suffix(&browser_btn_box);
    browser_group.add(&browser_action_row);
    page.add(&browser_group);

    dialog.add(&page);

    // --- Состояние авторизации ---
    let state: std::rc::Rc<std::cell::RefCell<Option<VkDirectAuthState>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    // sid сессии после подтверждения кода: по нему VK X просит пароль
    let otp_sid: std::rc::Rc<std::cell::RefCell<Option<String>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    // Код из письма тоже уходит в финальный запрос пароля (как в VK X)
    let otp_code: std::rc::Rc<std::cell::RefCell<Option<String>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));

    let fetch_captcha_image = {
        let captcha_image = captcha_image.clone();
        let captcha_status = captcha_status.clone();
        move |img_url: String| {
            let image = captcha_image.clone();
            let status = captcha_status.clone();
            gtk4::glib::spawn_future_local(async move {
                let client = match reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(15))
                    .build()
                {
                    Ok(c) => c,
                    Err(e) => {
                        status.set_text(&format!("Ошибка сети: {e}"));
                        status.set_visible(true);
                        return;
                    }
                };
                match client.get(&img_url).send().await {
                    Ok(resp) if resp.status().is_success() => {
                        if let Ok(bytes) = resp.bytes().await {
                            let bytes = gtk4::glib::Bytes::from(bytes.as_ref());
                            if let Ok(texture) = gtk4::gdk::Texture::from_bytes(&bytes) {
                                image.set_paintable(Some(&texture));
                            }
                        }
                    }
                    _ => {
                        status.set_text("Не удалось загрузить капчу");
                        status.set_visible(true);
                    }
                }
            });
        }
    };

    // Запуск первичного входа (логин + пароль)
    let start_direct_auth: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
        let player = player.clone();
        let dialog = dialog.downgrade();
        let state = state.clone();
        let otp_sid = otp_sid.clone();
        let otp_code = otp_code.clone();
        let login_entry = login_entry.clone();
        let password_entry = password_entry.clone();
        let login_status = login_status.clone();
        let login_spinner = login_spinner.clone();
        let submit_login_btn = submit_login_btn.clone();
        let login_group = login_group.clone();
        let password_group = password_group.clone();
        let code_group = code_group.clone();
        let code_entry = code_entry.clone();
        let code_status = code_status.clone();
        let resend_email_btn = resend_email_btn.clone();
        let captcha_group = captcha_group.clone();
        let captcha_entry = captcha_entry.clone();
        let captcha_status = captcha_status.clone();
        let fetch_captcha_image = fetch_captcha_image.clone();

        move || {
            let username = login_entry.text().trim().to_string();
            if username.is_empty() {
                login_status.set_text("Введите номер телефона или почту");
                login_status.set_visible(true);
                login_entry.grab_focus();
                return;
            }

            let password = password_entry.text().to_string();
            if password.is_empty() {
                login_status.set_text("Введите пароль");
                login_status.set_visible(true);
                password_entry.grab_focus();
                return;
            }

            login_status.set_visible(false);
            submit_login_btn.set_sensitive(false);
            login_spinner.set_visible(true);
            login_spinner.start();

            *state.borrow_mut() = Some(VkDirectAuthState {
                username: username.clone(),
                password: zeroize::Zeroizing::new(password.clone()),
                validation_sid: None,
                captcha_sid: None,
            });

            let player = player.clone();
            let dialog = dialog.clone();
            let state = state.clone();
            let login_status = login_status.clone();
            let login_spinner = login_spinner.clone();
            let submit_login_btn = submit_login_btn.clone();
            let login_group = login_group.clone();
            let password_group = password_group.clone();
            let code_group = code_group.clone();
            let code_entry = code_entry.clone();
            let code_status = code_status.clone();
            let resend_email_btn = resend_email_btn.clone();
            let captcha_group = captcha_group.clone();
            let captcha_entry = captcha_entry.clone();
            let captcha_status = captcha_status.clone();
            let fetch_captcha = fetch_captcha_image.clone();

            let otp_sid = otp_sid.clone();
            let otp_code = otp_code.clone();
            gtk4::glib::spawn_future_local(async move {
                let http = match reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(20))
                    .build()
                {
                    Ok(c) => c,
                    Err(e) => {
                        login_spinner.stop();
                        login_spinner.set_visible(false);
                        submit_login_btn.set_sensitive(true);
                        login_status.set_text(&format!("Ошибка сети: {e}"));
                        login_status.set_visible(true);
                        return;
                    }
                };

                let credentials = crate::api::vk::auth::VkCredentials::new(username, password);
                // После подтверждения кодом VK ждёт сессию в параметре `sid`,
                // а не `validation_sid` (тот формат — для звонка/2FA).
                let challenge = crate::api::vk::auth::VkAuthChallengeAnswer {
                    login_sid: otp_sid.borrow().clone(),
                    code: otp_code.borrow().clone(),
                    ..Default::default()
                };
                let outcome = crate::api::vk::auth::direct_auth(&http, &credentials, &challenge).await;

                login_spinner.stop();
                login_spinner.set_visible(false);
                submit_login_btn.set_sensitive(true);

                match outcome {
                    Ok(crate::api::vk::auth::VkAuthOutcome::Success(success)) => {
                        let _ = apply_vk_auth_success(
                            success,
                            &player,
                            dialog.upgrade().as_ref(),
                            &login_status,
                        );
                    }
                    Ok(crate::api::vk::auth::VkAuthOutcome::TwoFactorRequired(required)) => {
                        let (title, sub, placeholder) = required.ui_hints();
                        code_group.set_title(title);
                        code_group.set_description(Some(&sub));
                        code_entry.set_placeholder_text(Some(placeholder));
                        code_entry.set_text("");
                        code_status.set_visible(false);

                        let should_show_resend = required.validation_resend.as_deref() == Some("2fa_email")
                            || required.validation_type.contains("callreset");
                        resend_email_btn.set_visible(should_show_resend);
                        resend_email_btn.set_sensitive(true);
                        resend_email_btn.set_label("Отправить код на почту");

                        if let Some(st) = state.borrow_mut().as_mut() {
                            st.validation_sid = required.validation_sid;
                        }

                        login_group.set_visible(false);
                        password_group.set_visible(false);
                        captcha_group.set_visible(false);
                        code_group.set_visible(true);
                        code_entry.grab_focus();
                    }
                    Ok(crate::api::vk::auth::VkAuthOutcome::CaptchaRequired(required)) => {
                        if let Some(st) = state.borrow_mut().as_mut() {
                            st.captcha_sid = Some(required.captcha_sid.clone());
                        }

                        // VK отключил картинку-капчу для сторонних клиентов:
                        // captcha.php отдаёт заглушку. Показываем объяснение.
                        let _ = &required;
                        login_status.set_text(
                            "VK требует капчу, но картинка для сторонних клиентов отключена. Используйте пароль для приложения (ВК → Настройки → Безопасность → Пароли для приложений) или вход через браузер",
                        );
                        login_status.set_visible(true);
                    }
                    Ok(crate::api::vk::auth::VkAuthOutcome::Error(err)) => {
                        let msg = err
                            .error_description
                            .as_deref()
                            .unwrap_or("Неверный логин или пароль");
                        login_status.set_text(msg);
                        login_status.set_visible(true);
                    }
                    Err(e) => {
                        login_status.set_text(&e.to_string());
                        login_status.set_visible(true);
                    }
                }
            });
        }
    });

    // Подтверждение 2FA кода
    let submit_2fa_code = {
        let player = player.clone();
        let dialog = dialog.downgrade();
        let state = state.clone();
        let code_entry = code_entry.clone();
        let code_status = code_status.clone();
        let code_spinner = code_spinner.clone();
        let submit_code_btn = submit_code_btn.clone();
        let resend_email_btn = resend_email_btn.clone();
        let code_group = code_group.clone();
        let captcha_group = captcha_group.clone();
        let captcha_entry = captcha_entry.clone();
        let captcha_status = captcha_status.clone();
        let fetch_captcha_image = fetch_captcha_image.clone();

        move || {
            let code = code_entry.text().trim().to_string();
            if code.is_empty() {
                code_status.set_text("Введите код подтверждения");
                code_status.set_visible(true);
                return;
            }

            let (username, password, validation_sid, captcha_sid) = {
                let borrow = state.borrow();
                let Some(st) = borrow.as_ref() else {
                    code_status.set_text("Сессия устарела. Вернитесь к логину");
                    code_status.set_visible(true);
                    return;
                };
                (
                    st.username.clone(),
                    st.password.clone(),
                    st.validation_sid.clone(),
                    st.captcha_sid.clone(),
                )
            };

            code_status.set_visible(false);
            submit_code_btn.set_sensitive(false);
            resend_email_btn.set_sensitive(false);
            code_spinner.set_visible(true);
            code_spinner.start();

            let player = player.clone();
            let dialog = dialog.clone();
            let state = state.clone();
            let code_entry = code_entry.clone();
            let code_status = code_status.clone();
            let code_spinner = code_spinner.clone();
            let submit_code_btn = submit_code_btn.clone();
            let resend_email_btn = resend_email_btn.clone();
            let code_group = code_group.clone();
            let captcha_group = captcha_group.clone();
            let captcha_entry = captcha_entry.clone();
            let captcha_status = captcha_status.clone();
            let fetch_captcha = fetch_captcha_image.clone();

            gtk4::glib::spawn_future_local(async move {
                let http = match reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(20))
                    .build()
                {
                    Ok(c) => c,
                    Err(e) => {
                        code_spinner.stop();
                        code_spinner.set_visible(false);
                        submit_code_btn.set_sensitive(true);
                        resend_email_btn.set_sensitive(true);
                        code_status.set_text(&format!("Ошибка сети: {e}"));
                        code_status.set_visible(true);
                        return;
                    }
                };

                let credentials = crate::api::vk::auth::VkCredentials {
                    username,
                    password,
                };
                let challenge = crate::api::vk::auth::VkAuthChallengeAnswer {
                    code: Some(code),
                    validation_sid,
                    login_sid: None,
                    captcha_sid,
                    captcha_key: None,
                    resend: false,
                };
                let outcome = crate::api::vk::auth::direct_auth(&http, &credentials, &challenge).await;

                code_spinner.stop();
                code_spinner.set_visible(false);
                submit_code_btn.set_sensitive(true);
                resend_email_btn.set_sensitive(true);

                match outcome {
                    Ok(crate::api::vk::auth::VkAuthOutcome::Success(success)) => {
                        let _ = apply_vk_auth_success(
                            success,
                            &player,
                            dialog.upgrade().as_ref(),
                            &code_status,
                        );
                    }
                    Ok(crate::api::vk::auth::VkAuthOutcome::TwoFactorRequired(required)) => {
                        let (title, sub, placeholder) = required.ui_hints();
                        code_group.set_title(title);
                        code_group.set_description(Some(&sub));
                        code_entry.set_placeholder_text(Some(placeholder));
                        code_status.set_text("Неверный код. Попробуйте снова");
                        code_status.set_visible(true);
                        code_entry.set_text("");
                        code_entry.grab_focus();

                        let should_show_resend = required.validation_resend.as_deref() == Some("2fa_email")
                            || required.validation_type.contains("callreset");
                        resend_email_btn.set_visible(should_show_resend);

                        if let Some(st) = state.borrow_mut().as_mut() {
                            st.validation_sid = required.validation_sid;
                        }
                    }
                    Ok(crate::api::vk::auth::VkAuthOutcome::CaptchaRequired(required)) => {
                        if let Some(st) = state.borrow_mut().as_mut() {
                            st.captcha_sid = Some(required.captcha_sid.clone());
                        }

                        captcha_status.set_visible(false);
                        captcha_entry.set_text("");
                        code_group.set_visible(false);
                        captcha_group.set_visible(true);
                        captcha_entry.grab_focus();
                        fetch_captcha(required.captcha_img);
                    }
                    Ok(crate::api::vk::auth::VkAuthOutcome::Error(err)) => {
                        let msg = err
                            .error_description
                            .as_deref()
                            .unwrap_or("Неверный код подтверждения");
                        code_status.set_text(msg);
                        code_status.set_visible(true);
                    }
                    Err(e) => {
                        code_status.set_text(&e.to_string());
                        code_status.set_visible(true);
                    }
                }
            });
        }
    };

    // Запрос отправки кода на почту (resend 2FA)
    let request_resend_email = {
        let player = player.clone();
        let dialog = dialog.downgrade();
        let state = state.clone();
        let code_entry = code_entry.clone();
        let code_status = code_status.clone();
        let code_spinner = code_spinner.clone();
        let submit_code_btn = submit_code_btn.clone();
        let resend_email_btn = resend_email_btn.clone();
        let code_group = code_group.clone();
        let captcha_group = captcha_group.clone();
        let captcha_entry = captcha_entry.clone();
        let captcha_status = captcha_status.clone();
        let fetch_captcha_image = fetch_captcha_image.clone();

        move || {
            let (username, password, validation_sid) = {
                let borrow = state.borrow();
                let Some(st) = borrow.as_ref() else {
                    code_status.set_text("Сессия устарела. Вернитесь к логину");
                    code_status.set_visible(true);
                    return;
                };
                (
                    st.username.clone(),
                    st.password.clone(),
                    st.validation_sid.clone(),
                )
            };

            code_status.set_visible(false);
            resend_email_btn.set_sensitive(false);
            submit_code_btn.set_sensitive(false);
            code_spinner.set_visible(true);
            code_spinner.start();

            let player = player.clone();
            let dialog = dialog.clone();
            let state = state.clone();
            let code_entry = code_entry.clone();
            let code_status = code_status.clone();
            let code_spinner = code_spinner.clone();
            let submit_code_btn = submit_code_btn.clone();
            let resend_email_btn = resend_email_btn.clone();
            let code_group = code_group.clone();
            let captcha_group = captcha_group.clone();
            let captcha_entry = captcha_entry.clone();
            let captcha_status = captcha_status.clone();
            let fetch_captcha = fetch_captcha_image.clone();

            gtk4::glib::spawn_future_local(async move {
                let http = match reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(20))
                    .build()
                {
                    Ok(c) => c,
                    Err(e) => {
                        code_spinner.stop();
                        code_spinner.set_visible(false);
                        resend_email_btn.set_sensitive(true);
                        submit_code_btn.set_sensitive(true);
                        code_status.set_text(&format!("Ошибка сети: {e}"));
                        code_status.set_visible(true);
                        return;
                    }
                };

                let credentials = crate::api::vk::auth::VkCredentials {
                    username,
                    password,
                };
                let challenge = crate::api::vk::auth::VkAuthChallengeAnswer {
                    code: None,
                    validation_sid,
                    login_sid: None,
                    captcha_sid: None,
                    captcha_key: None,
                    resend: true,
                };
                let outcome = crate::api::vk::auth::direct_auth(&http, &credentials, &challenge).await;

                code_spinner.stop();
                code_spinner.set_visible(false);
                submit_code_btn.set_sensitive(true);
                resend_email_btn.set_sensitive(true);

                match outcome {
                    Ok(crate::api::vk::auth::VkAuthOutcome::Success(success)) => {
                        let _ = apply_vk_auth_success(
                            success,
                            &player,
                            dialog.upgrade().as_ref(),
                            &code_status,
                        );
                    }
                    Ok(crate::api::vk::auth::VkAuthOutcome::TwoFactorRequired(required)) => {
                        let (title, sub, placeholder) = required.ui_hints();
                        code_group.set_title(title);
                        code_group.set_description(Some(&sub));
                        code_entry.set_placeholder_text(Some(placeholder));
                        code_entry.set_text("");
                        code_status.set_visible(false);

                        if let Some(st) = state.borrow_mut().as_mut() {
                            st.validation_sid = required.validation_sid;
                        }

                        resend_email_btn.set_visible(false);
                        code_entry.grab_focus();
                    }
                    Ok(crate::api::vk::auth::VkAuthOutcome::CaptchaRequired(required)) => {
                        if let Some(st) = state.borrow_mut().as_mut() {
                            st.captcha_sid = Some(required.captcha_sid.clone());
                        }

                        captcha_status.set_visible(false);
                        captcha_entry.set_text("");
                        code_group.set_visible(false);
                        captcha_group.set_visible(true);
                        captcha_entry.grab_focus();
                        fetch_captcha(required.captcha_img);
                    }
                    Ok(crate::api::vk::auth::VkAuthOutcome::Error(err)) => {
                        let msg = err
                            .error_description
                            .as_deref()
                            .unwrap_or("Не удалось отправить код на почту");
                        code_status.set_text(msg);
                        code_status.set_visible(true);
                    }
                    Err(e) => {
                        code_status.set_text(&e.to_string());
                        code_status.set_visible(true);
                    }
                }
            });
        }
    };

    // Подтверждение капчи
    let submit_captcha = {
        let player = player.clone();
        let dialog = dialog.downgrade();
        let state = state.clone();
        let code_entry = code_entry.clone();
        let code_group = code_group.clone();
        let _code_status = code_status.clone();
        let resend_email_btn = resend_email_btn.clone();
        let captcha_entry = captcha_entry.clone();
        let captcha_status = captcha_status.clone();
        let captcha_spinner = captcha_spinner.clone();
        let submit_captcha_btn = submit_captcha_btn.clone();
        let captcha_group = captcha_group.clone();
        let fetch_captcha_image = fetch_captcha_image.clone();

        move || {
            let key = captcha_entry.text().trim().to_string();
            if key.is_empty() {
                captcha_status.set_text("Введите символы с картинки");
                captcha_status.set_visible(true);
                return;
            }

            let (username, password, validation_sid, captcha_sid) = {
                let borrow = state.borrow();
                let Some(st) = borrow.as_ref() else {
                    captcha_status.set_text("Сессия устарела. Вернитесь к логину");
                    captcha_status.set_visible(true);
                    return;
                };
                (
                    st.username.clone(),
                    st.password.clone(),
                    st.validation_sid.clone(),
                    st.captcha_sid.clone(),
                )
            };

            let code = code_entry.text().trim().to_string();
            let code_opt = if code.is_empty() { None } else { Some(code) };

            captcha_status.set_visible(false);
            submit_captcha_btn.set_sensitive(false);
            captcha_spinner.set_visible(true);
            captcha_spinner.start();

            let player = player.clone();
            let dialog = dialog.clone();
            let state = state.clone();
            let code_group = code_group.clone();
            let code_entry = code_entry.clone();
            let resend_email_btn = resend_email_btn.clone();
            let captcha_entry = captcha_entry.clone();
            let captcha_status = captcha_status.clone();
            let captcha_spinner = captcha_spinner.clone();
            let submit_captcha_btn = submit_captcha_btn.clone();
            let captcha_group = captcha_group.clone();
            let fetch_captcha = fetch_captcha_image.clone();

            gtk4::glib::spawn_future_local(async move {
                let http = match reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(20))
                    .build()
                {
                    Ok(c) => c,
                    Err(e) => {
                        captcha_spinner.stop();
                        captcha_spinner.set_visible(false);
                        submit_captcha_btn.set_sensitive(true);
                        captcha_status.set_text(&format!("Ошибка сети: {e}"));
                        captcha_status.set_visible(true);
                        return;
                    }
                };

                let credentials = crate::api::vk::auth::VkCredentials {
                    username,
                    password,
                };
                let challenge = crate::api::vk::auth::VkAuthChallengeAnswer {
                    code: code_opt,
                    validation_sid,
                    login_sid: None,
                    captcha_sid,
                    captcha_key: Some(key),
                    resend: false,
                };
                let outcome = crate::api::vk::auth::direct_auth(&http, &credentials, &challenge).await;

                captcha_spinner.stop();
                captcha_spinner.set_visible(false);
                submit_captcha_btn.set_sensitive(true);

                match outcome {
                    Ok(crate::api::vk::auth::VkAuthOutcome::Success(success)) => {
                        let _ = apply_vk_auth_success(
                            success,
                            &player,
                            dialog.upgrade().as_ref(),
                            &captcha_status,
                        );
                    }
                    Ok(crate::api::vk::auth::VkAuthOutcome::TwoFactorRequired(required)) => {
                        let (title, sub, placeholder) = required.ui_hints();
                        code_group.set_title(title);
                        code_group.set_description(Some(&sub));
                        code_entry.set_placeholder_text(Some(placeholder));
                        code_entry.set_text("");

                        let should_show_resend = required.validation_resend.as_deref() == Some("2fa_email")
                            || required.validation_type.contains("callreset");
                        resend_email_btn.set_visible(should_show_resend);
                        resend_email_btn.set_sensitive(true);
                        resend_email_btn.set_label("Отправить код на почту");

                        if let Some(st) = state.borrow_mut().as_mut() {
                            st.validation_sid = required.validation_sid;
                            st.captcha_sid = None;
                        }

                        captcha_group.set_visible(false);
                        code_group.set_visible(true);
                        code_entry.grab_focus();
                    }
                    Ok(crate::api::vk::auth::VkAuthOutcome::CaptchaRequired(required)) => {
                        if let Some(st) = state.borrow_mut().as_mut() {
                            st.captcha_sid = Some(required.captcha_sid.clone());
                        }

                        captcha_status.set_text("Неверный код с картинки. Попробуйте снова");
                        captcha_status.set_visible(true);
                        captcha_entry.set_text("");
                        captcha_entry.grab_focus();
                        fetch_captcha(required.captcha_img);
                    }
                    Ok(crate::api::vk::auth::VkAuthOutcome::Error(err)) => {
                        let msg = err
                            .error_description
                            .as_deref()
                            .unwrap_or("Ошибка капчи");
                        captcha_status.set_text(msg);
                        captcha_status.set_visible(true);
                    }
                    Err(e) => {
                        captcha_status.set_text(&e.to_string());
                        captcha_status.set_visible(true);
                    }
                }
            });
        }
    };

    {
        let state: std::rc::Rc<std::cell::RefCell<Option<(String, Option<String>)>>> =
            std::rc::Rc::default();
        let (player_c, dialog_weak, status, code_row, btn, otp_sid_confirm) = (
            player.clone(),
            dialog.downgrade(),
            otp_status.clone(),
            otp_row.clone(),
            email_code_btn.clone(),
            otp_sid.clone(),
        );
        let state_req = state.clone();
        let login_for_req = login_entry.clone();
        let code_row_confirm = otp_row.clone();
        let captcha_state: std::rc::Rc<std::cell::RefCell<Option<String>>> =
            std::rc::Rc::default();
        let captcha_entry = otp_captcha_entry.clone();
        let captcha_row = otp_captcha_row.clone();
        let captcha_image = otp_captcha_image.clone();
        email_code_btn.connect_clicked(move |_| {
            let login = login_for_req.text().to_string();
            if login.trim().is_empty() {
                status.set_text("Введите телефон или почту");
                status.set_visible(true);
                return;
            }
            btn.set_sensitive(false);
            let http = reqwest::Client::new();
            let status = status.clone();
            let code_row = code_row.clone();
            let state = state_req.clone();
            let btn = btn.clone();
            let captcha = captcha_state
                .borrow()
                .clone()
                .zip(Some(captcha_entry.text().to_string()).filter(|t| !t.trim().is_empty()));
            let captcha_state = captcha_state.clone();
            let captcha_row = captcha_row.clone();
            let captcha_image = captcha_image.clone();
            glib::spawn_future_local(async move {
                let captcha_ref = captcha
                    .as_ref()
                    .map(|(sid, key)| (sid.as_str(), key.as_str()));
                match crate::api::vk::auth::validate_account(&http, &login, captcha_ref).await {
                    Ok(crate::api::vk::auth::VkValidateOutcome::CodeRequired {
                        sid,
                        method,
                        mask,
                        tg_flow,
                    }) => {
                        if method != "email" && method != "sms" {
                            status.set_text(&format!(
                                "VK просит подтверждение «{method}» — войдите по паролю"
                            ));
                            status.set_visible(true);
                            btn.set_sensitive(true);
                            return;
                        }
                        // Письмо отправляем явно: без sendOtpEmail VK код не шлёт.
                        match crate::api::vk::auth::send_otp_email(
                            &http,
                            &sid,
                            tg_flow.as_deref(),
                        )
                        .await
                        {
                            Ok(()) => {
                                *state.borrow_mut() = Some((sid, tg_flow));
                                let target = if mask.is_empty() {
                                    "привязанную почту".to_string()
                                } else {
                                    mask
                                };
                                status.set_text(&format!("Код отправлен на {target}"));
                                status.set_visible(true);
                                code_row.set_visible(true);
                            }
                            Err(error) => {
                                status.set_text(&format!("VK не отправил код: {error}"));
                                status.set_visible(true);
                            }
                        }
                        btn.set_sensitive(true);
                    }
                    Ok(crate::api::vk::auth::VkValidateOutcome::PasswordOnly { .. }) => {
                        status.set_text("VK просит пароль — введите его и нажмите «Войти»");
                        status.set_visible(true);
                        btn.set_sensitive(true);
                    }
                    Ok(crate::api::vk::auth::VkValidateOutcome::CaptchaRequired {
                        captcha_sid,
                        captcha_img,
                    }) => {
                        *captcha_state.borrow_mut() = Some(captcha_sid);
                        // VK отключил картинку для сторонних клиентов (captcha.php
                        // отдаёт «обновите версию»), остаётся веб-капча id.vk.com.
                        captcha_row.set_visible(false);
                        status.set_text(
                            "VK временно ограничил попытки (капча). Кнопка разблокируется через 10 минут — либо используйте пароль для приложения",
                        );
                        // Не долбим VK: каждая попытка продлевает блокировку
                        btn.set_sensitive(false);
                        let btn_later = btn.clone();
                        glib::timeout_add_local_once(
                            std::time::Duration::from_secs(10 * 60),
                            move || btn_later.set_sensitive(true),
                        );
                        status.set_visible(true);
                        let image = captcha_image.clone();
                        let http = reqwest::Client::new();
                        glib::spawn_future_local(async move {
                            if let Ok(bytes) = http.get(&captcha_img).send().await
                                && let Ok(bytes) = bytes.bytes().await
                                && let Ok(texture) = gtk4::gdk::Texture::from_bytes(&gtk4::glib::Bytes::from(&bytes[..]))
                            {
                                image.set_paintable(Some(&texture));
                            }
                        });
                        btn.set_sensitive(true);
                    }
                    Ok(crate::api::vk::auth::VkValidateOutcome::Error(message)) => {
                        status.set_text(&message);
                        status.set_visible(true);
                        btn.set_sensitive(true);
                    }
                    Err(error) => {
                        status.set_text(&format!("Ошибка VK: {error}"));
                        status.set_visible(true);
                        btn.set_sensitive(true);
                    }
                }
            });
        });

        let status_confirm = otp_status.clone();
        let code_entry = otp_code_entry.clone();
        let otp_code_confirm = otp_code.clone();
        let email_code_btn_confirm = email_code_btn.clone();
        let submit_login_btn_confirm = submit_login_btn.clone();
        let password_entry_confirm = password_entry.clone();
        let player_c = player_c;
        let dialog_weak = dialog_weak;
        otp_confirm_btn.connect_clicked(move |btn| {
            let Some((sid, _)) = state.borrow().clone() else {
                status_confirm.set_text("Сначала нажмите «Код на почту»");
                status_confirm.set_visible(true);
                return;
            };
            let code = code_entry.text().to_string();
            if code.trim().len() < 4 {
                status_confirm.set_text("Введите код из письма");
                status_confirm.set_visible(true);
                return;
            }
            btn.set_sensitive(false);
            let http = reqwest::Client::new();
            let status_confirm = status_confirm.clone();
            let otp_sid_confirm = otp_sid_confirm.clone();
            let code_row_confirm = code_row_confirm.clone();
            let player = player_c.clone();
            let dialog_weak = dialog_weak.clone();
            let otp_code_confirm = otp_code_confirm.clone();
            let email_code_btn_confirm = email_code_btn_confirm.clone();
            let submit_login_btn_confirm = submit_login_btn_confirm.clone();
            let password_entry_confirm = password_entry_confirm.clone();
            glib::spawn_future_local(async move {
                match crate::api::vk::auth::check_otp(&http, &sid, &code, "email").await {
                    Ok(crate::api::vk::auth::VkCheckOtpOutcome::Success {
                        sid: next_sid,
                        ..
                    }) => {
                        // VK X после кода просит пароль и отправляет его с
                        // grant_type=phone_confirmation_sid. Обмен на токен
                        // без пароля здесь не работает (invalid token type).
                        *otp_sid_confirm.borrow_mut() = Some(next_sid);
                        *otp_code_confirm.borrow_mut() = Some(code.clone());
                        code_row_confirm.set_visible(false);
                        // Больше не даём запросить второй код: следующий шаг — пароль.
                        // Это же защищает от лишних sendOtpEmail и их лимитов.
                        email_code_btn_confirm.set_sensitive(false);
                        submit_login_btn_confirm.set_label("Войти с паролем");
                        status_confirm.set_text(
                            "Код принят. Введите пароль в поле «Пароль» ниже и нажмите «Войти с паролем»",
                        );
                        status_confirm.set_visible(true);
                        password_entry_confirm.grab_focus();
                    }
                    Ok(crate::api::vk::auth::VkCheckOtpOutcome::Error(message)) => {
                        status_confirm.set_text(&message);
                        status_confirm.set_visible(true);
                    }
                    Err(error) => {
                        status_confirm.set_text(&format!("Ошибка проверки кода: {error}"));
                        status_confirm.set_visible(true);
                    }
                }
            });
        });
    }

    submit_login_btn.connect_clicked({
        let start = start_direct_auth.clone();
        move |_| start()
    });

    password_entry.connect_activate({
        let start = start_direct_auth.clone();
        move |_| start()
    });

    login_entry.connect_activate({
        let start = start_direct_auth.clone();
        let pwd_entry = password_entry.clone();
        move |entry| {
            if !entry.text().trim().is_empty() && !pwd_entry.text().is_empty() {
                start();
            } else {
                pwd_entry.grab_focus();
            }
        }
    });

    submit_code_btn.connect_clicked({
        let submit = submit_2fa_code.clone();
        move |_| submit()
    });

    code_entry.connect_activate({
        let submit = submit_2fa_code.clone();
        move |_| submit()
    });

    resend_email_btn.connect_clicked({
        let request = request_resend_email.clone();
        move |_| request()
    });

    submit_captcha_btn.connect_clicked({
        let submit = submit_captcha.clone();
        move |_| submit()
    });

    captcha_entry.connect_activate({
        let submit = submit_captcha.clone();
        move |_| submit()
    });

    back_to_login_from_code_btn.connect_clicked({
        let login_group = login_group.clone();
        let password_group = password_group.clone();
        let code_group = code_group.clone();
        let password_entry = password_entry.clone();
        move |_| {
            code_group.set_visible(false);
            login_group.set_visible(true);
            password_group.set_visible(true);
            password_entry.grab_focus();
        }
    });

    back_to_login_from_captcha_btn.connect_clicked({
        let login_group = login_group.clone();
        let password_group = password_group.clone();
        let captcha_group = captcha_group.clone();
        let password_entry = password_entry.clone();
        move |_| {
            captcha_group.set_visible(false);
            login_group.set_visible(true);
            password_group.set_visible(true);
            password_entry.grab_focus();
        }
    });

    to_browser_btn.connect_clicked({
        let login_group = login_group.clone();
        let password_group = password_group.clone();
        let browser_group = browser_group.clone();
        let token_entry = token_entry.clone();
        move |_| {
            login_group.set_visible(false);
            password_group.set_visible(false);
            browser_group.set_visible(true);
            token_entry.grab_focus();
        }
    });

    back_from_browser_btn.connect_clicked({
        let login_group = login_group.clone();
        let password_group = password_group.clone();
        let browser_group = browser_group.clone();
        let password_entry = password_entry.clone();
        move |_| {
            browser_group.set_visible(false);
            login_group.set_visible(true);
            password_group.set_visible(true);
            password_entry.grab_focus();
        }
    });

    // Валидация ссылки или токена из браузера
    let verify_browser_token = {
        let player = player.clone();
        let dialog = dialog.downgrade();
        let token_entry = token_entry.clone();
        let browser_status = browser_status.clone();
        let browser_spinner = browser_spinner.clone();
        let submit_token_btn = submit_token_btn.clone();

        move || {
            let input = token_entry.text().trim().to_string();
            if input.is_empty() {
                browser_status.set_text("Вставьте ссылку или токен");
                browser_status.set_visible(true);
                return;
            }

            browser_status.set_visible(false);
            submit_token_btn.set_sensitive(false);
            browser_spinner.set_visible(true);
            browser_spinner.start();

            let player = player.clone();
            let dialog = dialog.clone();
            let browser_status = browser_status.clone();
            let browser_spinner = browser_spinner.clone();
            let submit_token_btn = submit_token_btn.clone();

            gtk4::glib::spawn_future_local(async move {
                let http = match reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(20))
                    .build()
                {
                    Ok(c) => c,
                    Err(e) => {
                        browser_spinner.stop();
                        browser_spinner.set_visible(false);
                        submit_token_btn.set_sensitive(true);
                        browser_status.set_text(&format!("Ошибка сети: {e}"));
                        browser_status.set_visible(true);
                        return;
                    }
                };

                let outcome = crate::api::vk::auth::parse_and_validate_token(&http, &input).await;
                browser_spinner.stop();
                browser_spinner.set_visible(false);
                submit_token_btn.set_sensitive(true);

                match outcome {
                    Ok((token, user_id)) => {
                        if let Err(e) = crate::api::vk::auth::save_vk_token(&token) {
                            browser_status.set_text(&format!("Не удалось сохранить токен: {e}"));
                            browser_status.set_visible(true);
                            return;
                        }
                        if let Err(e) = crate::api::auth::save_vk_user_id(user_id) {
                            browser_status.set_text(&format!("Не удалось сохранить ID пользователя: {e}"));
                            browser_status.set_visible(true);
                            return;
                        }

                        match crate::api::vk::provider::VkProvider::new(token, user_id) {
                            Ok(provider) => {
                                player.register_vk_provider(provider);
                                player.switch_provider(crate::api::provider::ProviderId::Vk);
                                player.emit(crate::player::PlayerUiEvent::StatusMessage(
                                    "ВКонтакте подключён".to_string(),
                                ));
                                if let Some(dialog) = dialog.upgrade() {
                                    dialog.close();
                                }
                            }
                            Err(e) => {
                                browser_status.set_text(&e.to_string());
                                browser_status.set_visible(true);
                            }
                        }
                    }
                    Err(err_msg) => {
                        browser_status.set_text(&err_msg);
                        browser_status.set_visible(true);
                    }
                }
            });
        }
    };

    submit_token_btn.connect_clicked({
        let verify = verify_browser_token.clone();
        move |_| verify()
    });

    token_entry.connect_activate({
        let verify = verify_browser_token.clone();
        move |_| verify()
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
