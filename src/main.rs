#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

pub mod api;
pub mod cache;
pub mod media_controls;
pub mod mpris;
pub mod player;
pub mod ui;

use libadwaita::prelude::*;
use libadwaita::Application;

use crate::api::auth::load_token;
use crate::api::client::YandexMusicClient;
use crate::player::PlayerManager;
use crate::ui::VolnaWindow;

const APP_ID: &str = "io.github.fallkdm.Volna";

#[cfg(windows)]
fn init_windows_environment() {
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let gst_plugins = exe_dir.join("lib").join("gstreamer-1.0");
            if gst_plugins.is_dir() {
                unsafe {
                    std::env::set_var("GST_PLUGIN_PATH_1_0", &gst_plugins);
                    std::env::set_var("GST_PLUGIN_SYSTEM_PATH_1_0", &gst_plugins);
                }
            }

            let gio_modules = exe_dir.join("lib").join("gio").join("modules");
            if gio_modules.is_dir() {
                unsafe {
                    std::env::set_var("GIO_MODULE_DIR", &gio_modules);
                }
            }

            let glib_schemas = exe_dir.join("share").join("glib-2.0").join("schemas");
            if glib_schemas.is_dir() {
                unsafe {
                    std::env::set_var("GSETTINGS_SCHEMA_DIR", &glib_schemas);
                }
            }

            let gst_scanner = exe_dir.join("libexec").join("gstreamer-1.0").join("gst-plugin-scanner.exe");
            if gst_scanner.is_file() {
                unsafe {
                    std::env::set_var("GST_PLUGIN_SCANNER", &gst_scanner);
                }
            }
        }
    }
}

fn main() {
    #[cfg(windows)]
    init_windows_environment();

    // Регистрируем встроенные GResource ресурсы (иконки и ассеты)
    gio::resources_register_include!("volna.gresource").expect("Не удалось зарегистрировать GResource");

    // Инициализируем многопоточный Tokio runtime для фоновых сетевых запросов и MPRIS D-Bus
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Не удалось инициализировать Tokio runtime");
    let _guard = rt.enter();

    // Создаем приложение Libadwaita
    let app = Application::builder()
        .application_id(APP_ID)
        .build();

    app.connect_activate(|app| {
        // Подключаем кастомные стили оформления
        ui::load_css();

        // Регистрируем ресурсный путь к встроенным иконкам
        if let Some(display) = gtk4::gdk::Display::default() {
            let icon_theme = gtk4::IconTheme::for_display(&display);
            icon_theme.add_resource_path("/io/github/fallkdm/Volna/icons");
        }

        // Загружаем сохраненный токен и создаем клиент API
        let token = load_token();
        let client = YandexMusicClient::new(token);

        // Создаем каналы событий для UI и для MPRIS D-Bus сервера
        let (ui_tx, ui_rx) = async_channel::unbounded();
        let (mpris_tx, mpris_rx) = async_channel::unbounded();

        // Инициализируем плеер со связкой каналов
        match PlayerManager::new(client, vec![ui_tx, mpris_tx]) {
            Ok(player) => {
                // Создаем главное окно и подключаем системные медиаклавиши (MPRIS / SMTC)
                let window = VolnaWindow::new(app, player.clone(), ui_rx);
                media_controls::start(&window.window, player, mpris_rx);
                window.present();
            }
            Err(e) => {
                eprintln!("Ошибка инициализации аудио-движка: {}", e);
            }
        }
    });

    app.run();
}
