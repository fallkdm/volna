use std::sync::{Arc, Mutex};
use async_channel::Sender;

use crate::api::client::YandexMusicClient;
use crate::api::models::Track;
use super::audio::AudioBackend;

/// События плеера для обновления интерфейса
#[derive(Debug, Clone)]
pub enum PlayerUiEvent {
    TrackStarted {
        track: Track,
        index: usize,
        total_queue: usize,
    },
    PlaybackToggled(bool),
    Progress {
        current_sec: u64,
        total_sec: u64,
    },
    LikeStateChanged(bool),
    DislikeStateChanged(bool),
    QueueUpdated(Vec<Track>),
    LoadingState(bool),
    StatusMessage(String),
    Error(String),
}

/// Состояние очереди и воспроизведения
pub struct PlayerState {
    pub current_track: Option<Track>,
    pub queue: Vec<Track>,
    pub current_index: usize,
    pub station_id: String,
    pub batch_id: Option<String>,
    pub is_liked: bool,
    pub is_disliked: bool,
    pub played_seconds: f64,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            current_track: None,
            queue: Vec::new(),
            current_index: 0,
            station_id: "user:onyourwave".to_string(),
            batch_id: None,
            is_liked: false,
            is_disliked: false,
            played_seconds: 0.0,
        }
    }
}

/// Менеджер плеера
#[derive(Clone)]
pub struct PlayerManager {
    pub audio: Arc<AudioBackend>,
    pub client: Arc<Mutex<YandexMusicClient>>,
    pub state: Arc<Mutex<PlayerState>>,
    pub event_sender: Sender<PlayerUiEvent>,
}

impl PlayerManager {
    /// Создает новый экземпляр менеджера
    pub fn new(
        client: YandexMusicClient,
        event_sender: Sender<PlayerUiEvent>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let audio = Arc::new(AudioBackend::new()?);
        let client = Arc::new(Mutex::new(client));
        let state = Arc::new(Mutex::new(PlayerState::default()));

        let manager = Self {
            audio,
            client,
            state,
            event_sender,
        };

        // Наполняем стартовыми треками
        manager.init_default_tracks();

        Ok(manager)
    }

    /// Отправляет событие в канал UI
    pub fn emit(&self, event: PlayerUiEvent) {
        let _ = self.event_sender.send_blocking(event);
    }

    /// Инициализирует стартовый набор треков для демонстрации/быстрого старта
    fn init_default_tracks(&self) {
        let default_tracks = vec![
            Track {
                id: "101".to_string(),
                title: "Midnight City".to_string(),
                artists: vec![crate::api::models::Artist {
                    id: "1".to_string(),
                    name: "M83".to_string(),
                    cover: None,
                }],
                albums: vec![crate::api::models::Album {
                    id: "1".to_string(),
                    title: Some("Hurry Up, We're Dreaming".to_string()),
                    year: Some(2011),
                    cover_uri: None,
                }],
                duration_ms: Some(243000),
                cover_uri: Some("avatars.yandex.net/get-music-content/175991/1a5cb1ad.a.872242-1/%%".to_string()),
                og_image: None,
                available: Some(true),
            },
            Track {
                id: "102".to_string(),
                title: "Star Guitar".to_string(),
                artists: vec![crate::api::models::Artist {
                    id: "2".to_string(),
                    name: "The Chemical Brothers".to_string(),
                    cover: None,
                }],
                albums: vec![crate::api::models::Album {
                    id: "2".to_string(),
                    title: Some("Come with Us".to_string()),
                    year: Some(2002),
                    cover_uri: None,
                }],
                duration_ms: Some(239000),
                cover_uri: Some("avatars.yandex.net/get-music-content/2358262/a949b251.a.100222-1/%%".to_string()),
                og_image: None,
                available: Some(true),
            },
            Track {
                id: "103".to_string(),
                title: "Resonance".to_string(),
                artists: vec![crate::api::models::Artist {
                    id: "3".to_string(),
                    name: "HOME".to_string(),
                    cover: None,
                }],
                albums: vec![crate::api::models::Album {
                    id: "3".to_string(),
                    title: Some("Odyssey".to_string()),
                    year: Some(2014),
                    cover_uri: None,
                }],
                duration_ms: Some(212000),
                cover_uri: Some("avatars.yandex.net/get-music-content/42107/8a6998ff.a.379201-1/%%".to_string()),
                og_image: None,
                available: Some(true),
            },
        ];

        if let Ok(mut state) = self.state.lock() {
            state.queue = default_tracks.clone();
            state.current_track = state.queue.first().cloned();
            state.current_index = 0;
        }

        self.emit(PlayerUiEvent::QueueUpdated(default_tracks.clone()));
        if let Some(first) = default_tracks.first() {
            self.emit(PlayerUiEvent::TrackStarted {
                track: first.clone(),
                index: 0,
                total_queue: default_tracks.len(),
            });
        }
    }

