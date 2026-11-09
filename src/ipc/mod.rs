pub mod mcp;

use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

use crate::player::PlayerManager;

/// Путь к Unix domain сокету плеера
pub fn socket_path() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        if !runtime_dir.trim().is_empty() {
            return PathBuf::from(runtime_dir).join("volna.sock");
        }
    }
    PathBuf::from("/tmp/volna.sock")
}

/// Запускает встроенный IPC сервер в Tokio runtime
pub fn spawn_ipc_server(player: PlayerManager) {
    let sock_path = socket_path();
    tokio::spawn(async move {
        // Удаляем устаревший файл сокета, если он остался от предыдущего сеанса
        let _ = std::fs::remove_file(&sock_path);

        let listener = match UnixListener::bind(&sock_path) {
            Ok(l) => {
                eprintln!("[ipc] Unix domain socket listener started on {:?}", sock_path);
                l
            }
            Err(e) => {
                eprintln!("[ipc] Ошибка создания сокета {:?}: {}", sock_path, e);
                return;
            }
        };

        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    let player = player.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(stream, player).await {
                            eprintln!("[ipc] Ошибка соединения: {}", e);
                        }
                    });
                }
                Err(e) => {
                    eprintln!("[ipc] Ошибка listener.accept: {}", e);
                }
            }
        }
    });
}

/// Обработка одного входящего подключения к Unix сокету
async fn handle_connection(
    mut stream: UnixStream,
    player: PlayerManager,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (reader, mut writer) = stream.split();
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let cmd_val: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                let err_resp = serde_json::json!({
                    "ok": false,
                    "error": format!("Invalid JSON: {}", e)
                });
                let mut out = serde_json::to_string(&err_resp)?;
                out.push('\n');
                writer.write_all(out.as_bytes()).await?;
                writer.flush().await?;
                continue;
            }
        };

        let resp = handle_command(&player, &cmd_val).await;
        let mut out = serde_json::to_string(&resp)?;
        out.push('\n');
        writer.write_all(out.as_bytes()).await?;
        writer.flush().await?;
    }

    Ok(())
}

