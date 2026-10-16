use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use async_channel::Sender;

use crate::api::client::YandexMusicClient;
use crate::api::models::Track;
use super::audio::AudioBackend;

/// Режим повтора: нет, один трек, вся очередь (для Волны «вся очередь» недоступна)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepeatMode {
    #[default]
    Off,
    One,
    All,
}

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
    /// Путь к скачанному превью-клипу трека для фона Волны; `None` — клипа нет или он выключен
    ClipUpdated(Option<String>),
    RepeatChanged(RepeatMode),
    /// Тип включённой Волны для плашки сверху: «По артисту: …», «По треку: …»; `None` — простая Волна или не Волна
    WaveContext(Option<String>),
    /// Реальный формат потока, например «MP3 192»
    StreamQuality(String),
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
    pub repeat: RepeatMode,
    /// Очередь из станции Ротора (бесконечная): повтор всей очереди ей не нужен
    pub wave: bool,
    /// Подпись Волны по артисту/треку (у простой Волны её нет)
    pub wave_label: Option<String>,
    /// Показан сохранённый последний трек, воспроизведение ещё не запускалось
    pub restored: bool,
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
            repeat: RepeatMode::Off,
            wave: false,
            wave_label: None,
            restored: false,
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
    /// Номер последнего запроса воспроизведения: устаревшие асинхронные задачи (быстрые клики) отбрасываются
    generation: Arc<AtomicU64>,
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
            generation: Arc::new(AtomicU64::new(0)),
        };

        // Подключаем автоматический переход к следующему треку по окончании воспроизведения
        let mgr_eos = manager.clone();
        manager.audio.set_on_eos(move || {
            mgr_eos.on_track_finished();
        });

        // Демо для скриншотов: вымышленный трек, без сети и без пользовательских данных
        if std::env::var_os("VOLNA_DEMO").is_some() {
            manager.start_demo();
            return Ok(manager);
        }

        // Вместо заглушки показываем последний проигранный трек (звук не включается)
        manager.restore_last_track();

        // Проверяем статус авторизации при старте
        manager.check_auth_on_startup();

        Ok(manager)
    }

    /// Режим `VOLNA_DEMO=1|2|3` для скриншотов README: вымышленный исполнитель, свои тексты,
    /// обложка из `VOLNA_DEMO_COVER`, позиция из `VOLNA_DEMO_POS_MS`. Ни токена, ни сети, ни кэша пользователя
    fn start_demo(&self) {
        let n: usize = std::env::var("VOLNA_DEMO").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
        let (title, artist, accent, average, wave_text) = match n {
            2 => ("Neon Rain", "Night Courier", "#c95cff", "#5a2a7a", "#f0c8ff"),
            3 => ("Slow Tide", "Harbor Lights", "#3fa8ff", "#1f4a73", "#bfe3ff"),
            _ => ("Golden Hour", "Demo Artist", "#ff8a3d", "#7a3a14", "#ffd9b8"),
        };
        let track: Track = serde_json::from_value(serde_json::json!({
            "id": "demo", "title": title, "durationMs": 214_000,
            "artists": [{"id": "1", "name": artist}],
            "albums": [{"id": "1", "title": "Sample Album", "year": 2026}],
            "derivedColors": {"average": average, "accent": accent, "miniPlayer": accent, "waveText": wave_text}
        })).expect("демо-трек");
        {
            let mut state = self.state.lock().unwrap();
            state.queue = vec![track.clone()];
            state.current_track = Some(track.clone());
        }
        let lrc = [
            "Evening folds over the harbor wall", "Lamps come on one at a time",
            "We walk the long way home", "Counting the ships by their light",
            "Nothing is asked of us now", "The water keeps the time",
            "Hold the last of the day", "Let it burn down slow",
            "Carry it into the dark", "And we will know the way",
        ]
        .iter()
        .enumerate()
        .map(|(i, l)| format!("[{:02}:{:02}.00] {}", i * 4 / 60, i * 4 % 60, l))
        .collect::<Vec<_>>()
        .join("\n");
        self.emit(PlayerUiEvent::UserLoggedIn("demo".into()));
        self.emit(PlayerUiEvent::TrackStarted { track, index: 0, total_queue: 1 });
        if let Ok(path) = std::env::var("VOLNA_DEMO_COVER") {
            self.emit(PlayerUiEvent::CoverUpdated(path));
        }
        if let Ok(path) = std::env::var("VOLNA_DEMO_CLIP") {
            self.emit(PlayerUiEvent::ClipUpdated(Some(path)));
        }
        self.emit(PlayerUiEvent::StreamQuality("MP3 192".into()));
        self.emit(PlayerUiEvent::LyricsUpdated(lrc));
        self.emit(PlayerUiEvent::PlaybackToggled(true));
    }

    /// Показывает последний проигранный трек: заголовок, цвета, обложку и текст. Воспроизведение не стартует
    fn restore_last_track(&self) {
        let Some(track) = crate::api::history::load_last_track() else { return };
        {
            let mut state = self.state.lock().unwrap();
            state.queue = vec![track.clone()];
            state.current_index = 0;
            state.current_track = Some(track.clone());
            state.restored = true;
        }
        self.emit(PlayerUiEvent::TrackStarted { track: track.clone(), index: 0, total_queue: 1 });
        let client = self.client.lock().unwrap().clone();
        spawn_cover_and_lyrics(&self.event_senders, client, &track, self.generation.load(Ordering::SeqCst), &self.generation);
    }

    /// Если на экране сохранённый трек, первое нажатие Play/вперёд/назад запускает его (с продолжением Волной по треку)
    fn resume_restored(&self) -> bool {
        let track = {
            let mut state = self.state.lock().unwrap();
            if !state.restored {
                return false;
            }
            state.restored = false;
            state.current_track.clone()
        };
        if let Some(track) = track {
            self.play_single_track(track);
        }
        true
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
        if self.resume_restored() {
            return;
        }
        let has_tracks = {
            let state = self.state.lock().unwrap();
            !state.queue.is_empty()
        };

        if !has_tracks {
            let has_token = self.client.lock().unwrap().has_token();
            if !has_token {
                self.emit(PlayerUiEvent::AuthRequired);
                self.emit(PlayerUiEvent::StatusMessage(
                    "Для запуска Волны выполните вход через Яндекс ID".to_string(),
                ));
            } else {
                self.emit(PlayerUiEvent::StatusMessage(
                    "Загружаем персональную Волну... 🌊".to_string(),
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

        let (mode, wave, at_end, current) = {
            let state = self.state.lock().unwrap();
            (state.repeat, state.wave, state.current_index + 1 >= state.queue.len(), state.current_track.clone())
        };
        match (mode, current) {
            (RepeatMode::One, Some(track)) => {
                self.play_track_stream(&track);
                return;
            }
            // обычная очередь без повтора на последнем треке заканчивается; Волна идёт бесконечно
            (RepeatMode::Off, _) if !wave && at_end => {
                self.emit(PlayerUiEvent::PlaybackToggled(false));
                return;
            }
            _ => {}
        }
        self.next_track();
    }

    /// Переключает повтор по кругу: нет → один трек → вся очередь (кроме Волны) → нет
    pub fn cycle_repeat(&self) {
        let next = {
            let mut state = self.state.lock().unwrap();
            state.repeat = next_repeat(state.repeat, state.wave);
            state.repeat
        };
        self.emit(PlayerUiEvent::RepeatChanged(next));
    }

    /// Запоминает, что играет: станция Ротора или обычная очередь; для Волны повтор очереди сбрасывается
    fn set_wave(&self, wave: bool) {
        let reset = {
            let mut state = self.state.lock().unwrap();
            state.wave = wave;
            if !wave {
                state.wave_label = None;
            }
            let reset = wave && state.repeat == RepeatMode::All;
            if reset {
                state.repeat = RepeatMode::Off;
            }
            reset
        };
        if reset {
            self.emit(PlayerUiEvent::RepeatChanged(RepeatMode::Off));
        }
        if !wave {
            self.emit(PlayerUiEvent::WaveContext(None));
        }
    }

    /// Запоминает и показывает подпись Волны по артисту/треку
    fn set_wave_label(&self, label: Option<String>) {
        self.state.lock().unwrap().wave_label = label.clone();
        self.emit(PlayerUiEvent::WaveContext(label));
    }

    /// Запускает следующий трек
    pub fn next_track(&self) {
        if self.resume_restored() {
            return;
        }
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
                    let new_tracks = dedup_tracks(&state.queue, new_tracks);
                    state.queue.extend(new_tracks);
                    state.queue.clone()
                };

                emit_all(&senders, PlayerUiEvent::QueueUpdated(updated_queue));
            }
        });
    }

    /// Запускает предыдущий трек
    pub fn prev_track(&self) {
        if self.resume_restored() {
            return;
        }
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

    /// Воспроизводит трек и продолжает «Моей волной по этому треку»
    pub fn play_single_track(&self, track: Track) {
        let station = format!("track:{}", track.id);
        let label = format!("По треку: {}", track.title);
        self.start_station(station, Some(track), label);
    }

    /// Запускает «Волну по артисту»
    pub fn play_artist_wave(&self, artist_id: &str, artist_name: &str) {
        self.start_station(format!("artist:{}", artist_id), None, format!("По артисту: {}", artist_name));
    }

    /// Запускает станцию Ротора (`track:ID`, `artist:ID`). Если задан `first`, он играет сразу,
    /// а треки станции дописываются в очередь следом.
    fn start_station(&self, station_id: String, first: Option<Track>, label: String) {
        {
            let mut state = self.state.lock().unwrap();
            state.station_id = station_id.clone();
            state.batch_id = None;
            state.current_mood = None;
            state.current_language = None;
            state.current_character = None;
            if let Some(t) = &first {
                state.queue = vec![t.clone()];
                state.current_index = 0;
                state.current_track = Some(t.clone());
                state.is_liked = false;
                state.is_disliked = false;
            }
        }
        self.set_wave(true);
        self.set_wave_label(Some(label.clone()));

        match &first {
            Some(t) => {
                self.emit(PlayerUiEvent::TrackStarted { track: t.clone(), index: 0, total_queue: 1 });
                self.play_track_stream(t);
            }
            None => self.emit(PlayerUiEvent::LoadingState(true)),
        }

        let me = self.clone();
        tokio::spawn(async move {
            let client = me.client.lock().unwrap().clone();
            if !client.has_token() {
                me.emit(PlayerUiEvent::AuthRequired);
                return;
            }
            let result = client.get_station_tracks(&station_id).await;
            if first.is_none() {
                me.emit(PlayerUiEvent::LoadingState(false));
            }
            match result {
                Ok(station) => {
                    let mut tracks: Vec<Track> = station.sequence.into_iter().map(|s| s.track).collect();
                    tracks = dedup_tracks(&[], tracks);
                    // за время загрузки пользователь мог запустить другую станцию
                    if me.state.lock().unwrap().station_id != station_id {
                        return;
                    }
                    me.state.lock().unwrap().batch_id = station.batch_id;
                    match &first {
                        Some(seed) => {
                            let queue = {
                                let mut state = me.state.lock().unwrap();
                                let tracks = dedup_tracks(std::slice::from_ref(seed), tracks);
                                let tracks = dedup_tracks(&state.queue, tracks);
                                state.queue.extend(tracks);
                                state.queue.clone()
                            };
                            me.emit(PlayerUiEvent::QueueUpdated(queue));
                        }
                        None => {
                            me.set_queue_and_play(tracks, 0);
                            me.set_wave(true);
                            me.set_wave_label(Some(label));
                        }
                    }
                }
                Err(e) => me.emit(PlayerUiEvent::Error(format!("Ошибка загрузки Волны: {}", e))),
            }
        });
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

        self.set_wave(false);
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
        let audio_clone = self.audio.clone();
        let track_id = track.id.clone();
        let station_id = self.state.lock().unwrap().station_id.clone();
        let batch_id = self.state.lock().unwrap().batch_id.clone();
        let senders = self.event_senders.clone();
        let epoch = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let current_gen = self.generation.clone();
        let is_stale = move || current_gen.load(Ordering::SeqCst) != epoch;

        let client = self.client.lock().unwrap().clone();
        spawn_cover_and_lyrics(&self.event_senders, client.clone(), track, epoch, &self.generation);
        crate::api::history::save_last_track(track);
        self.state.lock().unwrap().restored = false;

        tokio::spawn(async move {
            if client.has_token() {
                emit_all(&senders, PlayerUiEvent::LoadingState(true));
                let result = client.get_track_direct_url(&track_id).await;
                if is_stale() {
                    return;
                }
                match result {
                    Ok((url, label)) => {
                        emit_all(&senders, PlayerUiEvent::LoadingState(false));
                        emit_all(&senders, PlayerUiEvent::StreamQuality(label));
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
            state.restored = false;
        }
        self.set_wave(true);
        // плашку с выбранным фильтром рисует сам интерфейс; для простой Волны плашка убирается
        if mood.is_none() && language.is_none() && character.is_none() {
            self.set_wave_label(None);
        } else {
            self.state.lock().unwrap().wave_label = None;
        }

        let audio_clone = self.audio.clone();
        let epoch = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let current_gen = self.generation.clone();

        tokio::spawn(async move {
            let client = client_clone.lock().unwrap().clone();
            if !client.has_token() {
                emit_all(&senders, PlayerUiEvent::AuthRequired);
                emit_all(&senders, PlayerUiEvent::StatusMessage(
                    "Войдите в Яндекс ID для запуска персональной Волны".to_string(),
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
                    let tracks = dedup_tracks(&[], station_tracks.sequence.into_iter().map(|s| s.track).collect());
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

                        // Обложка и текст первого трека Волны (в остальных случаях их грузит play_track_stream)
                        spawn_cover_and_lyrics(&senders, client.clone(), &track, epoch, &current_gen);
                        crate::api::history::save_last_track(&track);

                        // Сразу запускаем воспроизведение первого трека Волны
                        let track_id = track.id.clone();
                        let batch_id = station_tracks.batch_id;
                        let result = client.get_track_direct_url(&track_id).await;
                        if current_gen.load(Ordering::SeqCst) != epoch {
                            return;
                        }
                        match result {
                            Ok((url, label)) => {
                                emit_all(&senders, PlayerUiEvent::StreamQuality(label));
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

/// Обложка и текст песни грузятся отдельными задачами, чтобы не задерживать воспроизведение.
/// Если у трека текста нет, UI получает пустую строку: старый текст предыдущего трека не остаётся.
fn spawn_cover_and_lyrics(
    senders: &[Sender<PlayerUiEvent>],
    client: YandexMusicClient,
    track: &Track,
    epoch: u64,
    generation: &Arc<AtomicU64>,
) {
    if let Some(cover_url) = track.cover_url(400) {
        let (senders, generation) = (senders.to_vec(), generation.clone());
        tokio::spawn(async move {
            if let Some(path) = crate::cache::fetch_cover_cached(&cover_url).await {
                if generation.load(Ordering::SeqCst) == epoch {
                    emit_all(&senders, PlayerUiEvent::CoverUpdated(path.to_string_lossy().to_string()));
                }
            }
        });
    }

    if crate::api::auth::load_clips() {
        let (senders, generation, client, track_id) = (senders.to_vec(), generation.clone(), client.clone(), track.id.clone());
        tokio::spawn(async move {
            let path = match client.get_track_clip_url(&track_id).await {
                Some(url) => crate::cache::fetch_clip_cached(&url).await,
                None => None,
            };
            if generation.load(Ordering::SeqCst) == epoch {
                emit_all(&senders, PlayerUiEvent::ClipUpdated(path.map(|p| p.to_string_lossy().to_string())));
            }
        });
    }

    let (senders, generation) = (senders.to_vec(), generation.clone());
    let (track_id, title, artist) = (track.id.clone(), track.title.clone(), track.artists_string());
    let duration = track.duration_ms.map(|d| d / 1000);
    tokio::spawn(async move {
        if !client.has_token() {
            return;
        }
        // без ограничения по времени зависший запрос навсегда оставлял бы экран текста пустым
        let lookup = client.get_track_lyrics(&track_id, &artist, &title, duration);
        let text = tokio::time::timeout(std::time::Duration::from_secs(12), lookup)
            .await
            .ok()
            .and_then(|r| r.ok())
            .flatten()
            .unwrap_or_default();
        if generation.load(Ordering::SeqCst) == epoch {
            emit_all(&senders, PlayerUiEvent::LyricsUpdated(text));
        }
    });
}

/// Следующий режим повтора по кругу; «вся очередь» пропускается для Волны
fn next_repeat(mode: RepeatMode, wave: bool) -> RepeatMode {
    match (mode, wave) {
        (RepeatMode::Off, _) => RepeatMode::One,
        (RepeatMode::One, false) => RepeatMode::All,
        _ => RepeatMode::Off,
    }
}

/// Один и тот же трек в Яндексе бывает в нескольких альбомах (сингл, сборка) под разными id:
/// узнаём его по названию и исполнителям
fn track_key(t: &Track) -> String {
    let artists: Vec<String> = t.artists.iter().map(|a| a.name.trim().to_lowercase()).collect();
    format!("{}|{}", t.title.trim().to_lowercase(), artists.join(","))
}

/// Оставляет в `new` только треки, которых ещё нет в `existing` и среди самих `new`
fn dedup_tracks(existing: &[Track], new: Vec<Track>) -> Vec<Track> {
    let mut seen: std::collections::HashSet<String> = existing.iter().map(track_key).collect();
    new.into_iter().filter(|t| seen.insert(track_key(t))).collect()
}

/// Рассылает событие во все переданные каналы
fn emit_all(senders: &[Sender<PlayerUiEvent>], event: PlayerUiEvent) {
    for s in senders {
        let _ = s.send_blocking(event.clone());
    }
}

#[cfg(test)]
mod dedup_tests {
    use super::*;

    fn track(id: &str, title: &str, artist: &str) -> Track {
        serde_json::from_value(serde_json::json!({
            "id": id, "title": title, "artists": [{"id": "1", "name": artist}]
        })).unwrap()
    }

    #[test]
    fn same_song_from_other_album_is_dropped() {
        let queue = vec![track("1", "Firework", "Katy Perry")];
        let new = vec![track("2", "Firework ", "katy perry"), track("3", "Roar", "Katy Perry"), track("4", "Roar", "Katy Perry")];
        let kept: Vec<_> = dedup_tracks(&queue, new).into_iter().map(|t| t.id).collect();
        assert_eq!(kept, ["3"]);
    }
}

#[cfg(test)]
mod repeat_tests {
    use super::*;

    #[test]
    fn repeat_cycle_skips_all_for_wave() {
        let cycle = |wave| {
            let (mut m, mut seen) = (RepeatMode::Off, vec![]);
            for _ in 0..3 {
                m = next_repeat(m, wave);
                seen.push(m);
            }
            seen
        };
        assert_eq!(cycle(false), [RepeatMode::One, RepeatMode::All, RepeatMode::Off]);
        assert_eq!(cycle(true), [RepeatMode::One, RepeatMode::Off, RepeatMode::One]);
    }
}