    /// Переключает воспроизведение (Play/Pause)
    pub fn toggle_play(&self) {
        let is_playing = self.audio.toggle_play();
        self.emit(PlayerUiEvent::PlaybackToggled(is_playing));
    }

    /// Запускает следующий трек
    pub fn next_track(&self) {
        let (next_track, idx, total) = {
            let mut state = self.state.lock().unwrap();
            if state.queue.is_empty() {
                return;
            }
            state.current_index = (state.current_index + 1) % state.queue.len();
            let idx = state.current_index;
            let track = state.queue[idx].clone();
            state.current_track = Some(track.clone());
            state.is_liked = false;
            state.is_disliked = false;
            state.played_seconds = 0.0;
            (track, idx, state.queue.len())
        };

        self.emit(PlayerUiEvent::LikeStateChanged(false));
        self.emit(PlayerUiEvent::DislikeStateChanged(false));
        self.emit(PlayerUiEvent::TrackStarted {
            track: next_track.clone(),
            index: idx,
            total_queue: total,
        });

        self.play_track_stream(&next_track);
    }

    /// Запускает предыдущий трек
    pub fn prev_track(&self) {
        let (prev_track, idx, total) = {
            let mut state = self.state.lock().unwrap();
            if state.queue.is_empty() {
                return;
            }
            if state.current_index == 0 {
                state.current_index = state.queue.len().saturating_sub(1);
            } else {
                state.current_index -= 1;
            }
            let idx = state.current_index;
            let track = state.queue[idx].clone();
            state.current_track = Some(track.clone());
            state.is_liked = false;
            state.is_disliked = false;
            state.played_seconds = 0.0;
            (track, idx, state.queue.len())
        };

        self.emit(PlayerUiEvent::LikeStateChanged(false));
        self.emit(PlayerUiEvent::DislikeStateChanged(false));
        self.emit(PlayerUiEvent::TrackStarted {
            track: prev_track.clone(),
            index: idx,
            total_queue: total,
        });

        self.play_track_stream(&prev_track);
    }

    /// Перемотка к заданной секунде
    pub fn seek(&self, seconds: u64) {
        self.audio.seek_seconds(seconds);
    }

    /// Установка громкости
    pub fn set_volume(&self, volume: f64) {
        self.audio.set_volume(volume);
    }

    /// Отметка лайка
    pub fn toggle_like(&self) {
        let (new_like, station_id, track_id, batch_id) = {
            let mut state = self.state.lock().unwrap();
            state.is_liked = !state.is_liked;
            if state.is_liked {
                state.is_disliked = false;
            }
            let track_id = state.current_track.as_ref().map(|t| t.id.clone());
            (state.is_liked, state.station_id.clone(), track_id, state.batch_id.clone())
        };

        self.emit(PlayerUiEvent::LikeStateChanged(new_like));
        if new_like {
            self.emit(PlayerUiEvent::DislikeStateChanged(false));
            self.emit(PlayerUiEvent::StatusMessage("Понравилось в Волне ❤️".into()));
        }

        if let Some(t_id) = track_id {
            let client = self.client.clone();
            tokio::spawn(async move {
                let client_guard = client.lock().unwrap().clone();
                if client_guard.has_token() && new_like {
                    let _ = client_guard.feedback_like(&station_id, &t_id, batch_id).await;
                }
            });
        }
    }

