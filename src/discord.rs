//! Discord Rich Presence: в статусе показывается, что играет в приложении.
//! Мост слушает события плеера, а общение с Discord идёт в отдельном потоке (IPC синхронный).

use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_channel::Receiver;
use discord_rich_presence::activity::{Activity, ActivityType, Assets, Button, Timestamps};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};

use crate::api::models::Track;
use crate::player::{PlayerManager, PlayerUiEvent};

/// Application ID приложения «Волна» в Discord. Он публичный (не секрет); создаётся один раз владельцем проекта
/// на discord.com/developers, пользователям создавать ничего не нужно. Пока не задан, статус работает
/// только с ID, введённым вручную в настройках.
pub const DEFAULT_APP_ID: Option<&str> = Some("1564970064304808128");

/// Какой Application ID использовать: введённый вручную, иначе встроенный
pub fn effective_app_id(custom: Option<String>) -> Option<String> {
    custom.or_else(|| DEFAULT_APP_ID.map(String::from))
}

/// То, что показывается в статусе
#[derive(Debug, Clone, PartialEq)]
struct Presence {
    client_id: String,
    details: String,
    state: String,
    cover: Option<String>,
    album: String,
    url: String,
    start: i64,
    end: Option<i64>,
}

enum Msg {
    Show(Presence),
    /// Убрать статус и закрыть соединение (выключено, пауза, нет трека)
    Clear,
}

fn presence_for(track: &Track, client_id: &str, position_ms: u64, now_secs: i64) -> Presence {
    let start = now_secs - (position_ms / 1000) as i64;
    let title = match track.version.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) => format!("{} ({})", track.title, v),
        None => track.title.clone(),
    };
    let album = track.albums.first();
    Presence {
        client_id: client_id.to_string(),
        details: title,
        state: track.artists_string(),
        cover: track.cover_url(400),
        album: album.and_then(|a| a.title.clone()).unwrap_or_default(),
        url: match album {
            Some(a) => format!("https://music.yandex.ru/album/{}/track/{}", a.id, track.id),
            None => format!("https://music.yandex.ru/track/{}", track.id),
        },
        start,
        end: track.duration_ms.map(|ms| start + (ms / 1000) as i64),
    }
}

