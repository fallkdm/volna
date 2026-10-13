use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Entry, Label, Orientation, Spinner};
use libadwaita::prelude::*;
use libadwaita::{ActionRow, PreferencesDialog, PreferencesGroup, PreferencesPage};

use crate::api::auth::{extract_token_from_input, save_token};
use crate::api::client::YandexMusicClient;
use crate::player::PlayerManager;

const YANDEX_OAUTH_URL: &str = "https://oauth.yandex.ru/authorize?response_type=token&client_id=23cabbbdc6cd418abb4b4c73e3300b97";

/// Отображает диалог входа в аккаунт Яндекс Музыки
pub fn show_auth_dialog(parent: &impl IsA<gtk4::Widget>, player: &PlayerManager) {
    let dialog = PreferencesDialog::new();
    dialog.set_title("Вход в Яндекс Музыку");

    let page = PreferencesPage::new();
    page.set_title("Авторизация");
    page.set_icon_name(Some("avatar-default-symbolic"));

    let group = PreferencesGroup::new();
    group.set_title("Яндекс ID");
    group.set_description(Some(
        "Войдите через Яндекс ID для доступа к персональной Моей волне, рекомендациям и вашей фонотеке."
    ));

    // Шаг 1: Кнопка открытия браузера
    let step1_row = ActionRow::builder()
        .title("1. Получить токен")
        .subtitle("Откроется страница авторизации в браузере по умолчанию")
        .build();

    let open_browser_btn = Button::with_label("Открыть Яндекс ID");
    open_browser_btn.add_css_class("suggested-action");

    open_browser_btn.connect_clicked(|_| {
        let _ = gio::AppInfo::launch_default_for_uri(YANDEX_OAUTH_URL, None::<&gio::AppLaunchContext>);
    });

    step1_row.add_suffix(&open_browser_btn);
    group.add(&step1_row);

    // Шаг 2: Ввод токена
    let step2_row = ActionRow::builder()
        .title("2. Вставить токен или URL")
        .subtitle("Скопируйте полученный access_token или всю адресную строку из браузера")
        .build();

    let input_box = GtkBox::new(Orientation::Horizontal, 8);
    let token_entry = Entry::new();
    token_entry.set_width_chars(28);
    token_entry.set_placeholder_text(Some("y0_AgAAAA... или https://..."));
    token_entry.set_hexpand(true);

    let submit_btn = Button::with_label("Войти");
    submit_btn.add_css_class("suggested-action");

    let spinner = Spinner::new();
    spinner.set_visible(false);

    input_box.append(&token_entry);
    input_box.append(&spinner);
    input_box.append(&submit_btn);
    step2_row.add_suffix(&input_box);
    group.add(&step2_row);

    // Сообщение об ошибке/статусе
    let status_label = Label::new(None);
    status_label.set_halign(Align::Center);
    status_label.set_margin_top(8);
    status_label.add_css_class("error");
    status_label.set_visible(false);

    let group_box = GtkBox::new(Orientation::Vertical, 4);
    group_box.append(&status_label);

    page.add(&group);
    dialog.add(&page);

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
                    s.set_text("Пожалуйста, вставьте токен");
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
                            s.set_text(&format!("Неверный токен или ошибка: {}", e));
                            s.set_visible(true);
                        }
                    }
                }
            });
        });
    }

    dialog.present(Some(parent));
}