/// Выполнение команды плеера
pub async fn handle_command(player: &PlayerManager, cmd_json: &serde_json::Value) -> serde_json::Value {
    let cmd = match cmd_json.get("cmd").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => {
            return serde_json::json!({
                "ok": false,
                "error": "missing 'cmd' field"
            });
        }
    };

    match cmd {
        "status" => {
            let state = player.state.lock().unwrap();
            let provider_id = player.providers.active_id();
            let provider_str = match provider_id {
                crate::api::provider::ProviderId::Yandex => "yandex",
                crate::api::provider::ProviderId::YouTubeMusic => "ytmusic",
                crate::api::provider::ProviderId::Vk => "vk",
            };

            let current_track = state.current_track.as_ref().map(|t| {
                let source_str = match t.source {
                    crate::api::models::TrackSource::Yandex => "yandex",
                    crate::api::models::TrackSource::YouTubeMusic => "ytmusic",
                    crate::api::models::TrackSource::Vk => "vk",
                };
                serde_json::json!({
                    "id": t.id,
                    "title": t.title,
                    "artists": t.artists_string(),
                    "album": t.albums.first().and_then(|a| a.title.clone()),
                    "source": source_str,
                    "is_liked": state.is_liked,
                })
            });

            serde_json::json!({
                "ok": true,
                "is_playing": player.audio.is_playing(),
                "volume": player.get_volume(),
                "position_ms": player.audio.position_ms(),
                "duration_ms": player.audio.duration_ms(),
                "provider": provider_str,
                "current_track": current_track,
                "queue_len": state.queue.len(),
                "current_index": state.current_index,
            })
        }
        "play" => {
            player.resume();
            serde_json::json!({ "ok": true })
        }
        "pause" => {
            player.pause();
            serde_json::json!({ "ok": true })
        }
        "toggle" => {
            player.toggle_play();
            serde_json::json!({ "ok": true })
        }
        "next" => {
            player.next_track();
            serde_json::json!({ "ok": true })
        }
        "prev" => {
            player.prev_track();
            serde_json::json!({ "ok": true })
        }
        "volume" => {
            if let Some(val) = cmd_json.get("val").and_then(|v| v.as_f64()) {
                player.set_volume(val);
                serde_json::json!({ "ok": true })
            } else {
                serde_json::json!({ "ok": false, "error": "missing or invalid 'val'" })
            }
        }
        "switch_provider" => {
            let prov = cmd_json.get("provider").and_then(|v| v.as_str()).unwrap_or("");
            let target_id = match prov {
                "yandex" => Some(crate::api::provider::ProviderId::Yandex),
                "ytmusic" | "youtube" => Some(crate::api::provider::ProviderId::YouTubeMusic),
                "vk" => Some(crate::api::provider::ProviderId::Vk),
                _ => None,
            };
            if let Some(id) = target_id {
                player.switch_provider(id);
                serde_json::json!({ "ok": true })
            } else {
                serde_json::json!({ "ok": false, "error": format!("unknown provider '{}'", prov) })
            }
        }
        "start_wave" => {
            let station_id = cmd_json
                .get("station_id")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(|| "user:onyourwave".into());
            player.start_station(station_id, None, "Моя Волна".into());
            serde_json::json!({ "ok": true })
        }
        "filter" => {
            let mood = cmd_json.get("mood").and_then(|v| v.as_str()).map(String::from);
            let language = cmd_json
                .get("language")
                .and_then(|v| v.as_str())
                .map(String::from);
            let character = cmd_json
                .get("character")
                .and_then(|v| v.as_str())
                .map(String::from);
            player.apply_rotor_filter(mood, language, character);
            serde_json::json!({ "ok": true })
        }
        "clip" => {
            let action = cmd_json
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("status");
            match action {
                "on" => {
                    player.set_clips_enabled(true);
                    serde_json::json!({ "ok": true, "enabled": true })
                }
                "off" => {
                    player.set_clips_enabled(false);
                    serde_json::json!({ "ok": true, "enabled": false })
                }
                "reset" => {
                    let cleared = player.reset_clip_negative_cache();
                    serde_json::json!({ "ok": true, "cleared": cleared })
                }
                "status" => {
                    let status = player.clips.telemetry().snapshot();
                    let audio_ms = player.audio.position_ms();
                    let drift = match (status.position_ms, audio_ms) {
                        (Some(clip), Some(audio)) => Some(clip as i64 - audio as i64),
                        _ => None,
                    };
                    serde_json::json!({
                        "ok": true,
                        "enabled": status.enabled,
                        "clip_source": status.source.as_deref().unwrap_or("none"),
                        "clip_track_id": status.track_id,
                        "clip_format": status.format,
                        "attempt_id": status.attempt_id,
                        "clip_position_ms": status.position_ms,
                        "audio_position_ms": audio_ms,
                        "clip_drift_ms": drift,
                        "clip_sync_degraded": status.sync_degraded,
                        "clip_error": status.error,
                        "clip_download_ms": status.download_ms,
                        "clip_bytes": status.bytes,
                    })
                }
                other => serde_json::json!({
                    "ok": false,
                    "error": format!("unknown clip action: {other}"),
                }),
            }
        }
        "search_and_play" => {
            if let Some(query) = cmd_json.get("query").and_then(|v| v.as_str()) {
                let provider = player.providers.current();
                match provider.search(query).await {
                    Ok(res) => {
                        if let Some(track) = res.tracks.and_then(|s| s.results.into_iter().next()) {
                            player.play_single_track(track.clone());
                            serde_json::json!({ "ok": true, "track": track })
                        } else {
                            serde_json::json!({ "ok": false, "error": "track not found" })
                        }
                    }
                    Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
                }
            } else {
                serde_json::json!({ "ok": false, "error": "missing 'query'" })
            }
        }
        "set_cookie" => {
            if let Some(cookie) = cmd_json.get("cookie").and_then(|v| v.as_str()) {
                let sanitized = crate::api::ytmusic::sanitize_cookie(cookie);
                if let Err(e) = crate::api::auth::save_ytmusic_cookie(&sanitized) {
                    eprintln!("[ipc] Ошибка сохранения cookie: {e}");
                }
                let client = Arc::new(crate::api::ytmusic::YtMusicClient::with_cookie(sanitized));
                player.providers.register(client);
                player.emit(crate::player::PlayerUiEvent::ProviderChanged(
                    crate::api::provider::ProviderId::YouTubeMusic,
                ));
                serde_json::json!({ "ok": true })
            } else {
                serde_json::json!({ "ok": false, "error": "missing 'cookie'" })
            }
        }
        "like" => {
            if let Some(liked) = cmd_json.get("liked").and_then(|v| v.as_bool()) {
                let track = player.state.lock().unwrap().current_track.clone();
                if let Some(t) = track {
                    player.set_track_liked(&t, liked);
                    serde_json::json!({ "ok": true })
                } else {
                    serde_json::json!({ "ok": false, "error": "no active track to like" })
                }
            } else {
                player.toggle_like();
                serde_json::json!({ "ok": true })
            }
        }
        "dislike" => {
            player.toggle_dislike();
            serde_json::json!({ "ok": true })
        }
        _ => serde_json::json!({ "ok": false, "error": "unknown command" }),
    }
}

