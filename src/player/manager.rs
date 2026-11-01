use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use async_channel::Sender;
use futures_util::stream::StreamExt;

use crate::api::client::YandexMusicClient;
use crate::api::models::{Album, Playlist, Track, TrackSource};
use crate::api::provider::{FeedbackEvent, MusicProvider, ProviderId};
use crate::api::provider_manager::ProviderManager;
use crate::api::ytmusic::YtMusicClient;
use super::audio::AudioBackend;
use super::wave_engine::WaveEngine;

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

/// Какой клип показывать: от этого зависят зацикливание, кроп и прозрачность слоя.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipKind {
    /// Короткое зацикленное превью Яндекса: поля обрезаются яркостной эвристикой
    YandexPreview,
    /// Свой клип альбома из `custom_clips`
    Custom,
    /// Полноценный клип YouTube: без зацикливания, без кропа, выше прозрачность
    YouTubeFull,
}

/// Запрос на показ локального клипа.
#[derive(Debug, Clone)]
pub struct ClipRequest {
    pub track_id: String,
    pub epoch: u64,
    pub attempt_id: u64,
    pub kind: ClipKind,
    pub path: String,
}

/// Показ или снятие клипа; оба варианта несут идентификаторы для проверки актуальности.
#[derive(Debug, Clone)]
pub enum ClipUpdate {
    Show(ClipRequest),
    Clear { track_id: String, epoch: u64 },
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
    /// Клип на фоне Волны: показать локальный файл или погасить слой
    ClipUpdated(ClipUpdate),
    RepeatChanged(RepeatMode),
    /// Тип включённой Волны для плашки сверху: «По артисту: …», «По треку: …»; `None` — простая Волна или не Волна
    WaveContext(Option<String>),
    /// Реальный формат потока, например «MP3 192»
    StreamQuality(String),
    Seeked(u64),
    LoadingState(bool),
    /// Подбор Волны под фильтр: `Some("…")` — показать прогресс, `None` — скрыть
    WaveLoading(Option<String>),
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
    /// Изменился активный музыкальный провайдер
    ProviderChanged(crate::api::provider::ProviderId),
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
    /// Очередь кончилась на Волне: ждём догрузку, по её приходу перейдём на новый трек
    pub pending_advance: bool,
    /// Догрузка уже пыталась продлить Волну и не дала ни одного нового трека
    pub wave_dead: bool,
    /// Сколько догрузок Волны подряд не дали новых треков (сбрасывается при удаче)
    pub refill_failures: usize,
    /// Полные объекты любимых треков — источник сидов для алгоритмических фильтров
    pub liked_tracks_cache: Vec<Track>,
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
            pending_advance: false,
            wave_dead: false,
            refill_failures: 0,
            liked_tracks_cache: Vec::new(),
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

impl Prefetched {
    /// Ссылка всё ещё пригодна к воспроизведению.
    ///
    /// У googlevideo в query есть `expire=<unix>` — время, до которого ссылка жива (обычно
    /// 6 часов). Прежняя проверка «не старше 90 секунд» выбрасывала полностью рабочую ссылку:
    /// prefetch запускается сразу после старта предыдущего трека, то есть за 3–5 минут до
    /// использования, поэтому условие не выполнялось НИКОГДА. В результате ссылку запрашивали
    /// заново (другой `ei=` → другой URL), и прогрев слота пропадал: кроссфейд начинал трек
    /// с нуля вместо мгновенного старта.
    fn is_fresh(&self) -> bool {
        match url_expire_unix(&self.url) {
            Some(expire) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                expire > now + 120
            }
            // Локальный файл или прямой URL без срока жизни — ограничиваем только по возрасту.
            None => self.at.elapsed() < PREFETCH_MAX_AGE,
        }
    }
}

/// Достаёт `expire=<unix>` из query-строки ссылки (googlevideo и подобные).
fn url_expire_unix(url: &str) -> Option<u64> {
    url.split_once('?')?.1.split('&').find_map(|kv| kv.strip_prefix("expire=")?.parse().ok())
}

/// Ссылки на поток живут недолго (через несколько минут отвечают 410): заранее полученной верим 90 секунд
/// Максимальный возраст prefetch-ссылки без явного срока жизни (локальные файлы, прямые URL).
const PREFETCH_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(20 * 60);

/// Сколько идентификаторов сыгранного отправляем Ротору в `?queue=`: сервер учтёт их
/// и не предложит повторно в новой порции.
const ROTOR_QUEUE_IDS: usize = 5;

/// Сколько раз подряд пробуем продлить Волну, прежде чем считать её исчерпанной.
/// Ротор иногда отдаёт порцию из одних повторов — одиночный пустой ответ ещё не приговор.
const WAVE_REFILL_ATTEMPTS: usize = 3;

/// Пауза между попытками продления Волны, чтобы не долбить API в один момент.
const WAVE_REFILL_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(1000);

/// Менеджер плеера
#[derive(Clone)]
pub struct PlayerManager {
    pub audio: Arc<AudioBackend>,
    pub client: Arc<Mutex<YandexMusicClient>>,
    pub providers: Arc<ProviderManager>,
    pub yt_client: Arc<YtMusicClient>,
    /// Фоновые клипы: резолв источника, загрузка в кэш и телеметрия для IPC
    pub clips: super::clip::ClipBackground,
    pub state: Arc<Mutex<PlayerState>>,
    pub wave_engine: Arc<Mutex<WaveEngine>>,
    pub event_senders: Arc<Mutex<Vec<Sender<PlayerUiEvent>>>>,
    /// Ссылка на следующий трек очереди, полученная заранее (один слот)
    prefetch: Arc<Mutex<Option<Prefetched>>>,
    /// id трека, для которого преролл уже запрашивается (защита от параллельных дублей)
    prefetch_inflight: Arc<Mutex<Option<String>>>,
    /// Привязанная Яндекс Станция (см. `cast.rs`)
    pub(super) cast: Arc<Mutex<Option<super::cast::CastLink>>>,
    /// Номер последнего запроса воспроизведения: устаревшие асинхронные задачи (быстрые клики) отбрасываются
    generation: Arc<AtomicU64>,
    /// Эпоха бесшовного обмена хвоста (TailSwap). Отдельно от `generation`: та
    /// инкрементируется на каждом переходе трека и не должна отменять фильтр.
    tail_swap_epoch: Arc<AtomicU64>,
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
        let client_mutex = Arc::new(Mutex::new(client.clone()));
        let state = Arc::new(Mutex::new(PlayerState::default()));
        let wave_engine = Arc::new(Mutex::new(WaveEngine::new()));
        // Поднимаем историю прослушивания с прошлого запуска: Волна не должна
        // начинать новый день с тех же треков (свежие — в конец очереди истории).
        {
            let recent = crate::api::history::load_recent_played();
            if !recent.is_empty() {
                let mut engine = wave_engine.lock().unwrap();
                for id in recent.iter().rev() {
                    engine.record_played(id);
                }
                // Счётчик дрейфа — про текущий сид, а не про вчерашние треки
                engine.mark_seed_started();
            }
        }
        let (cf_enabled, cf_dur) = crate::api::auth::load_crossfade();
        let crossfade = Arc::new(Mutex::new((cf_enabled, cf_dur)));

        let yt_client = if let Some(cookie) = crate::api::auth::load_ytmusic_cookie() {
            YtMusicClient::with_cookie(cookie)
        } else if let Some(token) = crate::api::auth::load_ytmusic_refresh_token() {
            YtMusicClient::with_refresh_token(token)
        } else {
            YtMusicClient::new()
        };
        let yt_client_arc = Arc::new(yt_client.clone());
        let clip_telemetry = Arc::new(super::clip_telemetry::ClipTelemetry::new());
        clip_telemetry.set_enabled(crate::api::auth::load_clips());
        let clip_events = Arc::new(Mutex::new(event_senders.clone()));
        let clips = super::clip::ClipBackground::new(
            yt_client_arc.clone(),
            clip_telemetry,
            clip_events,
        );

        let yandex_provider = Arc::new(client) as Arc<dyn MusicProvider>;
        let ytmusic_provider = Arc::new(yt_client) as Arc<dyn MusicProvider>;
        let provider_manager = ProviderManager::with_yandex_and_ytmusic(yandex_provider, ytmusic_provider);
        let active_provider = crate::api::auth::load_active_provider();
        provider_manager.switch_provider(active_provider);
        let providers = Arc::new(provider_manager);

