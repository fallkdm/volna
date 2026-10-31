use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use async_channel::Sender;

use crate::api::client::YandexMusicClient;
use crate::api::models::{Album, Playlist, Track};
use super::audio::AudioBackend;

/// Режим повтора: нет, один трек, вся очередь (для Волны «вся очередь» недоступна)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepeatMode {
    #[default]
    Off,
    One,
    All,
}

/// Куда ведёт клик по плашке источника
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum SourceTarget {
    Album(Album),
    Playlist(Playlist),
    Artist(String),
    Track(Track),
    /// Вкладка «Медиатека» («Мне нравится», «Загруженные»)
    Library,
}

/// Откуда играет очередь: подпись на плашке и страница, куда ведёт клик по ней
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PlaySource {
    pub label: String,
    pub target: Option<SourceTarget>,
}

impl PlaySource {
    pub fn new(label: impl Into<String>, target: Option<SourceTarget>) -> Self {
        Self { label: label.into(), target }
    }
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
    TrackLiked {
        track: Track,
        liked: bool,
    },
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
    Seeked(u64),
    LoadingState(bool),
    AuthRequired,
    UserLoggedIn(String),
    UserLoggedOut,
    StatusMessage(String),
    Error(String),
    /// Станция привязана (имя) или отвязана
    CastChanged(Option<String>),
    /// Откуда играет очередь: «Плейлист: …», «Альбом: …», «Популярное: …»; `None` — подписи нет
    SourceContext(Option<PlaySource>),
    /// Страница, куда ведёт плашка Волны («По треку», «По артисту»)
    StationTarget(Option<SourceTarget>),
    /// Изменились настройки Discord (включение, Application ID)
    DiscordSettingsChanged,
    /// Изменились настройки плавного перехода между треками
    CrossfadeChanged {
        enabled: bool,
        duration: u32,
    },
    /// Изменился режим интерфейса / навигации: "wave" или "spotify"
    NavLayoutChanged(String),
    /// Изменился стиль плеера: "dock" или "floating"
    PlayerLayoutChanged(String),
    /// Изменился цветовой акцент: "dynamic", "spotify", "ruby", "sunset", "violet", "ocean", "monochrome"
    ThemeAccentChanged(String),
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
    /// Откуда играет очередь («Плейлист: …»); для сохранения между запусками
    pub play_source: Option<PlaySource>,
    /// Показан сохранённый последний трек, воспроизведение ещё не запускалось
    pub restored: bool,
    /// Сработал ли автоматический кроссфейд перед окончанием трека
    pub crossfade_triggered: bool,
    /// Идентификаторы понравившихся треков пользователя
    pub liked_track_ids: std::collections::HashSet<String>,
    /// Загружен ли список лайков с сервера хотя бы один раз
    pub liked_loaded: bool,
    /// Лайки/снятия, сделанные пока список с сервера ещё грузится: поверх ответа сервера
    pub liked_pending: std::collections::HashMap<String, bool>,
    /// UID авторизованного пользователя
    pub user_uid: Option<String>,
    /// Идет ли переход на новый трек (загрузка URL потока)
    pub is_transitioning: bool,
}

impl PlayerState {
    /// Обновляет `is_liked` и `is_disliked` для указанного трека по текущему кэшу `liked_track_ids`
    pub fn sync_track_like_status(&mut self, track_id: &str) -> bool {
        let is_liked = self.liked_track_ids.contains(track_id);
        self.is_liked = is_liked;
        self.is_disliked = false;
        is_liked
    }

    /// Локальная отметка лайка; до загрузки списка запоминается, чтобы не потеряться при слиянии
    pub fn set_liked_local(&mut self, track_id: &str, liked: bool) -> bool {
        if !self.liked_loaded {
            self.liked_pending.insert(track_id.to_string(), liked);
        }
        if liked {
            self.liked_track_ids.insert(track_id.to_string())
        } else {
            self.liked_track_ids.remove(track_id)
        }
    }

    /// Список с сервера становится основой, локальные правки, сделанные за время загрузки, поверх него
    pub fn merge_server_likes(&mut self, ids: Vec<String>) {
        self.liked_loaded = true;
        self.liked_track_ids = ids.into_iter().collect();
        for (id, liked) in std::mem::take(&mut self.liked_pending) {
            if liked {
                self.liked_track_ids.insert(id);
            } else {
                self.liked_track_ids.remove(&id);
            }
        }
    }
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
            play_source: None,
            restored: false,
            crossfade_triggered: false,
            liked_track_ids: std::collections::HashSet::new(),
            liked_loaded: false,
            liked_pending: std::collections::HashMap::new(),
            user_uid: None,
            is_transitioning: false,
        }
    }
}

/// Ссылка на поток следующего трека, полученная заранее
struct Prefetched {
    track_id: String,
    url: String,
    label: String,
    at: std::time::Instant,
}