/// Отправка команды в запущенный плеер Волна через сокет
pub async fn send_cmd(
    cmd_json: &serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    let sock_path = socket_path();
    let mut stream = UnixStream::connect(&sock_path).await?;

    let (reader, mut writer) = stream.split();
    let mut lines = BufReader::new(reader).lines();

    let mut payload = serde_json::to_string(cmd_json)?;
    payload.push('\n');
    writer.write_all(payload.as_bytes()).await?;
    writer.flush().await?;

    if let Some(line) = lines.next_line().await? {
        let val: serde_json::Value = serde_json::from_str(&line)?;
        Ok(val)
    } else {
        Err("IPC connection closed without response".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::client::YandexMusicClient;

    fn setup_test_player() -> Option<PlayerManager> {
        let _ = gstreamer::init();
        let client = YandexMusicClient::new(None);
        let (tx, _rx) = async_channel::unbounded();
        PlayerManager::new(client, vec![tx]).ok()
    }

    #[tokio::test]
    async fn test_ipc_status_command() {
        let Some(player) = setup_test_player() else { return; };
        let cmd = serde_json::json!({ "cmd": "status" });
        let resp = handle_command(&player, &cmd).await;
        assert_eq!(resp["ok"], true);
        let expected = match player.providers.active_id() {
            crate::api::provider::ProviderId::Yandex => "yandex",
            crate::api::provider::ProviderId::YouTubeMusic => "ytmusic",
            crate::api::provider::ProviderId::Vk => "vk",
        };
        assert_eq!(resp["provider"], expected);
        let (expected_queue_len, expected_index) = {
            let s = player.state.lock().unwrap();
            (s.queue.len(), s.current_index)
        };
        assert_eq!(resp["queue_len"], expected_queue_len);
        assert_eq!(resp["current_index"], expected_index);
    }

    #[tokio::test]
    async fn test_ipc_volume_command() {
        let Some(player) = setup_test_player() else { return; };
        let cmd = serde_json::json!({ "cmd": "volume", "val": 0.75 });
        let resp = handle_command(&player, &cmd).await;
        assert_eq!(resp["ok"], true);
        assert!((player.get_volume() - 0.75).abs() < 0.05);
    }

    #[tokio::test]
    async fn test_ipc_switch_provider_command() {
        let Some(player) = setup_test_player() else { return; };
        let cmd = serde_json::json!({ "cmd": "switch_provider", "provider": "ytmusic" });
        let resp = handle_command(&player, &cmd).await;
        assert_eq!(resp["ok"], true);
        assert_eq!(player.providers.active_id(), crate::api::provider::ProviderId::YouTubeMusic);

        let cmd_back = serde_json::json!({ "cmd": "switch_provider", "provider": "yandex" });
        let resp_back = handle_command(&player, &cmd_back).await;
        assert_eq!(resp_back["ok"], true);
        assert_eq!(player.providers.active_id(), crate::api::provider::ProviderId::Yandex);

        let vk_prov = crate::api::vk::provider::VkProvider::new("test_token", 12345).unwrap();
        player.register_vk_provider(vk_prov);

        let cmd_vk = serde_json::json!({ "cmd": "switch_provider", "provider": "vk" });
        let resp_vk = handle_command(&player, &cmd_vk).await;
        assert_eq!(resp_vk["ok"], true);
        assert_eq!(player.providers.active_id(), crate::api::provider::ProviderId::Vk);
    }

    #[tokio::test]
    async fn test_ipc_unknown_command() {
        let Some(player) = setup_test_player() else { return; };
        let cmd = serde_json::json!({ "cmd": "non_existent_command" });
        let resp = handle_command(&player, &cmd).await;
        assert_eq!(resp["ok"], false);
        assert_eq!(resp["error"], "unknown command");
    }
}
