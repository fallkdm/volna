pub mod api;
pub mod cache;
pub mod discord;
pub mod mpris;
pub mod player;
pub mod station;
pub mod ui;

use libadwaita::prelude::*;
use libadwaita::Application;

use crate::api::auth::load_token;
use crate::api::client::YandexMusicClient;
use crate::player::PlayerManager;
use crate::ui::VolnaWindow;

const APP_ID: &str = "io.github.fallkdm.Volna";

fn main() {
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

        // Загружаем сохраненный токен и создаем клиент API
        let token = load_token();
        let client = YandexMusicClient::new(token);

        // Создаем каналы событий для UI и для MPRIS D-Bus сервера
        let (ui_tx, ui_rx) = async_channel::unbounded();
        let (mpris_tx, mpris_rx) = async_channel::unbounded();
        let (discord_tx, discord_rx) = async_channel::unbounded();

        // Инициализируем плеер со связкой каналов
        match PlayerManager::new(client, vec![ui_tx, mpris_tx, discord_tx]) {
            Ok(player) => {
                // Запускаем системный MPRIS2 D-Bus сервер
                mpris::spawn_mpris_server(player.clone(), mpris_rx);
                discord::spawn_bridge(player.clone(), discord_rx);

                // закрытие приложения останавливает музыку, которую оно включило на Станции
                let exit_player = player.clone();
                app.connect_shutdown(move |_| exit_player.cast_stop_on_exit());

                // Создаем и отображаем главное окно приложения
                let window = VolnaWindow::new(app, player, ui_rx);
                window.present();
            }
            Err(e) => {
                eprintln!("Ошибка инициализации аудио-движка: {}", e);
            }
        }
    });

    app.run();
}