    /// Отметка дизлайка
    pub fn toggle_dislike(&self) {
        let (new_dislike, station_id, track_id, batch_id) = {
            let mut state = self.state.lock().unwrap();
            state.is_disliked = !state.is_disliked;
            if state.is_disliked {
                state.is_liked = false;
            }
            let track_id = state.current_track.as_ref().map(|t| t.id.clone());
            (state.is_disliked, state.station_id.clone(), track_id, state.batch_id.clone())
        };

        self.emit(PlayerUiEvent::DislikeStateChanged(new_dislike));
        if new_dislike {
            self.emit(PlayerUiEvent::LikeStateChanged(false));
            self.emit(PlayerUiEvent::StatusMessage("Исключено из потока 👎".into()));
        }

        if let Some(t_id) = track_id {
            let client = self.client.clone();
            tokio::spawn(async move {
                let client_guard = client.lock().unwrap().clone();
                if client_guard.has_token() && new_dislike {
                    let _ = client_guard.feedback_dislike(&station_id, &t_id, batch_id).await;
                }
            });
        }
    }

    /// Загружает прямую ссылку на трек и начинает воспроизведение через GStreamer
    fn play_track_stream(&self, track: &Track) {
        let client_clone = self.client.clone();
        let audio_clone = self.audio.clone();
        let track_id = track.id.clone();
        let station_id = self.state.lock().unwrap().station_id.clone();
        let batch_id = self.state.lock().unwrap().batch_id.clone();
        let event_sender = self.event_sender.clone();

        tokio::spawn(async move {
            let client = client_clone.lock().unwrap().clone();
            if client.has_token() {
                let _ = event_sender.send_blocking(PlayerUiEvent::LoadingState(true));
                match client.get_track_direct_url(&track_id).await {
                    Ok(url) => {
                        let _ = event_sender.send_blocking(PlayerUiEvent::LoadingState(false));
                        audio_clone.play_url(&url);
                        let _ = event_sender.send_blocking(PlayerUiEvent::PlaybackToggled(true));
                        let _ = client.feedback_track_started(&station_id, &track_id, batch_id).await;
                    }
                    Err(e) => {
                        let _ = event_sender.send_blocking(PlayerUiEvent::LoadingState(false));
                        let _ = event_sender.send_blocking(PlayerUiEvent::Error(format!("Ошибка потока: {}", e)));
                    }
                }
            } else {
                audio_clone.play();
                let _ = event_sender.send_blocking(PlayerUiEvent::PlaybackToggled(true));
            }
        });
    }

    /// Запрашивает новую порцию треков станции Волны через API
    pub fn fetch_wave_tracks(&self, station_tag: &str) {
        let client_clone = self.client.clone();
        let state_clone = self.state.clone();
        let event_sender = self.event_sender.clone();
        let station_tag = station_tag.to_string();
        let station_id = format!("user:onyourwave:{}", station_tag);

        tokio::spawn(async move {
            let client = client_clone.lock().unwrap().clone();
            if !client.has_token() {
                let _ = event_sender.send_blocking(PlayerUiEvent::StatusMessage(
                    format!("Фильтр: {}. Для реального потока укажите токен в настройках ⚙", station_tag)
                ));
                return;
            }

            let _ = event_sender.send_blocking(PlayerUiEvent::LoadingState(true));
            match client.get_station_tracks(&station_id).await {
                Ok(station_tracks) => {
                    let tracks: Vec<Track> = station_tracks.sequence.into_iter().map(|s| s.track).collect();
                    if let Ok(mut state) = state_clone.lock() {
                        state.batch_id = station_tracks.batch_id;
                        state.station_id = station_id;
                        if !tracks.is_empty() {
                            state.queue = tracks.clone();
                            state.current_index = 0;
                            state.current_track = state.queue.first().cloned();
                        }
                    }
                    let _ = event_sender.send_blocking(PlayerUiEvent::LoadingState(false));
                    let _ = event_sender.send_blocking(PlayerUiEvent::QueueUpdated(tracks.clone()));
                    if let Some(first) = tracks.first() {
                        let _ = event_sender.send_blocking(PlayerUiEvent::TrackStarted {
                            track: first.clone(),
                            index: 0,
                            total_queue: tracks.len(),
                        });
                    }
                    let _ = event_sender.send_blocking(PlayerUiEvent::StatusMessage(
                        format!("Поток обновлен: {}", station_tag)
                    ));
                }
                Err(e) => {
                    let _ = event_sender.send_blocking(PlayerUiEvent::LoadingState(false));
                    let _ = event_sender.send_blocking(PlayerUiEvent::Error(format!("Ошибка загрузки Волны: {}", e)));
                }
            }
        });
    }
}
