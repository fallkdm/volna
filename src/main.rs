use libadwaita::prelude::*;
use libadwaita::Application;

#[allow(unused_imports)]
use volna::{api, cache, cast, discord, ipc, mpris, player, station, ui, trim_allocator};
use volna::api::auth::load_token;
use volna::api::client::YandexMusicClient;
use volna::player::PlayerManager;
use volna::ui::VolnaWindow;

const APP_ID: &str = "io.github.fallkdm.Volna";

/// Ограничивает число malloc-арен glibc.
///
/// У плеера много потоков (GTK, GStreamer, Tokio, сеть), и glibc создаёт
/// отдельную арену почти под каждый. К моменту, когда все они разрастаются,
/// в памяти висит 200+ МБ пустых арен. Двух арен хватает с запасом.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn tune_allocator() {
    unsafe extern "C" {
        fn mallopt(param: std::os::raw::c_int, value: std::os::raw::c_int) -> std::os::raw::c_int;
    }
    // M_ARENA_MAX из <malloc.h>; тестовый harness может задать 2/4/env.
    const M_ARENA_MAX: std::os::raw::c_int = -8;
    let arena_max = std::env::var("M_ARENA_MAX")
        .ok()
        .and_then(|value| value.trim().parse::<i32>().ok())
        .unwrap_or(2)
        .clamp(1, 64);
    unsafe {
        let _ = mallopt(M_ARENA_MAX, arena_max);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn tune_allocator() {}

fn main() {
    tune_allocator();

    // Инициализируем многопоточный Tokio runtime для фоновых сетевых запросов и MPRIS D-Bus
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Не удалось инициализировать Tokio runtime");
    let _guard = rt.enter();

    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "mcp" {
        // Запускаем MCP сервер без инициализации GTK!
        rt.block_on(volna::ipc::mcp::run_mcp_server());
        return;
    }
    if args.len() > 1 && args[1] == "cmd" {
        // Простой CLI-клиент для вызова команд из терминала
        let cmd_name = args.get(2).map(|s| s.as_str()).unwrap_or("status");
        let cmd_json = match cmd_name {
            "status" | "play" | "pause" | "toggle" | "next" | "prev" | "like" | "dislike" => {
                serde_json::json!({ "cmd": cmd_name })
            }
            "volume" => {
                let val = args.get(3).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.5);
                serde_json::json!({ "cmd": "volume", "val": val })
            }
            "switch_provider" => {
                let prov = args.get(3).map(|s| s.as_str()).unwrap_or("yandex");
                serde_json::json!({ "cmd": "switch_provider", "provider": prov })
            }
            "wave" | "start_wave" => {
                let station = args.get(3).map(|s| s.to_string());
                serde_json::json!({ "cmd": "start_wave", "station_id": station })
            }
            "search" | "search_and_play" => {
                let query = args[3..].join(" ");
                serde_json::json!({ "cmd": "search_and_play", "query": query })
            }
            "filter" => {
                // волна cmd filter <mood|-> <language|-> <character|-> ; «-» = сбросить
                let pick = |i: usize| -> Option<String> {
                    args.get(i)
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty() && *s != "-")
                        .map(String::from)
                };
                let mut cmd = serde_json::Map::new();
                cmd.insert("cmd".into(), serde_json::json!("filter"));
                if let Some(v) = pick(3) {
                    cmd.insert("mood".into(), serde_json::json!(v));
                }
                if let Some(v) = pick(4) {
                    cmd.insert("language".into(), serde_json::json!(v));
                }
                if let Some(v) = pick(5) {
                    cmd.insert("character".into(), serde_json::json!(v));
                }
                serde_json::Value::Object(cmd)
            }
            "clip" => {
                // волна cmd clip on|off|status|reset (по умолчанию status)
                let action = args.get(3).map(|a| a.trim()).unwrap_or("status");
                serde_json::json!({ "cmd": "clip", "action": action })
            }
            "set_cookie" => {
                let cookie = args.get(3).cloned().unwrap_or_default();
                serde_json::json!({ "cmd": "set_cookie", "cookie": cookie })
            }
            other => {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(other) {
                    val
                } else {
                    serde_json::json!({ "cmd": other })
                }
            }
        };

        rt.block_on(async {
            match volna::ipc::send_cmd(&cmd_json).await {
                Ok(resp) => {
                    println!("{}", serde_json::to_string_pretty(&resp).unwrap_or_else(|_| resp.to_string()));
                }
                Err(e) => {
                    eprintln!("Ошибка IPC: {}", e);
                    std::process::exit(1);
                }
            }
        });
        return;
    }

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
                // Запускаем IPC сервер
                ipc::spawn_ipc_server(player.clone());

                // Запускаем системный MPRIS2 D-Bus сервер
                mpris::spawn_mpris_server(player.clone(), mpris_rx);
                discord::spawn_bridge(player.clone(), discord_rx);

                // закрытие приложения останавливает музыку, которую оно включило на Станции
                let exit_player = player.clone();
                app.connect_shutdown(move |_| exit_player.cast_stop_on_exit());

                // Создаем и отображаем главное окно приложения
                let window = VolnaWindow::new(app, player, ui_rx);
                window.present();

                // Фоновый возврат освободившейся памяти ОС каждые 45 секунд
                gtk4::glib::timeout_add_seconds_local(45, || {
                    std::thread::spawn(|| volna::trim_allocator());
                    glib::ControlFlow::Continue
                });
            }
            Err(e) => {
                eprintln!("Ошибка инициализации аудио-движка: {}", e);
            }
        }
    });

    app.run();
}