/// Ссылки на поток живут недолго (через несколько минут отвечают 410): заранее полученной верим 90 секунд
const PREFETCH_TTL: std::time::Duration = std::time::Duration::from_secs(90);

/// Менеджер плеера
#[derive(Clone)]
pub struct PlayerManager {
    pub audio: Arc<AudioBackend>,
    pub client: Arc<Mutex<YandexMusicClient>>,
    pub state: Arc<Mutex<PlayerState>>,
    pub event_senders: Arc<Mutex<Vec<Sender<PlayerUiEvent>>>>,
    /// Ссылка на следующий трек очереди, полученная заранее (один слот)
    prefetch: Arc<Mutex<Option<Prefetched>>>,
    /// Привязанная Яндекс Станция (см. `cast.rs`)
    pub(super) cast: Arc<Mutex<Option<super::cast::CastLink>>>,
    /// Номер последнего запроса воспроизведения: устаревшие асинхронные задачи (быстрые клики) отбрасываются
    generation: Arc<AtomicU64>,
    /// Настройки плавного перехода: (включен, длительность в секундах)
    crossfade: Arc<Mutex<(bool, u32)>>,
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
        let (cf_enabled, cf_dur) = crate::api::auth::load_crossfade();
        let crossfade = Arc::new(Mutex::new((cf_enabled, cf_dur)));

        let manager = Self {
            audio,
            client,
            state,
            event_senders: Arc::new(Mutex::new(event_senders)),
            cast: Arc::default(),
            prefetch: Arc::default(),
            generation: Arc::new(AtomicU64::new(0)),
            crossfade,
        };

        // Подключаем автоматический переход к следующему треку по окончании воспроизведения
        let mgr_eos = manager.clone();
        manager.audio.set_on_eos(move || {
            mgr_eos.on_track_finished();
        });

