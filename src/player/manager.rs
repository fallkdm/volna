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
    CoverUpdated(String),
    LyricsUpdated(String),
    LoadingState(bool),
    AuthRequired,
    UserLoggedIn(String),
    UserLoggedOut,
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
    pub current_mood: Option<String>,
    pub current_language: Option<String>,
    pub current_character: Option<String>,
    pub user_login: Option<String>,
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
            current_mood: None,
            current_language: None,
            current_character: None,
            user_login: None,
        }
    }
}

/// Менеджер плеера
#[derive(Clone)]
pub struct PlayerManager {
    pub audio: Arc<AudioBackend>,
    pub client: Arc<Mutex<YandexMusicClient>>,
    pub state: Arc<Mutex<PlayerState>>,
    pub event_senders: Vec<Sender<PlayerUiEvent>>,
}

impl PlayerManager {
    /// Создает новый экземпляр менеджера
    pub fn new(
        client: YandexMusicClient,
        event_senders: Vec<Sender<PlayerUiEvent>>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let audio = Arc::new(AudioBackend::new()?);
        let client = Arc::new(Mutex::new(client));
        let state = Arc::new(Mutex::new(PlayerState::default()));

        let manager = Self {
            audio,
            client,
            state,
            event_senders,
        };

        // Подключаем автоматический переход к следующему треку по окончании воспроизведения
        let mgr_eos = manager.clone();
        manager.audio.set_on_eos(move || {
            mgr_eos.on_track_finished();
        });

        // Проверяем статус авторизации при старте
        manager.check_auth_on_startup();

        Ok(manager)
    }

    /// Отправляет событие во все зарегистрированные каналы (UI, MPRIS)
    pub fn emit(&self, event: PlayerUiEvent) {
        for s in &self.event_senders {
            let _ = s.send_blocking(event.clone());
        }
    }

    /// Проверяет наличие токена при старте и запрашивает профиль пользователя
    fn check_auth_on_startup(&self) {
        let client_clone = self.client.clone();
        let state_clone = self.state.clone();
        let senders = self.event_senders.clone();

        tokio::spawn(async move {
            let client = client_clone.lock().unwrap().clone();
            if client.has_token() {
                match client.get_account_status().await {
                    Ok(acc) => {
                        let login = acc.account.as_ref().and_then(|a| a.login.clone())
                            .or_else(|| acc.account.as_ref().and_then(|a| a.full_name.clone()))
                            .unwrap_or_else(|| "Яндекс Пользователь".to_string());

                        if let Ok(mut state) = state_clone.lock() {
                            state.user_login = Some(login.clone());
                        }

                        emit_all(&senders, PlayerUiEvent::UserLoggedIn(login.clone()));
                        emit_all(&senders, PlayerUiEvent::StatusMessage(
                            format!("С возвращением, {}! Нажмите «Включить волну» 🌊", login)
                        ));
                    }
                    Err(e) => {
                        eprintln!("Ошибка проверки аккаунта: {}", e);
                        emit_all(&senders, PlayerUiEvent::AuthRequired);
                    }
                }
            } else {
                emit_all(&senders, PlayerUiEvent::AuthRequired);
            }
        });
    }

    /// Запускает или переключает воспроизведение (Play/Pause)
    pub fn toggle_play(&self) {
        let has_tracks = {
            let state = self.state.lock().unwrap();
            !state.queue.is_empty()
        };

        if !has_tracks {
            let has_token = self.client.lock().unwrap().has_token();
            if !has_token {
                self.emit(PlayerUiEvent::AuthRequired);
                self.emit(PlayerUiEvent::StatusMessage(
                    "Для запуска Моей волны выполните вход через Яндекс ID".to_string(),
                ));
            } else {
                self.emit(PlayerUiEvent::StatusMessage(
                    "Загружаем персональную Мою волну... 🌊".to_string(),
                ));
                self.fetch_wave_tracks("onyourwave");
            }
            return;
        }

        let is_playing = self.audio.toggle_play();
        self.emit(PlayerUiEvent::PlaybackToggled(is_playing));
    }

    /// Обработчик естественного окончания воспроизведения трека (EndOfStream)
    pub fn on_track_finished(&self) {
        let (station_id, track_id, total_secs, batch_id) = {
            let state = self.state.lock().unwrap();
            let track_id = state.current_track.as_ref().map(|t| t.id.clone());
            let total_secs = state
                .current_track
                .as_ref()
                .and_then(|t| t.duration_ms)
                .map(|ms| ms as f64 / 1000.0)
                .unwrap_or(state.played_seconds);
            (
                state.station_id.clone(),
                track_id,
                total_secs,
                state.batch_id.clone(),
            )
        };

        if let Some(t_id) = track_id {
            let client = self.client.clone();
            tokio::spawn(async move {
                let client_guard = client.lock().unwrap().clone();
                if client_guard.has_token() {
                    let _ = client_guard
                        .feedback_track_finished(&station_id, &t_id, total_secs, batch_id)
                        .await;
                }
            });
        }

        self.next_track();
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

        // Если в очереди осталось менее 2 треков, подгружаем следующую пачку станции Ротора
        let remaining = total.saturating_sub(idx + 1);
        if remaining < 2 {
            self.load_more_wave_tracks();
        }

        self.emit(PlayerUiEvent::LikeStateChanged(false));
        self.emit(PlayerUiEvent::DislikeStateChanged(false));
        self.emit(PlayerUiEvent::TrackStarted {
            track: next_track.clone(),
            index: idx,
            total_queue: total,
        });

        self.play_track_stream(&next_track);
    }