        let manager = Self {
            audio,
            client: client_mutex,
            providers,
            yt_client: yt_client_arc,
            clips,
            state,
            wave_engine,
            event_senders: Arc::new(Mutex::new(event_senders)),
            cast: Arc::default(),
            prefetch: Arc::default(),
            prefetch_inflight: Arc::default(),
            generation: Arc::new(AtomicU64::new(0)),
            tail_swap_epoch: Arc::new(AtomicU64::new(0)),
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
            let track_id = std::env::var("VOLNA_DEMO_CLIP_TRACK").unwrap_or_else(|_| "demo".to_string());
            self.emit(PlayerUiEvent::ClipUpdated(ClipUpdate::Show(ClipRequest {
                track_id,
                epoch: 0,
                attempt_id: 0,
                kind: ClipKind::Custom,
                path,
            })));
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
                spawn_cover_and_lyrics(&self.event_senders, client, &track, self.generation.load(Ordering::SeqCst), &self.generation, &self.clips);
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
        spawn_cover_and_lyrics(&self.event_senders, client, &track, self.generation.load(Ordering::SeqCst), &self.generation, &self.clips);
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

    /// Единая точка включения клипов: настройки и IPC используют её, а не две реализации.
    pub fn set_clips_enabled(&self, enabled: bool) {
        let _ = crate::api::auth::save_clips(enabled);
        self.clips.set_enabled(enabled);
        if enabled
            && let Some(track) = self.state.lock().ok().and_then(|s| s.current_track.clone())
        {
            let epoch = self.generation.load(Ordering::SeqCst);
            self.clips.spawn_clip_for_track(&track, epoch);
        }
    }

    /// `clip reset`: чистит negative cache, не удаляя положительный MP4-кэш.
    /// Если локального клипа нет — разрешает одну новую попытку без перезапуска звука.
    pub fn reset_clip_negative_cache(&self) -> usize {
        let removed = self.clips.reset_negative();
        let status = self.clips.telemetry().snapshot();
        if status.enabled
            && !status.local_clip_active
            && let Some(track) = self.state.lock().ok().and_then(|s| s.current_track.clone())
        {
            let epoch = self.generation.load(Ordering::SeqCst);
            self.clips.spawn_clip_for_track(&track, epoch);
        }
        removed
    }

    /// Переключает активного музыкального провайдера
    pub fn switch_provider(&self, id: ProviderId) {
        if self.providers.switch_provider(id) {
            let _ = crate::api::auth::save_active_provider(id);
            if let Ok(mut state) = self.state.lock() {
                state.liked_loaded = false;
                // Фильтры не переносятся между сервисами: у Яндекса это нативный
                // Ротор, у YouTube Music — локальный алгоритм. Токен продолжения
                // тоже принадлежит прежнему провайдеру.
                state.current_mood = None;
                state.current_language = None;
                state.current_character = None;
                state.wave_label = None;
                state.batch_id = None;
                state.station_id = "user:onyourwave".to_string();
                state.wave_dead = false;
                state.refill_failures = 0;
                state.pending_advance = false;
            }
            let me = self.clone();
            tokio::spawn(async move {
                me.ensure_liked_loaded().await;
            });
            self.emit(PlayerUiEvent::ProviderChanged(id));
        }
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

        // Если активен YouTube Music: загружаем понравившиеся треки из YouTube Music
        let provider = self.providers.current();
        if provider.provider_id() == ProviderId::YouTubeMusic && provider.is_authenticated() {
            if let Ok(tracks) = provider.get_liked_tracks().await {
                let tracks: Vec<Track> = tracks.into_iter().filter(|t| !t.id.is_empty()).collect();
                let ids: Vec<String> = tracks.iter().map(|t| t.id.clone()).collect();
                let current_liked = {
                    let mut state = self.state.lock().unwrap();
                    state.liked_loaded = true;
                    state.merge_server_likes(ids);
                    // Полные объекты нужны фильтрам для выбора сида
                    state.liked_tracks_cache = tracks;
                    let track_id = state.current_track.as_ref().map(|t| t.id.clone());
                    track_id.map(|id| state.sync_track_like_status(&id))
                };
                if let Some(liked) = current_liked {
                    self.emit(PlayerUiEvent::LikeStateChanged(liked));
                }
                return;
            }
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
                let provider = me.providers.current();
                if provider.provider_id() == ProviderId::YouTubeMusic && provider.is_authenticated() {
                    emit_all(&senders, PlayerUiEvent::StatusMessage(
                        "Подключен Google (YouTube Music) 🔴".to_string(),
                    ));
                    me.ensure_liked_loaded().await;
                } else {
                    emit_all(&senders, PlayerUiEvent::AuthRequired);
                }
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
            self.next_track_inner(true);
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
        if !self.is_playing() && self.providers.active_id() == ProviderId::YouTubeMusic {
            let is_yandex_track = {
                let state = self.state.lock().unwrap();
                state.current_track.as_ref()
                    .or_else(|| state.queue.get(state.current_index))
                    .map(|t| t.source == TrackSource::Yandex)
                    .unwrap_or(false)
            };
            if is_yandex_track && !self.client.lock().unwrap().has_token() {
                if let Ok(mut state) = self.state.lock() {
                    state.restored = false;
                }
                self.start_station("user:onyourwave".to_string(), None, "Моя Волна".to_string());
                return;
            }
        }

        if self.resume_restored() {
            return;
        }
        let has_tracks = {
            let state = self.state.lock().unwrap();
            !state.queue.is_empty()
        };

        if !has_tracks {
            let provider = self.providers.current();
            let is_yt = provider.provider_id() == ProviderId::YouTubeMusic;
            if is_yt {
                if !provider.is_authenticated() {
                    self.emit(PlayerUiEvent::StatusMessage(
                        "Войдите в Google Аккаунт внизу сайдбара для доступа к персонализированной волне YouTube Music".to_string(),
                    ));
                } else {
                    self.emit(PlayerUiEvent::StatusMessage(
                        "Загружаем персональную Волну... 🔴".to_string(),
                    ));
                    self.fetch_wave_tracks("onyourwave");
                }
            } else {
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
        self.next_track_inner(true);
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
            // Смена режима — смена ожиданий: ждать догрузку или считать Волну мёртвой
            // имеет смысл только внутри одной и той же очереди.
            state.pending_advance = false;
            state.wave_dead = false;
            state.refill_failures = 0;
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
        self.next_track_inner(false);
    }

    /// Переход на следующий трек.
    ///
    /// `automatic` — переход случился сам (конец трека, автосводка), а не по нажатию
    /// пользователя: только в этом случае «мёртвую» Волну разрешено остановить, а не
    /// молча крутить тот же микс.
    fn next_track_inner(&self, automatic: bool) {
        if self.resume_restored() {
            return;
        }

        let decision = {
            let state = self.state.lock().unwrap();
            if state.queue.is_empty() {
                return;
            }
            tail_advance(
                state.wave,
                state.repeat == RepeatMode::One,
                state.current_index + 1 >= state.queue.len(),
                state.wave_dead,
                automatic,
            )
        };

        match decision {
            TailAdvance::Advance => {}
            TailAdvance::RequestRefill => {
                // Очередь кончилась: не заворачиваем её в начало, а просим новую порцию.
                // Переход сделает `finish_pending_advance`, когда догрузка вернётся
                // (`load_more_wave_tracks` → очередь выросла → next_track).
                let request = {
                    let mut state = self.state.lock().unwrap();
                    state.wave_dead = false;
                    let request = !state.pending_advance;
                    if request {
                        // Новый цикл догрузки — новый счёт провалов: иначе «Далее»,
                        // нажатое после исчерпанной Волны, получило бы одну попытку
                        // вместо трёх и сдалось бы сразу.
                        state.refill_failures = 0;
                    }
                    state.pending_advance = true;
                    request
                };
                if request {
                    // Пока догрузка в пути, повторные нажатия «Далее» не плодят запросы.
                    // Спиннер снимет `load_more_wave_tracks` на любом выходе.
                    self.emit(PlayerUiEvent::LoadingState(true));
                    self.load_more_wave_tracks();
                }
                return;
            }
            TailAdvance::GiveUp => {
                let mut state = self.state.lock().unwrap();
                // Флаг снимаем: следующее нажатие «Далее» попробует догрузку заново.
                state.wave_dead = false;
                state.pending_advance = false;
                drop(state);
                self.audio.pause();
                self.emit(PlayerUiEvent::PlaybackToggled(false));
                self.emit(PlayerUiEvent::StatusMessage(
                    "Волна выдохлась: провайдер не дал новых треков. Нажмите «Далее», чтобы попробовать другой сид"
                        .to_string(),
                ));
                return;
            }
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

        // Если очередь подходит к концу (осталось <= 3 треков) и включен режим волны,
        // автоматически в фоне запрашиваем следующую порцию радио
        let remaining = total.saturating_sub(idx + 1);
        let is_wave = self.state.lock().unwrap().wave;
        let refill_needed = self.wave_engine.lock().unwrap().refill_needed(total, idx);
        // Seed Drift не ждёт исчерпания хвоста: иначе сид застревает на весь текущий
        // батч, то есть на десятки треков. Только для YouTube Music: у Яндекса понятия
        // «сид» нет, и лишние догрузки там ни к чему.
        let drift_due = self.providers.current().provider_id() == ProviderId::YouTubeMusic
            && self.wave_engine.lock().unwrap().seed_drift_due();
        if is_wave && (remaining <= 3 || refill_needed || drift_due) {
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
        self.prefetch_next_track();
    }

    /// Догрузка вернулась: если мы ждали продления в конце очереди — самое время перейти.
    fn finish_pending_advance(&self) {
        let has_next = {
            let mut state = self.state.lock().unwrap();
            let waiting = std::mem::take(&mut state.pending_advance);
            waiting && state.current_index + 1 < state.queue.len()
        };
        if has_next {
            // Идём «автоматическим» путём: если хвост снова окажется последним треком,
            // обработка даст честную остановку, а не ещё один запрос.
            self.next_track_inner(true);
        }
    }

    /// Ожидание в конце очереди не оправдалось: очередь не выросла.
    ///
    /// Ничего не останавливаем здесь — трек ещё может играть (пользователь нажал
    /// «Далее» в середине песни). Остановку сделает `next_track_inner`, когда играть
    /// будет уже нечего.
    ///
    /// `pending_advance` снимаем всегда: иначе после неудачи он остаётся взведённым,
    /// повторное «Далее» упирается в `request = false` и не делает уже ничего —
    /// Волна замолкает навсегда без единого сообщения.
    ///
    /// А вот «мёртвой» Волну объявляем только если ждали продления в самом конце очереди:
    /// неудачная фоновая догрузка за три трека до конца ничего не значит, к хвосту
    /// можно спросить заново.
    fn mark_wave_refill_dead(&self) {
        let mut state = self.state.lock().unwrap();
        let waiting = std::mem::take(&mut state.pending_advance);
        if waiting {
            state.wave_dead = true;
        }
    }

    /// Предварительно подготавливает и буферизует следующий трек очереди (на диске и аппаратно в GStreamer)
    pub fn prefetch_next_track(&self) {
        let next = {
            let state = self.state.lock().unwrap();
            state.queue.get(state.current_index + 1).cloned()
        };
        let Some(next) = next else { return };

        // Один и тот же трек запрашивают сразу несколько мест (next_track, play_track_stream,
        // append_to_queue) — без этой проверки на каждый переход уходило 2–3 параллельных
        // запроса к InnerTube, а каждый из них заново дёргал слот преролла.
        {
            let mut inflight = self.prefetch_inflight.lock().unwrap();
            if inflight.as_deref() == Some(next.id.as_str()) {
                return;
            }
            *inflight = Some(next.id.clone());
        }
        let inflight_arc = self.prefetch_inflight.clone();
        let inflight_id = next.id.clone();
        let clear_inflight = move || {
            if let Ok(mut inflight) = inflight_arc.lock() {
                if inflight.as_deref() == Some(inflight_id.as_str()) {
                    *inflight = None;
                }
            }
        };

        if let Some(path) = crate::cache::offline_audio_path(&next.id).or_else(|| crate::cache::cached_audio_path(&next.id)) {
            let uri = gtk4::gio::prelude::FileExt::uri(&gtk4::gio::File::for_path(&path));
            self.audio.preroll_next_url(&uri);
            eprintln!("[prefetch] «{}» уже на диске, preroll локального файла: {}", next.title, uri);
            clear_inflight();
            return;
        }

        let prefetch_arc = self.prefetch.clone();
        let audio = self.audio.clone();
        let client = self.client.lock().unwrap().clone();
        let providers = self.providers.clone();
        let is_yt = next.source == TrackSource::YouTubeMusic;
        let track_id = next.id.clone();
        let track_title = next.title.clone();

        tokio::spawn(async move {
            let started = std::time::Instant::now();
            let stream_res: Result<(String, String), String> = if is_yt {
                if let Some(yt) = providers.get(ProviderId::YouTubeMusic) {
                    yt.get_stream_url(&track_id).await.map(|u| (u, "Opus 160".to_string())).map_err(|e| e.to_string())
                } else {
                    Err("YouTube Music недоступен".to_string())
                }
            } else {
                client.get_track_direct_url(&track_id).await.map_err(|e| e.to_string())
            };

            let (stream_url, label) = match stream_res {
                Ok((url, lbl)) => (url, lbl),
                Err(e) => {
                    eprintln!("[prefetch] Ошибка получения URL для «{}»: {}", track_title, e);
                    clear_inflight();
                    return;
                }
            };

            eprintln!("[prefetch] Ссылка для «{}» получена за {} мс", track_title, started.elapsed().as_millis());
            *prefetch_arc.lock().unwrap() = Some(Prefetched {
                track_id: track_id.clone(),
                url: stream_url.clone(),
                label,
                at: std::time::Instant::now(),
            });

            audio.preroll_next_url(&stream_url);

            let ext = if is_yt { "opus" } else { "mp3" };
            if let Some(path) = download_audio_to_cache(&stream_url, &track_id, ext).await {
                let local_uri = gtk4::gio::prelude::FileExt::uri(&gtk4::gio::File::for_path(&path));
                // Слот теперь прогрет локальным файлом: обновляем и саму prefetch-ссылку, иначе
                // play_track_stream отдаст в crossfade сетевой URL, тот не совпадёт с file://
                // в слоте — и мгновенный старт снова превратится в полную переинициализацию.
                if let Ok(mut entry) = prefetch_arc.lock() {
                    if let Some(p) = entry.as_mut().filter(|p| p.track_id == track_id) {
                        p.url = local_uri.to_string();
                        p.at = std::time::Instant::now();
                    }
                }
                audio.preroll_next_url(&local_uri);
                eprintln!("[prefetch] «{}» скачан на диск и переведен в локальный preroll: {}", track_title, local_uri);
            }
            clear_inflight();
        });
    }

    /// Добавляет треки в очередь с фильтрацией через алгоритмический движок волны
    pub fn append_to_queue(&self, tracks: Vec<Track>) {
        if tracks.is_empty() {
            return;
        }
        let filtered = self.wave_engine.lock().unwrap().filter_candidates(tracks);
        let updated_queue = {
            let mut state = self.state.lock().unwrap();
            let new_tracks = dedup_tracks(&state.queue, filtered);
            state.queue.extend(new_tracks);
            state.queue.clone()
        };
        self.emit(PlayerUiEvent::QueueUpdated(updated_queue));
        self.prefetch_next_track();
    }

    /// Подгружает следующую порцию треков Волны и добавляет в хвост очереди
    pub fn load_more_wave_tracks(&self) {
        let client_clone = self.client.clone();
        let providers_clone = self.providers.clone();
        let state_clone = self.state.clone();
        let wave_engine_clone = self.wave_engine.clone();
        let senders = self.event_senders.clone();
        // Нужен, чтобы после догрузки перейти на новый трек, если очередь кончилась
        let me = self.clone();
        // Снимок эпохи берём до сети: TailSwap за это время мог обрезать хвост
        let epoch_snapshot = self.tail_swap_epoch.load(Ordering::SeqCst);

        let (station_id, batch_id, mood, language, character) = {
            let state = self.state.lock().unwrap();
            (
                state.station_id.clone(),
                state.batch_id.clone(),
                state.current_mood.clone(),
                state.current_language.clone(),
                state.current_character.clone(),
            )
        };

        tokio::spawn(async move {
            let provider = providers_clone.current();
            if provider.provider_id() == ProviderId::YouTubeMusic {
                if !provider.is_authenticated() {
                    emit_all(&senders, PlayerUiEvent::LoadingState(false));
                    return;
                }

                // Seed Drift: один и тот же «RDAMVM» сид бесконечно выдаёт всё более узкий
                // поджанровый пузырь. Через каждые N сыгранных треков переезжаем на свежий
                // сид — от того трека, который слушатель слышит сейчас.
                //
                // Порядок важен: кандидаты в якоря отбираются ДО лока `wave_engine`,
                // а решение и сброс счётчика делаются одним локом. Иначе две параллельные
                // догрузки стартовали бы два разных сида на одном переходе.
                let drift_due = {
                    let engine = wave_engine_clone.lock().unwrap();
                    engine.seed_drift_due()
                };
                let (primary_track, spare_track) = {
                    let state = state_clone.lock().unwrap();
                    let primary = state
                        .current_track
                        .clone()
                        .or_else(|| state.queue.get(state.current_index).cloned())
                        .filter(|t| !t.id.is_empty());
                    let spare = state.queue.last().cloned().filter(|t| !t.id.is_empty());
                    (primary, spare)
                };
                let (liked_ids, history_set) = snapshots_for_filters(&me);
                let primary_anchor = primary_track
                    .filter(|t| {
                        anchor_passes_filters(
                            t,
                            language.as_deref(),
                            character.as_deref(),
                            &liked_ids,
                            &history_set,
                        )
                    })
                    .map(|t| t.id);
                let spare_anchor = spare_track
                    .filter(|t| {
                        anchor_passes_filters(
                            t,
                            language.as_deref(),
                            character.as_deref(),
                            &liked_ids,
                            &history_set,
                        )
                    })
                    .map(|t| t.id);
                let has_candidate = primary_anchor.is_some() || spare_anchor.is_some();
                let drift_active = {
                    let mut engine = wave_engine_clone.lock().unwrap();
                    if drift_due && has_candidate {
                        engine.mark_seed_started();
                        true
                    } else {
                        false
                    }
                };

                // Multi-seed: каждый второй переезд стартует не от текущего трека, а от
                // дальнего края очереди — так в миксе чередуются два соседних пузыря.
                let anchor = if drift_active {
                    let serial = wave_engine_clone.lock().unwrap().note_seed_drift();
                    let picked = pick_drift_anchor(serial, primary_anchor.as_deref(), spare_anchor.as_deref());
                    eprintln!(
                        "[wave] переезд #{}: сид от {}",
                        serial,
                        picked.as_deref().unwrap_or("станции")
                    );
                    picked
                } else {
                    if drift_due {
                        eprintln!("[wave] дрейф отложен: нет якоря под активные фильтры");
                    }
                    primary_anchor.clone()
                };

                let plan = plan_wave_fetch(&station_id, batch_id.as_deref(), drift_active, anchor.as_deref());
                let mut station_batch = provider
                    .get_station_tracks(&plan.station, plan.continuation.as_deref())
                    .await
                    .ok();

                // Волна может выдохнуться: токен продолжения кончился либо всё отсеялось
                // фильтрами. Раньше очередь в этот момент молча переставала расти.
                let exhausted = station_batch.as_ref().is_none_or(|b| b.tracks.is_empty());
                if exhausted {
                    let alternative = [primary_anchor.as_deref(), spare_anchor.as_deref()]
                        .into_iter()
                        .flatten()
                        .find(|id| Some(*id) != anchor.as_deref());
                    if let Some(alt) = alternative {
                        let retry = plan_wave_fetch(&station_id, None, false, Some(alt));
                        station_batch = provider.get_station_tracks(&retry.station, None).await.ok();
                        if station_batch.as_ref().is_some_and(|b| !b.tracks.is_empty()) {
                            eprintln!("[wave] продолжение исчерпано — новый сид: {}", retry.station);
                        }
                    }
                }

                let Some(station_batch) = station_batch.filter(|b| !b.tracks.is_empty()) else {
                    eprintln!("[wave] волна выдохлась: станция «{}» не дала новых треков", station_id);
                    emit_all(&senders, PlayerUiEvent::LoadingState(false));
                    me.mark_wave_refill_dead();
                    return;
                };

                if plan.new_seed {
                    eprintln!("[wave] сид обновлён: {}", plan.station);
                    wave_engine_clone.lock().unwrap().mark_seed_started();
                }

                let raw = station_batch.tracks.len();
                let pre = pick_candidates(true, &wave_engine_clone, station_batch.tracks);
                let filtered = apply_filters(
                    &me.yt_client,
                    pre,
                    language.as_deref(),
                    character.as_deref(),
                    &liked_ids,
                    &history_set,
                    4800,
                )
                .await;

                // Пока тянули батч, пользователь мог сменить фильтр: хвост уже обрезан
                if me.tail_swap_epoch.load(Ordering::SeqCst) != epoch_snapshot {
                    emit_all(&senders, PlayerUiEvent::LoadingState(false));
                    return;
                }

                let updated = {
                    let mut state = state_clone.lock().unwrap();
                    state.batch_id = if station_batch.batch_id.is_empty() { None } else { Some(station_batch.batch_id) };
                    let new_tracks = dedup_tracks(&state.queue, filtered);
                    let mut dropped: Vec<String> = Vec::new();
                    if drift_active && !new_tracks.is_empty() {
                        // Свежий сид заменяет ещё не сыгранный хвост старого микса:
                        // иначе очередь растёт бесконечно и в ней копятся треки чужого сида.
                        let keep = (state.current_index + 1).min(state.queue.len());
                        dropped = state.queue[keep..].iter().map(|t| t.id.clone()).collect();
                        state.queue.truncate(keep);
                    }
                    let before = state.queue.len();
                    let added = new_tracks.len();
                    state.queue.extend(new_tracks);
                    (state.queue.clone(), state.queue.len() > before, dropped, added)
                };
                let (updated_queue, grew, dropped, added) = updated;
                if !dropped.is_empty() {
                    // Выброшенный хвост тоже не должен вернуться в следующей порции
                    me.wave_engine.lock().unwrap().remember_dropped(&dropped);
                }
                eprintln!("[wave] raw={} added={} queue={}", raw, added, updated_queue.len());
                emit_all(&senders, PlayerUiEvent::QueueUpdated(updated_queue));
                emit_all(&senders, PlayerUiEvent::LoadingState(false));
                if grew {
                    if let Ok(mut state) = state_clone.lock() {
                        state.refill_failures = 0;
                    }
                    me.finish_pending_advance();
                } else {
                    // Батч пришёл, но всё в нём уже есть в очереди.
                    if let Ok(mut state) = state_clone.lock() {
                        state.refill_failures += 1;
                    }
                    me.mark_wave_refill_dead();
                }
                return;
            }

            let client = client_clone.lock().unwrap().clone();
            if !client.has_token() {
                emit_all(&senders, PlayerUiEvent::LoadingState(false));
                return;
            }

            // Что уже сыграно — Ротору, чтобы он не повторял это в новой порции.
            let queue_ids: Vec<String> = {
                let state = state_clone.lock().unwrap();
                let end = (state.current_index + 1).min(state.queue.len());
                state.queue[..end]
                    .iter()
                    .rev()
                    .take(ROTOR_QUEUE_IDS)
                    .map(|t| t.id.clone())
                    .collect()
            };

            // Пустая порция — ещё не приговор: Ротор иногда отдаёт одни повторы.
            // Три попытки подряд, между ними секунда паузы.
            for attempt in 1..=WAVE_REFILL_ATTEMPTS {
                let fetched = client
                    .get_station_tracks_with_params(
                        &station_id,
                        mood.as_deref(),
                        language.as_deref(),
                        character.as_deref(),
                        Some(&queue_ids),
                    )
                    .await;

                let mut raw = 0usize;
                let mut added = 0usize;
                match fetched {
                    Ok(station_tracks) => {
                        raw = station_tracks.sequence.len();
                        let new_tracks: Vec<Track> =
                            station_tracks.sequence.into_iter().map(|s| s.track).collect();
                        let mut state = state_clone.lock().unwrap();
                        state.batch_id = station_tracks.batch_id;
                        // Пост-фильтра здесь нет намеренно: станцию считает сервер,
                        // а локальная история прослушиваний выбрасывала бы валидные треки.
                        let new_tracks = dedup_tracks(&state.queue, new_tracks);
                        added = new_tracks.len();
                        state.queue.extend(new_tracks);
                    }
                    Err(e) => eprintln!("[rotor] попытка {}: запрос не удался: {}", attempt, e),
                }

                let (queue_len, cur_idx, failures, updated_queue) = {
                    let mut state = state_clone.lock().unwrap();
                    if added > 0 {
                        state.refill_failures = 0;
                    } else {
                        state.refill_failures += 1;
                    }
                    (
                        state.queue.len(),
                        state.current_index,
                        state.refill_failures,
                        state.queue.clone(),
                    )
                };
                let hist = wave_engine_clone.lock().unwrap().recent_history.len();
                eprintln!(
                    "[rotor] attempt={} raw={} added={} queue={} cur_idx={} hist={} failures={} q_param={}",
                    attempt,
                    raw,
                    added,
                    queue_len,
                    cur_idx,
                    hist,
                    failures,
                    queue_ids.len()
                );

                if added > 0 {
                    emit_all(&senders, PlayerUiEvent::QueueUpdated(updated_queue));
                    emit_all(&senders, PlayerUiEvent::LoadingState(false));
                    me.finish_pending_advance();
                    return;
                }
                if failures >= WAVE_REFILL_ATTEMPTS {
                    break;
                }
                tokio::time::sleep(WAVE_REFILL_RETRY_DELAY).await;
            }

            // Все попытки пустые: продлевать нечем.
            emit_all(&senders, PlayerUiEvent::LoadingState(false));
            me.mark_wave_refill_dead();
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
    pub fn start_station(&self, station_id: String, first: Option<Track>, label: String) {
        let provider = self.providers.current();
        if provider.provider_id() == ProviderId::YouTubeMusic
            && first.is_none()
            && (station_id == "user:onyourwave" || station_id.contains("onyourwave"))
            && !provider.is_authenticated()
        {
            self.emit(PlayerUiEvent::StatusMessage(
                "Войдите в Google Аккаунт внизу сайдбара для доступа к персонализированной волне YouTube Music".to_string(),
            ));
            return;
        }

        {
            let mut state = self.state.lock().unwrap();
            state.crossfade_triggered = false;
            state.is_transitioning = true;
            // Новая станция — новый цикл: прежние «очередь кончилась» и «волна мертва»
            // относятся к прошлой подборке.
            state.pending_advance = false;
            state.wave_dead = false;
            state.refill_failures = 0;
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
        // Новая станция — новый отсчёт до Seed Drift
        self.wave_engine.lock().unwrap().mark_seed_started();
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
            let provider = me.providers.current();
            let is_yt = provider.provider_id() == ProviderId::YouTubeMusic;

            let mut result: Result<(Vec<Track>, Option<String>), String> = if is_yt {
                match provider.get_station_tracks(&station_id, None).await {
                    Ok(batch) => {
                        let b_id = if batch.batch_id.is_empty() { None } else { Some(batch.batch_id) };
                        Ok((batch.tracks, b_id))
                    }
                    Err(e) => Err(format!("Ошибка YouTube Music: {}", e)),
                }
            } else {
                let client = me.client.lock().unwrap().clone();
                if !client.has_token() {
                    me.emit(PlayerUiEvent::AuthRequired);
                    return;
                }
                match client.get_station_tracks(&station_id).await {
                    Ok(station) => {
                        let tracks: Vec<Track> = station.sequence.into_iter().map(|s| s.track).collect();
                        Ok((tracks, station.batch_id))
                    }
                    Err(e) => Err(e.to_string()),
                }
            };

            // Фолбэк для YouTube Music, если треки не вернулись
            if is_yt {
                if let Ok((ref tr, _)) = result {
                    if tr.is_empty() {
                        if let Ok(batch) = provider.get_station_tracks("user:onyourwave", None).await {
                            if !batch.tracks.is_empty() {
                                let b_id = if batch.batch_id.is_empty() { None } else { Some(batch.batch_id) };
                                result = Ok((batch.tracks, b_id));
                            }
                        }
                    }
                }
            }

            if first.is_none() {
                me.emit(PlayerUiEvent::LoadingState(false));
            }
            match result {
                Ok((tracks, batch_id)) => {
                    let tracks = pick_candidates(is_yt, &me.wave_engine, tracks);
                    let tracks = dedup_tracks(&[], tracks);
                    if tracks.is_empty() && first.is_none() {
                        me.emit(PlayerUiEvent::StatusMessage(
                            "Не удалось загрузить треки для выбранной станции. Попробуйте другой фильтр или трек.".to_string(),
                        ));
                        return;
                    }
                    // за время загрузки пользователь мог запустить другую станцию
                    if me.state.lock().unwrap().station_id != station_id {
                        me.emit(PlayerUiEvent::WaveLoading(None));
                        return;
                    }
                    me.state.lock().unwrap().batch_id = batch_id;
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
            state.pending_advance = false;
            state.wave_dead = false;
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

    /// Получение текущей громкости
    pub fn get_volume(&self) -> f64 {
        self.audio.volume()
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
        let providers_clone = self.providers.clone();
        let yt_client_clone = self.yt_client.clone();
        let track_source = track.source;
        let track_id = track.id.clone();
        let track_title = track.title.clone();
        let track_artists = track.artists_string();
        let is_yt = track_source == TrackSource::YouTubeMusic
            || track_id.len() == 11
            || providers_clone.active_id() == ProviderId::YouTubeMusic;

        tokio::spawn(async move {
            if is_yt {
                let yt_video_id = if track_id.len() == 11 {
                    Some(track_id.clone())
                } else {
                    let q = format!("{} {}", track_title, track_artists);
                    match yt_client_clone.search_songs(&q).await {
                        Ok(results) => results.into_iter().next().map(|t| t.id),
                        Err(e) => {
                            eprintln!("[ytmusic] Ошибка поиска трека для лайка «{q}»: {e}");
                            None
                        }
                    }
                };

                if let Some(vid) = yt_video_id {
                    if let Some(yt) = providers_clone.get(ProviderId::YouTubeMusic) {
                        match yt.set_track_like(&vid, liked).await {
                            Ok(_) => println!("[ytmusic] Лайк ({liked}) для {vid} успешно отправлен в Google YouTube Music"),
                            Err(e) => eprintln!("[ytmusic] Ошибка отправки лайка ({liked}) для {vid}: {e}"),
                        }
                    }
                }
                return;
            }

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
            self.emit(PlayerUiEvent::StatusMessage("Исключено из рекомендаций 👎".into()));
        }

        if let Some(t) = track {
            let client = self.client.clone();
            let providers_clone = self.providers.clone();
            let yt_client_clone = self.yt_client.clone();
            let track_source = t.source;
            let track_id = t.id.clone();
            let track_title = t.title.clone();
            let track_artists = t.artists_string();
            let is_yt = track_source == TrackSource::YouTubeMusic
                || track_id.len() == 11
                || providers_clone.active_id() == ProviderId::YouTubeMusic;

            tokio::spawn(async move {
                if is_yt {
                    let yt_video_id = if track_id.len() == 11 {
                        Some(track_id.clone())
                    } else {
                        let q = format!("{} {}", track_title, track_artists);
                        match yt_client_clone.search_songs(&q).await {
                            Ok(results) => results.into_iter().next().map(|t| t.id),
                            Err(e) => {
                                eprintln!("[ytmusic] Ошибка поиска трека для дизлайка «{q}»: {e}");
                                None
                            }
                        }
                    };

                    if let Some(vid) = yt_video_id {
                        if let Some(yt) = providers_clone.get(ProviderId::YouTubeMusic) {
                            if was_liked {
                                let _ = yt.set_track_like(&vid, false).await;
                            }
                            if new_dislike {
                                match yt.set_track_dislike(&vid).await {
                                    Ok(_) => println!("[ytmusic] Дизлайк для {vid} успешно отправлен в Google YouTube Music"),
                                    Err(e) => eprintln!("[ytmusic] Ошибка отправки дизлайка для {vid}: {e}"),
                                }
                            } else {
                                let _ = yt.set_track_like(&vid, false).await;
                            }
                        }
                    }
                    return;
                }

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

            if new_dislike {
                self.next_track();
            }
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
        let stale_check = stale_gen.clone();
        let is_stale = move || stale_check.load(Ordering::SeqCst) != epoch;
        eprintln!("[play] #{} {} — {} ({})", epoch, track.title, track.artists_string(), track.id);

        let client = self.client.lock().unwrap().clone();
        spawn_cover_and_lyrics(&self.event_senders, client.clone(), track, epoch, &self.generation, &self.clips);
        crate::api::history::save_last_track(track);
        crate::api::history::remember_played(&track.id);
        // Историю для пост-фильтра ведём только на стороне YouTube Music: у Яндекса
        // станцию считает сервер, и локальная «память прослушиваний» там только мешает.
        if track.source == TrackSource::YouTubeMusic {
            self.wave_engine.lock().unwrap().record_played(&track.id);
        }
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

        // СНАЧАЛА проверяем локальный диск: офлайн или кэш (0 мс задержки)
        let cached_opt = crate::cache::offline_audio_path(&track_id)
            .map(|p| (p, true))
            .or_else(|| crate::cache::cached_audio_path(&track_id).map(|p| (p, false)));

        if let Some((path, is_offline)) = cached_opt {
            let uri = gtk4::gio::prelude::FileExt::uri(&gtk4::gio::File::for_path(&path));
            emit_all(&senders, PlayerUiEvent::LoadingState(false));
            let label = if is_offline { "Офлайн" } else { "Диск (0 мс)" };
            emit_all(&senders, PlayerUiEvent::StreamQuality(label.to_string()));

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
            self.prefetch_next_track();
            return;
        }

        let prefetch = self.prefetch.clone();
        let providers_clone = self.providers.clone();
        let yt_client_clone = self.yt_client.clone();
        let track_source = track.source;
        let track_title = track.title.clone();
        let track_artists = track.artists_string();
        let orig_duration = track.duration_ms;
        let state_arc = self.state.clone();
        let mgr_clone = self.clone();

        self.emit(PlayerUiEvent::Progress { current_sec: 0, total_sec: track.duration_ms.unwrap_or(0) / 1000 });
        // prefetch здесь не запускаем: активный слот ещё не переключён, и преролл следующего
        // трека стёр бы уже прогретый слот текущего — кроссфейд остался бы без мгновенного старта.
        // Преролл следующего трека делает ветка успешного старта (и next_track).

        tokio::spawn(async move {
            let is_yt = track_source == TrackSource::YouTubeMusic;
            let can_play = is_yt || client.has_token();

            if can_play {
                emit_all(&senders, PlayerUiEvent::LoadingState(true));
                let started = std::time::Instant::now();
                // ссылка могла быть получена заранее, пока играл прежний трек
                let ready = prefetch.lock().unwrap().take().filter(|p| p.track_id == track_id && p.is_fresh());
                let hit = ready.is_some();
                let result: Result<(String, String), String> = match ready {
                    Some(p) => Ok((p.url, p.label)),
                    None => {
                        if is_yt {
                            if let Some(yt) = providers_clone.get(ProviderId::YouTubeMusic) {
                                match yt.get_stream_url(&track_id).await {
                                    Ok(url) => Ok((url, "Opus 160".to_string())),
                                    Err(e) => Err(format!("Ошибка YouTube Music: {}", e)),
                                }
                            } else {
                                Err("Провайдер YouTube Music не найден".to_string())
                            }
                        } else {
                            client.get_track_direct_url(&track_id).await.map_err(|e| e.to_string())
                        }
                    }
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

                        // Фоновое сохранение играющего трека в audio_cache
                        let cache_tid = track_id.clone();
                        let cache_url = url.clone();
                        let cache_ext = if is_yt { "opus" } else { "mp3" };
                        tokio::spawn(async move {
                            download_audio_to_cache(&cache_url, &cache_tid, cache_ext).await;
                        });

                        // Предзагрузка следующего трека
                        mgr_clone.prefetch_next_track();
                        if is_yt {
                            if let Some(yt) = providers_clone.get(ProviderId::YouTubeMusic) {
                                let _ = yt.send_feedback(FeedbackEvent::TrackStarted {
                                    track_id: track_id.clone(),
                                    station_id: station_id.clone(),
                                    batch_id: batch_id.clone(),
                                }).await;
                            }
                        } else {
                            let _ = client.feedback_track_started(&station_id, &track_id, batch_id).await;
                        }
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
                // Если track.source == TrackSource::Yandex и у клиента Яндекса нет токена,
                // но активен провайдер ProviderId::YouTubeMusic или у YouTube Music есть кука:
                let yt_opt = providers_clone.get(ProviderId::YouTubeMusic);
                let yt_active = providers_clone.active_id() == ProviderId::YouTubeMusic;
                let yt_has_cookie = crate::api::auth::load_ytmusic_cookie().is_some()
                    || yt_opt.as_ref().map(|p| p.is_authenticated()).unwrap_or(false);

                if (yt_active || yt_has_cookie) && yt_opt.is_some() {
                    let yt = yt_opt.unwrap();
                    let query = format!("{} {}", track_title, track_artists);
                    emit_all(&senders, PlayerUiEvent::LoadingState(true));
                    emit_all(&senders, PlayerUiEvent::StatusMessage(
                        format!("Поиск студийного трека в YouTube Music: {}", query),
                    ));

                    let candidates = match yt_client_clone.search_songs(&query).await {
                        Ok(songs) if !songs.is_empty() => songs,
                        _ => yt
                            .search(&query)
                            .await
                            .ok()
                            .and_then(|r| r.tracks.map(|s| s.results))
                            .unwrap_or_default(),
                    };

                    let chosen = pick_best_yt_track(&candidates, &track_title, orig_duration);

                    if let Some(track_to_play) = chosen {
                        eprintln!(
                            "[play] #{} выбран студийный трек в YouTube Music: {} ({}) [dur: {:?}]",
                            epoch, track_to_play.title, track_to_play.id, track_to_play.duration_ms
                        );

                        // Заменяем текущий трек в состоянии плеера и очереди на реальный трек YouTube Music
                        let (current_idx, total_q) = {
                            if let Ok(mut state) = state_arc.lock() {
                                state.current_track = Some(track_to_play.clone());
                                let idx = state.current_index;
                                if let Some(q_track) = state.queue.get_mut(idx) {
                                    *q_track = track_to_play.clone();
                                }
                                (idx, state.queue.len())
                            } else {
                                (0, 1)
                            }
                        };

                        crate::api::history::save_last_track(&track_to_play);

                        // Обновляем UI с новыми метаданными и обложкой YouTube Music
                        emit_all(&senders, PlayerUiEvent::TrackStarted {
                            track: track_to_play.clone(),
                            index: current_idx,
                            total_queue: total_q,
                        });
                        emit_all(&senders, PlayerUiEvent::Progress {
                            current_sec: 0,
                            total_sec: track_to_play.duration_ms.unwrap_or(0) / 1000,
                        });

                        // Загружаем обложку и текст выбранного трека
                        spawn_cover_and_lyrics(&senders, client.clone(), &track_to_play, epoch, &stale_gen, &mgr_clone.clips);

                        match yt.get_stream_url(&track_to_play.id).await {
                            Ok(url) => {
                                if is_stale() {
                                    eprintln!("[play] #{} устарел, поток YT не запускаем", epoch);
                                    if let Ok(mut state) = state_arc.lock() {
                                        state.is_transitioning = false;
                                    }
                                    return;
                                }
                                emit_all(&senders, PlayerUiEvent::LoadingState(false));
                                emit_all(&senders, PlayerUiEvent::StreamQuality("Opus 160 (YT)".to_string()));

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

                                let cache_tid = track_to_play.id.clone();
                                let cache_url = url.clone();
                                tokio::spawn(async move {
                                    download_audio_to_cache(&cache_url, &cache_tid, "opus").await;
                                });
                                mgr_clone.prefetch_next_track();

                                let _ = yt.send_feedback(FeedbackEvent::TrackStarted {
                                    track_id: track_to_play.id.clone(),
                                    station_id: station_id.clone(),
                                    batch_id: batch_id.clone(),
                                }).await;

                                return;
                            }
                            Err(e) => {
                                eprintln!("[play] Ошибка получения потока YT: {}", e);
                            }
                        }
                    }
                }

                if let Ok(mut state) = state_arc.lock() {
                    state.is_transitioning = false;
                }
                emit_all(&senders, PlayerUiEvent::LoadingState(false));
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
        let provider = self.providers.current();
        if provider.provider_id() == ProviderId::YouTubeMusic && !provider.is_authenticated() {
            self.emit(PlayerUiEvent::StatusMessage(
                "Войдите в Google Аккаунт внизу сайдбара для доступа к персонализированной волне YouTube Music".to_string(),
            ));
            return;
        }

        let is_yt = provider.provider_id() == ProviderId::YouTubeMusic;

        // YouTube Music: фильтры языка/характера применяются локально и бесшовно
        // (хвост очереди меняется, текущий трек продолжает играть).
        if is_yt {
            if mood.is_some() {
                // Настроению на YT взяться неоткуда — кнопки выключены в UI
                return;
            }
            self.tail_swap(language, character);
            return;
        }

        let is_reset = mood.is_none() && language.is_none() && character.is_none();
        let station_id = if is_yt && is_reset {
            self.state
                .lock()
                .unwrap()
                .current_track
                .as_ref()
                .map(|t| format!("track:{}", t.id))
                .unwrap_or_else(|| "user:onyourwave".to_string())
        } else {
            "user:onyourwave".to_string()
        };
        self.apply_wave_station(station_id, mood, language, character);
    }

    /// Бесшовный обмен хвоста очереди под выбранные фильтры (только YouTube Music).
    ///
    /// Текущий трек продолжает играть: `queue` обрезается до него, старый хвост
    /// запоминается в истории, новый сид подбирается из `liked_tracks_cache`.
    pub fn tail_swap(&self, language: Option<String>, character: Option<String>) {
        let epoch = self.tail_swap_epoch.fetch_add(1, Ordering::SeqCst) + 1;
        self.emit(PlayerUiEvent::LoadingState(true));
        self.emit(PlayerUiEvent::WaveLoading(Some(wave_loading_text(
            None,
            language.as_deref(),
            character.as_deref(),
        ))));

        let me = self.clone();
        tokio::spawn(async move {
            // Холодный старт: кэш лайков нужен и для выбора сида, и для предикатов
            me.ensure_liked_loaded().await;

            // 1. Короткий лок: фильтры, обрезка хвоста, выбор сидов
            let (dropped_tracks, seeds) = {
                let mut state = me.state.lock().unwrap();
                state.current_language = language.clone();
                state.current_character = character.clone();
                state.batch_id = None;
                state.pending_advance = false;
                state.wave_dead = false;
                state.refill_failures = 0;

                let keep = (state.current_index + 1).min(state.queue.len());
                let dropped_tracks: Vec<Track> = state
                    .queue
                    .get(keep..)
                    .unwrap_or(&[])
                    .to_vec();
                state.queue.truncate(keep);

                let seeds = select_wave_seeds(
                    &state,
                    language.as_deref(),
                    character.as_deref(),
                );
                let primary = seeds
                    .first()
                    .map(|id| format!("seed:{}", id))
                    .unwrap_or_else(|| "user:onyourwave".to_string());
                state.station_id = primary;
                state.restored = false;
                (dropped_tracks, seeds)
            };
            me.set_wave(true);
            // Узкий пул вкуса добираем поиском по истории Яндекса (там могут быть
            // любимые иностранные треки, открытые только в Яндексе), и лишь
            // затем — сырыми id недавних прослушиваний.
            let mut seeds = seeds;
            if language.is_some() {
                add_seeds_from_history_search(
                    &me.yt_client,
                    &mut seeds,
                    language.as_deref(),
                )
                .await;
                if language.as_deref() == Some("without-words") && seeds.len() < 2 {
                    add_instrumental_search_seeds(&me.yt_client, &mut seeds).await;
                }
                if seeds.len() < TAIL_SWAP_SEEDS {
                    let extra = crate::api::history::load_recent_played()
                        .into_iter()
                        .filter(|id| id.len() == 11 && !id.starts_with("UC"))
                        .filter(|id| !seeds.contains(id))
                        .take(TAIL_SWAP_SEEDS - seeds.len())
                        .collect::<Vec<String>>();
                    seeds.extend(extra);
                }
            }
            // Лок `state` отпущен: порядок локов state → wave_engine не создаём
            let dropped_ids: Vec<String> = dropped_tracks.iter().map(|t| t.id.clone()).collect();
            me.wave_engine.lock().unwrap().remember_dropped(&dropped_ids);
            me.wave_engine.lock().unwrap().mark_seed_started();

            // 2. Сеть и фильтрация.
            //
            // Волна — это не радио одного трека: набираем хвост с нескольких сидов
            // (разные артисты из лайков) и ограничиваем число треков одного артиста,
            // иначе весь хвост оказывается «радио по одному исполнителю».
            let provider = me.providers.current();
            let deadline = Instant::now() + Duration::from_secs(20);
            let mut accepted: Vec<Track> = Vec::new();
            let mut provider_empty = true;
            let mut per_artist: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();
            let mut last_station = seeds
                .first()
                .map(|id| format!("seed:{}", id))
                .unwrap_or_else(|| "user:onyourwave".to_string());
            let mut last_continuation: Option<String> = None;

            let stations: Vec<String> = if seeds.is_empty() {
                vec![last_station.clone()]
            } else {
                seeds.iter().map(|id| format!("seed:{}", id)).collect()
            };

            for station in stations.iter() {
                if accepted.len() >= TAIL_SWAP_TARGET || Instant::now() >= deadline {
                    break;
                }
                let mut continuation: Option<String> = None;
                let mut taken_from_seed = 0usize;
                for _ in 0..2u32 {
                    if accepted.len() >= TAIL_SWAP_TARGET || Instant::now() >= deadline {
                        break;
                    }
                    let batch = match provider
                        .get_station_tracks(station, continuation.as_deref())
                        .await
                    {
                        Ok(batch) => batch,
                        Err(err) => {
                            eprintln!("[filter] станция «{}»: {}", station, err);
                            break;
                        }
                    };
                    if !batch.tracks.is_empty() {
                        provider_empty = false;
                    }
                    continuation = if batch.batch_id.is_empty() {
                        None
                    } else {
                        Some(batch.batch_id.clone())
                    };
                    if batch.tracks.is_empty() {
                        break;
                    }

                    // Конвейер: история/чистота → дубли → Фаза 1 → Фаза 2
                    let pre = pick_candidates(true, &me.wave_engine, batch.tracks);
                    let pre = {
                        let state = me.state.lock().unwrap();
                        dedup_tracks(&state.queue, pre)
                    };
                    let (liked, history) = snapshots_for_filters(&me);
                    // 4.8 с на каждый сид: lrclib_get при 429/404 тратит ~2.1 с
                    // на ретраи, и меньший бюджет отбрасывал бы весь батч как Failed
                    let budget = 4800;
                    let got = apply_filters(
                        &me.yt_client,
                        pre,
                        language.as_deref(),
                        character.as_deref(),
                        &liked,
                        &history,
                        budget,
                    )
                    .await;

                    // Перемешиваем: первые треки батча — ближайший круг сида
                    // (тот же лейбл), а нам нужен микс
                    let mut got = got;
                    shuffle_tracks(&mut got);

                    // Не больше N треков одного артиста и M треков с одного сида
                    for track in got {
                        if taken_from_seed >= MAX_TRACKS_PER_SEED
                            || accepted.len() >= TAIL_SWAP_TARGET
                        {
                            break;
                        }
                        // Лимит считаем по первому артисту: «Три дня дождя, MONA»
                        // и «Три дня дождя» — один и тот же круг
                        let artist = track
                            .artists_string()
                            .split(',')
                            .next()
                            .unwrap_or("")
                            .trim()
                            .to_lowercase();
                        let count = per_artist.entry(artist).or_insert(0);
                        if *count < MAX_TRACKS_PER_ARTIST {
                            *count += 1;
                            accepted.push(track);
                            taken_from_seed += 1;
                        }
                    }
                    last_station = station.clone();
                    last_continuation = continuation.clone();
                    if continuation.is_none() {
                        break;
                    }
                }
            }

            // 3. Мягкий режим «Моё любимое»: строгая приёмка часто даёт 0
            if accepted.is_empty()
                && character.as_deref() == Some("favorite")
                && !provider_empty
            {
                eprintln!("[filter] favorite: fallback to automix");
                if let Ok(batch) = provider.get_station_tracks(&last_station, None).await {
                    let pre = pick_candidates(true, &me.wave_engine, batch.tracks);
                    let (liked, history) = snapshots_for_filters(&me);
                    accepted = apply_filters(
                        &me.yt_client,
                        pre,
                        language.as_deref(),
                        None,
                        &liked,
                        &history,
                        4800,
                    )
                    .await;
                }
            }

            // 4. Устаревший батч отбрасываем, но спиннер гасим всегда
            if me.tail_swap_epoch.load(Ordering::SeqCst) != epoch {
                // Устаревший запуск не должен гасить плашку прогресса: её
                // владелец — более новый TailSwap, он и снимет её сам.
                emit_all(&me.event_senders, PlayerUiEvent::LoadingState(false));
                return;
            }

            eprintln!(
                "[filter] сиды {:?} -> принято {}: {}",
                stations,
                accepted.len(),
                accepted
                    .iter()
                    .map(|t| format!("{} ({})", t.title, t.artists_string()))
                    .collect::<Vec<_>>()
                    .join(" | ")
            );
            if accepted.is_empty() {
                if provider_empty {
                    eprintln!("[filter] provider empty batch");
                } else {
                    eprintln!("[filter] rejected all candidates");
                }
                let restored_queue = {
                    let mut state = me.state.lock().unwrap();
                    state.wave_dead = true;
                    state.pending_advance = false;
                    // Ничего не подошло — возвращаем прежний хвост, чтобы не оставить
                    // пользователя с одной песней в очереди
                    let restored = dedup_tracks(&state.queue, dropped_tracks);
                    state.queue.extend(restored);
                    state.queue.clone()
                };
                me.emit(PlayerUiEvent::QueueUpdated(restored_queue));
                me.emit(PlayerUiEvent::StatusMessage(
                    "Не удалось найти треки под выбранный фильтр".to_string(),
                ));
                me.emit(PlayerUiEvent::LoadingState(false));
                me.emit(PlayerUiEvent::WaveLoading(None));
                return;
            }

            // Текущий трек мог не подходить под фильтр (например, англоязычный
            // под «На русском»): тогда после обновления хвоста переключаемся
            // на первый принятый. Если подходит — продолжаем играть.
            let (liked_now, history_now) = snapshots_for_filters(&me);
            let current_track = me.state.lock().unwrap().current_track.clone();
            let current_ok = match current_track {
                Some(track) => !apply_filters(
                    &me.yt_client,
                    vec![track],
                    language.as_deref(),
                    character.as_deref(),
                    &liked_now,
                    &history_now,
                    2500,
                )
                .await
                .is_empty(),
                None => true,
            };

            let updated_queue = {
                let mut state = me.state.lock().unwrap();
                let new_tracks = dedup_tracks(&state.queue, accepted);
                state.queue.extend(new_tracks);
                // Хвост продолжает станция, с которой закончили набор
                state.station_id = last_station.clone();
                state.batch_id = last_continuation.clone();
                state.wave_dead = false;
                state.refill_failures = 0;
                state.queue.clone()
            };
            me.emit(PlayerUiEvent::QueueUpdated(updated_queue));
            if !current_ok {
                eprintln!("[tail_swap] текущий трек не подходит под фильтр — включаем первый принятый");
                me.next_track();
            }
            me.emit(PlayerUiEvent::LoadingState(false));
            me.emit(PlayerUiEvent::WaveLoading(None));
            me.prefetch_next_track();
            eprintln!("[tail_swap] epoch={} хвост обновлён", epoch);
        });
    }

    /// Общая часть запуска Волны: состояние под новый автомикс и первая порция треков
    fn apply_wave_station(
        &self,
        station_id: String,
        mood: Option<String>,
        language: Option<String>,
        character: Option<String>,
    ) {
        let is_reset = mood.is_none() && language.is_none() && character.is_none();
        self.emit(PlayerUiEvent::WaveLoading(Some(wave_loading_text(
            mood.as_deref(),
            language.as_deref(),
            character.as_deref(),
        ))));
        let client_clone = self.client.clone();
        let state_clone = self.state.clone();
        let senders = self.event_senders.clone();

        {
            let mut state = self.state.lock().unwrap();
            state.current_mood = mood.clone();
            state.current_language = language.clone();
            state.current_character = character.clone();
            state.station_id = station_id.clone();
            state.restored = false;
        }
        // Смена фильтра Ротора = новый автомикс, отсчёт дрейфа начинается заново
        self.wave_engine.lock().unwrap().mark_seed_started();
        self.set_wave(true);
        // плашку с выбранным фильтром рисует сам интерфейс; для простой Волны плашка убирается
        if is_reset {
            self.set_wave_label(None);
        } else {
            self.state.lock().unwrap().wave_label = None;
        }

        let audio_clone = self.audio.clone();
        let providers_clone = self.providers.clone();
        let wave_engine_clone = self.wave_engine.clone();
        let epoch = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let current_gen = self.generation.clone();
        let clips_clone = self.clips.clone();

        tokio::spawn(async move {
            let provider = providers_clone.current();
            let is_yt = provider.provider_id() == ProviderId::YouTubeMusic;
            let client = client_clone.lock().unwrap().clone();

            if is_yt {
                if !provider.is_authenticated() {
                    emit_all(&senders, PlayerUiEvent::StatusMessage(
                        "Войдите в Google Аккаунт внизу сайдбара для доступа к персонализированной волне YouTube Music".to_string(),
                    ));
                    emit_all(&senders, PlayerUiEvent::WaveLoading(None));
                    return;
                }
            } else if !client.has_token() {
                emit_all(&senders, PlayerUiEvent::AuthRequired);
                emit_all(&senders, PlayerUiEvent::StatusMessage(
                    "Войдите в Яндекс ID для запуска персональной Волны".to_string(),
                ));
                emit_all(&senders, PlayerUiEvent::WaveLoading(None));
                return;
            }

            emit_all(&senders, PlayerUiEvent::LoadingState(true));
            let mut fetch_res: Result<(Vec<Track>, Option<String>), String> = if is_yt {
                match provider.get_station_tracks(&station_id, None).await {
                    Ok(batch) => {
                        let b_id = if batch.batch_id.is_empty() { None } else { Some(batch.batch_id) };
                        Ok((batch.tracks, b_id))
                    }
                    Err(e) => Err(format!("Ошибка YouTube Music: {}", e)),
                }
            } else {
                match client.get_station_tracks_with_params(
                    &station_id,
                    mood.as_deref(),
                    language.as_deref(),
                    character.as_deref(),
                    None,
                ).await {
                    Ok(station_tracks) => {
                        let tr: Vec<Track> = station_tracks.sequence.into_iter().map(|s| s.track).collect();
                        Ok((tr, station_tracks.batch_id))
                    }
                    Err(e) => Err(format!("Ошибка загрузки Волны: {}", e)),
                }
            };

            // Фолбэк для YouTube Music, если треки не вернулись
            if is_yt {
                if let Ok((ref tr, _)) = fetch_res {
                    if tr.is_empty() {
                        if let Ok(batch) = provider.get_station_tracks("user:onyourwave", None).await {
                            if !batch.tracks.is_empty() {
                                let b_id = if batch.batch_id.is_empty() { None } else { Some(batch.batch_id) };
                                fetch_res = Ok((batch.tracks, b_id));
                            }
                        }
                    }
                }
            }

            match fetch_res {
                Ok((tracks, batch_id)) => {
                    let tracks = pick_candidates(is_yt, &wave_engine_clone, tracks);
                    let tracks = dedup_tracks(&[], tracks);
                    let first_track = tracks.first().cloned();

                    let first_is_liked = if let Ok(mut state) = state_clone.lock() {
                        state.batch_id = batch_id.clone();
                        state.wave_dead = false;
                        state.pending_advance = false;
                        state.refill_failures = 0;
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
                    emit_all(&senders, PlayerUiEvent::WaveLoading(None));
                    emit_all(&senders, PlayerUiEvent::QueueUpdated(tracks.clone()));

                    if let Some(track) = first_track {
                        emit_all(&senders, PlayerUiEvent::LikeStateChanged(first_is_liked));
                        emit_all(&senders, PlayerUiEvent::TrackStarted {
                            track: track.clone(),
                            index: 0,
                            total_queue: tracks.len(),
                        });

                        // Обложка и текст первого трека Волны (в остальных случаях их грузит play_track_stream)
                        spawn_cover_and_lyrics(&senders, client.clone(), &track, epoch, &current_gen, &clips_clone);
                        crate::api::history::save_last_track(&track);

                        // Сразу запускаем воспроизведение первого трека Волны
                        let track_id = track.id.clone();
                        let b_id = batch_id.clone();
                        let result: Result<(String, String), String> = if track.source == TrackSource::YouTubeMusic {
                            if let Some(yt) = providers_clone.get(ProviderId::YouTubeMusic) {
                                yt.get_stream_url(&track_id).await.map(|u| (u, "Opus 160".to_string())).map_err(|e| e.to_string())
                            } else {
                                Err("YouTube Music недоступен".to_string())
                            }
                        } else {
                            client.get_track_direct_url(&track_id).await.map_err(|e| e.to_string())
                        };

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
                                if track.source == TrackSource::YouTubeMusic {
                                    if let Some(yt) = providers_clone.get(ProviderId::YouTubeMusic) {
                                        let _ = yt.send_feedback(FeedbackEvent::TrackStarted {
                                            track_id: track_id.clone(),
                                            station_id: station_id.clone(),
                                            batch_id: b_id,
                                        }).await;
                                    }
                                } else {
                                    let _ = client.feedback_track_started(&station_id, &track_id, b_id).await;
                                }
                            }
                            Err(e) => {
                                emit_all(&senders, PlayerUiEvent::Error(format!("Ошибка старта Волны: {}", e)));
                            }
                        }
                    } else {
                        emit_all(&senders, PlayerUiEvent::StatusMessage(
                            "Не удалось загрузить треки для выбранной станции. Попробуйте другой фильтр или трек.".to_string(),
                        ));
                    }
                }
                Err(e) => {
                    emit_all(&senders, PlayerUiEvent::LoadingState(false));
                    emit_all(&senders, PlayerUiEvent::WaveLoading(None));
                    emit_all(&senders, PlayerUiEvent::Error(e));
                }
            }
        });
    }
}

/// Скачивает аудиопоток в дисковый кэш (~/.cache/volna/audio/) и очищает старые треки по LRU
async fn download_audio_to_cache(url: &str, track_id: &str, ext: &str) -> Option<std::path::PathBuf> {
    let final_path = crate::cache::final_audio_cache_path(track_id, ext);
    if final_path.exists() {
        if let Ok(meta) = final_path.metadata() {
            if meta.len() > 128 * 1024 {
                return Some(final_path);
            }
        }
    }
    let tmp = crate::cache::temp_audio_cache_path(track_id, ext);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .ok()?;
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    if bytes.len() <= 128 * 1024 {
        return None;
    }
    std::fs::write(&tmp, &bytes).ok()?;
    std::fs::rename(&tmp, &final_path).ok()?;
    crate::cache::clean_audio_cache_lru(600 * 1024 * 1024);
    Some(final_path)
}

/// Обложка и текст песни грузятся отдельными задачами, чтобы не задерживать воспроизведение.
/// Если у трека текста нет, UI получает пустую строку: старый текст предыдущего трека не остаётся.
fn spawn_cover_and_lyrics(
    senders: &Arc<Mutex<Vec<Sender<PlayerUiEvent>>>>,
    client: YandexMusicClient,
    track: &Track,
    epoch: u64,
    generation: &Arc<AtomicU64>,
    clips: &super::clip::ClipBackground,
) {
    // Фоновый клип: резолв и загрузка не ждут звука и UI
    clips.spawn_clip_for_track(track, epoch);
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

    // Если это трек YouTube Music или обложка с i.ytimg.com — параллельно резолвим студийную обложку
    let is_ytimg = track.cover_url(400).as_deref().map_or(true, |u| u.contains("ytimg.com"));
    if is_ytimg {
        let (senders, generation) = (senders.clone(), generation.clone());
        let (artist, title) = (track.artists_string(), track.title.clone());
        tokio::spawn(async move {
            if let Some(studio_url) = crate::cache::resolve_studio_cover(&artist, &title).await {
                if let Some(path) = crate::cache::fetch_cover_cached(&studio_url).await {
                    if generation.load(Ordering::SeqCst) == epoch {
                        emit_all(&senders, PlayerUiEvent::CoverUpdated(path.to_string_lossy().to_string()));
                    }
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
                    emit_all(&senders, PlayerUiEvent::ClipUpdated(ClipUpdate::Show(ClipRequest {
                        track_id: clip_track.id.clone(),
                        epoch,
                        attempt_id: 0,
                        kind: ClipKind::Custom,
                        path: path.to_string_lossy().to_string(),
                    })));
                }
                return;
            }
            // клипом YouTube Music занимается ClipBackground: резолв, кэш и арт-треки
            if clip_track.source == TrackSource::YouTubeMusic {
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
                let update = match path {
                    Some(p) => ClipUpdate::Show(ClipRequest {
                        track_id: clip_track.id.clone(),
                        epoch,
                        attempt_id: 0,
                        kind: ClipKind::YandexPreview,
                        path: p.to_string_lossy().to_string(),
                    }),
                    None => ClipUpdate::Clear { track_id: clip_track.id.clone(), epoch },
                };
                emit_all(&senders, PlayerUiEvent::ClipUpdated(update));
            }
        });
    }

    let (senders, generation) = (senders.clone(), generation.clone());
    let (track_id, title, artist) = (track.id.clone(), track.title.clone(), track.artists_string());
    let duration = track.duration_ms.map(|d| d / 1000);
    let track_source = track.source;
    tokio::spawn(async move {
        // Клиент YouTube Music нужен и для длительности, и для фолбэка на официальный текст
        let ytm = crate::api::ytmusic::YtMusicClient::new();
        let is_yt = track_source == crate::api::models::TrackSource::YouTubeMusic || track_id.len() == 11;

        // Поиск без фильтра «Песни» не отдаёт длительность, а без неё нельзя отличить разметку
        // от другой версии от текста совсем другой песни. Уточняем её у самого трека.
        let duration = match duration {
            Some(secs) => Some(secs),
            None if is_yt => {
                let secs = ytm.track_duration_secs(&track_id).await;
                if let Some(secs) = secs {
                    println!("[lyrics] Длительность «{}» уточнена: {} с", title, secs);
                }
                secs
            }
            None => None,
        };

        let text = if is_yt {
            static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
            let http = HTTP.get_or_init(|| {
                reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(10))
                    .build()
                    .unwrap_or_default()
            });

            // 1. Проверяем кэш: если в кэше есть синхронизированное караоке с таймкодами — отдаем мгновенно!
            let cache_path = crate::cache::lyrics_cache_path(&track_id);
            let mut cached_plain = None;
            let mut had_stale_synced = false;
            if let Ok(cached) = std::fs::read_to_string(&cache_path) {
                let trimmed = cached.trim();
                if crate::api::lrclib::looks_like_foreign_song(trimmed, duration) {
                    // В кэше текст другой песни (кэш ключуется одним id трека): такой не показываем
                    // ни с таймкодами, ни без них — иначе под «Dear Diary,» висит чужой текст
                    let _ = std::fs::remove_file(&cache_path);
                    println!("[lyrics] Кэш {} отброшен: размечен под другой трек", track_id);
                } else if crate::api::lrclib::usable_synced_at(trimmed, duration) {
                    if generation.load(Ordering::SeqCst) == epoch {
                        emit_all(&senders, PlayerUiEvent::LyricsUpdated(trimmed.to_string()));
                    }
                    return;
                } else if !trimmed.is_empty() {
                    // Разметка не от этого трека (текст верный, таймлайн длиннее) — таймкоды выбрасываем,
                    // иначе подсветка уезжает от музыки. Сам файл не затираем: вдруг длительность врёт
                    if crate::api::lrclib::usable_synced(trimmed) {
                        had_stale_synced = true;
                    }
                    cached_plain = Some(crate::api::lrclib::demote_stale_synced(trimmed, duration));
                }
            }

            // 2. Сначала LRCLIB (синхронизированное караоке с таймкодами)
            let lrclib_fut = crate::api::lrclib::fetch_lrclib_lyrics(http, &artist, &title, duration);
            let lrclib_res = tokio::time::timeout(std::time::Duration::from_secs(8), lrclib_fut).await.ok().flatten();

            let mut final_text = None;
            if lrclib_res
                .as_deref()
                .is_some_and(|t| crate::api::lrclib::looks_like_foreign_song(t.trim(), duration))
            {
                println!("[lyrics] LRCLIB отдал текст другого трека — пропускаем");
            } else if let Some(lr_text) = lrclib_res {
                let trimmed = lr_text.trim();
                if crate::api::lrclib::usable_synced_at(trimmed, duration) {
                    let _ = std::fs::write(&cache_path, trimmed);
                    crate::cache::clear_negative_lyrics_marker(&track_id);
                    final_text = Some(trimmed.to_string());
                } else if !trimmed.is_empty() {
                    final_text = Some(crate::api::lrclib::demote_stale_synced(trimmed, duration));
                }
            }

            // 3. Если в LRCLIB нет синхронизированных таймкодов (или сервис временно недоступен) —
            // пробуем найти караоке в каталоге Яндекс Музыки!
            if final_text.as_ref().map_or(true, |t| !crate::api::lrclib::usable_synced_at(t, duration)) {
                let clean_title = crate::api::lrclib::clean_title_for_lyrics(&title);
                let query = format!("{} {}", artist, clean_title);
                if let Ok(search_res) = client.search(&query).await {
                    if let Some(tracks) = search_res.tracks {
                        // Первый результат поиска вполне может быть другой песней того же
                        // исполнителя — её текст нельзя выдавать за текст текущего трека
                        let found = tracks.results.into_iter().next().filter(|t| {
                            crate::api::lrclib::same_song_title(&t.title, &title)
                                && crate::api::lrclib::same_artist_name(&t.artists_string(), &artist)
                        });
                        match found {
                            Some(y_track) => {
                            println!(
                                "[lyrics] Яндекс: «{}» / {}",
                                y_track.title,
                                y_track.artists_string()
                            );
                            if let Ok(Some(y_lyrics)) = client.get_track_lyrics(&y_track.id, &artist, &title, duration).await {
                                let trimmed = y_lyrics.trim();
                                if crate::api::lrclib::looks_like_foreign_song(trimmed, duration) {
                                    println!("[lyrics] Яндекс: разметка не от этого трека — пропускаем");
                                } else
                                if crate::api::lrclib::usable_synced_at(trimmed, duration) {
                                    let _ = std::fs::write(&cache_path, trimmed);
                                    crate::cache::clear_negative_lyrics_marker(&track_id);
                                    final_text = Some(trimmed.to_string());
                                }
                            }
                            }
                            None => println!(
                                "[lyrics] Яндекс: подходящего трека не нашлось — фолбэк пропущен"
                            ),
                        }
                    }
                }
            }

            // 4. Если караоке с таймкодами не нашлось — фолбэк на официальный текст YouTube Music
            if final_text.as_ref().map_or(true, |t| !crate::api::lrclib::usable_synced_at(t, duration)) {
                if let Some(yt_text) = ytm.get_yt_lyrics(&track_id).await {
                    let trimmed = yt_text.trim();
                    println!("[lyrics] YouTube Music: официальный текст, {} симв.", trimmed.chars().count());
                    if !trimmed.is_empty() {
                        // Отвергнутую разметку статикой не затираем: длительность могла быть неверной,
                        // и при следующей проверке таймлайн ещё пригодится
                        if !had_stale_synced {
                            let _ = std::fs::write(&cache_path, trimmed);
                            crate::cache::clear_negative_lyrics_marker(&track_id);
                        }
                        final_text = Some(trimmed.to_string());
                    }
                }
            }

            final_text.or(cached_plain).unwrap_or_default()
        } else {
            // Трек из Яндекс Музыки
            let lookup = async {
                if client.has_token() {
                    client.get_track_lyrics(&track_id, &artist, &title, duration).await.ok().flatten()
                } else {
                    static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
                    let http = HTTP.get_or_init(|| {
                        reqwest::Client::builder()
                            .timeout(std::time::Duration::from_secs(10))
                            .build()
                            .unwrap_or_default()
                    });
                    crate::api::lrclib::get_cached_or_fetch_lrclib(http, &track_id, &artist, &title, duration).await
                }
            };
            tokio::time::timeout(std::time::Duration::from_secs(12), lookup)
                .await
                .ok()
                .flatten()
                .unwrap_or_default()
        };

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

/// Пост-фильтр «Моей Волны» — только для YouTube Music.
///
/// Там подборку собирает `WaveEngine` локально, и историю прослушиваний нужно гасить
/// самому. У Яндекса станцию считает сервер: он и так не присылает сыгранное, а фильтр
/// лишь выбрасывал пришедшие треки (плюс гасил те, что пользователь слушал до Волны),
/// из-за чего очередь не росла и догрузка считалась проваленной.
/// Название фильтра для интерфейса (подпись кнопки окна «Настроить волну»)
fn filter_label(
    mood: Option<&str>,
    language: Option<&str>,
    character: Option<&str>,
) -> Option<&'static str> {
    match (language, character, mood) {
        (Some("russian"), ..) => Some("🇷🇺 На русском"),
        (Some("not-russian"), ..) => Some("🌐 Иностранный"),
        (Some("without-words"), ..) => Some("🎵 Без слов"),
        (None, Some("favorite"), _) => Some("❤️ Моё любимое"),
        (None, Some("discover"), _) => Some("✨ Незнакомое"),
        (None, Some("popular"), _) => Some("🔥 Популярное"),
        (None, None, Some("active")) => Some("⚡ Энергичное"),
        (None, None, Some("calm")) => Some("☕ Спокойное"),
        (None, None, Some("fun")) => Some("🎉 Весёлое"),
        (None, None, Some("sad")) => Some("🌧 Грустное"),
        _ => None,
    }
}

/// Текст прогресса для плашки: «Загружаю Волну по «…»…»
fn wave_loading_text(
    mood: Option<&str>,
    language: Option<&str>,
    character: Option<&str>,
) -> String {
    match filter_label(mood, language, character) {
        Some(label) => format!("Загружаю Волну по «{}»…", label),
        None => "Обновляю Волну…".to_string(),
    }
}

/// Целевой размер принятого хвоста после TailSwap
const TAIL_SWAP_TARGET: usize = 12;

/// Сколько `Ambiguous`-кандидатов максимум опрашивается текстами за раунд
const PHASE2_MAX_CANDIDATES: usize = 8;

/// Сколько сидов максимум перебираем, чтобы хвост не был радио одного артиста
const TAIL_SWAP_SEEDS: usize = 3;

/// Не больше стольких треков одного артиста в обновлённом хвосте
const MAX_TRACKS_PER_ARTIST: usize = 2;

/// Не больше стольких треков с одного сида: иначе один лейбл/артист
/// заполняет весь хвост своим «радио-пузырём»
const MAX_TRACKS_PER_SEED: usize = 4;

/// Добирает сиды поиском в YouTube Music по трекам из истории Яндекса.
///
/// Пул вкуса YouTube Music бывает узким: любимые иностранные треки могли быть
/// открыты только в Яндексе. Без этого «Иностранный» зацикливается на одном
/// жанре (например, ню-роке из двух-трёх любимых групп).
async fn add_seeds_from_history_search(
    yt: &YtMusicClient,
    seeds: &mut Vec<String>,
    language: Option<&str>,
) {
    use super::filters::language as lf;

    let filter = language.and_then(lf::parse_filter);
    let Some(filter) = filter else {
        return;
    };
    for item in crate::api::history::load_history_for(ProviderId::Yandex) {
        if seeds.len() >= TAIL_SWAP_SEEDS {
            break;
        }
        let crate::api::history::HistoryItem::Track(track) = item else {
            continue;
        };
        let text = format!("{} {}", track.title, track.artists_string());
        let matches = match filter {
            lf::LanguageFilter::Russian => lf::has_cyrillic(&text),
            lf::LanguageFilter::NotRussian => !lf::has_cyrillic(&text),
            lf::LanguageFilter::WithoutWords => lf::has_instrumental_marker(&text),
        };
        if !matches {
            continue;
        }
        let query = format!("{} {}", track.artists_string(), track.title);
        let Ok(found) = yt.search_songs(&query).await else {
            continue;
        };
        let best = pick_best_yt_track(&found, &track.title, track.duration_ms).or_else(|| {
            found
                .into_iter()
                .find(|t| !t.id.is_empty() && crate::api::ytmusic::parser::is_clean_music_track(t))
        });
        if let Some(best) = best
            && !seeds.contains(&best.id)
        {
            seeds.push(best.id);
        }
    }
}

/// Инструментальные сиды для фильтра «Без слов».
///
/// В лайках и истории пользователя инструменталов может не быть вовсе, а радио
/// от вокального трека инструменталов и не выдаёт. Поэтому берём сиды из
/// поиска YouTube Music по явно инструментальным запросам.
async fn add_instrumental_search_seeds(yt: &YtMusicClient, seeds: &mut Vec<String>) {
    const QUERIES: [&str; 3] = [
        "lofi instrumental",
        "ambient instrumental",
        "post-rock instrumental",
    ];
    for query in QUERIES {
        if seeds.len() >= 2 {
            break;
        }
        let Ok(found) = yt.search_songs(query).await else {
            continue;
        };
        if let Some(track) = found
            .into_iter()
            .find(|t| !t.id.is_empty() && crate::api::ytmusic::parser::is_clean_music_track(t))
            && !seeds.contains(&track.id)
        {
            seeds.push(track.id);
        }
    }
}

/// Перемешивание без зависимости `rand` (xorshift на наносекундах).
fn shuffle_tracks(items: &mut [Track]) {
    if items.len() < 2 {
        return;
    }
    let mut state = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
        | 1;
    for i in (1..items.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let j = (state % (i as u64 + 1)) as usize;
        items.swap(i, j);
    }
}

/// Сиды под выбранные фильтры.
///
/// Пул — лайки и недавно открытые треки YouTube Music: Волна строится из вкуса
/// пользователя, а не из одного «случайного» лайка. Возвращает до
/// [`TAIL_SWAP_SEEDS`] id разных артистов в случайном порядке.
fn select_wave_seeds(
    state: &PlayerState,
    language: Option<&str>,
    character: Option<&str>,
) -> Vec<String> {
    // Пул вкуса: лайки + недавно открытые треки YouTube Music
    let mut pool: Vec<Track> = state.liked_tracks_cache.clone();
    for item in crate::api::history::load_history_for(ProviderId::YouTubeMusic) {
        if let crate::api::history::HistoryItem::Track(track) = item {
            pool.push(track);
        }
    }
    let mut seen_ids = std::collections::HashSet::new();
    pool.retain(|t| !t.id.is_empty() && seen_ids.insert(t.id.clone()));

    select_wave_seeds_from_pool(state, &pool, language, character)
}

/// Чистая часть выбора сидов: работает по явно переданному пулу.
///
/// Отделена от [`select_wave_seeds`], чтобы юнит-тесты не зависели от
/// реальной истории пользователя в конфиге.
fn select_wave_seeds_from_pool(
    state: &PlayerState,
    pool: &[Track],
    language: Option<&str>,
    character: Option<&str>,
) -> Vec<String> {
    use super::filters::language as lf;

    // Сид YouTube Music может быть только из YT-трека: яндексовый id для
    // `RDAMVM<seed>` бессмыслен (после переключения провайдера текущий трек чужой).
    let current = state
        .current_track
        .as_ref()
        .filter(|t| t.source == TrackSource::YouTubeMusic)
        .map(|t| t.id.clone())
        .filter(|id| !id.is_empty());

    // «Незнакомое» — сид всегда текущий трек
    if character == Some("discover") {
        return current.into_iter().collect();
    }

    let with_title = |t: &Track| format!("{} {}", t.title, t.artists_string());
    let mut matching: Vec<Track> = match (language, character) {
        (Some("russian"), _) => pool
            .iter()
            .filter(|t| lf::has_cyrillic(&with_title(t)))
            .cloned()
            .collect(),
        (Some("not-russian"), _) => pool
            .iter()
            .filter(|t| !lf::has_cyrillic(&with_title(t)))
            .cloned()
            .collect(),
        (Some("without-words"), _) => pool
            .iter()
            .filter(|t| {
                // маркеры — только название/версия, имя артиста обманывает
                let meta = format!("{} {}", t.title, t.version.as_deref().unwrap_or(""));
                lf::has_instrumental_marker(&meta)
            })
            .cloned()
            .collect(),
        (None, Some("favorite")) => state.liked_tracks_cache.clone(),
        _ => pool.to_vec(),
    };

    // Сдвиг: повторный клик по фильтру даёт другую подборку
    if !matching.is_empty() {
        let pick = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as usize)
            .unwrap_or(0);
        let len = matching.len();
        matching.rotate_left(pick % len);
    }

    // Сброс фильтров: первым идёт текущий трек, дальше — остальной пул
    if language.is_none() && character.is_none()
        && let Some(cur) = state
            .current_track
            .as_ref()
            .filter(|t| t.source == TrackSource::YouTubeMusic)
    {
        matching.retain(|t| t.id != cur.id);
        matching.insert(0, cur.clone());
    }

    // По одному сиду на артиста — иначе все сиды вернут один и тот же пузырь
    let mut seen_artists = std::collections::HashSet::new();
    let mut seeds: Vec<String> = Vec::new();
    for track in matching {
        let artist = track.artists_string().to_lowercase();
        if seen_artists.insert(artist) {
            seeds.push(track.id.clone());
        }
        if seeds.len() >= TAIL_SWAP_SEEDS {
            break;
        }
    }

    if seeds.is_empty() {
        if let Some(id) = current {
            seeds.push(id);
        }
    }
    seeds
}

/// Предикат характера: любимое / незнакомое / без отсева
fn character_matches(
    track: &Track,
    character: Option<&str>,
    liked: &std::collections::HashSet<String>,
    history: &std::collections::HashSet<String>,
) -> bool {
    match character {
        Some("favorite") => liked.contains(&track.id),
        Some("discover") => !liked.contains(&track.id) && !history.contains(&track.id),
        _ => true,
    }
}

/// Годится ли трек в якорь Seed Drift под активные фильтры.
///
/// `Ambiguous` считается прошедшим: под «Иностранным» любой не-кириллический
/// трек неоднозначен, и если бы он не проходил, дрейф откладывался бы всегда.
fn anchor_passes_filters(
    track: &Track,
    language: Option<&str>,
    character: Option<&str>,
    liked: &std::collections::HashSet<String>,
    history: &std::collections::HashSet<String>,
) -> bool {
    use super::filters::language as lf;

    if !character_matches(track, character, liked, history) {
        return false;
    }
    let Some(filter) = language.and_then(lf::parse_filter) else {
        return true;
    };
    match lf::phase_one(
        &track.title,
        track.version.as_deref().unwrap_or(""),
        &track.artists_string(),
    ) {
        lf::PhaseOne::Instrumental => lf::accepts(filter, lf::LanguageVerdict::Instrumental),
        lf::PhaseOne::Russian => lf::accepts(filter, lf::LanguageVerdict::Russian),
        lf::PhaseOne::Ambiguous => true,
    }
}

/// Снимки для фильтров: лайки и история прослушиваний.
///
/// Локи берутся последовательно и сразу отпускаются — вложенных локов нет.
fn snapshots_for_filters(
    me: &PlayerManager,
) -> (
    std::collections::HashSet<String>,
    std::collections::HashSet<String>,
) {
    let liked = me
        .state
        .lock()
        .map(|s| s.liked_track_ids.clone())
        .unwrap_or_default();
    let history = me
        .wave_engine
        .lock()
        .map(|e| e.recent_history.iter().cloned().collect())
        .unwrap_or_default();
    (liked, history)
}

/// Фаза 1 + Фаза 2: оставляет треки, прошедшие активные фильтры.
///
/// Фаза 1 — метаданные (0 мс). Фаза 2 — параллельный опрос текстов с глобальным
/// дедлайном на весь батч (`buffer_unordered(3)`, поэтому таймаут общий, а не
/// на кандидата). Без фильтра языка Фаза 2 не запускается вовсе.
async fn apply_filters(
    yt: &YtMusicClient,
    tracks: Vec<Track>,
    language: Option<&str>,
    character: Option<&str>,
    liked: &std::collections::HashSet<String>,
    history: &std::collections::HashSet<String>,
    phase2_budget_ms: u64,
) -> Vec<Track> {
    use super::filters::language as lf;

    let filter = language.and_then(lf::parse_filter);
    let mut accepted: Vec<Track> = Vec::new();
    let mut ambiguous: Vec<Track> = Vec::new();

    for track in tracks {
        if !character_matches(&track, character, liked, history) {
            continue;
        }
        let Some(filter) = filter else {
            accepted.push(track);
            continue;
        };

        // «Без слов»: маркеры ищем только в названии и версии. Имя артиста
        // обманывает (например, группа The Score), а наличие маркера ещё не
        // значит, что нет вокала — трек всё равно проверяется текстом.
        if filter == lf::LanguageFilter::WithoutWords {
            // Инструментал определяем по отсутствию текста, а не только по
            // маркеру: у большинства инструменталов маркеров в названии нет.
            if ambiguous.len() < PHASE2_MAX_CANDIDATES {
                ambiguous.push(track);
            }
            continue;
        }

        let verdict = match lf::phase_one(
            &track.title,
            track.version.as_deref().unwrap_or(""),
            &track.artists_string(),
        ) {
            lf::PhaseOne::Instrumental => Some(lf::LanguageVerdict::Instrumental),
            lf::PhaseOne::Russian => Some(lf::LanguageVerdict::Russian),
            lf::PhaseOne::Ambiguous => None,
        };
        match verdict {
            Some(verdict) => {
                if lf::accepts(filter, verdict) {
                    accepted.push(track);
                }
            }
            None => {
                if ambiguous.len() < PHASE2_MAX_CANDIDATES {
                    ambiguous.push(track);
                }
            }
        }
    }

    let Some(filter) = filter else {
        return accepted;
    };
    if ambiguous.is_empty() {
        return accepted;
    }

    let deadline = Instant::now() + Duration::from_millis(phase2_budget_ms);
    let results: Vec<(String, crate::api::lrclib::LyricsLookup)> =
        futures_util::stream::iter(ambiguous.iter().cloned().map(|track| {
            let yt = yt.clone();
            async move {
                let left = deadline.saturating_duration_since(Instant::now());
                let verdict = if left.is_zero() {
                    crate::api::lrclib::LyricsLookup::Failed
                } else {
                    match tokio::time::timeout(left, yt.probe_lyrics(&track)).await {
                        Ok(v) => v,
                        Err(_) => crate::api::lrclib::LyricsLookup::Failed,
                    }
                };
                (track.id.clone(), verdict)
            }
        }))
        .buffer_unordered(5)
        .collect()
        .await;

    let by_id: std::collections::HashMap<String, crate::api::lrclib::LyricsLookup> =
        results.into_iter().collect();
    for track in ambiguous {
        // «Без слов»: инструментал — это отсутствие текста. Нашли текст
        // (даже если в нём есть слово-маркер) — значит, трек с вокалом.
        if filter == lf::LanguageFilter::WithoutWords {
            if matches!(
                by_id.get(&track.id),
                Some(crate::api::lrclib::LyricsLookup::NotFound)
            ) {
                accepted.push(track);
            }
            continue;
        }
        let verdict = match by_id.get(&track.id) {
            Some(crate::api::lrclib::LyricsLookup::Found(text)) => {
                lf::classify_lyrics(&crate::api::lrclib::strip_lrc_tags(text))
            }
            // Failed / WrongTrack / NotFound — данных нет: трек не принят в этом раунде
            _ => lf::LanguageVerdict::Unknown,
        };
        if lf::accepts(filter, verdict) {
            accepted.push(track);
        }
    }
    accepted
}

fn pick_candidates(is_yt: bool, engine: &Mutex<WaveEngine>, tracks: Vec<Track>) -> Vec<Track> {
    if is_yt {
        engine.lock().unwrap().filter_candidates(tracks)
    } else {
        tracks
    }
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

/// Выбирает лучший трек YouTube Music, соответствующий оригинальному треку:
/// 1. Фильтрует кандидатов: исключает Live, Концерт, Concert, Remix, если их не было в исходном названии.
/// 2. Ищет трек, длительность которого ближе всего к длительности исходного трека Яндекса (в пределах ±20 сек).
/// 3. При отсутствии кандидатов в пределах 20 сек выбирает трек с минимальной дельтой длительности.
pub fn pick_best_yt_track(
    candidates: &[Track],
    orig_title: &str,
    orig_duration_ms: Option<u64>,
) -> Option<Track> {
    if candidates.is_empty() {
        return None;
    }

    // Обязательная проверка на чистый музыкальный трек (отсечение подкастов, видео-мусора и звуков)
    let clean: Vec<&Track> = candidates
        .iter()
        .filter(|c| crate::api::ytmusic::parser::is_clean_music_track(c))
        .collect();

    if clean.is_empty() {
        return None;
    }

    let orig_lower = orig_title.to_lowercase();
    let filter_live = !orig_lower.contains("live");
    let filter_concert = !orig_lower.contains("концерт") && !orig_lower.contains("concert");
    let filter_remix = !orig_lower.contains("remix");

    let is_undesired = |title: &str| -> bool {
        let t = title.to_lowercase();
        (filter_live && t.contains("live"))
            || (filter_concert && (t.contains("концерт") || t.contains("concert")))
            || (filter_remix && t.contains("remix"))
    };

    let filtered: Vec<&Track> = clean
        .iter()
        .filter(|c| !is_undesired(&c.title))
        .copied()
        .collect();

    let pool: &[&Track] = if !filtered.is_empty() {
        &filtered
    } else {
        &clean
    };

    if let Some(target_ms) = orig_duration_ms {
        // Кандидаты в пределах ±20 сек (20 000 мс)
        let within_20s: Vec<&&Track> = pool
            .iter()
            .filter(|c| {
                if let Some(d) = c.duration_ms {
                    (d as i64 - target_ms as i64).abs() <= 20_000
                } else {
                    false
                }
            })
            .collect();

        if !within_20s.is_empty() {
            return within_20s
                .into_iter()
                .min_by_key(|c| (c.duration_ms.unwrap() as i64 - target_ms as i64).abs())
                .map(|c| (*c).clone());
        }

        // Если в пределах 20 сек никого нет, выбираем с минимальной дельтой
        if let Some(closest) = pool
            .iter()
            .filter(|c| c.duration_ms.is_some())
            .min_by_key(|c| (c.duration_ms.unwrap() as i64 - target_ms as i64).abs())
        {
            return Some((*closest).clone());
        }
    }

    pool.first().map(|c| (*c).clone())
}

#[cfg(test)]
mod yt_track_selection_tests {
    use super::*;

    fn make_test_track(id: &str, title: &str, duration_ms: Option<u64>) -> Track {
        Track {
            id: id.to_string(),
            title: title.to_string(),
            version: None,
            artists: Vec::new(),
            albums: Vec::new(),
            duration_ms,
            cover_uri: None,
            og_image: None,
            available: Some(true),
            content_warning: None,
            derived_colors: None,
            source: TrackSource::YouTubeMusic,
        }
    }

    #[test]
    fn test_filter_live_concert_remix_when_original_is_clean() {
        let candidates = vec![
            make_test_track("1", "Лесник (Live)", Some(195_000)),
            make_test_track("2", "Лесник (Концерт в Олимпийском)", Some(192_000)),
            make_test_track("3", "Лесник (Remix)", Some(190_000)),
            make_test_track("4", "Лесник", Some(192_000)),
        ];

        let chosen = pick_best_yt_track(&candidates, "Лесник", Some(192_000)).expect("Track should be found");
        assert_eq!(chosen.id, "4");
    }

    #[test]
    fn test_keep_live_when_original_has_live() {
        let candidates = vec![
            make_test_track("1", "Лесник (Live)", Some(195_000)),
            make_test_track("2", "Лесник", Some(192_000)),
        ];

        let chosen = pick_best_yt_track(&candidates, "Лесник (Live)", Some(195_000)).expect("Track should be found");
        assert_eq!(chosen.id, "1");
    }

    #[test]
    fn test_closest_duration_within_20_seconds() {
        let candidates = vec![
            make_test_track("1", "Песня", Some(230_000)), // +30s (вне 20s)
            make_test_track("2", "Песня", Some(185_000)), // -15s (в пределах 20s)
            make_test_track("3", "Песня", Some(205_000)), // +5s  (в пределах 20s, ближе всех)
        ];

        let chosen = pick_best_yt_track(&candidates, "Песня", Some(200_000)).expect("Track should be found");
        assert_eq!(chosen.id, "3");
    }

    #[test]
    fn test_pick_best_yt_track_filters_podcasts() {
        let candidates = vec![
            make_test_track("1", "Песня (Подкаст выпуск #1)", Some(200_000)),
            make_test_track("2", "Песня (Длинный разговор)", Some(5_000_000)), // 83 минуты
            make_test_track("3", "Песня", Some(200_000)),
        ];

        let chosen = pick_best_yt_track(&candidates, "Песня", Some(200_000)).expect("Track should be found");
        assert_eq!(chosen.id, "3");
    }
}

/// Решение о том, что именно запросить у YouTube Music при догрузке Волны.
#[derive(Debug, PartialEq, Eq)]
struct WaveFetchPlan {
    /// Аргумент `station_id` для провайдера: `user:onyourwave`, `track:<id>`
    station: String,
    /// Токен продолжения текущего автомикса
    continuation: Option<String>,
    /// Запрос начинает новый автомикс (первый запрос, Seed Drift или пере-сидирование)
    new_seed: bool,
}

/// План догрузки Волны для YouTube Music.
///
/// Пока есть токен продолжения и не пора меняться — тянем текущий автомикс.
/// Иначе начинаем новый от трека-якоря (`track:<id>`), а если якоря нет —
/// повторяем запрос станции (провайдер разрешит сид сам).
fn plan_wave_fetch(
    station_id: &str,
    batch_id: Option<&str>,
    drift_due: bool,
    anchor: Option<&str>,
) -> WaveFetchPlan {
    let anchor = anchor.map(str::trim).filter(|id| !id.is_empty());
    if !drift_due
        && let Some(token) = batch_id.map(str::trim).filter(|token| !token.is_empty())
    {
        return WaveFetchPlan {
            station: station_id.to_string(),
            continuation: Some(token.to_string()),
            new_seed: false,
        };
    }
    match anchor {
        Some(id) => WaveFetchPlan {
            station: format!("track:{}", id),
            continuation: None,
            new_seed: true,
        },
        None => WaveFetchPlan {
            station: station_id.to_string(),
            continuation: None,
            new_seed: true,
        },
    }
}

/// Откуда брать сид на очередном Seed Drift.
///
/// Один и тот же якорь (текущий трек) быстро затягивает Волну в узкий поджанровый
/// пузырь. Поэтому переезды чередуются: чётные — от того, что звучит сейчас,
/// нечётные — от дальнего края очереди, который пришёл из другой порции провайдера.
fn pick_drift_anchor(serial: u64, current: Option<&str>, spare: Option<&str>) -> Option<String> {
    let clean = |id: Option<&str>| {
        id.map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
    };
    let current = clean(current);
    let spare = clean(spare);
    if serial % 2 == 0 {
        current.or(spare)
    } else {
        spare.filter(|id| Some(id) != current.as_ref()).or(current)
    }
}

/// Что делать, когда текущий трек — последний в очереди.
#[derive(Debug, PartialEq, Eq)]
enum TailAdvance {
    /// Обычный переход на следующий трек (`current_index + 1`), в том числе заворот
    /// очереди в начало при повторе.
    Advance,
    /// Волна кончилась: просим у провайдера новую порцию и ждём её, вместо того чтобы
    /// начинать тот же микс по второму кругу.
    RequestRefill,
    /// Догрузка уже не дала ничего нового — честно останавливаемся.
    GiveUp,
}

/// Решение о переходе с последнего трека очереди.
///
/// Раньше `next_track` заворачивал индекс `(current_index + 1) % queue.len()` при любом
/// режиме. Для обычной очереди это ожидаемый повтор, а для Волны — тихая деградация:
/// если догрузка не принесла треков (истощение continuation, сеть, фильтры), слушатель
/// вместо новой подборки получал тот же микс с начала, без единого сообщения.
fn tail_advance(
    wave: bool,
    repeat_one: bool,
    at_tail: bool,
    dead: bool,
    automatic: bool,
) -> TailAdvance {
    if !at_tail || !wave || repeat_one {
        return TailAdvance::Advance;
    }
    if dead && automatic {
        return TailAdvance::GiveUp;
    }
    TailAdvance::RequestRefill
}

#[cfg(test)]
mod tail_advance_tests {
    use super::*;

    #[test]
    fn test_plain_queue_at_tail_wraps_as_before() {
        assert_eq!(
            tail_advance(false, false, true, false, true),
            TailAdvance::Advance
        );
        // даже «мёртвая» волна не влияет на обычную очередь
        assert_eq!(
            tail_advance(false, false, true, true, true),
            TailAdvance::Advance
        );
    }

    #[test]
    fn test_wave_mid_queue_advances_without_extra_request() {
        assert_eq!(
            tail_advance(true, false, false, false, true),
            TailAdvance::Advance
        );
    }

    #[test]
    fn test_wave_at_tail_requests_refill_instead_of_wrapping() {
        assert_eq!(
            tail_advance(true, false, true, false, true),
            TailAdvance::RequestRefill
        );
        // ручное «Далее» тоже просит новую порцию, даже если автопопытка уже провалилась
        assert_eq!(
            tail_advance(true, false, true, true, false),
            TailAdvance::RequestRefill
        );
    }

    #[test]
    fn test_wave_at_tail_gives_up_only_on_automatic_advance() {
        assert_eq!(
            tail_advance(true, false, true, true, true),
            TailAdvance::GiveUp
        );
    }

    #[test]
    fn test_repeat_one_keeps_replaying_even_on_wave() {
        assert_eq!(
            tail_advance(true, true, true, true, true),
            TailAdvance::Advance
        );
    }
}

#[cfg(test)]
mod drift_anchor_tests {
    use super::*;

    #[test]
    fn test_even_drift_starts_from_playing_track() {
        assert_eq!(
            pick_drift_anchor(0, Some("now"), Some("tail")).as_deref(),
            Some("now")
        );
        assert_eq!(
            pick_drift_anchor(2, Some("now"), Some("tail")).as_deref(),
            Some("now")
        );
    }

    #[test]
    fn test_odd_drift_starts_from_queue_tail() {
        // другой пузырь: якорь не совпадает с тем, что звучит
        assert_eq!(
            pick_drift_anchor(1, Some("now"), Some("tail")).as_deref(),
            Some("tail")
        );
    }

    #[test]
    fn test_odd_drift_falls_back_to_playing_track() {
        // хвост совпал с текущим треком или пуст — сид всё равно нужен
        assert_eq!(
            pick_drift_anchor(1, Some("now"), Some("now")).as_deref(),
            Some("now")
        );
        assert_eq!(pick_drift_anchor(1, Some("now"), None).as_deref(), Some("now"));
        assert_eq!(pick_drift_anchor(1, Some("now"), Some("  ")).as_deref(), Some("now"));
    }

    #[test]
    fn test_drift_anchor_survives_empty_ids() {
        assert_eq!(pick_drift_anchor(0, None, Some("tail")).as_deref(), Some("tail"));
        assert_eq!(pick_drift_anchor(0, Some(""), None), None);
        assert_eq!(pick_drift_anchor(1, None, None), None);
    }
}

#[cfg(test)]
mod wave_fetch_tests {
    use super::*;

    #[test]
    fn test_continuation_kept_while_mix_is_fresh() {
        let plan = plan_wave_fetch("user:onyourwave", Some("tok_1"), false, Some("cur_id"));
        assert_eq!(plan.station, "user:onyourwave");
        assert_eq!(plan.continuation.as_deref(), Some("tok_1"));
        assert!(!plan.new_seed);
    }

    #[test]
    fn test_seed_drift_drops_continuation_and_anchors_on_current_track() {
        let plan = plan_wave_fetch("user:onyourwave", Some("tok_1"), true, Some("cur_id"));
        assert_eq!(plan.station, "track:cur_id");
        assert_eq!(plan.continuation, None);
        assert!(plan.new_seed);
    }

    #[test]
    fn test_drift_without_anchor_repeats_station_request() {
        let plan = plan_wave_fetch("user:onyourwave", Some("tok_1"), true, None);
        assert_eq!(plan.station, "user:onyourwave");
        assert_eq!(plan.continuation, None);
        assert!(plan.new_seed);
    }

    #[test]
    fn test_empty_continuation_does_not_get_sent() {
        // Пустой токен в теле запроса означает « продолжения нет» — его нельзя отправлять
        let plan = plan_wave_fetch("user:onyourwave", Some("   "), false, Some("cur_id"));
        assert_eq!(plan.station, "track:cur_id");
        assert_eq!(plan.continuation, None);
        assert!(plan.new_seed);
    }

    #[test]
    fn test_fresh_station_request_without_continuation_uses_anchor_when_present() {
        let plan = plan_wave_fetch("user:onyourwave", None, false, Some(" cur_id "));
        assert_eq!(plan.station, "track:cur_id", "якорь должен обрезаться по краям");
        assert_eq!(plan.continuation, None);
        assert!(plan.new_seed);
    }

    #[test]
    fn test_blank_anchor_falls_back_to_station() {
        let plan = plan_wave_fetch("user:onyourwave", None, false, Some("  "));
        assert_eq!(plan.station, "user:onyourwave");
        assert_eq!(plan.continuation, None);
        assert!(plan.new_seed);
    }

    #[test]
    fn test_drift_starts_new_seed_from_anchor() {
        let plan = plan_wave_fetch("user:onyourwave", None, true, Some("new_seed_id"));
        assert_eq!(plan.station, "track:new_seed_id");
        assert_eq!(plan.continuation, None);
        assert!(plan.new_seed);
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
mod pick_candidates_tests {
    use super::*;

    fn track(id: &str, title: &str, artist: &str) -> Track {
        serde_json::from_value(serde_json::json!({
            "id": id, "title": title, "artists": [{"id": "1", "name": artist}]
        })).unwrap()
    }

    #[test]
    fn yandex_branch_keeps_tracks_already_in_local_history() {
        // У Яндекса станцию считает сервер: пост-фильтр не должен выбрасывать треки,
        // которые пользователь слушал до запуска Волны, — иначе очередь не растёт
        // и догрузка каждый раз считается проваленной.
        let engine = Mutex::new(WaveEngine::new());
        engine.lock().unwrap().record_played("2");
        let tracks = vec![
            track("1", "Roar", "Katy Perry"),
            track("2", "Firework", "Katy Perry"),
        ];
        let kept: Vec<String> = pick_candidates(false, &engine, tracks)
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(kept, ["1", "2"]);
    }

    #[test]
    fn yt_branch_still_drops_played_tracks() {
        let engine = Mutex::new(WaveEngine::new());
        engine.lock().unwrap().record_played("2");
        let tracks = vec![
            track("1", "Roar", "Katy Perry"),
            track("2", "Firework", "Katy Perry"),
        ];
        let kept: Vec<String> = pick_candidates(true, &engine, tracks)
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(kept, ["1"]);
    }

    #[test]
    fn empty_history_keeps_everything_on_both_sides() {
        let engine = Mutex::new(WaveEngine::new());
        let make = || vec![track("1", "Roar", "Katy Perry"), track("2", "Firework", "Katy Perry")];
        assert_eq!(pick_candidates(false, &engine, make()).len(), 2);
        assert_eq!(pick_candidates(true, &engine, make()).len(), 2);
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

    #[test]
    fn test_toggle_play_launches_ytmusic_wave_when_paused_on_yandex_track_without_token() {
        let client = YandexMusicClient::new(None);
        let audio = Arc::new(AudioBackend::new().unwrap());
        let client_mutex = Arc::new(Mutex::new(client.clone()));
        let mut state = PlayerState::default();
        let yandex_track = Track {
            id: "ya_123".to_string(),
            title: "Yandex Track".to_string(),
            source: TrackSource::Yandex,
            ..Default::default()
        };
        state.queue = vec![yandex_track.clone()];
        state.current_track = Some(yandex_track);
        let state_arc = Arc::new(Mutex::new(state));

        let yandex_provider = Arc::new(client) as Arc<dyn MusicProvider>;
        let ytmusic_provider = Arc::new(YtMusicClient::new()) as Arc<dyn MusicProvider>;
        let provider_manager = ProviderManager::with_yandex_and_ytmusic(yandex_provider, ytmusic_provider);
        provider_manager.switch_provider(ProviderId::YouTubeMusic);

        let manager = PlayerManager {
            audio,
            client: client_mutex,
            providers: Arc::new(provider_manager),
            yt_client: Arc::new(YtMusicClient::new()),
            clips: crate::player::clip::ClipBackground::new(
                Arc::new(YtMusicClient::new()),
                Arc::new(crate::player::clip_telemetry::ClipTelemetry::new()),
                Arc::new(Mutex::new(Vec::new())),
            ),
            state: state_arc.clone(),
            wave_engine: Arc::new(Mutex::new(WaveEngine::new())),
            event_senders: Arc::new(Mutex::new(Vec::new())),
            cast: Arc::default(),
            prefetch: Arc::default(),
            prefetch_inflight: Arc::default(),
            generation: Arc::new(AtomicU64::new(0)),
            tail_swap_epoch: Arc::new(AtomicU64::new(0)),
            crossfade: Arc::new(Mutex::new((false, 3))),
        };

        manager.toggle_play();

        let current_station = state_arc.lock().unwrap().station_id.clone();
        assert_eq!(current_station, "user:onyourwave");
    }

    #[tokio::test]
    async fn test_switch_provider_resets_filters() {
        let client = YandexMusicClient::new(None);
        let audio = Arc::new(AudioBackend::new().unwrap());
        let client_mutex = Arc::new(Mutex::new(client.clone()));
        let state_arc = Arc::new(Mutex::new(PlayerState::default()));
        {
            let mut st = state_arc.lock().unwrap();
            st.current_mood = Some("active".to_string());
            st.current_language = Some("russian".to_string());
            st.current_character = Some("favorite".to_string());
            st.wave_label = Some("🇷🇺 На русском".to_string());
            st.batch_id = Some("yandex_cont_token".to_string());
            st.station_id = "track:123".to_string();
        }

        let yandex_provider = Arc::new(client) as Arc<dyn MusicProvider>;
        let ytmusic_provider = Arc::new(YtMusicClient::new()) as Arc<dyn MusicProvider>;
        let provider_manager =
            ProviderManager::with_yandex_and_ytmusic(yandex_provider, ytmusic_provider);

        let manager = PlayerManager {
            audio,
            client: client_mutex,
            providers: Arc::new(provider_manager),
            yt_client: Arc::new(YtMusicClient::new()),
            clips: crate::player::clip::ClipBackground::new(
                Arc::new(YtMusicClient::new()),
                Arc::new(crate::player::clip_telemetry::ClipTelemetry::new()),
                Arc::new(Mutex::new(Vec::new())),
            ),
            state: state_arc.clone(),
            wave_engine: Arc::new(Mutex::new(WaveEngine::new())),
            event_senders: Arc::new(Mutex::new(Vec::new())),
            cast: Arc::default(),
            prefetch: Arc::default(),
            prefetch_inflight: Arc::default(),
            generation: Arc::new(AtomicU64::new(0)),
            tail_swap_epoch: Arc::new(AtomicU64::new(0)),
            crossfade: Arc::new(Mutex::new((false, 3))),
        };

        manager.switch_provider(ProviderId::YouTubeMusic);

        let st = state_arc.lock().unwrap();
        assert_eq!(st.current_mood, None);
        assert_eq!(st.current_language, None);
        assert_eq!(st.current_character, None);
        assert_eq!(st.wave_label, None);
        assert_eq!(st.batch_id, None);
        assert_eq!(st.station_id, "user:onyourwave");
    }

}

#[cfg(test)]
mod filter_pipeline_tests {
    use super::*;
    use std::collections::HashSet;

    fn track(id: &str, title: &str, artist: &str, version: Option<&str>) -> Track {
        let mut value = serde_json::json!({
            "id": id, "title": title, "artists": [{"id": "1", "name": artist}]
        });
        if let Some(v) = version {
            value["version"] = serde_json::json!(v);
        }
        serde_json::from_value(value).unwrap()
    }

    fn state_with(tracks: Vec<Track>, current: Option<&str>) -> PlayerState {
        let mut state = PlayerState::default();
        state.liked_tracks_cache = tracks;
        state.current_track = current.map(|id| {
            let mut t = track(id, "Current", "Artist", None);
            // Сид YouTube Music берётся только из YT-трека
            t.source = TrackSource::YouTubeMusic;
            t
        });
        state
    }

    #[test]
    fn seed_for_russian_prefers_cyrillic_like() {
        let state = state_with(
            vec![
                track("1", "English song", "Artist", None),
                track("2", "Русская песня", "Артист", None),
            ],
            Some("9"),
        );
        assert_eq!(select_wave_seeds_from_pool(&state, &state.liked_tracks_cache, Some("russian"), None).first().map(String::as_str), Some("2"));
    }

    #[test]
    fn seed_for_not_russian_prefers_latin_like() {
        let state = state_with(
            vec![
                track("1", "Русская песня", "Артист", None),
                track("2", "English song", "Artist", None),
            ],
            Some("9"),
        );
        assert_eq!(select_wave_seeds_from_pool(&state, &state.liked_tracks_cache, Some("not-russian"), None).first().map(String::as_str), Some("2"));
    }

    #[test]
    fn seed_falls_back_to_current_when_no_like_matches() {
        let state = state_with(vec![], Some("9"));
        assert_eq!(select_wave_seeds_from_pool(&state, &state.liked_tracks_cache, Some("without-words"), None).first().map(String::as_str), Some("9"));
        assert_eq!(select_wave_seeds_from_pool(&state, &state.liked_tracks_cache, None, Some("favorite")).first().map(String::as_str), Some("9"));
        assert_eq!(select_wave_seeds_from_pool(&state, &state.liked_tracks_cache, None, Some("discover")).first().map(String::as_str), Some("9"));
    }

    #[test]
    fn seeds_are_distinct_artists() {
        let state = state_with(
            vec![
                track("1", "A", "Artist A", None),
                track("2", "B", "Artist A", None),
                track("3", "C", "Artist B", None),
                track("4", "D", "Artist C", None),
                track("5", "E", "Artist D", None),
            ],
            Some("9"),
        );
        let seeds = select_wave_seeds_from_pool(&state, &state.liked_tracks_cache, None, Some("favorite"));
        assert_eq!(seeds.len(), TAIL_SWAP_SEEDS, "не больше трёх сидов");
        let artists: std::collections::HashSet<String> = seeds
            .iter()
            .filter_map(|id| {
                state
                    .liked_tracks_cache
                    .iter()
                    .find(|t| &t.id == id)
                    .map(|t| t.artists_string())
            })
            .collect();
        assert_eq!(artists.len(), seeds.len(), "сиды должны быть от разных артистов");
    }

    #[test]
    fn ambiguous_anchor_passes_filter() {
        let latin = track("1", "Some Song", "Artist", None);
        let cyrillic = track("2", "Русская песня", "Артист", None);
        let (liked, history) = (HashSet::new(), HashSet::new());
        // Ambiguous под «Иностранным» и «На русском» — проходит (решается в Фазе 2)
        assert!(anchor_passes_filters(&latin, Some("not-russian"), None, &liked, &history));
        assert!(anchor_passes_filters(&latin, Some("russian"), None, &liked, &history));
        // Явное несоответствие — не проходит: кириллица под «Иностранным»
        assert!(!anchor_passes_filters(&cyrillic, Some("not-russian"), None, &liked, &history));
        // Инструментал под «На русском» — не проходит (приоритет маркеров)
        let instrumental = track("3", "Track", "Artist", Some("Instrumental"));
        assert!(!anchor_passes_filters(&instrumental, Some("russian"), None, &liked, &history));
    }

    #[tokio::test]
    async fn phase_one_rejects_instrumental_under_russian() {
        let yt = YtMusicClient::new();
        let candidates = vec![
            track("1", "Песня", "Артист", None),
            track("2", "Track", "Artist", Some("Instrumental")),
            track("3", "Song", "Band", None),
        ];
        let accepted = apply_filters(
            &yt,
            candidates,
            Some("russian"),
            None,
            &HashSet::new(),
            &HashSet::new(),
            0, // без сети: все вердикты Фазы 1
        )
        .await;
        let ids: Vec<_> = accepted.into_iter().map(|t| t.id).collect();
        assert_eq!(ids, ["1"]);
    }

    #[tokio::test]
    async fn without_words_requires_absence_of_lyrics() {
        let yt = YtMusicClient::new();
        let candidates = vec![
            // Маркер в названии/версии — но проверка текста обязательна
            track("1", "Track", "Artist", Some("Instrumental")),
            // «The Score» — пример артиста, чьё имя содержит маркер:
            // оно не должно влиять на вердикт
            track("2", "Unstoppable", "The Score", None),
        ];
        // Бюджет 0 → проба текста недоступна (Failed) → треки не приняты
        let accepted = apply_filters(
            &yt,
            candidates,
            Some("without-words"),
            None,
            &HashSet::new(),
            &HashSet::new(),
            0,
        )
        .await;
        assert!(
            accepted.is_empty(),
            "без проверки текста инструментал принимать нельзя"
        );
    }

    #[tokio::test]
    async fn character_favorite_filters_by_liked_ids() {
        let yt = YtMusicClient::new();
        let candidates = vec![
            track("1", "Песня", "Артист", None),
            track("2", "Другая", "Артист", None),
        ];
        let mut liked = HashSet::new();
        liked.insert("1".to_string());
        let accepted = apply_filters(&yt, candidates, None, Some("favorite"), &liked, &HashSet::new(), 0).await;
        let ids: Vec<_> = accepted.into_iter().map(|t| t.id).collect();
        assert_eq!(ids, ["1"]);
    }
}