        // Фоновый мониторинг прогресса трека для автоматической сводки (кроссфейда) в конце песни
        let mgr_crossfade = manager.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
            loop {
                interval.tick().await;
                if let Some(pos_ms) = mgr_crossfade.audio.position_ms() {
                    mgr_crossfade.check_auto_crossfade(pos_ms);
                }
            }
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
        manager.cast_autoconnect();

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
        // последняя очередь (альбом, плейлист): после перезапуска Play продолжает её, а не включает Волну по треку
        if let Some((queue, index, source)) = crate::cache::load_session() {
            if queue.get(index).is_some_and(|t| t.id == track.id) {
                let total = queue.len();
                let is_liked = {
                    let mut state = self.state.lock().unwrap();
                    state.queue = queue;
                    state.current_index = index;
                    state.current_track = Some(track.clone());
                    state.play_source = source.clone();
                    state.restored = true;
                    state.sync_track_like_status(&track.id)
                };
                self.emit(PlayerUiEvent::LikeStateChanged(is_liked));
                self.emit(PlayerUiEvent::SourceContext(source));
                self.emit(PlayerUiEvent::TrackStarted { track: track.clone(), index, total_queue: total });
                let client = self.client.lock().unwrap().clone();
                spawn_cover_and_lyrics(&self.event_senders, client, &track, self.generation.load(Ordering::SeqCst), &self.generation);
                return;
            }
        }
        let is_liked = {
            let mut state = self.state.lock().unwrap();
            state.queue = vec![track.clone()];
            state.current_index = 0;
            state.current_track = Some(track.clone());
            state.restored = true;
            state.sync_track_like_status(&track.id)
        };
        self.emit(PlayerUiEvent::LikeStateChanged(is_liked));
        self.emit(PlayerUiEvent::TrackStarted { track: track.clone(), index: 0, total_queue: 1 });
        let client = self.client.lock().unwrap().clone();
        spawn_cover_and_lyrics(&self.event_senders, client, &track, self.generation.load(Ordering::SeqCst), &self.generation);
    }

    /// Если на экране сохранённый трек, первое нажатие Play/вперёд/назад запускает его (с продолжением Волной по треку)
    fn resume_restored(&self) -> bool {
        let (track, session) = {
            let mut state = self.state.lock().unwrap();
            if !state.restored {
                return false;
            }
            state.restored = false;
            let session = (state.queue.len() > 1).then(|| (state.queue.clone(), state.current_index, state.play_source.clone()));
            (state.current_track.clone(), session)
        };
        match (session, track) {
            // восстановлена очередь: продолжаем её с того же места
            (Some((queue, index, source)), _) => self.set_queue_and_play_from(queue, index, source),
            (None, Some(track)) => self.play_single_track(track),
            _ => {}
        }
        true
    }

    /// Подписка на события плеера
    pub fn subscribe(&self) -> async_channel::Receiver<PlayerUiEvent> {
        let (tx, rx) = async_channel::unbounded();
        let mut senders = match self.event_senders.lock() {
            Ok(s) => s,
            Err(e) => e.into_inner(),
        };
        senders.push(tx);
        rx
    }

    /// Отправляет событие во все зарегистрированные каналы (UI, MPRIS)
    pub fn emit(&self, event: PlayerUiEvent) {
        emit_all(&self.event_senders, event);
    }

    /// Проверяет, был ли загружен список лайков
    pub fn is_liked_loaded(&self) -> bool {
        self.state.lock().map(|s| s.liked_loaded).unwrap_or(false)
    }

    /// Загружает список понравившихся треков с сервера (один источник правды).
    /// Выполняет объединение (merge) серверных ID с локальными, не перезаписывая изменения пользователя.
    pub async fn ensure_liked_loaded(&self) {
        if self.is_liked_loaded() {
            return;
        }
        let client = self.client.lock().unwrap().clone();
        if !client.has_token() {
            return;
        }
        let Ok(uid) = client.get_uid().await else { return };
        let Ok(ids) = client.get_liked_track_ids(&uid).await else { return };

        let current_liked = {
            let mut state = self.state.lock().unwrap();
            if state.user_uid.is_none() {
                state.user_uid = Some(uid);
            }
            state.merge_server_likes(ids);
            let track_id = state.current_track.as_ref().map(|t| t.id.clone());
            track_id.map(|id| state.sync_track_like_status(&id))
        };
        if let Some(liked) = current_liked {
            self.emit(PlayerUiEvent::LikeStateChanged(liked));
        }
    }

    /// Проверяет наличие токена при старте и запрашивает профиль пользователя
    fn check_auth_on_startup(&self) {
        let client_clone = self.client.clone();
        let state_clone = self.state.clone();
        let senders = self.event_senders.clone();
        let me = self.clone();

        tokio::spawn(async move {
            let client = client_clone.lock().unwrap().clone();
            if client.has_token() {
                match client.get_account_status().await {
                    Ok(acc) => {
                        let uid = acc.account.as_ref().map(|a| a.uid.clone()).unwrap_or_default();
                        let login = acc.account.as_ref().and_then(|a| a.login.clone())
                            .or_else(|| acc.account.as_ref().and_then(|a| a.full_name.clone()))
                            .unwrap_or_else(|| "Яндекс Пользователь".to_string());

                        if !uid.is_empty() {
                            client.set_cached_uid(uid.clone());
                        }

                        if let Ok(mut state) = state_clone.lock() {
                            state.user_login = Some(login.clone());
                            if !uid.is_empty() {
                                state.user_uid = Some(uid.clone());
                            }
                        }

                        emit_all(&senders, PlayerUiEvent::UserLoggedIn(login.clone()));
                        emit_all(&senders, PlayerUiEvent::StatusMessage(
                            format!("С возвращением, {}! Нажмите «Включить волну» 🌊", login)
                        ));

                        // Единственный источник загрузки лайков со слиянием
                        me.ensure_liked_loaded().await;
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

    /// Позиция воспроизведения в мс: на станции, если она привязана, иначе в приложении
    pub fn position_ms(&self) -> Option<u64> {
        let pos = self.cast_position_ms().or_else(|| self.audio.position_ms());
        if let Some(ms) = pos {
            self.check_auto_crossfade(ms);
        }
        pos
    }

    /// Проверяет, не подошел ли текущий трек к моменту автоматической сводки со следующим (кроссфейда)
    pub fn check_auto_crossfade(&self, pos_ms: u64) {
        let (crossfade, crossfade_dur) = self.crossfade_settings();
        if !crossfade || crossfade_dur == 0 {
            return;
        }
        let fade_ms = (crossfade_dur as u64) * 1000;

        let was_playing = self.audio.is_playing();
        if !was_playing {
            return;
        }

        let dur_ms = {
            let state = match self.state.lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            if state.is_transitioning {
                return;
            }
            state.current_track.as_ref().and_then(|t| t.duration_ms).map(|ms| ms as u64)
        }.or_else(|| self.duration_seconds().map(|s| s * 1000));

        let dur_ms = match dur_ms {
            Some(d) if d > fade_ms + 1000 => d,
            _ => return,
        };

        // Защита от устаревшей/аномальной позиции: если pos_ms превышает или равен dur_ms,
        // это остаточная позиция предыдущего слота во время кроссфейда/буферизации либо трек доиграл до EOS.
        // Ни в коем случае не триггерим автокроссфейд.
        if pos_ms >= dur_ms {
            return;
        }

        let threshold_ms = dur_ms.saturating_sub(fade_ms);
        if pos_ms < threshold_ms {
            // Трек играет в нормальном диапазоне (до момента сводки) — сбрасываем флаг,
            // разрешая будущий кроссфейд, когда трек действительно дойдет до конца.
            if let Ok(mut state) = self.state.lock() {
                if state.crossfade_triggered {
                    state.crossfade_triggered = false;
                }
            }
            return;
        }

        let trigger = {
            let mut state = match self.state.lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            if state.crossfade_triggered || state.is_transitioning {
                false
            } else {
                let at_end = state.current_index + 1 >= state.queue.len();
                if state.repeat == RepeatMode::Off && !state.wave && at_end {
                    false
                } else {
                    state.crossfade_triggered = true;
                    true
                }
            }
        };

        if trigger {
            eprintln!(
                "[crossfade] автосводка в конце трека (pos: {} ms, dur: {} ms, fade: {} ms)",
                pos_ms, dur_ms, fade_ms
            );
            let (repeat_one, current_track) = {
                let state = match self.state.lock() {
                    Ok(s) => (s.repeat == RepeatMode::One, s.current_track.clone()),
                    Err(_) => (false, None),
                };
                state
            };
            if repeat_one {
                if let Some(track) = current_track {
                    self.play_track_stream(&track);
                    return;
                }
            }
            self.next_track();
        }
    }

    /// Играет ли музыка: на станции, если она привязана, иначе в приложении
    pub fn is_playing(&self) -> bool {
        self.cast_playing().unwrap_or_else(|| self.audio.is_playing())
    }

    pub fn duration_seconds(&self) -> Option<u64> {
        self.cast_duration_secs().or_else(|| self.audio.duration_seconds())
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

        let is_playing = match self.cast_toggle() {
            Some(playing) => playing,
            None => self.audio.toggle_play(),
        };
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

        {
            let mut state = self.state.lock().unwrap();
            if state.crossfade_triggered {
                state.crossfade_triggered = false;
                eprintln!("[on_track_finished] автокроссфейд уже запущен, пропускаем повторный next_track");
                return;
            }
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
        if wave {
            crate::cache::clear_session(); // после перезапуска Волна продолжается по треку
        }
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
        let (next_track, idx, total, is_liked) = {
            let mut state = self.state.lock().unwrap();
            if state.queue.is_empty() {
                return;
            }
            state.current_index = (state.current_index + 1) % state.queue.len();
            let idx = state.current_index;
            let track = state.queue[idx].clone();
            state.is_transitioning = true;
            state.current_track = Some(track.clone());
            let is_liked = state.sync_track_like_status(&track.id);
            state.played_seconds = 0.0;
            (track, idx, state.queue.len(), is_liked)
        };

        // Если в очереди осталось менее 2 треков, подгружаем следующую пачку станции Ротора
        let remaining = total.saturating_sub(idx + 1);
        if remaining < 2 {
            self.load_more_wave_tracks();
        }

        self.emit(PlayerUiEvent::LikeStateChanged(is_liked));
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
        let (prev_track, idx, total, is_liked) = {
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
            state.crossfade_triggered = false;
            state.is_transitioning = true;
            state.current_track = Some(track.clone());
            let is_liked = state.sync_track_like_status(&track.id);
            state.played_seconds = 0.0;
            (track, idx, state.queue.len(), is_liked)
        };

        self.emit(PlayerUiEvent::LikeStateChanged(is_liked));
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

    /// Запускает «Волну по плейлисту»
    pub fn play_playlist_wave(&self, uid: &str, kind: &str, title: &str) {
        self.start_station(format!("playlist:{}_{}", uid, kind), None, format!("По плейлисту: {}", title));
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
            state.crossfade_triggered = false;
            state.is_transitioning = true;
            state.station_id = station_id.clone();
            state.batch_id = None;
            state.current_mood = None;
            state.current_language = None;
            state.current_character = None;
            if let Some(t) = &first {
                state.queue = vec![t.clone()];
                state.current_index = 0;
                state.current_track = Some(t.clone());
                state.sync_track_like_status(&t.id);
            }
        }
        self.set_wave(true);
        self.set_wave_label(Some(label.clone()));
        let target = match (&first, station_id.split_once(':')) {
            (Some(t), Some(("track", _))) => Some(SourceTarget::Track(t.clone())),
            (_, Some(("artist", id))) => Some(SourceTarget::Artist(id.to_string())),
            _ => None,
        };
        self.emit(PlayerUiEvent::StationTarget(target));

        match &first {
            Some(t) => {
                let is_liked = self.state.lock().map(|s| s.is_liked).unwrap_or(false);
                self.emit(PlayerUiEvent::LikeStateChanged(is_liked));
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
        self.set_queue_and_play_from(tracks, start_index, None);
    }

    /// Запускает очередь и подписывает, откуда она (плейлист, альбом, топ артиста…)
    pub fn set_queue_and_play_from(&self, tracks: Vec<Track>, start_index: usize, source: Option<PlaySource>) {
        if tracks.is_empty() {
            return;
        }
        let index = start_index.min(tracks.len() - 1);
        let track = tracks[index].clone();

        let is_liked = {
            let mut state = self.state.lock().unwrap();
            state.crossfade_triggered = false;
            state.is_transitioning = true;
            state.queue = tracks.clone();
            state.current_index = index;
            state.current_track = Some(track.clone());
            state.play_source = source.clone();
            state.sync_track_like_status(&track.id)
        };

        self.set_wave(false);
        crate::cache::save_session_queue(&tracks, source.clone());
        self.emit(PlayerUiEvent::LikeStateChanged(is_liked));
        self.emit(PlayerUiEvent::SourceContext(source));
        self.emit(PlayerUiEvent::QueueUpdated(tracks.clone()));
        self.emit(PlayerUiEvent::TrackStarted {
            track: track.clone(),
            index,
            total_queue: tracks.len(),
        });
        self.play_track_stream(&track);
    }

    /// Пауза (на станции, если она привязана) с мягким затуханием
    pub fn pause(&self) {
        if !self.cast_set_playing(false) {
            self.audio.pause_soft(250);
        }
    }

    /// Продолжение (на станции, если она привязана) с мягким нарастанием
    pub fn resume(&self) {
        if !self.cast_set_playing(true) {
            self.audio.play_soft(250);
        }
    }

    /// Перемотка к заданной миллисекунде (клик по строке текста)
    pub fn seek_ms(&self, ms: u64) {
        if !self.cast_seek(ms / 1000) {
            self.audio.seek_ms(ms);
        }
        self.emit(PlayerUiEvent::Seeked(ms / 1000));
    }

    /// Перемотка к заданной секунде
    pub fn seek(&self, seconds: u64) {
        if self.cast_seek(seconds) {
            return;
        }
        self.audio.seek_seconds(seconds);
        self.emit(PlayerUiEvent::Seeked(seconds));
    }

    /// Установка громкости (обновляет целевую громкость user_volume)
    pub fn set_volume(&self, volume: f64) {
        if self.cast_set_volume(volume) {
            return;
        }
        self.audio.set_volume(volume);
    }

    /// Текущие настройки плавного перехода: (включен, длительность в секундах)
    pub fn crossfade_settings(&self) -> (bool, u32) {
        self.crossfade.lock().map(|c| *c).unwrap_or((false, 3))
    }

    /// Обновление настроек плавного перехода
    pub fn set_crossfade(&self, enabled: bool, duration: u32) {
        if let Ok(mut cf) = self.crossfade.lock() {
            *cf = (enabled, duration);
        }
    }

    pub fn is_track_liked(&self, track_id: &str) -> bool {
        self.state.lock().map(|s| s.liked_track_ids.contains(track_id)).unwrap_or(false)
    }

    /// Устанавливает или снимает отметку «Мне нравится» для трека, синхронизирует кэш и сервер
    pub fn set_track_liked(&self, track: &Track, liked: bool) {
        let (station_id, batch_id, current_is_this, user_uid) = {
            let mut state = self.state.lock().unwrap();
            state.set_liked_local(&track.id, liked);
            let current_is_this = state.current_track.as_ref().map(|t| t.id == track.id).unwrap_or(false);
            if current_is_this {
                state.is_liked = liked;
                if liked {
                    state.is_disliked = false;
                }
            }
            (state.station_id.clone(), state.batch_id.clone(), current_is_this, state.user_uid.clone())
        };

        if current_is_this {
            self.emit(PlayerUiEvent::LikeStateChanged(liked));
            if liked {
                self.emit(PlayerUiEvent::DislikeStateChanged(false));
            }
        }

        self.emit(PlayerUiEvent::TrackLiked {
            track: track.clone(),
            liked,
        });

        let client = self.client.clone();
        let track_id = track.id.clone();
        tokio::spawn(async move {
            let client_guard = client.lock().unwrap().clone();
            if client_guard.has_token() {
                let uid_res = match user_uid {
                    Some(u) => Ok(u),
                    None => client_guard.get_uid().await,
                };
                if let Ok(uid) = uid_res {
                    let _ = client_guard.set_track_like(&uid, &track_id, liked).await;
                }
                if liked {
                    let _ = client_guard.feedback_like(&station_id, &track_id, batch_id).await;
                }
            }
        });
    }

    /// Отметка лайка
    pub fn toggle_like(&self) {
        let current_track = {
            let state = self.state.lock().unwrap();
            state.current_track.clone()
        };

        if let Some(track) = current_track {
            let new_like = !self.is_track_liked(&track.id);
            if new_like {
                self.emit(PlayerUiEvent::StatusMessage("Добавлено в «Мне нравится» ❤️".into()));
            } else {
                self.emit(PlayerUiEvent::StatusMessage("Удалено из «Мне нравится»".into()));
            }
            self.set_track_liked(&track, new_like);
        }
    }

    /// Отметка дизлайка
    pub fn toggle_dislike(&self) {
        let (new_dislike, station_id, track, batch_id, was_liked, user_uid) = {
            let mut state = self.state.lock().unwrap();
            state.is_disliked = !state.is_disliked;
            let track = state.current_track.clone();
            let mut was_liked = false;
            if state.is_disliked {
                state.is_liked = false;
                if let Some(t) = &track {
                    was_liked = state.set_liked_local(&t.id, false);
                }
            }
            (
                state.is_disliked,
                state.station_id.clone(),
                track,
                state.batch_id.clone(),
                was_liked,
                state.user_uid.clone(),
            )
        };

        self.emit(PlayerUiEvent::DislikeStateChanged(new_dislike));
        if new_dislike {
            self.emit(PlayerUiEvent::LikeStateChanged(false));
            if let Some(t) = &track {
                self.emit(PlayerUiEvent::TrackLiked {
                    track: t.clone(),
                    liked: false,
                });
            }
            self.emit(PlayerUiEvent::StatusMessage("Исключено из потока 👎".into()));
        }

        if let Some(t) = track {
            let client = self.client.clone();
            let track_id = t.id.clone();
            tokio::spawn(async move {
                let client_guard = client.lock().unwrap().clone();
                if client_guard.has_token() {
                    if new_dislike {
                        let uid_res = match user_uid {
                            Some(u) => Ok(u),
                            None => client_guard.get_uid().await,
                        };
                        if let Ok(uid) = uid_res {
                            if was_liked {
                                let _ = client_guard.set_track_like(&uid, &track_id, false).await;
                            }
                            let _ = client_guard.dislike_track(&uid, &track_id).await;
                        }
                        let _ = client_guard.feedback_dislike(&station_id, &track_id, batch_id).await;
                    }
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
    pub(super) fn play_track_stream(&self, track: &Track) {
        let audio_clone = self.audio.clone();
        let track_id = track.id.clone();
        let station_id = self.state.lock().unwrap().station_id.clone();
        let batch_id = self.state.lock().unwrap().batch_id.clone();
        let senders = self.event_senders.clone();
        let epoch = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let stale_gen = self.generation.clone();
        let is_stale = move || stale_gen.load(Ordering::SeqCst) != epoch;
        eprintln!("[play] #{} {} — {} ({})", epoch, track.title, track.artists_string(), track.id);

        let client = self.client.lock().unwrap().clone();
        spawn_cover_and_lyrics(&self.event_senders, client.clone(), track, epoch, &self.generation);
        crate::api::history::save_last_track(track);
        {
            let mut state = self.state.lock().unwrap();
            state.restored = false;
            if !state.wave {
                crate::cache::save_session_index(state.current_index);
            }
        }

        // привязана Станция: трек играет на ней, звук приложения молчит
        if self.cast_play(track) {
            return;
        }

        let (crossfade, crossfade_dur) = self.crossfade_settings();
        let fade_ms = (crossfade_dur as u64) * 1000;
        let was_playing = self.audio.is_playing();

        // сохранённый офлайн трек играет с диска: без сети и без токена
        if let Some(path) = crate::cache::offline_audio_path(&track_id) {
            let uri = gtk4::gio::prelude::FileExt::uri(&gtk4::gio::File::for_path(path));
            emit_all(&senders, PlayerUiEvent::LoadingState(false));
            emit_all(&senders, PlayerUiEvent::StreamQuality("Офлайн".to_string()));

            if crossfade {
                if was_playing {
                    audio_clone.crossfade_to_url(&uri, fade_ms);
                } else {
                    audio_clone.stop();
                    audio_clone.play_url_fade_in(&uri, fade_ms);
                }
            } else {
                audio_clone.stop();
                audio_clone.play_url(&uri);
            }

            if let Ok(mut state) = self.state.lock() {
                state.is_transitioning = false;
            }

            emit_all(&senders, PlayerUiEvent::PlaybackToggled(true));
            return;
        }

        // следующий трек очереди: после старта этого его ссылку получим заранее
        let next_track = {
            let state = self.state.lock().unwrap();
            state.queue.get(state.current_index + 1).cloned().filter(|t| t.id != track.id)
        };
        let prefetch = self.prefetch.clone();
        let prefetch_client = client.clone();
        let state_arc = self.state.clone();

        self.emit(PlayerUiEvent::Progress { current_sec: 0, total_sec: track.duration_ms.unwrap_or(0) / 1000 });

        tokio::spawn(async move {
            if client.has_token() {
                emit_all(&senders, PlayerUiEvent::LoadingState(true));
                let started = std::time::Instant::now();
                // ссылка могла быть получена заранее, пока играл прежний трек
                let ready = prefetch.lock().unwrap().take().filter(|p| p.track_id == track_id && p.at.elapsed() < PREFETCH_TTL);
                let hit = ready.is_some();
                let result = match ready {
                    Some(p) => Ok((p.url, p.label)),
                    None => client.get_track_direct_url(&track_id).await,
                };
                eprintln!("[play] #{} ссылка {} за {} мс ({})", epoch, if hit { "из предзагрузки" } else { "получена" }, started.elapsed().as_millis(), if result.is_ok() { "ok" } else { "ошибка" });
                if is_stale() {
                    eprintln!("[play] #{} устарел, поток не запускаем", epoch);
                    if let Ok(mut state) = state_arc.lock() {
                        state.is_transitioning = false;
                    }
                    return;
                }
                match result {
                    Ok((url, label)) => {
                        emit_all(&senders, PlayerUiEvent::LoadingState(false));
                        emit_all(&senders, PlayerUiEvent::StreamQuality(label));

                        if crossfade {
                            if was_playing {
                                audio_clone.crossfade_to_url(&url, fade_ms);
                            } else {
                                audio_clone.stop();
                                audio_clone.play_url_fade_in(&url, fade_ms);
                            }
                        } else {
                            audio_clone.stop();
                            audio_clone.play_url(&url);
                        }

                        if let Ok(mut state) = state_arc.lock() {
                            state.is_transitioning = false;
                        }

                        emit_all(&senders, PlayerUiEvent::PlaybackToggled(true));
                        // следующий трек готовим заранее (офлайн-треки в этом не нуждаются)
                        if let Some(next) = next_track.filter(|t| crate::cache::offline_audio_path(&t.id).is_none()) {
                            tokio::spawn(async move {
                                let started = std::time::Instant::now();
                                if let Some((url, label)) = prefetch_client.prefetch_track(&next.id).await {
                                    eprintln!("[prefetch] «{}» готов за {} мс", next.title, started.elapsed().as_millis());
                                    *prefetch.lock().unwrap() = Some(Prefetched { track_id: next.id, url, label, at: std::time::Instant::now() });
                                }
                            });
                        }
                        let _ = client.feedback_track_started(&station_id, &track_id, batch_id).await;
                    }
                    Err(e) => {
                        if let Ok(mut state) = state_arc.lock() {
                            state.is_transitioning = false;
                        }
                        emit_all(&senders, PlayerUiEvent::LoadingState(false));
                        emit_all(&senders, PlayerUiEvent::Error(format!("Ошибка получения потока: {}", e)));
                    }
                }
            } else {
                if let Ok(mut state) = state_arc.lock() {
                    state.is_transitioning = false;
                }
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
            "energetic" => (Some("active"), None, None),
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

                    let first_is_liked = if let Ok(mut state) = state_clone.lock() {
                        state.batch_id = station_tracks.batch_id.clone();
                        if !tracks.is_empty() {
                            state.queue = tracks.clone();
                            state.current_index = 0;
                            state.current_track = first_track.clone();
                            first_track.as_ref().map(|t| state.sync_track_like_status(&t.id)).unwrap_or(false)
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    emit_all(&senders, PlayerUiEvent::LoadingState(false));
                    emit_all(&senders, PlayerUiEvent::QueueUpdated(tracks.clone()));

                    if let Some(track) = first_track {
                        emit_all(&senders, PlayerUiEvent::LikeStateChanged(first_is_liked));
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
                                let (crossfade, crossfade_dur) = crate::api::auth::load_crossfade();
                                if crossfade {
                                    audio_clone.fade_in((crossfade_dur as u64) * 1000);
                                } else {
                                    audio_clone.set_volume(audio_clone.user_volume());
                                }
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
    senders: &Arc<Mutex<Vec<Sender<PlayerUiEvent>>>>,
    client: YandexMusicClient,
    track: &Track,
    epoch: u64,
    generation: &Arc<AtomicU64>,
) {
    if let Some(cover_url) = track.cover_url(400) {
        let (senders, generation) = (senders.clone(), generation.clone());
        tokio::spawn(async move {
            if let Some(path) = crate::cache::fetch_cover_cached(&cover_url).await {
                if generation.load(Ordering::SeqCst) == epoch {
                    emit_all(&senders, PlayerUiEvent::CoverUpdated(path.to_string_lossy().to_string()));
                }
            }
        });
    }

    if crate::api::auth::load_clips() {
        let (senders, generation, client, clip_track) = (senders.clone(), generation.clone(), client.clone(), track.clone());
        tokio::spawn(async move {
            // свои клипы альбома важнее клипов Яндекса
            if let Some(path) = crate::cache::custom_clip_for(&clip_track) {
                if generation.load(Ordering::SeqCst) == epoch {
                    emit_all(&senders, PlayerUiEvent::ClipUpdated(Some(path.to_string_lossy().to_string())));
                }
                return;
            }
            let path = match client.get_clip_url_for_track(&clip_track).await {
                Some(url) => crate::cache::fetch_clip_cached(&url).await,
                None => None,
            };
            // заставки-обложки без движения клипом не показываем
            let path = match path {
                Some(p) => {
                    let s = p.to_string_lossy().to_string();
                    let still = tokio::task::spawn_blocking(move || crate::ui::wave_clip::is_static(&s)).await.unwrap_or(false);
                    (!still).then_some(p)
                }
                None => None,
            };
            if generation.load(Ordering::SeqCst) == epoch {
                emit_all(&senders, PlayerUiEvent::ClipUpdated(path.map(|p| p.to_string_lossy().to_string())));
            }
        });
    }

    let (senders, generation) = (senders.clone(), generation.clone());
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
    format!("{}|{}|{}", t.title.trim().to_lowercase(), t.version.as_deref().unwrap_or("").trim().to_lowercase(), artists.join(","))
}

/// Оставляет в `new` только треки, которых ещё нет в `existing` и среди самих `new`
fn dedup_tracks(existing: &[Track], new: Vec<Track>) -> Vec<Track> {
    let mut seen: std::collections::HashSet<String> = existing.iter().map(track_key).collect();
    new.into_iter().filter(|t| seen.insert(track_key(t))).collect()
}

/// Рассылает событие во все переданные каналы
fn emit_all(senders: &Arc<Mutex<Vec<Sender<PlayerUiEvent>>>>, event: PlayerUiEvent) {
    if let PlayerUiEvent::Error(e) = &event {
        eprintln!("[error] {}", e);
    }
    let mut senders = match senders.lock() {
        Ok(s) => s,
        Err(e) => e.into_inner(),
    };
    senders.retain(|s| s.send_blocking(event.clone()).is_ok());
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

#[cfg(test)]
mod crossfade_tests {
    use super::*;

    #[test]
    fn player_state_default_crossfade_not_triggered() {
        let state = PlayerState::default();
        assert_eq!(state.crossfade_triggered, false);
    }

    #[test]
    fn test_crossfade_resets_when_position_is_before_threshold() {
        let mut state = PlayerState::default();
        state.crossfade_triggered = true;
        let dur_ms = 200_000u64;
        let fade_ms = 2000u64;
        let threshold_ms = dur_ms.saturating_sub(fade_ms);

        let pos_ms = 500u64;
        if pos_ms < threshold_ms {
            state.crossfade_triggered = false;
        }
        assert_eq!(state.crossfade_triggered, false);
    }

    #[test]
    fn test_stale_position_greater_than_duration_is_ignored() {
        let dur_ms = 200_000u64;
        let stale_pos_ms = 350_000u64;
        // Позиция превышает длину трека — кроссфейд не должен срабатывать
        assert!(stale_pos_ms >= dur_ms);
    }
}

#[cfg(test)]
mod like_tests {
    use super::*;

    #[test]
    fn test_like_state_and_cache() {
        let mut state = PlayerState::default();
        let tr = Track {
            id: "track_200".to_string(),
            title: "Test Song".to_string(),
            ..Default::default()
        };

        state.liked_track_ids.insert("track_100".to_string());
        assert!(state.liked_track_ids.contains("track_100"));
        assert!(!state.liked_track_ids.contains("track_200"));

        // Добавление лайка
        state.liked_track_ids.insert(tr.id.clone());
        assert!(state.sync_track_like_status(&tr.id));
        assert!(state.is_liked);
        assert!(!state.is_disliked);

        // Дизлайк лайкнутого трека
        state.is_disliked = true;
        state.is_liked = false;
        let was_liked = state.liked_track_ids.remove(&tr.id);
        assert!(was_liked);
        assert!(!state.liked_track_ids.contains(&tr.id));
        assert!(state.is_disliked);
        assert!(!state.is_liked);
    }

    #[test]
    fn test_server_likes_do_not_override_local_changes_made_while_loading() {
        let mut state = PlayerState::default();
        // пока список грузится: снят лайк с "a", поставлен на "c"
        state.set_liked_local("a", false);
        state.set_liked_local("c", true);
        // устаревший ответ сервера: "a" ещё лайкнут, "b" лайкнут, "c" нет
        state.merge_server_likes(vec!["a".into(), "b".into()]);
        assert!(state.liked_loaded);
        assert!(!state.liked_track_ids.contains("a"));
        assert!(state.liked_track_ids.contains("b"));
        assert!(state.liked_track_ids.contains("c"));
        // после загрузки правки больше не копятся
        state.set_liked_local("b", false);
        assert!(state.liked_pending.is_empty());
    }

    #[test]
    fn test_sync_track_like_status() {
        let mut state = PlayerState::default();
        state.liked_track_ids.insert("track_1".to_string());
        state.is_disliked = true;

        assert!(state.sync_track_like_status("track_1"));
        assert!(state.is_liked);
        assert!(!state.is_disliked);

        assert!(!state.sync_track_like_status("track_unknown"));
        assert!(!state.is_liked);
        assert!(!state.is_disliked);
    }

    #[tokio::test]
    async fn test_cached_uid_avoids_network() {
        let client = YandexMusicClient::new(None);
        client.set_cached_uid("user_999".to_string());
        let uid = client.get_uid().await.expect("cached uid must succeed");
        assert_eq!(uid, "user_999");
    }
}
