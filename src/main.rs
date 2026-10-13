pub mod api;
pub mod player;
pub mod ui;

use libadwaita::prelude::*;
use libadwaita::Application;

use crate::api::auth::load_token;
use crate::api::client::YandexMusicClient;
use crate::player::PlayerManager;
use crate::ui::VolnaWindow;

const APP_ID: &str = "io.github.fallkdm.Volna";

fn main() {
    // Инициализируем многопоточный Tokio runtime для фоновых сетевых запросов
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

        // Загружаем токен и создаем клиент API
        let token = load_token();
        let client = YandexMusicClient::new(token);

        // Создаем канал для передачи событий от аудио-движка в интерфейс
        let (sender, receiver) = async_channel::unbounded();

        // Инициализируем плеер
        match PlayerManager::new(client, sender) {
            Ok(player) => {
                let window = VolnaWindow::new(app, player, receiver);
                window.present();
            }
            Err(e) => {
                eprintln!("Ошибка инициализации аудио-движка: {}", e);
            }
        }
    });

    app.run();
}