    /// Подгружает следующую порцию треков Волны и добавляет в хвост очереди
    pub fn load_more_wave_tracks(&self) {
        let client_clone = self.client.clone();
        let state_clone = self.state.clone();
        let senders = self.event_senders.clone();

        let (station_id, mood, language, character) = {
            let state = self.state.lock().unwrap();
            (
                state.station_id.clone(),
                state.current_mood.clone(),
                state.current_language.clone(),
                state.current_character.clone(),
            )
        };

        tokio::spawn(async move {
            let client = client_clone.lock().unwrap().clone();
            if !client.has_token() {
                return;
            }

            if let Ok(station_tracks) = client
                .get_station_tracks_with_params(
                    &station_id,
                    mood.as_deref(),
                    language.as_deref(),
                    character.as_deref(),
                )
                .await
            {
                let new_tracks: Vec<Track> =
                    station_tracks.sequence.into_iter().map(|s| s.track).collect();
                if new_tracks.is_empty() {
                    return;
                }

                let updated_queue = {
                    let mut state = state_clone.lock().unwrap();
                    state.batch_id = station_tracks.batch_id;
                    state.queue.extend(new_tracks);
                    state.queue.clone()
                };

                emit_all(&senders, PlayerUiEvent::QueueUpdated(updated_queue));
            }
        });
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

    /// Воспроизводит конкретный трек (например, из поиска)
    pub fn play_single_track(&self, track: Track) {
        {
            let mut state = self.state.lock().unwrap();
            state.queue.insert(0, track.clone());
            state.current_index = 0;
            state.current_track = Some(track.clone());
            state.is_liked = false;
            state.is_disliked = false;
        }

        self.emit(PlayerUiEvent::TrackStarted {
            track: track.clone(),
            index: 0,
            total_queue: 1,
        });
        self.play_track_stream(&track);
    }

    /// Задает новую очередь треков и запускает воспроизведение
    pub fn set_queue_and_play(&self, tracks: Vec<Track>, start_index: usize) {
        if tracks.is_empty() {
            return;
        }
        let index = start_index.min(tracks.len() - 1);
        let track = tracks[index].clone();

        {
            let mut state = self.state.lock().unwrap();
            state.queue = tracks.clone();
            state.current_index = index;
            state.current_track = Some(track.clone());
            state.is_liked = false;
            state.is_disliked = false;
        }

        self.emit(PlayerUiEvent::QueueUpdated(tracks.clone()));
        self.emit(PlayerUiEvent::TrackStarted {
            track: track.clone(),
            index,
            total_queue: tracks.len(),
        });
        self.play_track_stream(&track);
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

    /// Выход из аккаунта
    pub fn logout(&self) {
        let _ = crate::api::auth::clear_token();
        self.audio.pause();
        if let Ok(mut c) = self.client.lock() {
            c.set_token(None);
        }
        if let Ok(mut state) = self.state.lock() {
            state.user_login = None;
            state.queue.clear();
            state.current_track = None;
            state.current_index = 0;
        }
        self.emit(PlayerUiEvent::PlaybackToggled(false));
        self.emit(PlayerUiEvent::UserLoggedOut);
        self.emit(PlayerUiEvent::StatusMessage("Вы вышли из аккаунта Яндекс Музыки".into()));
    }

    /// Загружает прямую ссылку на трек, кэширует обложку и начинает воспроизведение через GStreamer
    fn play_track_stream(&self, track: &Track) {
        let client_clone = self.client.clone();
        let audio_clone = self.audio.clone();
        let track_id = track.id.clone();
        let cover_url = track.cover_url(400);
        let station_id = self.state.lock().unwrap().station_id.clone();
        let batch_id = self.state.lock().unwrap().batch_id.clone();
        let senders = self.event_senders.clone();

        tokio::spawn(async move {
            // Кэшируем обложку трека
            if let Some(c_url) = cover_url {
                if let Some(cached_path) = crate::cache::fetch_cover_cached(&c_url).await {
                    emit_all(&senders, PlayerUiEvent::CoverUpdated(
                        cached_path.to_string_lossy().to_string(),
                    ));
                }
            }

            // Запрашиваем текст песни (Lyrics)
            let client = client_clone.lock().unwrap().clone();
            if client.has_token() {
                if let Ok(Some(lyrics)) = client.get_track_lyrics(&track_id).await {
                    emit_all(&senders, PlayerUiEvent::LyricsUpdated(lyrics));
                }
            }

            if client.has_token() {
                emit_all(&senders, PlayerUiEvent::LoadingState(true));
                match client.get_track_direct_url(&track_id).await {
                    Ok(url) => {
                        emit_all(&senders, PlayerUiEvent::LoadingState(false));
                        audio_clone.play_url(&url);
                        emit_all(&senders, PlayerUiEvent::PlaybackToggled(true));
                        let _ = client.feedback_track_started(&station_id, &track_id, batch_id).await;
                    }
                    Err(e) => {
                        emit_all(&senders, PlayerUiEvent::LoadingState(false));
                        emit_all(&senders, PlayerUiEvent::Error(format!("Ошибка получения потока: {}", e)));
                    }
                }
            } else {
                emit_all(&senders, PlayerUiEvent::AuthRequired);
                emit_all(&senders, PlayerUiEvent::StatusMessage(
                    "Требуется авторизация для воспроизведения".to_string(),
                ));
            }
        });
    }

    /// Запрашивает новую порцию треков Волны с заданным тегом/фильтром
    pub fn fetch_wave_tracks(&self, station_tag: &str) {
        let (mood, lang, charac) = match station_tag {
            "energetic" => (Some("energetic"), None, None),
            "calm" => (Some("calm"), None, None),
            "russian" => (None, Some("russian"), None),
            "discover" => (None, None, Some("discover")),
            "party" => (Some("fun"), None, None),
            _ => (None, None, None),
        };
        self.apply_rotor_filter(
            mood.map(|s| s.to_string()),
            lang.map(|s| s.to_string()),
            charac.map(|s| s.to_string()),
        );
    }

    /// Применяет фильтры Ротора (настроение, язык, характер) и перестраивает поток Волны
    pub fn apply_rotor_filter(
        &self,
        mood: Option<String>,
        language: Option<String>,
        character: Option<String>,
    ) {
        let client_clone = self.client.clone();
        let state_clone = self.state.clone();
        let senders = self.event_senders.clone();
        let station_id = "user:onyourwave".to_string();

        {
            let mut state = self.state.lock().unwrap();
            state.current_mood = mood.clone();
            state.current_language = language.clone();
            state.current_character = character.clone();
            state.station_id = station_id.clone();
        }

        let audio_clone = self.audio.clone();

        tokio::spawn(async move {
            let client = client_clone.lock().unwrap().clone();
            if !client.has_token() {
                emit_all(&senders, PlayerUiEvent::AuthRequired);
                emit_all(&senders, PlayerUiEvent::StatusMessage(
                    "Войдите в Яндекс ID для запуска персональной Моей волны".to_string(),
                ));
                return;
            }

            emit_all(&senders, PlayerUiEvent::LoadingState(true));
            match client.get_station_tracks_with_params(
                &station_id,
                mood.as_deref(),
                language.as_deref(),
                character.as_deref(),
            ).await {
                Ok(station_tracks) => {
                    let tracks: Vec<Track> = station_tracks.sequence.into_iter().map(|s| s.track).collect();
                    let first_track = tracks.first().cloned();

                    if let Ok(mut state) = state_clone.lock() {
                        state.batch_id = station_tracks.batch_id.clone();
                        if !tracks.is_empty() {
                            state.queue = tracks.clone();
                            state.current_index = 0;
                            state.current_track = first_track.clone();
                        }
                    }
                    emit_all(&senders, PlayerUiEvent::LoadingState(false));
                    emit_all(&senders, PlayerUiEvent::QueueUpdated(tracks.clone()));

                    if let Some(track) = first_track {
                        emit_all(&senders, PlayerUiEvent::TrackStarted {
                            track: track.clone(),
                            index: 0,
                            total_queue: tracks.len(),
                        });

                        // Сразу запускаем воспроизведение первого трека Волны
                        let track_id = track.id.clone();
                        let batch_id = station_tracks.batch_id;
                        match client.get_track_direct_url(&track_id).await {
                            Ok(url) => {
                                audio_clone.play_url(&url);
                                emit_all(&senders, PlayerUiEvent::PlaybackToggled(true));
                                let _ = client.feedback_track_started(&station_id, &track_id, batch_id).await;
                            }
                            Err(e) => {
                                emit_all(&senders, PlayerUiEvent::Error(format!("Ошибка старта Волны: {}", e)));
                            }
                        }
                    } else {
                        emit_all(&senders, PlayerUiEvent::StatusMessage(
                            "В станции ротора нет треков".to_string(),
                        ));
                    }
                }
                Err(e) => {
                    emit_all(&senders, PlayerUiEvent::LoadingState(false));
                    emit_all(&senders, PlayerUiEvent::Error(format!("Ошибка загрузки Волны: {}", e)));
                }
            }
        });
    }
}

/// Рассылает событие во все переданные каналы
fn emit_all(senders: &[Sender<PlayerUiEvent>], event: PlayerUiEvent) {
    for s in senders {
        let _ = s.send_blocking(event.clone());
    }
}