/// Запускает мост: события плеера → статус Discord
pub fn spawn_bridge(player: PlayerManager, events: Receiver<PlayerUiEvent>) {
    let (tx, rx) = mpsc::channel::<Msg>();
    std::thread::spawn(move || worker(rx));
    tokio::spawn(async move {
        let (mut track, mut playing) = (None::<Track>, false);
        let mut debounce_timer: Option<tokio::time::Instant> = None;
        let mut pos_override = None;

        loop {
            tokio::select! {
                res = events.recv() => {
                    let Ok(event) = res else { break };
                    match event {
                        PlayerUiEvent::TrackStarted { track: t, .. } => {
                            track = Some(t);
                            pos_override = Some(0);
                            // Даем 350 мс на случай быстрого перелистывания треков
                            debounce_timer = Some(tokio::time::Instant::now() + std::time::Duration::from_millis(350));
                        }
                        PlayerUiEvent::PlaybackToggled(p) => {
                            playing = p;
                            debounce_timer = Some(tokio::time::Instant::now()); // Пауза/плей отправляются сразу
                        }
                        PlayerUiEvent::Seeked(sec) => {
                            if !playing {
                                continue;
                            }
                            pos_override = Some(sec * 1000);
                            debounce_timer = Some(tokio::time::Instant::now());
                        }
                        PlayerUiEvent::DiscordSettingsChanged => {
                            debounce_timer = Some(tokio::time::Instant::now());
                        }
                        _ => continue,
                    }
                }
                _ = async {
                    if let Some(target) = debounce_timer {
                        tokio::time::sleep_until(target).await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                }, if debounce_timer.is_some() => {
                    debounce_timer = None;
                    let (enabled, custom_id) = crate::api::auth::load_discord();
                    let msg = match (enabled, effective_app_id(custom_id), &track, playing) {
                        (true, Some(id), Some(t), true) => {
                            let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
                            let position_ms = pos_override.take().unwrap_or_else(|| player.position_ms().unwrap_or(0));
                            Msg::Show(presence_for(t, &id, position_ms, now))
                        }
                        _ => {
                            pos_override = None;
                            Msg::Clear
                        }
                    };
                    let _ = tx.send(msg);
                }
            }
        }
    });
}

/// Поток общения с Discord: подключается при первом статусе, переподключается после обрыва
fn worker(rx: mpsc::Receiver<Msg>) {
    let mut client: Option<(String, DiscordIpcClient)> = None;
    let mut last: Option<Presence> = None;
    while let Ok(mut msg) = rx.recv() {
        // Схлопываем быстрые промежуточные сообщения в очереди
        while let Ok(next) = rx.try_recv() {
            msg = next;
        }

        match msg {
            Msg::Clear => {
                last = None;
                if let Some((_, mut c)) = client.take() {
                    let _ = c.clear_activity();
                    let _ = c.close();
                    eprintln!("[discord] статус очищен");
                }
            }
            Msg::Show(p) => {
                // Тот же статус повторно не отправляем
                if last.as_ref() == Some(&p) && client.is_some() {
                    continue;
                }
                if client.as_ref().is_some_and(|(id, _)| *id != p.client_id) {
                    client = None; // Сменили Application ID
                }
                if client.is_none() {
                    let mut c = DiscordIpcClient::new(&p.client_id);
                    if let Err(e) = c.connect() {
                        eprintln!("[discord] не удалось подключиться к сокету Discord: {:?}", e);
                        continue;
                    }
                    eprintln!("[discord] подключено к сокету Discord");
                    client = Some((p.client_id.clone(), c));
                }
                let Some((_, c)) = client.as_mut() else { continue };
                let mut assets = Assets::new().large_text(&p.album);
                if let Some(cover) = &p.cover {
                    assets = assets.large_image(cover);
                }
                let mut ts = Timestamps::new().start(p.start);
                if let Some(end) = p.end {
                    ts = ts.end(end);
                }
                let activity = Activity::new()
                    .activity_type(ActivityType::Listening)
                    .details(&p.details)
                    .state(&p.state)
                    .assets(assets)
                    .timestamps(ts)
                    .buttons(vec![Button::new("Слушать в Яндекс Музыке", &p.url)]);

                eprintln!("[discord] отправка статуса: {} — {}", p.details, p.state);
                if let Err(e) = c.set_activity(activity.clone()) {
                    eprintln!("[discord] ошибка set_activity ({:?}), выполняем retry...", e);
                    let _ = c.close();
                    client = None;
                    last = None;

                    // Повторная попытка переподключения и отправки
                    let mut new_c = DiscordIpcClient::new(&p.client_id);
                    if let Ok(()) = new_c.connect() {
                        if let Ok(()) = new_c.set_activity(activity) {
                            eprintln!("[discord] статус успешно обновлен после retry: {} — {}", p.details, p.state);
                            client = Some((p.client_id.clone(), new_c));
                            last = Some(p);
                        } else {
                            eprintln!("[discord] повторная отправка тоже завершилась ошибкой");
                        }
                    }
                } else {
                    eprintln!("[discord] статус успешно обновлен: {} — {}", p.details, p.state);
                    last = Some(p);
                }

            }
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presence_has_version_times_and_link() {
        let t: Track = serde_json::from_value(serde_json::json!({
            "id": "5", "title": "Over", "version": "Remix", "durationMs": 200_000,
            "artists": [{"id": "1", "name": "A"}, {"id": "2", "name": "B"}],
            "albums": [{"id": "9", "title": "Alb"}]
        }))
        .unwrap();
        let p = presence_for(&t, "123", 30_000, 1_000);
        assert_eq!(p.details, "Over (Remix)");
        assert_eq!(p.state, "A, B");
        assert_eq!((p.start, p.end), (970, Some(1170)));
        assert_eq!(p.url, "https://music.yandex.ru/album/9/track/5");
    }
}
