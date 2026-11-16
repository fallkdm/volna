use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use async_channel::Sender;
use futures_util::stream::StreamExt;

use crate::api::client::YandexMusicClient;
use crate::api::models::{Track, TrackSource};
use crate::api::provider::{FeedbackEvent, MusicProvider, ProviderId};
use crate::api::provider_manager::ProviderManager;
use crate::api::vk::provider::VkProvider;
use crate::api::ytmusic::YtMusicClient;
use super::audio::{AudioBackend, StartOutcome};
use super::source::*;
use super::wave_engine::WaveEngine;

static AUDIO_DOWNLOAD_LOCKS: std::sync::LazyLock<tokio::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(std::collections::HashSet::new()));

/// Режим повтора: нет, один трек, вся очередь (для Волны «вся очередь» недоступна)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepeatMode {
    #[default]
    Off,
    One,
    All,
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
    /// Какой именно трек сейчас загружается/буферизируется (`None` — загрузка завершена).
    LoadingTrack(Option<Track>),
    /// Процент буферизации активного или стартующего потока (`Some(0..=99)` или `None`).
    BufferingProgress(Option<u8>),
    /// Подбор Волны под фильтр: `Some("…")` — показать прогресс, `None` — скрыть
    WaveLoading(Option<String>),
    AuthRequired,
    UserLoggedIn(String),
    UserLoggedOut,
    StatusMessage(String),
    Error(String),
    /// VK отозвал авторизацию (API error 5): нужно повторить Direct Auth.
    VkAuthRequired,
    /// Обычный `audio.*`-запрос получил captcha; ответ вернёт ожидающий запрос в VK API.
    VkCaptchaRequired {
        image_url: Option<String>,
        respond: async_channel::Sender<String>,
    },
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
    /// Питомец на плеере: включение/выключение и облик ("wave", "cat", "slime")
    PetChanged {
        enabled: bool,
        style: String,
    },
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
    /// Выполняется ли догрузка треков волны прямо сейчас
    pub wave_refill_in_flight: bool,
    /// Полные объекты любимых треков — источник сидов для алгоритмических фильтров
    pub liked_tracks_cache: Vec<Track>,
    /// Предыдущий реально играющий трек и его индекс для отката при ошибке переключения
    pub rollback_track: Option<(usize, Track)>,
    /// Сколько попыток получения потока подряд завершились ошибкой (сбрасывается при успешном старте)
    pub consecutive_resolve_failures: usize,
}

impl PlayerState {
    /// Фиксирует текущий трек для отката при ошибке перехода, если аудио ещё играет.
    /// Во время активного перехода уже сохранённый `rollback_track` не перезаписывается.
    pub fn save_rollback_if_needed(&mut self, audio_playing: bool) {
        if audio_playing
            && (!self.is_transitioning || self.rollback_track.is_none())
            && let Some(prev_track) = self.current_track.clone()
        {
            self.rollback_track = Some((self.current_index, prev_track));
        }
    }

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
            wave_refill_in_flight: false,
            liked_tracks_cache: Vec::new(),
            rollback_track: None,
            consecutive_resolve_failures: 0,
        }
    }
}

/// Контекст потока для однократного автоматического восстановления при ошибке старта GStreamer
#[derive(Clone)]
struct PlaybackRetryCtx {
    track_id: String,
    track_source: TrackSource,
    allow_retry: bool,
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
const ROTOR_QUEUE_IDS: usize = 20;

/// Сколько раз подряд пробуем продлить Волну, прежде чем считать её исчерпанной.
/// Ротор иногда отдаёт порцию из одних повторов — одиночный пустой ответ ещё не приговор.
const WAVE_REFILL_ATTEMPTS: usize = 3;

/// Пауза между попытками продления Волны, чтобы не долбить API в один момент.
const WAVE_REFILL_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(1000);

/// Порог зависания HTTP-потока для переключения на локальный кэш (мс)
const WATCHDOG_CACHE_SWITCH_MS: u64 = 4_000;
/// Порог зависания HTTP-потока для переподключения по сети (мс)
const WATCHDOG_RETRY_STREAM_MS: u64 = 12_000;
/// Минимальное приращение позиции, подтверждающее реальный прогресс воспроизведения (мс)
const WATCHDOG_PROGRESS_STEP_MS: u64 = 50;
/// Продвижение после переподключения, снимающее блокировку повторного retry в текущей эпохе (мс)
const WATCHDOG_RETRY_UNLOCK_DELTA_MS: u64 = 1_000;

/// Решение watchdog о восстановлении зависшего HTTP-потока
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WatchdogAction {
    None,
    SwitchToCache {
        track_id: String,
        resume_pos_ms: u64,
        stalled_ms: u64,
    },
    RetryStream {
        track_id: String,
        track_source: TrackSource,
        resume_pos_ms: u64,
        stalled_ms: u64,
    },
}

/// Конечный автомат отслеживания зависаний активного HTTP-потока (без async-зависимостей)
#[derive(Debug)]
pub(crate) struct PlaybackWatchdog {
    started_at: Instant,
    epoch: Option<u64>,
    last_pos_ms: u64,
    stalled_since_ms: Option<u64>,
    cache_swapped_epoch: Option<u64>,
    retried_epoch: Option<u64>,
    retry_pos_anchor_ms: u64,
}

impl Default for PlaybackWatchdog {
    fn default() -> Self {
        Self::new()
    }
}

impl PlaybackWatchdog {
    pub(crate) fn new() -> Self {
        Self {
            started_at: Instant::now(),
            epoch: None,
            last_pos_ms: 0,
            stalled_since_ms: None,
            cache_swapped_epoch: None,
            retried_epoch: None,
            retry_pos_anchor_ms: 0,
        }
    }

    /// Watchdog следит только за сетевыми HTTP/HTTPS-потоками
    pub(crate) fn is_http_uri(uri: Option<&str>) -> bool {
        uri.is_some_and(|u| u.starts_with("http://") || u.starts_with("https://"))
    }

    /// Фиксирует явную перемотку, чтобы движение с более ранней позиции не считалось зависанием
    pub(crate) fn note_seek(&mut self, pos_ms: u64) {
        self.last_pos_ms = pos_ms;
        self.stalled_since_ms = None;
        self.retry_pos_anchor_ms = pos_ms;
    }

    /// Выполняет шаг конечного автомата по текущему монотонному времени
    pub(crate) fn poll_now<F>(
        &mut self,
        eligible: bool,
        epoch: u64,
        track_id: &str,
        track_source: TrackSource,
        pos_ms: Option<u64>,
        has_cached_file: F,
    ) -> WatchdogAction
    where
        F: FnOnce() -> bool,
    {
        let now_ms = self.started_at.elapsed().as_millis() as u64;
        self.poll(now_ms, eligible, epoch, track_id, track_source, pos_ms, has_cached_file)
    }

    /// Детерминированный шаг автомата для заданной временной отметки `now_ms`.
    /// Замыкание `has_cached_file` вызывается только при достижении порога 4000 мс.
    pub(crate) fn poll<F>(
        &mut self,
        now_ms: u64,
        eligible: bool,
        epoch: u64,
        track_id: &str,
        track_source: TrackSource,
        pos_ms: Option<u64>,
        has_cached_file: F,
    ) -> WatchdogAction
    where
        F: FnOnce() -> bool,
    {
        if !eligible {
            self.stalled_since_ms = None;
            return WatchdogAction::None;
        }

        if self.epoch != Some(epoch) {
            self.epoch = Some(epoch);
            self.last_pos_ms = pos_ms.unwrap_or(0);
            self.stalled_since_ms = None;
            self.cache_swapped_epoch = None;
            self.retried_epoch = None;
            self.retry_pos_anchor_ms = 0;
        }

        if let Some(pos) = pos_ms
            && pos > self.last_pos_ms.saturating_add(WATCHDOG_PROGRESS_STEP_MS)
        {
            self.last_pos_ms = pos;
            self.stalled_since_ms = None;
            if self.retried_epoch == Some(epoch)
                && pos > self.retry_pos_anchor_ms.saturating_add(WATCHDOG_RETRY_UNLOCK_DELTA_MS)
            {
                self.retried_epoch = None;
            }
            return WatchdogAction::None;
        }

        let stalled_since = match self.stalled_since_ms {
            Some(since) => since,
            None => {
                self.stalled_since_ms = Some(now_ms);
                now_ms
            }
        };
        let stalled_ms = now_ms.saturating_sub(stalled_since);

        if stalled_ms >= WATCHDOG_CACHE_SWITCH_MS
            && self.cache_swapped_epoch != Some(epoch)
            && has_cached_file()
        {
            self.cache_swapped_epoch = Some(epoch);
            return WatchdogAction::SwitchToCache {
                track_id: track_id.to_string(),
                resume_pos_ms: self.last_pos_ms,
                stalled_ms,
            };
        }

        if stalled_ms >= WATCHDOG_RETRY_STREAM_MS && self.retried_epoch != Some(epoch) {
            self.retried_epoch = Some(epoch);
            self.retry_pos_anchor_ms = self.last_pos_ms;
            return WatchdogAction::RetryStream {
                track_id: track_id.to_string(),
                track_source,
                resume_pos_ms: self.last_pos_ms,
                stalled_ms,
            };
        }

        WatchdogAction::None
    }
}

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
    /// Watchdog зависания HTTP-потока
    watchdog: Arc<Mutex<PlaybackWatchdog>>,
    pub artist_registry: Arc<std::sync::Mutex<std::collections::HashMap<(TrackSource, String), String>>>,
}

async fn resolve_stream_for_source(
    providers: &ProviderManager,
    client: &YandexMusicClient,
    track_id: &str,
    source: TrackSource,
) -> Result<(String, String), String> {
    match source {
        TrackSource::YouTubeMusic => {
            let provider = providers
                .get(ProviderId::YouTubeMusic)
                .ok_or_else(|| "Провайдер YouTube Music не найден".to_string())?;
            provider
                .get_stream_url(track_id)
                .await
                .map(|url| (url, "Opus 160".to_string()))
                .map_err(|error| format!("Ошибка YouTube Music: {error}"))
        }
        TrackSource::Vk => {
            let provider = providers
                .get(ProviderId::Vk)
                .ok_or_else(|| "Требуется вход во ВКонтакте".to_string())?;
            let url = provider
                .get_stream_url(track_id)
                .await
                .map_err(|error| format!("Ошибка VK Музыки: {error}"))?;
            let label = if url.contains(".flac") || url.contains("type=flac") || url.contains("type=3") {
                "FLAC"
            } else if crate::player::audio::is_hls_url(&url) {
                "HLS"
            } else if url.ends_with(".mp3") || url.contains("type=2") {
                "MP3 320"
            } else {
                "AAC"
            };
            Ok((url, label.to_string()))
        }
        TrackSource::Yandex => client
            .get_track_direct_url(track_id)
            .await
            .map_err(|error| error.to_string()),
    }
}

pub async fn get_stream_url_internal(
    providers: &ProviderManager,
    client: &YandexMusicClient,
    track_id: &str,
    source: TrackSource,
) -> Result<(String, String), String> {
    resolve_stream_for_source(providers, client, track_id, source).await
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

        // Порядок: refresh-токен (долговечный) → свежая сессия из браузера →
        // сохранённая кука → гость. Свежая кука важнее сохранённой: браузер
        // обновляет её сам, а скопированная когда-то давно уже мертва.
        let mut needs_startup_ytcfg_sync = false;
        let yt_client = if let Some(token) = crate::api::auth::load_ytmusic_refresh_token() {
            println!("[ytmusic] старт с сохранённым OAuth refresh_token");
            YtMusicClient::with_refresh_token(token)
        } else if let Some(imported) = crate::api::browser_cookies::import_youtube_cookie() {
            println!("[ytmusic] сессия взята из браузера: {}", imported.source);
            let previous_cookie = crate::api::auth::load_ytmusic_cookie();
            let initial_authuser = if previous_cookie.as_deref() == Some(imported.cookie.as_str()) {
                crate::api::auth::load_ytmusic_authuser()
            } else {
                None
            };
            if let Err(e) = crate::api::auth::save_ytmusic_cookie(&imported.cookie) {
                eprintln!("[ytmusic] не удалось сохранить импортированную куку: {e}");
            }
            needs_startup_ytcfg_sync = true;
            YtMusicClient::with_cookie_and_authuser(imported.cookie, initial_authuser)
        } else {
            println!("[ytmusic] импорт сессии из браузера не дал результата (сессия не найдена или ключ недоступен)");
            if let Some(cookie) = crate::api::auth::load_ytmusic_cookie() {
                println!("[ytmusic] используем сохранённую cookie из конфигурации");
                needs_startup_ytcfg_sync = true;
                YtMusicClient::with_cookie_and_authuser(
                    cookie,
                    crate::api::auth::load_ytmusic_authuser(),
                )
            } else {
                println!("[ytmusic] сохранённой cookie нет — запуск YouTube Music в гостевом режиме");
                YtMusicClient::new()
            }
        };
        let yt_client_arc = Arc::new(yt_client);
        if needs_startup_ytcfg_sync && tokio::runtime::Handle::try_current().is_ok() {
            let yt_startup = yt_client_arc.clone();
            tokio::spawn(async move {
                yt_startup.validate_and_sync_session_on_startup().await;
            });
        }
        let clip_telemetry = Arc::new(super::clip_telemetry::ClipTelemetry::new());
        clip_telemetry.set_enabled(crate::api::auth::load_clips());
        let event_senders_arc = Arc::new(Mutex::new(event_senders));
        let clip_events = event_senders_arc.clone();
        let clips = super::clip::ClipBackground::new(
            yt_client_arc.clone(),
            clip_telemetry,
            clip_events,
        );

        let yandex_provider = Arc::new(client) as Arc<dyn MusicProvider>;
        let ytmusic_provider = yt_client_arc.clone() as Arc<dyn MusicProvider>;
        let provider_manager = ProviderManager::with_yandex_and_ytmusic(yandex_provider, ytmusic_provider);
        if let Some(token) = crate::api::vk::auth::load_vk_token() {
            let owner_id = crate::api::auth::load_vk_user_id().unwrap_or(0);
            if let Ok(vk) = VkProvider::new(token, owner_id) {
                let senders = event_senders_arc.clone();
                vk.set_captcha_handler(Arc::new(move |challenge, respond| {
                    emit_all(
                        &senders,
                        PlayerUiEvent::VkCaptchaRequired {
                            image_url: challenge.image_url,
                            respond,
                        },
                    );
                }));
                provider_manager.register(Arc::new(vk));
            }
        }
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
            event_senders: event_senders_arc,
            cast: Arc::default(),
            prefetch: Arc::default(),
            prefetch_inflight: Arc::default(),
            generation: Arc::new(AtomicU64::new(0)),
            tail_swap_epoch: Arc::new(AtomicU64::new(0)),
            crossfade,
            watchdog: Arc::new(Mutex::new(PlaybackWatchdog::new())),
            artist_registry: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        };

        // Подключаем автоматический переход к следующему треку по окончании воспроизведения
        let mgr_eos = manager.clone();
        manager.audio.set_on_eos(move || {
            mgr_eos.on_track_finished();
        });

        // Подключаем обработчик ошибок активного слота GStreamer
        let mgr_err = manager.clone();
        manager.audio.set_on_error(move |err| {
            mgr_err.on_playback_error(err);
        });

        // Подключаем прогресс буферизации активного или стартующего слота GStreamer
        let buf_senders = manager.event_senders.clone();
        let last_buf_pct = Arc::new(std::sync::atomic::AtomicU8::new(255));
        manager.audio.set_on_buffering(move |pct| {
            emit_buffering_if_changed(&last_buf_pct, &buf_senders, pct);
        });

        // Фоновый мониторинг прогресса трека для автосводки (кроссфейда) и восстановления зависших HTTP-потоков
        let mgr_monitor = manager.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
            loop {
                interval.tick().await;
                let pos_ms = mgr_monitor.audio.position_ms();
                if let Some(pos_ms) = pos_ms {
                    mgr_monitor.check_auto_crossfade(pos_ms);
                }
                mgr_monitor.check_playback_stall(pos_ms);
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

    pub fn register_artist(&self, source: TrackSource, id: &str, name: &str) {
        if !name.trim().is_empty() {
            let clean_id = id.trim();
            if let Ok(mut reg) = self.artist_registry.lock() {
                if !clean_id.is_empty() {
                    reg.insert((source, clean_id.to_string()), name.trim().to_string());
                }
                // Также регистрируем по имени для быстрого поиска
                reg.insert((source, name.trim().to_string()), name.trim().to_string());
            }
        }
    }

    pub fn get_registered_artist_name(&self, source: TrackSource, id: &str) -> Option<String> {
        self.artist_registry.lock().ok()?.get(&(source, id.trim().to_string())).cloned()
    }

    pub fn lookup_registered_artist_by_id(&self, raw_id: &str) -> Option<(TrackSource, String)> {
        let reg = self.artist_registry.lock().ok()?;
        lookup_registered_artist_in_map(&reg, raw_id)
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
        let active_provider = self.providers.active_id();
        let Some(track) = crate::api::history::load_last_track() else { return };
        let track_matches_provider = match active_provider {
            ProviderId::Vk => track.source == TrackSource::Vk,
            ProviderId::YouTubeMusic => track.source == TrackSource::YouTubeMusic,
            ProviderId::Yandex => track.source == TrackSource::Yandex,
        };
        if !track_matches_provider {
            return;
        }
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
                spawn_cover_and_lyrics(&self.event_senders, self.providers.clone(), client, &track, self.generation.load(Ordering::SeqCst), &self.generation, &self.clips);
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
        spawn_cover_and_lyrics(&self.event_senders, self.providers.clone(), client, &track, self.generation.load(Ordering::SeqCst), &self.generation, &self.clips);
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

    /// Регистрирует VK после входа и подключает captcha к живому GTK-циклу.
    pub fn register_vk_provider(&self, provider: VkProvider) {
        let senders = self.event_senders.clone();
        provider.set_captcha_handler(Arc::new(move |challenge, respond| {
            emit_all(
                &senders,
                PlayerUiEvent::VkCaptchaRequired {
                    image_url: challenge.image_url,
                    respond,
                },
            );
        }));
        self.providers.register(Arc::new(provider));
    }

    /// Переключает активного музыкального провайдера
    pub fn switch_provider(&self, id: ProviderId) {
        if self.providers.switch_provider(id) {
            let _ = crate::api::auth::save_active_provider(id);
            let default_station = match id {
                ProviderId::Vk => crate::api::vk::provider::VK_MIX_STATION_ID.to_string(),
                _ => "user:onyourwave".to_string(),
            };
            if let Ok(mut state) = self.state.lock() {
                state.liked_loaded = false;
                state.current_mood = None;
                state.current_language = None;
                state.current_character = None;
                state.wave_label = None;
                state.batch_id = None;
                state.station_id = default_station.clone();
                state.wave_dead = false;
                state.refill_failures = 0;
                state.pending_advance = false;
                state.queue.clear();
                state.current_index = 0;
                state.current_track = None;
                state.restored = false;
            }
            self.audio.stop();
            self.clear_loading_ui();
            self.emit(PlayerUiEvent::QueueUpdated(Vec::new()));
            self.emit(PlayerUiEvent::PlaybackToggled(false));
            let me = self.clone();
            tokio::spawn(async move {
                me.ensure_liked_loaded().await;
            });
            self.emit(PlayerUiEvent::ProviderChanged(id));
            let station_title = match id {
                ProviderId::Vk => "VK Стрим-микс".to_string(),
                ProviderId::YouTubeMusic => "YouTube Music Волна".to_string(),
                ProviderId::Yandex => "Моя Волна".to_string(),
            };
            self.start_station(default_station, None, station_title);
        } else if id == ProviderId::Vk {
            self.emit(PlayerUiEvent::VkAuthRequired);
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

        // Если активен YouTube Music или VK: загружаем понравившиеся треки из соответствующего провайдера
        let provider = self.providers.current();
        if provider.provider_id() == ProviderId::YouTubeMusic || provider.provider_id() == ProviderId::Vk {
            if provider.is_authenticated() {
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
                }
            }
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
            let active_id = me.providers.active_id();
            if active_id == ProviderId::Vk {
                let provider = me.providers.get(ProviderId::Vk);
                if provider.as_ref().is_some_and(|p| p.is_authenticated()) {
                    emit_all(&senders, PlayerUiEvent::StatusMessage(
                        "Подключен ВКонтакте 🔵".to_string(),
                    ));
                    me.ensure_liked_loaded().await;
                } else {
                    emit_all(&senders, PlayerUiEvent::VkAuthRequired);
                }
                return;
            }

            if active_id == ProviderId::YouTubeMusic {
                let provider = me.providers.get(ProviderId::YouTubeMusic);
                if provider.as_ref().is_some_and(|p| p.is_authenticated()) {
                    emit_all(&senders, PlayerUiEvent::StatusMessage(
                        "Подключен Google (YouTube Music) 🔴".to_string(),
                    ));
                    me.ensure_liked_loaded().await;
                } else {
                    emit_all(&senders, PlayerUiEvent::AuthRequired);
                }
                return;
            }

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
            self.next_track_inner(true);
        }
    }

    /// Проверяет, не завис ли активный HTTP-поток; при превышении порогов
    /// переключает на локальный кэш или переполучает URL потока с сохранением позиции.
    pub fn check_playback_stall(&self, pos_ms: Option<u64>) {
        let (is_transitioning, current_track) = match self.state.lock() {
            Ok(state) => (state.is_transitioning, state.current_track.clone()),
            Err(_) => return,
        };

        let active_uri = self.audio.active_uri();
        let eligible = self.audio.is_playing()
            && !is_transitioning
            && !self.audio.is_fading()
            && !self.cast_active()
            && PlaybackWatchdog::is_http_uri(active_uri.as_deref())
            && current_track.is_some();

        let (track_id, track_source, track_title) = match current_track {
            Some(track) => (track.id, track.source, track.title),
            None => (String::new(), TrackSource::Yandex, String::new()),
        };
        let epoch = self.generation.load(Ordering::SeqCst);

        let mut cached_path: Option<std::path::PathBuf> = None;
        let action = {
            let Ok(mut watchdog) = self.watchdog.lock() else {
                return;
            };
            watchdog.poll_now(eligible, epoch, &track_id, track_source, pos_ms, || {
                cached_path = crate::cache::offline_audio_path(&track_id)
                    .or_else(|| crate::cache::cached_audio_path(&track_id));
                cached_path.is_some()
            })
        };

        match action {
            WatchdogAction::None => {}
            WatchdogAction::SwitchToCache {
                track_id,
                resume_pos_ms,
                stalled_ms,
            } => {
                let Some(path) = cached_path.or_else(|| {
                    crate::cache::offline_audio_path(&track_id)
                        .or_else(|| crate::cache::cached_audio_path(&track_id))
                }) else {
                    return;
                };
                let uri = gtk4::gio::prelude::FileExt::uri(&gtk4::gio::File::for_path(&path));
                eprintln!(
                    "[watchdog] HTTP-поток «{}» завис на {} мс (pos={} мс) — переключаем на локальный кэш",
                    track_title, stalled_ms, resume_pos_ms
                );
                self.audio.play_url_at_ms(&uri, resume_pos_ms);
            }
            WatchdogAction::RetryStream {
                track_id,
                track_source,
                resume_pos_ms,
                stalled_ms,
            } => {
                eprintln!(
                    "[watchdog] HTTP-поток «{}» завис на {} мс (pos={} мс) — переподключаем поток",
                    track_title, stalled_ms, resume_pos_ms
                );
                let providers = self.providers.clone();
                let client = match self.client.lock() {
                    Ok(c) => c.clone(),
                    Err(_) => return,
                };
                let audio = self.audio.clone();
                let generation = self.generation.clone();
                tokio::spawn(async move {
                    let is_still_stalled = || {
                        if generation.load(Ordering::SeqCst) != epoch || !audio.is_playing() {
                            return false;
                        }
                        // Если за время поиска кэша или сетевого запроса поток сам «ожил» и ушёл вперёд,
                        // переподключение отменяем
                        let jumped_ahead = audio
                            .position_ms()
                            .is_some_and(|p| p > resume_pos_ms.saturating_add(1500));
                        !jumped_ahead
                    };

                    // На случай, если кэш успел докачаться к моменту повторного подключения
                    if let Some(path) = crate::cache::offline_audio_path(&track_id)
                        .or_else(|| crate::cache::cached_audio_path(&track_id))
                    {
                        if is_still_stalled() {
                            let local_uri =
                                gtk4::gio::prelude::FileExt::uri(&gtk4::gio::File::for_path(&path));
                            eprintln!(
                                "[watchdog] HTTP-поток «{}» переведён в локальный кэш (pos={} мс): {}",
                                track_title, resume_pos_ms, local_uri
                            );
                            audio.play_url_at_ms(&local_uri, resume_pos_ms);
                        }
                        return;
                    }

                    match resolve_stream_for_source(&providers, &client, &track_id, track_source).await {
                        Ok((fresh_url, _label)) => {
                            if is_still_stalled() {
                                if fresh_url.starts_with("file://") {
                                    eprintln!(
                                        "[watchdog] Поток «{}» переведён в локальный кэш (pos={} мс): {}",
                                        track_title, resume_pos_ms, fresh_url
                                    );
                                }
                                audio.play_url_at_ms(&fresh_url, resume_pos_ms);
                            }
                        }
                        Err(err) => {
                            eprintln!(
                                "[watchdog] Не удалось переподключить поток «{}»: {}",
                                track_title, err
                            );
                        }
                    }
                });
            }
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
        let active_id = self.providers.active_id();
        if !self.is_playing() {
            if active_id == ProviderId::Vk {
                let is_not_vk = {
                    let state = self.state.lock().unwrap();
                    state.current_track.as_ref()
                        .or_else(|| state.queue.get(state.current_index))
                        .map(|t| t.source != TrackSource::Vk)
                        .unwrap_or(true)
                };
                if is_not_vk {
                    if let Ok(mut state) = self.state.lock() {
                        state.restored = false;
                        state.queue.clear();
                        state.current_index = 0;
                        state.current_track = None;
                    }
                    self.start_station(crate::api::vk::provider::VK_MIX_STATION_ID.to_string(), None, "VK Стрим-микс".to_string());
                    return;
                }
            } else if active_id == ProviderId::YouTubeMusic {
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
            if provider.provider_id() == ProviderId::Vk {
                if !provider.is_authenticated() {
                    self.emit(PlayerUiEvent::VkAuthRequired);
                    self.emit(PlayerUiEvent::StatusMessage(
                        "Для запуска Волны выполните вход через VK ID".to_string(),
                    ));
                } else {
                    self.emit(PlayerUiEvent::StatusMessage(
                        "Загружаем Стрим-микс VK... ⚡".to_string(),
                    ));
                    self.start_station(crate::api::vk::provider::VK_MIX_STATION_ID.to_string(), None, "VK Стрим-микс".to_string());
                }
            } else if provider.provider_id() == ProviderId::YouTubeMusic {
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
        let (track, station_id, total_secs, batch_id) = {
            let state = self.state.lock().unwrap();
            let track = state.current_track.clone();
            let total_secs = track
                .as_ref()
                .and_then(|t| t.duration_ms)
                .map(|ms| ms as f64 / 1000.0)
                .unwrap_or(state.played_seconds);
            (
                track,
                state.station_id.clone(),
                total_secs,
                state.batch_id.clone(),
            )
        };

        if let Some(track) = track {
            let client = self.client.clone();
            let providers = self.providers.clone();
            tokio::spawn(async move {
                let track_id = track.id;
                match track.source {
                    TrackSource::YouTubeMusic | TrackSource::Vk => {
                        let provider_id = match track.source {
                            TrackSource::YouTubeMusic => ProviderId::YouTubeMusic,
                            TrackSource::Vk => ProviderId::Vk,
                            TrackSource::Yandex => unreachable!(),
                        };
                        if let Some(provider) = providers.get(provider_id) {
                            let _ = provider
                                .send_feedback(FeedbackEvent::TrackFinished {
                                    track_id,
                                    station_id,
                                    duration_secs: total_secs as u32,
                                    batch_id,
                                })
                                .await;
                        }
                    }
                    TrackSource::Yandex => {
                        let client_guard = client.lock().unwrap().clone();
                        if client_guard.has_token() {
                            let _ = client_guard
                                .feedback_track_finished(&station_id, &track_id, total_secs, batch_id)
                                .await;
                        }
                    }
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

    /// Снимает индикацию загрузки и буферизации трека в UI.
    fn clear_loading_ui(&self) {
        emit_clear_loading(&self.event_senders);
    }

    /// Откатывает `current_index`, `current_track` и `is_liked` на реально играющий в активном
    /// слоте трек, если переход сорвался при живом аудио. Не испускает `TrackStarted`, чтобы UI
    /// не сбрасывал активный клип, позицию и обложку.
    fn rollback_track_if_playing(&self, audio_playing: bool) {
        let rolled_back = if let Ok(mut state) = self.state.lock() {
            state.is_transitioning = false;
            if audio_playing {
                if let Some((prev_idx, prev_track)) = state.rollback_track.take() {
                    let restored_idx = if !state.queue.is_empty() {
                        prev_idx.min(state.queue.len() - 1)
                    } else {
                        prev_idx
                    };
                    state.current_index = restored_idx;
                    state.current_track = Some(prev_track.clone());
                    let is_liked = state.sync_track_like_status(&prev_track.id);
                    let is_wave = state.wave;
                    Some((prev_track, restored_idx, is_liked, is_wave))
                } else {
                    None
                }
            } else {
                state.rollback_track = None;
                None
            }
        } else {
            None
        };
        if let Some((prev_track, prev_idx, is_liked, is_wave)) = rolled_back {
            crate::api::history::save_last_track(&prev_track);
            if !is_wave {
                crate::cache::save_session_index(prev_idx);
            }
            self.emit(PlayerUiEvent::LikeStateChanged(is_liked));
        }
    }

    /// Обрабатывает ошибку воспроизведения активного слота GStreamer:
    /// снимает состояние загрузки/перехода, откатывает индекс при живом аудио и уведомляет UI.
    pub fn on_playback_error(&self, err: String) {
        eprintln!("[play] Ошибка GStreamer: {}", err);
        let audio_playing = self.audio.is_playing();
        self.rollback_track_if_playing(audio_playing);
        self.clear_loading_ui();
        if !audio_playing {
            self.emit(PlayerUiEvent::PlaybackToggled(false));
        }
        self.emit(PlayerUiEvent::Error(format!("Ошибка воспроизведения: {}", err)));
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

        let is_empty = self.state.lock().unwrap().queue.is_empty();
        if is_empty {
            self.toggle_play();
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

        let audio_playing = self.audio.is_playing();
        let (next_track, idx, total, is_liked) = {
            let mut state = self.state.lock().unwrap();
            if state.queue.is_empty() {
                return;
            }
            state.save_rollback_if_needed(audio_playing);
            state.current_index = (state.current_index + 1) % state.queue.len();
            let idx = state.current_index;
            let track = state.queue[idx].clone();
            state.is_transitioning = true;
            state.current_track = Some(track.clone());
            let is_liked = state.sync_track_like_status(&track.id);
            state.played_seconds = 0.0;
            (track, idx, state.queue.len(), is_liked)
        };

        // Если очередь подходит к концу (осталось < 7 треков) и включен режим волны,
        // автоматически в фоне запрашиваем следующую порцию радио
        let remaining = total.saturating_sub(idx + 1);
        let is_wave = self.state.lock().unwrap().wave;
        let refill_needed = remaining < 7 || self.wave_engine.lock().unwrap().refill_needed(total, idx);
        // Seed Drift не ждёт исчерпания хвоста: иначе сид застревает на весь текущий
        // батч, то есть на десятки треков. Только для YouTube Music: у Яндекса понятия
        // «сид» нет, и лишние догрузки там ни к чему.
        let drift_due = self.providers.current().provider_id() == ProviderId::YouTubeMusic
            && self.wave_engine.lock().unwrap().seed_drift_due();
        if is_wave && (remaining < 7 || refill_needed || drift_due) {
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

    /// Догрузка вернулась: если мы ждали продления в конце очереди — самое время перейти.
    fn finish_pending_advance(&self) {
        let has_next = {
            let mut state = self.state.lock().unwrap();
            state.wave_refill_in_flight = false;
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
        state.wave_refill_in_flight = false;
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
        let state_arc = self.state.clone();
        let audio = self.audio.clone();
        let client = self.client.lock().unwrap().clone();
        let providers = self.providers.clone();
        let track_source = next.source;
        let is_yt = track_source == TrackSource::YouTubeMusic;
        let track_id = next.id.clone();
        let track_title = next.title.clone();

        tokio::spawn(async move {
            let is_still_next = || {
                state_arc
                    .lock()
                    .ok()
                    .and_then(|state| {
                        state
                            .queue
                            .get(state.current_index + 1)
                            .map(|t| t.id.as_str() == track_id.as_str())
                    })
                    .unwrap_or(false)
            };

            let started = std::time::Instant::now();
            let stream_res = resolve_stream_for_source(&providers, &client, &track_id, track_source).await;

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

            let is_hls = (track_source == TrackSource::Vk && crate::player::audio::is_hls_url(&stream_url)) || stream_url.contains(".m3u8");
            if !is_hls && is_still_next() {
                audio.preroll_next_url(&stream_url);
            }

            let ext = audio_cache_ext_for_url(&stream_url, is_hls, is_yt);

            // Скачиваем трек на диск (для HLS — ffmpeg remux в .mka, для остальных — прямой download)
            if let Some(path) = download_audio_to_cache(&stream_url, &track_id, ext).await {
                let local_uri = gtk4::gio::prelude::FileExt::uri(&gtk4::gio::File::for_path(&path));
                if let Ok(mut entry) = prefetch_arc.lock() {
                    if let Some(p) = entry.as_mut().filter(|p| p.track_id == track_id) {
                        p.url = local_uri.to_string();
                        p.at = std::time::Instant::now();
                    }
                }
                if is_still_next() {
                    audio.preroll_next_url(&local_uri);
                    eprintln!("[prefetch] «{}» сохранен на диск и переведен в локальный preroll: {}", track_title, local_uri);
                }
            } else if is_hls && is_still_next() {
                audio.preroll_next_url(&stream_url);
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
        {
            let mut state = self.state.lock().unwrap();
            if state.wave_refill_in_flight {
                return;
            }
            state.wave_refill_in_flight = true;
        }

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
                    if let Ok(mut state) = state_clone.lock() {
                        state.wave_refill_in_flight = false;
                    }
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
                    if let Ok(mut state) = state_clone.lock() {
                        state.wave_refill_in_flight = false;
                    }
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

            if provider.provider_id() == ProviderId::Vk {
                if !provider.is_authenticated() {
                    if let Ok(mut state) = state_clone.lock() {
                        state.wave_refill_in_flight = false;
                    }
                    emit_all(&senders, PlayerUiEvent::LoadingState(false));
                    emit_all(&senders, PlayerUiEvent::VkAuthRequired);
                    return;
                }

                let batch_opt = batch_id.as_deref().filter(|b| !b.is_empty());
                let station_batch = provider
                    .get_station_tracks(&station_id, batch_opt)
                    .await
                    .ok();

                let Some(station_batch) = station_batch.filter(|b| !b.tracks.is_empty()) else {
                    eprintln!("[vk wave] волна VK не дала новых треков");
                    emit_all(&senders, PlayerUiEvent::LoadingState(false));
                    me.mark_wave_refill_dead();
                    return;
                };

                let raw = station_batch.tracks.len();
                let (updated_queue, grew, added, should_start) = {
                    let mut state = state_clone.lock().unwrap();
                    state.batch_id = if station_batch.batch_id.is_empty() {
                        None
                    } else {
                        Some(station_batch.batch_id)
                    };
                    let incoming = station_batch.tracks;
                    let start = state.current_index.saturating_sub(15);
                    let active_window = &state.queue[start..];
                    let mut new_tracks = dedup_tracks(active_window, incoming.clone());
                    if new_tracks.is_empty() {
                        let unplayed = &state.queue[state.current_index..];
                        new_tracks = dedup_tracks(unplayed, incoming);
                    }
                    let before = state.queue.len();
                    let added = new_tracks.len();
                    state.queue.extend(new_tracks);
                    let should_start = state.current_track.is_none() && !state.queue.is_empty();
                    (state.queue.clone(), state.queue.len() > before, added, should_start)
                };
                eprintln!("[vk wave] raw={} added={} queue={}", raw, added, updated_queue.len());
                emit_all(&senders, PlayerUiEvent::QueueUpdated(updated_queue));
                emit_all(&senders, PlayerUiEvent::LoadingState(false));
                if grew {
                    if let Ok(mut state) = state_clone.lock() {
                        state.refill_failures = 0;
                    }
                    if should_start {
                        let (track, total, is_liked) = {
                            let mut state = state_clone.lock().unwrap();
                            state.wave_refill_in_flight = false;
                            state.current_index = 0;
                            let track = state.queue[0].clone();
                            state.current_track = Some(track.clone());
                            let is_liked = state.sync_track_like_status(&track.id);
                            (track, state.queue.len(), is_liked)
                        };
                        emit_all(&senders, PlayerUiEvent::LikeStateChanged(is_liked));
                        emit_all(&senders, PlayerUiEvent::TrackStarted {
                            track: track.clone(),
                            index: 0,
                            total_queue: total,
                        });
                        me.play_track_stream(&track);
                    } else {
                        me.finish_pending_advance();
                    }
                } else {
                    if let Ok(mut state) = state_clone.lock() {
                        state.refill_failures += 1;
                    }
                    me.mark_wave_refill_dead();
                }
                return;
            }

            let client = client_clone.lock().unwrap().clone();
            if !client.has_token() {
                if let Ok(mut state) = state_clone.lock() {
                    state.wave_refill_in_flight = false;
                }
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
                        let incoming: Vec<Track> =
                            station_tracks.sequence.into_iter().map(|s| s.track).collect();
                        let mut state = state_clone.lock().unwrap();
                        state.batch_id = station_tracks.batch_id;

                        // 1. Дедуплицируем против скользящего окна (несыгранный хвост + 15 недавних треков)
                        let start = state.current_index.saturating_sub(15);
                        let active_window = &state.queue[start..];
                        let mut new_tracks = dedup_tracks(active_window, incoming.clone());

                        // 2. Если все кандидаты совпали со старой историей,
                        // не даём волне выдохнуться: берём треки, которых нет только в ещё не сыгранном хвосте
                        if new_tracks.is_empty() {
                            let unplayed = &state.queue[state.current_index..];
                            new_tracks = dedup_tracks(unplayed, incoming);
                        }

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
        let audio_playing = self.audio.is_playing();
        let (prev_track, idx, total, is_liked) = {
            let mut state = self.state.lock().unwrap();
            if state.queue.is_empty() {
                return;
            }
            state.save_rollback_if_needed(audio_playing);
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
        let was_playing = self.audio.is_playing();
        let is_transitioning = self
            .state
            .lock()
            .map(|mut s| {
                s.rollback_track = None;
                s.is_transitioning
            })
            .unwrap_or(false);
        eprintln!(
            "[play] play_single_track id={} title=\"{}\" source={:?} was_playing={} is_transitioning={}",
            track.id, track.title, track.source, was_playing, is_transitioning
        );
        self.emit(PlayerUiEvent::LoadingTrack(Some(track.clone())));
        self.emit(PlayerUiEvent::BufferingProgress(None));
        self.emit(PlayerUiEvent::LoadingState(true));
        let station = format!("track:{}", track.id);
        let label = format!("По треку: {}", track.title);
        self.start_station(station, Some(track), label);
    }

    /// Запускает «Волну по плейлисту»
    pub fn play_playlist_wave(&self, uid: &str, kind: &str, title: &str) {
        self.start_station(format!("playlist:{}_{}", uid, kind), None, format!("По плейлисту: {}", title));
    }

    /// Запускает «Волну по артисту»
    pub fn play_artist_wave(&self, artist_id: &str, artist_name: &str, source: TrackSource) {
        let clean_id = match source {
            TrackSource::Yandex => artist_id.strip_prefix("ya:").unwrap_or(artist_id),
            TrackSource::YouTubeMusic => artist_id.strip_prefix("yt:").unwrap_or(artist_id),
            TrackSource::Vk => artist_id.strip_prefix("vk:").unwrap_or(artist_id),
        };
        self.start_station_for_source(format!("artist:{}", clean_id), None, format!("По артисту: {}", artist_name), source);
    }

    /// Запускает станцию Ротора (`track:ID`, `artist:ID`). Если задан `first`, он играет сразу,
    /// а треки станции дописываются в очередь следом.
    pub fn start_station(&self, station_id: String, first: Option<Track>, label: String) {
        self.start_station_for_source(station_id, first, label, self.providers.current().provider_id().into());
    }

    /// Запускает станцию для указанного источника.
    pub fn start_station_for_source(&self, station_id: String, first: Option<Track>, label: String, source: TrackSource) {
        eprintln!(
            "[station] start_station_for_source station_id={} source={:?} first_id={} label=\"{}\"",
            station_id,
            source,
            first.as_ref().map(|t| t.id.as_str()).unwrap_or("none"),
            label
        );
        let provider_id: ProviderId = source.into();
        let Some(provider) = self.providers.get(provider_id) else {
            self.clear_loading_ui();
            self.emit(PlayerUiEvent::StatusMessage(format!("Сервис {:?} не подключён", source)));
            return;
        };
        if provider.provider_id() == ProviderId::Vk {
            if !provider.is_authenticated() {
                self.clear_loading_ui();
                self.emit(PlayerUiEvent::VkAuthRequired);
                self.emit(PlayerUiEvent::StatusMessage(
                    "Войдите в VK ID для запуска Стрим-микса".to_string(),
                ));
                return;
            }
        } else if provider.provider_id() == ProviderId::YouTubeMusic
            && first.is_none()
            && (station_id == "user:onyourwave" || station_id.contains("onyourwave"))
            && !provider.is_authenticated()
        {
            self.clear_loading_ui();
            self.emit(PlayerUiEvent::StatusMessage(
                "Войдите в Google Аккаунт внизу сайдбара для доступа к персонализированной волне YouTube Music".to_string(),
            ));
            return;
        }

        {
            let mut state = self.state.lock().unwrap();
            state.crossfade_triggered = false;
            state.is_transitioning = true;
            state.rollback_track = None;
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
            (_, Some(("artist", id))) => {
                let prefixed = match source {
                    TrackSource::Yandex => if id.starts_with("ya:") { id.to_string() } else { format!("ya:{}", id) },
                    TrackSource::YouTubeMusic => if id.starts_with("yt:") { id.to_string() } else { format!("yt:{}", id) },
                    TrackSource::Vk => if id.starts_with("vk:") { id.to_string() } else { format!("vk:{}", id) },
                };
                Some(SourceTarget::Artist(prefixed))
            }
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
            let Some(provider) = me.providers.get(provider_id) else {
                return;
            };
            let is_vk = provider.provider_id() == ProviderId::Vk;
            let is_yt = provider.provider_id() == ProviderId::YouTubeMusic;

            let mut result: Result<(Vec<Track>, Option<String>), String> = if is_vk {
                match provider.get_station_tracks(&station_id, None).await {
                    Ok(batch) => {
                        let b_id = if batch.batch_id.is_empty() { None } else { Some(batch.batch_id) };
                        Ok((batch.tracks, b_id))
                    }
                    Err(e) => Err(format!("Ошибка VK Музыки: {}", e)),
                }
            } else if is_yt {
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
                    let tracks = pick_candidates(is_yt || is_vk, &me.wave_engine, tracks);
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
            state.rollback_track = None;
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
        self.audio.stop();
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
            if let Ok(mut watchdog) = self.watchdog.lock() {
                watchdog.note_seek(ms);
            }
            self.audio.seek_ms(ms);
        }
        self.emit(PlayerUiEvent::Seeked(ms / 1000));
    }

    /// Перемотка к заданной секунде
    pub fn seek(&self, seconds: u64) {
        if self.cast_seek(seconds) {
            return;
        }
        if let Ok(mut watchdog) = self.watchdog.lock() {
            watchdog.note_seek(seconds.saturating_mul(1000));
        }
        self.audio.seek_seconds(seconds);
        self.emit(PlayerUiEvent::Seeked(seconds));
    }

    /// Установка громкости (обновляет целевую громкость user_volume)
    pub fn set_volume(&self, volume: f64) {
        crate::api::auth::save_volume(volume);
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
            if track_source == TrackSource::Vk {
                if let Some(vk) = providers_clone.get(ProviderId::Vk) {
                    if let Err(error) = vk.set_track_like(&track_id, liked).await {
                        eprintln!("[vk] Ошибка синхронизации лайка: {error}");
                    }
                }
                return;
            }
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
                if track_source == TrackSource::Vk {
                    if let Some(vk) = providers_clone.get(ProviderId::Vk) {
                        if was_liked {
                            let _ = vk.set_track_like(&track_id, false).await;
                        }
                        if new_dislike {
                            if let Err(error) = vk.set_track_dislike(&track_id).await {
                                eprintln!("[vk] Ошибка синхронизации дизлайка: {error}");
                            }
                        }
                    }
                    return;
                }
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
            state.rollback_track = None;
        }
        self.emit(PlayerUiEvent::PlaybackToggled(false));
        self.emit(PlayerUiEvent::UserLoggedOut);
        self.emit(PlayerUiEvent::StatusMessage("Вы вышли из аккаунта Яндекс Музыки".into()));
    }

    /// Запускает воспроизведение подготовленного URI (прямо или с кроссфейдом) и гарантирует
    /// согласованный перевод UI из состояния загрузки в `PlaybackToggled(true)` или `Error(...)`.
    #[allow(clippy::too_many_arguments)]
    fn dispatch_stream_playback<F>(
        &self,
        epoch: u64,
        url: &str,
        quality_label: String,
        crossfade: bool,
        was_playing: bool,
        fade_ms: u64,
        fade_in_when_idle: bool,
        retry_ctx: Option<PlaybackRetryCtx>,
        on_started: F,
    ) where
        F: FnOnce(&PlayerManager) + Send + 'static,
    {
        self.dispatch_stream_playback_boxed(
            epoch,
            url,
            quality_label,
            crossfade,
            was_playing,
            fade_ms,
            fade_in_when_idle,
            retry_ctx,
            Box::new(on_started),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn dispatch_stream_playback_boxed(
        &self,
        epoch: u64,
        url: &str,
        quality_label: String,
        crossfade: bool,
        was_playing: bool,
        fade_ms: u64,
        fade_in_when_idle: bool,
        retry_ctx: Option<PlaybackRetryCtx>,
        on_started: Box<dyn FnOnce(&PlayerManager) + Send>,
    ) {
        self.emit(PlayerUiEvent::StreamQuality(quality_label.clone()));

        let mgr = self.clone();
        let failed_url = url.to_string();
        let mut on_started = Some(on_started);
        let on_outcome = move |outcome: StartOutcome| {
            if mgr.generation.load(Ordering::SeqCst) != epoch {
                return;
            }
            match outcome {
                StartOutcome::Started => {
                    if let Ok(mut state) = mgr.state.lock() {
                        state.is_transitioning = false;
                        state.rollback_track = None;
                        state.consecutive_resolve_failures = 0;
                    }
                    mgr.clear_loading_ui();
                    mgr.emit(PlayerUiEvent::PlaybackToggled(true));
                    if let Some(cb) = on_started.take() {
                        cb(&mgr);
                    }
                }
                StartOutcome::Failed(err) => {
                    let is_http_url =
                        failed_url.starts_with("http://") || failed_url.starts_with("https://");
                    if is_http_url
                        && let Some(ctx) = retry_ctx
                        && ctx.allow_retry
                        && let Ok(handle) = tokio::runtime::Handle::try_current()
                    {
                        let retry_cb = on_started.take();
                        let mgr_retry = mgr.clone();
                        handle.spawn(async move {
                            mgr_retry
                                .recover_and_retry_playback(
                                    epoch,
                                    failed_url,
                                    quality_label,
                                    crossfade,
                                    fade_ms,
                                    fade_in_when_idle,
                                    ctx,
                                    err,
                                    retry_cb,
                                )
                                .await;
                        });
                        return;
                    }
                    mgr.on_playback_error(err);
                }
                StartOutcome::Cancelled => {
                    if let Ok(mut state) = mgr.state.lock() {
                        state.is_transitioning = false;
                    }
                    mgr.clear_loading_ui();
                }
            }
        };

        if crossfade && was_playing {
            self.audio
                .crossfade_to_url_with_outcome(url, fade_ms, on_outcome);
        } else {
            self.audio.stop();
            if fade_in_when_idle && crossfade {
                self.audio
                    .play_url_fade_in_with_outcome(url, fade_ms, on_outcome);
            } else {
                self.audio.play_url_with_outcome(url, on_outcome);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn recover_and_retry_playback(
        &self,
        epoch: u64,
        failed_url: String,
        quality_label: String,
        crossfade: bool,
        fade_ms: u64,
        fade_in_when_idle: bool,
        ctx: PlaybackRetryCtx,
        original_err: String,
        on_started: Option<Box<dyn FnOnce(&PlayerManager) + Send>>,
    ) {
        let is_stale = || self.generation.load(Ordering::SeqCst) != epoch;
        if is_stale() {
            return;
        }
        eprintln!(
            "[play] #{} ошибка запуска HTTP-потока ({}), пробуем восстановление для {}...",
            epoch, original_err, ctx.track_id
        );

        if ctx.track_source == TrackSource::YouTubeMusic || failed_url.contains("googlevideo.com") {
            crate::api::ytmusic::remember_blocked_ggc_node_for_url(&failed_url);
        }

        // 1. Проверяем локальный офлайн / дисковый кэш
        if let Some(path) = crate::cache::offline_audio_path(&ctx.track_id)
            .or_else(|| crate::cache::cached_audio_path(&ctx.track_id))
        {
            if is_stale() {
                return;
            }
            let local_uri =
                gtk4::gio::prelude::FileExt::uri(&gtk4::gio::File::for_path(&path)).to_string();
            let cur_was_playing = self.audio.is_playing();
            self.dispatch_stream_playback_boxed(
                epoch,
                &local_uri,
                quality_label,
                crossfade,
                cur_was_playing,
                fade_ms,
                fade_in_when_idle,
                Some(PlaybackRetryCtx {
                    allow_retry: false,
                    ..ctx
                }),
                on_started.unwrap_or_else(|| Box::new(|_| {})),
            );
            return;
        }

        // 2. Запрашиваем свежую ссылку потока
        let client = match self.client.lock() {
            Ok(c) => c.clone(),
            Err(_) => {
                if !is_stale() {
                    self.on_playback_error(original_err);
                }
                return;
            }
        };

        let resolved = resolve_stream_for_source(
            &self.providers,
            &client,
            &ctx.track_id,
            ctx.track_source,
        )
        .await;

        if is_stale() {
            return;
        }

        let (mut next_url, fresh_label) = match resolved {
            Ok(res) => res,
            Err(resolve_err) => {
                eprintln!(
                    "[play] #{} восстановление потока для {} не удалось: {}",
                    epoch, ctx.track_id, resolve_err
                );
                if !is_stale() {
                    self.on_playback_error(original_err);
                }
                return;
            }
        };

        // 3. Для подходящих потоков (YT/googlevideo <= 16 МБ, VK HLS) пробуем скачать в кэш,
        // чтобы повторный запуск шёл с локального file:// без повторного сетевого обрыва
        let is_hls = (ctx.track_source == TrackSource::Vk
            && crate::player::audio::is_hls_url(&next_url))
            || next_url.contains(".m3u8");
        if is_hls || should_predownload_segmented(ctx.track_source, &next_url) {
            let is_yt_stream =
                ctx.track_source == TrackSource::YouTubeMusic || next_url.contains("googlevideo.com");
            let ext = audio_cache_ext_for_url(&next_url, is_hls, is_yt_stream);
            if let Some(path) =
                download_audio_to_cache_if_fresh(&next_url, &ctx.track_id, ext, &is_stale).await
            {
                if is_stale() {
                    return;
                }
                next_url = gtk4::gio::prelude::FileExt::uri(&gtk4::gio::File::for_path(&path))
                    .to_string();
            }
        }

        if is_stale() {
            return;
        }

        let effective_label = if quality_label.contains("(YT)") {
            quality_label
        } else {
            fresh_label
        };
        let cur_was_playing = self.audio.is_playing();
        eprintln!(
            "[play] #{} повторный запуск потока для {} (local={})",
            epoch,
            ctx.track_id,
            next_url.starts_with("file://")
        );
        self.dispatch_stream_playback_boxed(
            epoch,
            &next_url,
            effective_label,
            crossfade,
            cur_was_playing,
            fade_ms,
            fade_in_when_idle,
            Some(PlaybackRetryCtx {
                allow_retry: false,
                ..ctx
            }),
            on_started.unwrap_or_else(|| Box::new(|_| {})),
        );
    }

    #[allow(clippy::too_many_arguments)]
    async fn prepare_and_dispatch_stream(
        &self,
        epoch: u64,
        client: &YandexMusicClient,
        track_id: String,
        track_source: TrackSource,
        mut url: String,
        quality_label: String,
        crossfade: bool,
        was_playing: bool,
        fade_ms: u64,
        fade_in_when_idle: bool,
        station_id: String,
        batch_id: Option<String>,
        log_mode: &str,
        resolve_ms: u128,
        prefetch_hit: bool,
    ) {
        let is_stale = || self.generation.load(Ordering::SeqCst) != epoch;
        if is_stale() {
            eprintln!("[play] #{} устарел, поток не запускаем", epoch);
            return;
        }

        if track_source == TrackSource::Vk && !url.starts_with("file://") {
            if let Some(cached_path) = crate::cache::cached_audio_path(&track_id) {
                let local_uri =
                    gtk4::gio::prelude::FileExt::uri(&gtk4::gio::File::for_path(&cached_path))
                        .to_string();
                eprintln!("[play] Используем закэшированный трек: {}", local_uri);
                url = local_uri;
            } else if crate::player::audio::is_hls_url(&url) || url.contains(".m3u8") {
                // Холодный старт VK HLS трека: скачиваем и упаковываем в .mka перед стартом,
                // чтобы исключить разрывы потока на 2-й секунде
                eprintln!("[play] Холодный remux HLS трека в mka...");
                self.audio.fade_out(300);
                if let Some(cached_path) =
                    download_audio_to_cache_if_fresh(&url, &track_id, "mka", &is_stale).await
                {
                    let local_uri =
                        gtk4::gio::prelude::FileExt::uri(&gtk4::gio::File::for_path(&cached_path))
                            .to_string();
                    eprintln!("[play] HLS трек сконвертирован и запущен локально: {}", local_uri);
                    url = local_uri;
                }
            }
        }

        if should_predownload_segmented(track_source, &url) {
            let is_yt_stream =
                track_source == TrackSource::YouTubeMusic || url.contains("googlevideo.com");
            let cache_ext = audio_cache_ext_for_url(&url, false, is_yt_stream);
            let (resolved_url, cached_path) = download_audio_to_cache_with_retry(
                &self.providers,
                client,
                &track_id,
                track_source,
                url,
                cache_ext,
                &is_stale,
            )
            .await;
            url = resolved_url;
            if is_stale() {
                eprintln!(
                    "[play] #{} устарел во время предзагрузки, поток не запускаем",
                    epoch
                );
                return;
            }
            if let Some(path) = cached_path {
                let local_uri =
                    gtk4::gio::prelude::FileExt::uri(&gtk4::gio::File::for_path(&path)).to_string();
                eprintln!(
                    "[play] #{} трек {} предзагружен в кэш до старта GStreamer: {}",
                    epoch, track_id, local_uri
                );
                url = local_uri;
            }
        }

        if is_stale() {
            eprintln!("[play] #{} устарел после подготовки потока, не запускаем", epoch);
            return;
        }

        let is_yt_stream =
            track_source == TrackSource::YouTubeMusic || url.contains("googlevideo.com");
        let is_hls = (track_source == TrackSource::Vk && crate::player::audio::is_hls_url(&url))
            || url.contains(".m3u8");
        let cache_ext = audio_cache_ext_for_url(&url, is_hls, is_yt_stream);
        let cache_tid = track_id.clone();
        let cache_url = url.clone();
        let fb_providers = self.providers.clone();
        let fb_client = client.clone();
        let fb_track_id = track_id.clone();
        let fb_station_id = station_id;
        let fb_batch_id = batch_id;
        let cur_was_playing = was_playing && self.audio.is_playing();

        let dispatch_mode = if crossfade {
            if cur_was_playing {
                "crossfade"
            } else if fade_in_when_idle {
                "fade_in"
            } else {
                "direct"
            }
        } else {
            "direct"
        };
        eprintln!(
            "[play] #{} track={} source={:?} mode={} resolve_ms={} prefetch={} local={} dispatch={}",
            epoch,
            track_id,
            track_source,
            log_mode,
            resolve_ms,
            prefetch_hit,
            url.starts_with("file://"),
            dispatch_mode
        );

        self.dispatch_stream_playback(
            epoch,
            &url,
            quality_label,
            crossfade,
            cur_was_playing,
            fade_ms,
            fade_in_when_idle,
            Some(PlaybackRetryCtx {
                track_id,
                track_source,
                allow_retry: true,
            }),
            move |mgr| {
                // Фоновое сохранение играющего трека в audio_cache (если ещё не запущен с file://)
                let active_url = mgr.audio.active_uri().unwrap_or_else(|| cache_url.clone());
                if !active_url.starts_with("file://") && !is_hls {
                    if let Ok(handle) = tokio::runtime::Handle::try_current() {
                        handle.spawn(async move {
                            // Даем 4 секунды конвейеру GStreamer спокойно наполнить буфер звука без конкуренции за сеть
                            tokio::time::sleep(tokio::time::Duration::from_secs(4)).await;
                            download_audio_to_cache(&active_url, &cache_tid, cache_ext).await;
                        });
                    }
                }

                // Предзагрузка следующего трека
                mgr.prefetch_next_track();

                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        match track_source {
                            TrackSource::YouTubeMusic | TrackSource::Vk => {
                                if let Some(provider) = fb_providers.get(match track_source {
                                    TrackSource::YouTubeMusic => ProviderId::YouTubeMusic,
                                    TrackSource::Vk => ProviderId::Vk,
                                    TrackSource::Yandex => unreachable!(),
                                }) {
                                    let _ = provider
                                        .send_feedback(FeedbackEvent::TrackStarted {
                                            track_id: fb_track_id,
                                            station_id: fb_station_id,
                                            batch_id: fb_batch_id,
                                        })
                                        .await;
                                }
                            }
                            TrackSource::Yandex => {
                                let _ = fb_client
                                    .feedback_track_started(&fb_station_id, &fb_track_id, fb_batch_id)
                                    .await;
                            }
                        }
                    });
                }
            },
        );
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
        spawn_cover_and_lyrics(&self.event_senders, self.providers.clone(), client.clone(), track, epoch, &self.generation, &self.clips);
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
            if let Ok(mut state) = self.state.lock() {
                state.is_transitioning = false;
            }
            self.clear_loading_ui();
            return;
        }

        self.emit(PlayerUiEvent::LoadingTrack(Some(track.clone())));
        self.emit(PlayerUiEvent::BufferingProgress(None));
        self.emit(PlayerUiEvent::LoadingState(true));

        let (crossfade, crossfade_dur) = self.crossfade_settings();
        let fade_ms = (crossfade_dur as u64) * 1000;
        let was_playing = self.audio.is_playing();

        // СНАЧАЛА проверяем локальный диск: офлайн или кэш (0 мс задержки)
        let cached_opt = crate::cache::offline_audio_path(&track_id)
            .map(|p| (p, true))
            .or_else(|| crate::cache::cached_audio_path(&track_id).map(|p| (p, false)));

        if let Some((path, is_offline)) = cached_opt {
            let uri = gtk4::gio::prelude::FileExt::uri(&gtk4::gio::File::for_path(&path));
            let ext_label = path.extension().and_then(|e| e.to_str()).map(|ext| {
                match ext {
                    "flac" => "FLAC (диск)",
                    "mp3" => "MP3 320 (диск)",
                    "opus" => "Opus (диск)",
                    "mka" => "Диск (0 мс)",
                    _ => "Диск (0 мс)",
                }
            }).unwrap_or("Диск (0 мс)");
            let label = if is_offline { "Офлайн" } else { ext_label };
            let dispatch_mode = if crossfade && was_playing { "crossfade" } else { "direct" };
            eprintln!(
                "[play] #{} track={} source={:?} mode=cache({}) dispatch={}",
                epoch,
                track_id,
                track.source,
                if is_offline { "offline" } else { "disk" },
                dispatch_mode
            );
            self.dispatch_stream_playback(
                epoch,
                &uri,
                label.to_string(),
                crossfade,
                was_playing,
                fade_ms,
                false,
                None,
                |mgr| {
                    mgr.prefetch_next_track();
                },
            );
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
            let can_play = match track_source {
                TrackSource::YouTubeMusic => true,
                TrackSource::Vk => providers_clone
                    .get(ProviderId::Vk)
                    .is_some_and(|provider| provider.is_authenticated()),
                TrackSource::Yandex => client.has_token(),
            };

            if can_play {
                let started = std::time::Instant::now();
                // ссылка могла быть получена заранее, пока играл прежний трек
                let ready = prefetch.lock().unwrap().take().filter(|p| p.track_id == track_id && p.is_fresh());
                let hit = ready.is_some();
                let result: Result<(String, String), String> = match ready {
                    Some(p) => Ok((p.url, p.label)),
                    None => {
                        resolve_stream_for_source(
                            &providers_clone,
                            &client,
                            &track_id,
                            track_source,
                        )
                        .await
                    }
                };
                let resolve_ms = started.elapsed().as_millis();
                eprintln!("[play] #{} ссылка {} за {} мс ({})", epoch, if hit { "из предзагрузки" } else { "получена" }, resolve_ms, if result.is_ok() { "ok" } else { "ошибка" });
                if is_stale() {
                    eprintln!("[play] #{} устарел, поток не запускаем", epoch);
                    return;
                }
                match result {
                    Ok((url, label)) => {
                        mgr_clone
                            .prepare_and_dispatch_stream(
                                epoch,
                                &client,
                                track_id,
                                track_source,
                                url,
                                label,
                                crossfade,
                                was_playing,
                                fade_ms,
                                false,
                                station_id,
                                batch_id,
                                "http",
                                resolve_ms,
                                hit,
                            )
                            .await;
                    }
                    Err(e) => {
                        if is_stale() {
                            return;
                        }
                        let audio_playing = audio_clone.is_playing();
                        let (is_wave, failures, queue_len) = {
                            if let Ok(mut state) = state_arc.lock() {
                                state.consecutive_resolve_failures += 1;
                                (
                                    state.wave,
                                    state.consecutive_resolve_failures,
                                    state.queue.len(),
                                )
                            } else {
                                (false, 99, 0)
                            }
                        };

                        let can_skip = if is_wave {
                            failures <= 4
                        } else if audio_playing {
                            failures <= 4 && failures < queue_len
                        } else {
                            false
                        };

                        if can_skip {
                            if is_wave {
                                eprintln!(
                                    "[wave] трек {} ({}) недоступен ({}), пропускаем...",
                                    track_id, track_title, e
                                );
                            } else {
                                eprintln!(
                                    "[play] трек {} ({}) недоступен ({}), пропускаем...",
                                    track_id, track_title, e
                                );
                            }
                            emit_clear_loading(&senders);
                            mgr_clone.next_track_inner(true);
                            return;
                        }

                        if let Ok(mut state) = state_arc.lock() {
                            state.consecutive_resolve_failures = 0;
                        }
                        mgr_clone.rollback_track_if_playing(audio_playing);
                        emit_clear_loading(&senders);
                        if !audio_playing {
                            emit_all(&senders, PlayerUiEvent::PlaybackToggled(false));
                        }
                        if track_source == TrackSource::Vk && e.contains("Требуется повторный вход") {
                            emit_all(&senders, PlayerUiEvent::VkAuthRequired);
                        }
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

                    if is_stale() {
                        return;
                    }

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
                        emit_all(&senders, PlayerUiEvent::LoadingTrack(Some(track_to_play.clone())));
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
                        spawn_cover_and_lyrics(&senders, providers_clone.clone(), client.clone(), &track_to_play, epoch, &stale_gen, &mgr_clone.clips);

                        let started = std::time::Instant::now();
                        match yt.get_stream_url(&track_to_play.id).await {
                            Ok(url) => {
                                let resolve_ms = started.elapsed().as_millis();
                                mgr_clone
                                    .prepare_and_dispatch_stream(
                                        epoch,
                                        &client,
                                        track_to_play.id,
                                        track_to_play.source,
                                        url,
                                        "Opus 160 (YT)".to_string(),
                                        crossfade,
                                        was_playing,
                                        fade_ms,
                                        true,
                                        station_id,
                                        batch_id,
                                        "http(yt-fallback)",
                                        resolve_ms,
                                        false,
                                    )
                                    .await;
                                return;
                            }
                            Err(e) => {
                                if is_stale() {
                                    return;
                                }
                                eprintln!("[play] Ошибка получения потока YT: {}", e);
                            }
                        }
                    }
                }

                if is_stale() {
                    return;
                }
                let audio_playing = audio_clone.is_playing();
                mgr_clone.rollback_track_if_playing(audio_playing);
                emit_clear_loading(&senders);
                if !audio_playing {
                    emit_all(&senders, PlayerUiEvent::PlaybackToggled(false));
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
        let provider = self.providers.current();
        if provider.provider_id() == ProviderId::YouTubeMusic && !provider.is_authenticated() {
            self.emit(PlayerUiEvent::StatusMessage(
                "Войдите в Google Аккаунт внизу сайдбара для доступа к персонализированной волне YouTube Music".to_string(),
            ));
            return;
        }

        let is_yt = provider.provider_id() == ProviderId::YouTubeMusic;
        let is_vk = provider.provider_id() == ProviderId::Vk;

        if is_vk {
            if !provider.is_authenticated() {
                self.emit(PlayerUiEvent::VkAuthRequired);
                self.emit(PlayerUiEvent::StatusMessage(
                    "Войдите в VK ID для запуска Стрим-микса".to_string(),
                ));
                return;
            }
            let is_reset = mood.is_none() && language.is_none() && character.is_none();
            let station_id = if is_reset {
                crate::api::vk::provider::VK_MIX_STATION_ID.to_string()
            } else {
                let mut settings = Vec::new();
                if let Some(m) = mood.as_deref().filter(|s| !s.is_empty()) {
                    settings.push(m);
                }
                if let Some(l) = language.as_deref().filter(|s| !s.is_empty()) {
                    settings.push(l);
                }
                if let Some(c) = character.as_deref().filter(|s| !s.is_empty()) {
                    settings.push(c);
                }
                if settings.is_empty() {
                    crate::api::vk::provider::VK_MIX_STATION_ID.to_string()
                } else {
                    format!("vk:mix?settings={}", settings.join(","))
                }
            };
            self.apply_wave_station(station_id, mood, language, character);
            return;
        }

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
        let me = self.clone();

        tokio::spawn(async move {
            let provider = providers_clone.current();
            let is_vk = provider.provider_id() == ProviderId::Vk;
            let is_yt = provider.provider_id() == ProviderId::YouTubeMusic;
            let client = client_clone.lock().unwrap().clone();

            if is_vk {
                if !provider.is_authenticated() {
                    emit_all(&senders, PlayerUiEvent::VkAuthRequired);
                    emit_all(&senders, PlayerUiEvent::StatusMessage(
                        "Войдите в VK ID для запуска персональной Волны".to_string(),
                    ));
                    emit_all(&senders, PlayerUiEvent::WaveLoading(None));
                    return;
                }
            } else if is_yt {
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
            let mut fetch_res: Result<(Vec<Track>, Option<String>), String> = if is_vk {
                match provider.get_station_tracks(&station_id, None).await {
                    Ok(batch) => {
                        let b_id = if batch.batch_id.is_empty() { None } else { Some(batch.batch_id) };
                        Ok((batch.tracks, b_id))
                    }
                    Err(e) => Err(format!("Ошибка VK Музыки: {}", e)),
                }
            } else if is_yt {
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
                        spawn_cover_and_lyrics(&senders, providers_clone.clone(), client.clone(), &track, epoch, &current_gen, &clips_clone);
                        crate::api::history::save_last_track(&track);

                        // Сразу запускаем воспроизведение первого трека Волны
                        let track_id = track.id.clone();
                        let b_id = batch_id.clone();
                        let result: Result<(String, String), String> = resolve_stream_for_source(
                            &providers_clone,
                            &client,
                            &track_id,
                            track.source,
                        )
                        .await;

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
                                match track.source {
                                    TrackSource::YouTubeMusic | TrackSource::Vk => {
                                        let provider_id = match track.source {
                                            TrackSource::YouTubeMusic => ProviderId::YouTubeMusic,
                                            TrackSource::Vk => ProviderId::Vk,
                                            TrackSource::Yandex => unreachable!(),
                                        };
                                        if let Some(provider) = providers_clone.get(provider_id) {
                                            let _ = provider
                                                .send_feedback(FeedbackEvent::TrackStarted {
                                                    track_id: track_id.clone(),
                                                    station_id: station_id.clone(),
                                                    batch_id: b_id,
                                                })
                                                .await;
                                        }
                                    }
                                    TrackSource::Yandex => {
                                        let _ = client.feedback_track_started(&station_id, &track_id, b_id).await;
                                    }
                                }
                                if tracks.len() < 8 {
                                    me.load_more_wave_tracks();
                                }
                            }
                            Err(e) => {
                                if track.source == TrackSource::Vk && e.contains("Требуется повторный вход") {
                                    emit_all(&senders, PlayerUiEvent::VkAuthRequired);
                                }
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

/// Определяет расширение файла для дискового кэша (`~/.cache/volna/audio/`) по URL потока.
pub(crate) fn audio_cache_ext_for_url(stream_url: &str, is_hls: bool, is_yt: bool) -> &'static str {
    if is_hls {
        "mka"
    } else if is_yt {
        "opus"
    } else if stream_url.contains("flac") || stream_url.contains("type=3") {
        "flac"
    } else {
        "mp3"
    }
}

/// Предел размера (16 МБ) для обязательной сегментированной предзагрузки в кэш до старта GStreamer.
const AUDIO_PREDOWNLOAD_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// Нужно ли предварительно скачать поток сегментами по 2 МБ до старта GStreamer.
fn should_predownload_segmented(source: TrackSource, url: &str) -> bool {
    !url.starts_with("file://")
        && (source == TrackSource::YouTubeMusic || url.contains("googlevideo.com"))
        && parse_clen_from_url(url).is_none_or(|c| c <= AUDIO_PREDOWNLOAD_MAX_BYTES)
}

/// Предварительно скачивает аудиопоток в кэш с однократным переключением GGC-ноды при сбое.
async fn download_audio_to_cache_with_retry(
    providers: &ProviderManager,
    client: &YandexMusicClient,
    track_id: &str,
    track_source: TrackSource,
    initial_url: String,
    ext: &str,
    is_stale: &(impl Fn() -> bool + Sync),
) -> (String, Option<std::path::PathBuf>) {
    let mut url = initial_url;
    for attempt in 0..2 {
        if is_stale() || url.starts_with("file://") {
            return (url, None);
        }
        if parse_clen_from_url(&url).is_some_and(|c| c > AUDIO_PREDOWNLOAD_MAX_BYTES) {
            return (url, None);
        }
        if let Some(path) = download_audio_to_cache_if_fresh(&url, track_id, ext, is_stale).await {
            return (url, Some(path));
        }
        if is_stale() {
            return (url, None);
        }
        if url.contains("googlevideo.com") || track_source == TrackSource::YouTubeMusic {
            crate::api::ytmusic::remember_blocked_ggc_node_for_url(&url);
        }
        if attempt == 0 {
            eprintln!(
                "[play] Сегментированная предзагрузка для {} не удалась, пробуем другую ноду GGC...",
                track_id
            );
            match resolve_stream_for_source(providers, client, track_id, track_source).await {
                Ok((fresh_url, _)) => {
                    if is_stale() {
                        return (fresh_url, None);
                    }
                    url = fresh_url;
                }
                Err(err) => {
                    eprintln!(
                        "[play] Повторный резолв потока для {} не удался: {}",
                        track_id, err
                    );
                    return (url, None);
                }
            }
        }
    }
    (url, None)
}

/// Скачивает аудиопоток в дисковый кэш (~/.cache/volna/audio/) и очищает старые треки по LRU
async fn download_audio_to_cache(url: &str, track_id: &str, ext: &str) -> Option<std::path::PathBuf> {
    download_audio_to_cache_if_fresh(url, track_id, ext, &|| false).await
}

/// Вариант `download_audio_to_cache` с проверкой отмены `should_cancel` между сегментами/чанками.
async fn download_audio_to_cache_if_fresh(
    url: &str,
    track_id: &str,
    ext: &str,
    should_cancel: &(impl Fn() -> bool + Sync),
) -> Option<std::path::PathBuf> {
    if url.starts_with("file://") || should_cancel() {
        return None;
    }
    let final_path = crate::cache::final_audio_cache_path(track_id, ext);
    if final_path.exists() {
        if let Ok(meta) = final_path.metadata() {
            if meta.len() > 128 * 1024 {
                return Some(final_path);
            }
        }
    }
    let _guard = {
        let mut locks = AUDIO_DOWNLOAD_LOCKS.lock().await;
        while locks.contains(track_id) {
            drop(locks);
            if should_cancel() {
                return None;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            if should_cancel() {
                return None;
            }
            // Если пока мы ждали, другой поток уже скачал файл:
            let final_path = crate::cache::final_audio_cache_path(track_id, ext);
            if final_path.exists() {
                if let Ok(meta) = final_path.metadata() {
                    if meta.len() > 128 * 1024 {
                        return Some(final_path);
                    }
                }
            }
            locks = AUDIO_DOWNLOAD_LOCKS.lock().await;
        }
        if should_cancel() {
            return None;
        }
        locks.insert(track_id.to_string());
        // Возвращаем RAII guard, который при выходе из функции удалит track_id из locks
        struct DownloadGuard(String);
        impl Drop for DownloadGuard {
            fn drop(&mut self) {
                if let Ok(mut locks) = AUDIO_DOWNLOAD_LOCKS.try_lock() {
                    locks.remove(&self.0);
                    return;
                }
                let id = self.0.clone();
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        let mut locks = AUDIO_DOWNLOAD_LOCKS.lock().await;
                        locks.remove(&id);
                    });
                }
            }
        }
        DownloadGuard(track_id.to_string())
    };

    if should_cancel() {
        return None;
    }

    let tmp = crate::cache::temp_audio_cache_path(track_id, ext);

    if crate::player::audio::is_hls_url(url) || ext == "mka" {
        let mut cmd = tokio::process::Command::new("ffmpeg");
        cmd.arg("-y")
            .arg("-nostdin")
            .arg("-loglevel")
            .arg("error")
            .arg("-user_agent")
            .arg("Mozilla/5.0 (Macintosh; Intel Mac OS X 15_7_3) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15")
            .arg("-rw_timeout")
            .arg("15000000") // 15 секунд в микросекундах
            .arg("-i")
            .arg(url)
            .arg("-vn")
            .arg("-map")
            .arg("0:a:0")
            .arg("-c")
            .arg("copy")
            .arg("-f")
            .arg("matroska")
            .arg(&tmp);
        cmd.kill_on_drop(true);

        let output = cmd.output().await.ok()?;
        if should_cancel() {
            let _ = std::fs::remove_file(&tmp);
            return None;
        }
        if output.status.success() && tmp.exists() {
            if let Ok(meta) = tmp.metadata() {
                if meta.len() > 128 * 1024 {
                    std::fs::rename(&tmp, &final_path).ok()?;
                    crate::cache::clean_audio_cache_lru(600 * 1024 * 1024);
                    eprintln!("[cache] HLS трек успешно сохранен в mka: {:?}", final_path);
                    return Some(final_path);
                }
            }
        } else {
            let err_msg = String::from_utf8_lossy(&output.stderr);
            eprintln!("[cache] ffmpeg remux ошибка: {}", err_msg);
        }
        let _ = std::fs::remove_file(&tmp);
        return None;
    }

    if url.contains("googlevideo.com") || ext == "opus" {
        if download_audio_range_segmented_cancellable(url, &tmp, should_cancel)
            .await
            .is_some()
        {
            if should_cancel() {
                let _ = std::fs::remove_file(&tmp);
                return None;
            }
            std::fs::rename(&tmp, &final_path).ok()?;
            crate::cache::clean_audio_cache_lru(600 * 1024 * 1024);
            return Some(final_path);
        }
        let _ = std::fs::remove_file(&tmp);
        return None;
    }

    let resp = crate::shared_http_client()
        .get(url)
        .timeout(std::time::Duration::from_secs(60))
        .header("User-Agent", crate::api::ytmusic::UA_SAFARI_26)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() || should_cancel() {
        return None;
    }

    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    struct TempFileGuard<'a>(&'a std::path::Path);
    impl Drop for TempFileGuard<'_> {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(self.0);
        }
    }

    let tmp_guard = TempFileGuard(&tmp);
    let mut file = tokio::fs::File::create(&tmp).await.ok()?;
    let mut stream = resp.bytes_stream();
    let mut total_written: u64 = 0;

    while let Some(chunk_res) = stream.next().await {
        if should_cancel() {
            return None;
        }
        let chunk = chunk_res.ok()?;
        if chunk.is_empty() {
            continue;
        }
        total_written = total_written.saturating_add(chunk.len() as u64);
        if total_written > AUDIO_RANGE_MAX_BYTES {
            return None;
        }
        file.write_all(&chunk).await.ok()?;
    }
    file.flush().await.ok()?;
    drop(file);

    if should_cancel() || total_written <= 128 * 1024 {
        return None;
    }
    std::fs::rename(&tmp, &final_path).ok()?;
    std::mem::forget(tmp_guard);
    crate::cache::clean_audio_cache_lru(600 * 1024 * 1024);
    Some(final_path)
}

/// Размер одного Range-сегмента при скачивании аудио с googlevideo.com (2 МБ)
const AUDIO_RANGE_SEGMENT_BYTES: u64 = 2 * 1024 * 1024;
/// Таймаут одного Range-сегмента (12 секунд)
const AUDIO_RANGE_SEGMENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);
/// Максимальный допустимый размер аудиофайла в кэше (64 МБ)
const AUDIO_RANGE_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Извлекает полный размер ресурса из заголовка `Content-Range: bytes start-end/total`
fn parse_content_range_total(header_val: &str) -> Option<u64> {
    let total_str = header_val.rsplit('/').next()?.trim();
    if total_str == "*" {
        return None;
    }
    total_str.parse::<u64>().ok().filter(|&n| n > 0)
}

/// Извлекает параметр `clen=<bytes>` из query-строки URL googlevideo.com
fn parse_clen_from_url(url: &str) -> Option<u64> {
    let (_, query) = url.split_once('?')?;
    for pair in query.split('&') {
        if let Some(val) = pair.strip_prefix("clen=") {
            if let Ok(n) = val.trim().parse::<u64>()
                && n > 0
            {
                return Some(n);
            }
        }
    }
    None
}

/// Быстрая сегментированная докачка аудиопотока по `Range: bytes={start}-{end}` чанками по 2 МБ.
/// На `googlevideo.com` (`c=VISIONOS`) поток без `Range` троттлится до ~20 КБ/с, а короткие
/// `Range`-сегменты отдаются на полной скорости CDN.
#[cfg_attr(not(test), allow(dead_code))]
async fn download_audio_range_segmented(url: &str, tmp_path: &std::path::Path) -> Option<u64> {
    download_audio_range_segmented_cancellable(url, tmp_path, &|| false).await
}

/// Отменяемая между Range-сегментами версия `download_audio_range_segmented`.
async fn download_audio_range_segmented_cancellable(
    url: &str,
    tmp_path: &std::path::Path,
    should_cancel: &(impl Fn() -> bool + Sync),
) -> Option<u64> {
    if should_cancel() {
        let _ = std::fs::remove_file(tmp_path);
        return None;
    }
    let client = crate::no_keepalive_http_client();

    let mut total: Option<u64> = parse_clen_from_url(url);
    if total.is_some_and(|t| t > AUDIO_RANGE_MAX_BYTES) {
        return None;
    }

    let mut out: Vec<u8> = Vec::with_capacity(
        total
            .map(|t| t.min(8 * 1024 * 1024) as usize)
            .unwrap_or(4 * 1024 * 1024),
    );

    loop {
        if should_cancel() {
            let _ = std::fs::remove_file(tmp_path);
            return None;
        }
        let start = out.len() as u64;
        if let Some(t) = total
            && start >= t
        {
            break;
        }
        if start > AUDIO_RANGE_MAX_BYTES {
            return None;
        }

        let end = match total {
            Some(t) if t > 0 => (start + AUDIO_RANGE_SEGMENT_BYTES - 1).min(t - 1),
            _ => start + AUDIO_RANGE_SEGMENT_BYTES - 1,
        };
        let requested_len = end.saturating_sub(start) + 1;

        let mut segment_bytes = None;
        let mut got_full_ok = false;
        for _attempt in 0..2 {
            if should_cancel() {
                let _ = std::fs::remove_file(tmp_path);
                return None;
            }
            let req = client
                .get(url)
                .timeout(AUDIO_RANGE_SEGMENT_TIMEOUT)
                .header("User-Agent", crate::api::ytmusic::UA_SAFARI_26)
                .header("Referer", "https://www.youtube.com/")
                .header("Origin", "https://www.youtube.com")
                .header(reqwest::header::RANGE, format!("bytes={start}-{end}"));

            let Ok(resp) = tokio::time::timeout(AUDIO_RANGE_SEGMENT_TIMEOUT, req.send()).await else {
                continue;
            };
            if should_cancel() {
                let _ = std::fs::remove_file(tmp_path);
                return None;
            }
            let Ok(resp) = resp else {
                continue;
            };
            let status = resp.status();
            if !status.is_success() {
                continue;
            }
            if total.is_none() {
                total = resp
                    .headers()
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok())
                    .and_then(parse_content_range_total);
                if total.is_some_and(|t| t > AUDIO_RANGE_MAX_BYTES) {
                    return None;
                }
            }
            let Ok(bytes) = tokio::time::timeout(AUDIO_RANGE_SEGMENT_TIMEOUT, resp.bytes()).await
            else {
                continue;
            };
            if should_cancel() {
                let _ = std::fs::remove_file(tmp_path);
                return None;
            }
            let Ok(bytes) = bytes else {
                continue;
            };
            if status == reqwest::StatusCode::OK {
                got_full_ok = true;
            }
            segment_bytes = Some(bytes);
            break;
        }

        let chunk = segment_bytes?;
        if chunk.is_empty() {
            break;
        }

        if got_full_ok {
            // Сервер проигнорировал Range и вернул весь файл со статусом 200 OK
            if start == 0 {
                out.extend_from_slice(&chunk);
            } else {
                out = chunk.to_vec();
            }
            break;
        }

        out.extend_from_slice(&chunk);

        if let Some(t) = total {
            if (out.len() as u64) >= t {
                break;
            }
        } else if (chunk.len() as u64) < requested_len {
            break;
        }
    }

    if should_cancel() {
        let _ = std::fs::remove_file(tmp_path);
        return None;
    }
    if let Some(t) = total
        && (out.len() as u64) < t
    {
        return None;
    }
    if out.len() <= 128 * 1024 {
        return None;
    }

    std::fs::write(tmp_path, &out).ok()?;
    Some(out.len() as u64)
}

/// Обложка и текст песни грузятся отдельными задачами, чтобы не задерживать воспроизведение.
/// Если у трека текста нет, UI получает пустую строку: старый текст предыдущего трека не остаётся.
fn spawn_cover_and_lyrics(
    senders: &Arc<Mutex<Vec<Sender<PlayerUiEvent>>>>,
    providers: Arc<ProviderManager>,
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

    if crate::api::auth::load_clips() && track.source != TrackSource::Vk {
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
            if clip_track.source == TrackSource::YouTubeMusic || clip_track.source == TrackSource::Vk {
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
    let track_for_lyrics = track.clone();
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

        let text = if track_source == TrackSource::Vk {
            match providers.get(ProviderId::Vk) {
                Some(provider) => provider
                    .get_track_lyrics(&track_for_lyrics)
                    .await
                    .unwrap_or_default(),
                None => String::new(),
            }
        } else if is_yt {
            let http = crate::shared_http_client();

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

            // 2. Параллельно запрашиваем LRCLIB (синхронизированное караоке) и официальный текст YTM
            let lyrics_timeout = std::time::Duration::from_secs(8);
            let (lrclib_res, yt_res) = tokio::join!(
                async {
                    tokio::time::timeout(
                        lyrics_timeout,
                        crate::api::lrclib::fetch_lrclib_lyrics(http, &artist, &title, duration),
                    )
                    .await
                    .ok()
                    .flatten()
                },
                async {
                    tokio::time::timeout(lyrics_timeout, ytm.get_yt_lyrics(&track_id))
                        .await
                        .ok()
                        .flatten()
                }
            );
            let yt_text = yt_res
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());

            // При наличии официального текста YTM отсекаем отравленный ответ LRCLIB и переискиваем с эталоном
            let verified_lrclib = match yt_text.as_deref() {
                Some(yt) => {
                    crate::api::lrclib::verify_or_refetch_lrclib(
                        http,
                        lrclib_res,
                        &artist,
                        &title,
                        duration,
                        yt,
                        Some(std::time::Duration::from_secs(6)),
                    )
                    .await
                }
                None => lrclib_res,
            };

            let has_synced_lrclib = accept_synced_yt_candidate(
                verified_lrclib.as_deref(),
                yt_text.as_deref(),
                duration,
            )
            .is_some();

            // 3. Если в LRCLIB нет подтверждённых синхронизированных таймкодов —
            // пробуем найти караоке в каталоге Яндекс Музыки!
            let mut yandex_lyrics: Option<String> = None;
            if !has_synced_lrclib {
                let clean_title = crate::api::lrclib::clean_title_for_lyrics(&title);
                let query = format!("{} {}", artist, clean_title);
                if let Ok(search_res) = client.search(&query).await
                    && let Some(tracks) = search_res.tracks
                {
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
                            if let Ok(Some(y_lyrics)) = client
                                .get_track_lyrics_with_reference(
                                    &y_track.id,
                                    &artist,
                                    &title,
                                    duration,
                                    yt_text.as_deref(),
                                )
                                .await
                            {
                                let trimmed = y_lyrics.trim();
                                if crate::api::lrclib::looks_like_foreign_song(trimmed, duration) {
                                    println!("[lyrics] Яндекс: разметка не от этого трека — пропускаем");
                                } else if yt_text.as_deref().is_some_and(|yt| {
                                    !crate::api::lrclib::lyrics_match_reference(trimmed, yt)
                                }) {
                                    println!("[lyrics] Яндекс: текст не совпал с официальным текстом YTM — пропускаем");
                                } else {
                                    yandex_lyrics = Some(trimmed.to_string());
                                }
                            }
                        }
                        None => println!(
                            "[lyrics] Яндекс: подходящего трека не нашлось — фолбэк пропущен"
                        ),
                    }
                }
            }

            // 4. Выбираем итоговый текст: подтверждённое караоке → официальный текст YTM → статика LRCLIB → старый кэш
            match resolve_yt_lyrics_outcome(
                verified_lrclib.as_deref(),
                yandex_lyrics.as_deref(),
                yt_text.as_deref(),
                cached_plain.as_deref(),
                had_stale_synced,
                duration,
            ) {
                Some(resolved) => {
                    if resolved.is_yt_official {
                        println!(
                            "[lyrics] YouTube Music: официальный текст, {} симв.",
                            resolved.text.chars().count()
                        );
                    }
                    if resolved.cache_text {
                        let _ = std::fs::write(&cache_path, &resolved.text);
                        crate::cache::clear_negative_lyrics_marker(&track_id);
                    }
                    resolved.text
                }
                None => String::new(),
            }
        } else {
            // Трек из Яндекс Музыки
            let lookup = async {
                if client.has_token() {
                    client.get_track_lyrics(&track_id, &artist, &title, duration).await.ok().flatten()
                } else {
                    crate::api::lrclib::get_cached_or_fetch_lrclib(
                        crate::shared_http_client(),
                        &track_id,
                        &artist,
                        &title,
                        duration,
                    )
                    .await
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

#[derive(Debug, PartialEq, Eq)]
struct ResolvedYtLyrics {
    text: String,
    cache_text: bool,
    is_yt_official: bool,
}

fn accept_synced_yt_candidate(
    candidate: Option<&str>,
    yt_text: Option<&str>,
    duration: Option<u64>,
) -> Option<String> {
    let trimmed = candidate?.trim();
    if trimmed.is_empty() || crate::api::lrclib::looks_like_foreign_song(trimmed, duration) {
        return None;
    }
    if let Some(yt) = yt_text
        && !crate::api::lrclib::lyrics_match_reference(trimmed, yt)
    {
        return None;
    }
    if crate::api::lrclib::usable_synced_at(trimmed, duration) {
        return Some(trimmed.to_string());
    }
    None
}

fn resolve_yt_lyrics_outcome(
    lrclib_text: Option<&str>,
    yandex_text: Option<&str>,
    yt_text: Option<&str>,
    cached_plain: Option<&str>,
    had_stale_synced: bool,
    duration: Option<u64>,
) -> Option<ResolvedYtLyrics> {
    if let Some(synced) = accept_synced_yt_candidate(lrclib_text, yt_text, duration) {
        return Some(ResolvedYtLyrics {
            text: synced,
            cache_text: true,
            is_yt_official: false,
        });
    }
    if let Some(synced) = accept_synced_yt_candidate(yandex_text, yt_text, duration) {
        return Some(ResolvedYtLyrics {
            text: synced,
            cache_text: true,
            is_yt_official: false,
        });
    }
    if let Some(yt) = yt_text.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(ResolvedYtLyrics {
            text: yt.to_string(),
            cache_text: !had_stale_synced,
            is_yt_official: true,
        });
    }
    if let Some(lr) = lrclib_text
        .map(str::trim)
        .filter(|s| !s.is_empty() && !crate::api::lrclib::looks_like_foreign_song(s, duration))
    {
        return Some(ResolvedYtLyrics {
            text: crate::api::lrclib::demote_stale_synced(lr, duration),
            cache_text: false,
            is_yt_official: false,
        });
    }
    cached_plain
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| ResolvedYtLyrics {
            text: s.to_string(),
            cache_text: false,
            is_yt_official: false,
        })
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

/// Полностью снимает состояние загрузки, активный загружаемый трек и процент буфера в UI.
fn emit_clear_loading(senders: &Arc<Mutex<Vec<Sender<PlayerUiEvent>>>>) {
    emit_all(senders, PlayerUiEvent::LoadingState(false));
    emit_all(senders, PlayerUiEvent::LoadingTrack(None));
    emit_all(senders, PlayerUiEvent::BufferingProgress(None));
}

/// Дедуплицирует и рассылает процент буферизации (`Some(0..=99)` или `None` при 100%).
fn emit_buffering_if_changed(
    last_pct: &std::sync::atomic::AtomicU8,
    senders: &Arc<Mutex<Vec<Sender<PlayerUiEvent>>>>,
    pct: u8,
) {
    let clamped = pct.min(100);
    if clamped >= 100 {
        if last_pct.swap(255, Ordering::SeqCst) != 255 {
            emit_all(senders, PlayerUiEvent::BufferingProgress(None));
        }
    } else if last_pct.swap(clamped, Ordering::SeqCst) != clamped {
        emit_all(senders, PlayerUiEvent::BufferingProgress(Some(clamped)));
    }
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
            watchdog: Arc::new(Mutex::new(PlaybackWatchdog::new())),
            artist_registry: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
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
            watchdog: Arc::new(Mutex::new(PlaybackWatchdog::new())),
            artist_registry: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
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

    #[test]
    fn test_player_manager_lookup_registered_artist_by_id() {
        let client = YandexMusicClient::new(None);
        let audio = Arc::new(AudioBackend::new().unwrap());
        let client_mutex = Arc::new(Mutex::new(client.clone()));
        let state_arc = Arc::new(Mutex::new(PlayerState::default()));
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
            state: state_arc,
            wave_engine: Arc::new(Mutex::new(WaveEngine::new())),
            event_senders: Arc::new(Mutex::new(Vec::new())),
            cast: Arc::default(),
            prefetch: Arc::default(),
            prefetch_inflight: Arc::default(),
            generation: Arc::new(AtomicU64::new(0)),
            tail_swap_epoch: Arc::new(AtomicU64::new(0)),
            crossfade: Arc::new(Mutex::new((false, 3))),
            watchdog: Arc::new(Mutex::new(PlaybackWatchdog::new())),
            artist_registry: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        };

        // 1. Регистрация Yandex 123 -> находит
        manager.register_artist(TrackSource::Yandex, "123", "Radiohead");
        assert_eq!(
            manager.lookup_registered_artist_by_id("123"),
            Some((TrackSource::Yandex, "Radiohead".to_string()))
        );

        // 2. Коллизия Yandex 123 и VK 123 -> отказ (None)
        manager.register_artist(TrackSource::Vk, "123", "Три дня дождя");
        assert_eq!(manager.lookup_registered_artist_by_id("123"), None);

        // 3. Неизвестный ID -> None
        assert_eq!(manager.lookup_registered_artist_by_id("456"), None);
        assert_eq!(manager.lookup_registered_artist_by_id(""), None);
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

#[cfg(test)]
mod audio_range_download_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn test_audio_cache_ext_for_url() {
        assert_eq!(
            audio_cache_ext_for_url("https://vkuseraudio.net/s/v1/ac/index.m3u8", true, false),
            "mka"
        );
        assert_eq!(
            audio_cache_ext_for_url(
                "https://rr2---sn-u15hn5-cc.googlevideo.com/videoplayback?itag=251",
                false,
                true
            ),
            "opus"
        );
        assert_eq!(
            audio_cache_ext_for_url(
                "https://s547sas.storage.yandex.net/get-mp3/abc123/00063b963d7ca2ac/rmusic/U2FsdGVkX1/d98d66/flac-mp4?track-id=133282800",
                false,
                false
            ),
            "flac"
        );
        assert_eq!(
            audio_cache_ext_for_url("https://example.com/audio/track.flac", false, false),
            "flac"
        );
        assert_eq!(
            audio_cache_ext_for_url("https://example.com/audio?type=flac", false, false),
            "flac"
        );
        assert_eq!(
            audio_cache_ext_for_url("https://example.com/audio?type=3", false, false),
            "flac"
        );
        assert_eq!(
            audio_cache_ext_for_url(
                "https://s222vla.storage.yandex.net/get-mp3/abc123/00063b963d7c/rmusic/U2FsdGVkX1/track?track-id=35302834",
                false,
                false
            ),
            "mp3"
        );
    }

    #[test]
    fn test_parse_content_range_and_clen() {
        assert_eq!(
            parse_content_range_total("bytes 0-2097151/3845120"),
            Some(3845120)
        );
        assert_eq!(
            parse_content_range_total("bytes 2097152-3845119/3845120"),
            Some(3845120)
        );
        assert_eq!(parse_content_range_total("bytes 0-1023/*"), None);
        assert_eq!(parse_content_range_total("invalid"), None);

        assert_eq!(
            parse_clen_from_url(
                "https://rr2---sn-u15hn5-cc.googlevideo.com/videoplayback?itag=251&clen=3456789&mime=audio%2Fwebm"
            ),
            Some(3456789)
        );
        assert_eq!(
            parse_clen_from_url("https://rr2---sn-u15hn5-cc.googlevideo.com/videoplayback?itag=251"),
            None
        );
    }

    #[tokio::test]
    async fn test_download_audio_range_segmented_chunks_and_headers() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local test server");
        let addr = listener.local_addr().unwrap();

        // Создаём тестовый payload размером 2.5 МБ (> 2 МБ, чтобы гарантировать 2 Range-запроса)
        let total_size: usize = 2 * 1024 * 1024 + 512 * 1024;
        let payload: Vec<u8> = (0..total_size).map(|i| (i % 251) as u8).collect();
        let payload_server = payload.clone();
        let seen_ranges = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let seen_ranges_srv = seen_ranges.clone();

        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                let Ok(n) = socket.read(&mut buf).await else {
                    break;
                };
                if n == 0 {
                    break;
                }
                let req_str = String::from_utf8_lossy(&buf[..n]);
                let req_lower = req_str.to_lowercase();
                assert!(
                    req_lower.contains("safari/605.1.15"),
                    "запрос должен содержать User-Agent UA_SAFARI_26: {req_str}"
                );

                let range_line = req_str
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("range:"))
                    .unwrap_or("")
                    .trim()
                    .to_string();
                seen_ranges_srv.lock().unwrap().push(range_line.clone());

                let (start, end) = range_line
                    .split_once("bytes=")
                    .and_then(|(_, r)| r.split_once('-'))
                    .map(|(s, e)| {
                        (
                            s.trim().parse::<usize>().unwrap_or(0),
                            e.trim().parse::<usize>().unwrap_or(total_size - 1),
                        )
                    })
                    .unwrap_or((0, total_size - 1));

                let actual_end = end.min(total_size - 1);
                let slice = &payload_server[start..=actual_end];
                let header = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Type: audio/webm\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                    slice.len(),
                    start,
                    actual_end,
                    total_size
                );
                let _ = socket.write_all(header.as_bytes()).await;
                let _ = socket.write_all(slice).await;
                let _ = socket.shutdown().await;
            }
        });

        let tmp_path = std::env::temp_dir().join(format!(
            "volna_range_test_{}.opus.part",
            std::process::id()
        ));
        let url = format!("http://{addr}/videoplayback?itag=251");
        let downloaded = download_audio_range_segmented(&url, &tmp_path)
            .await
            .expect("segmented download should succeed");

        assert_eq!(downloaded as usize, total_size);
        let written = std::fs::read(&tmp_path).expect("read tmp file");
        assert_eq!(written.len(), total_size);
        assert_eq!(written, payload);

        let ranges = seen_ranges.lock().unwrap().clone();
        assert_eq!(ranges.len(), 2, "ожидалось ровно 2 сегмента для 2.5 МБ");
        assert!(ranges[0].contains("bytes=0-2097151"));
        assert!(ranges[1].contains("bytes=2097152-"));

        let _ = std::fs::remove_file(&tmp_path);
    }
}

#[cfg(test)]
mod playback_watchdog_tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn test_watchdog_resets_timer_on_pause_ineligible() {
        let mut wd = PlaybackWatchdog::new();

        assert_eq!(
            wd.poll(1_000, true, 1, "t1", TrackSource::Yandex, Some(5_000), || false),
            WatchdogAction::None
        );
        // Зависаем на 3000 мс (меньше порога 4000 мс)
        assert_eq!(
            wd.poll(4_000, true, 1, "t1", TrackSource::Yandex, Some(5_000), || true),
            WatchdogAction::None
        );

        // Переходим на паузу (!eligible) — таймер сбрасывается
        assert_eq!(
            wd.poll(4_100, false, 1, "t1", TrackSource::Yandex, Some(5_000), || true),
            WatchdogAction::None
        );

        // Снятие с паузы: отсчёт 4000 мс должен начаться заново с 4200, а не с 1000
        assert_eq!(
            wd.poll(4_200, true, 1, "t1", TrackSource::Yandex, Some(5_000), || true),
            WatchdogAction::None
        );
        assert_eq!(
            wd.poll(8_100, true, 1, "t1", TrackSource::Yandex, Some(5_000), || true),
            WatchdogAction::None
        );
        assert_eq!(
            wd.poll(8_200, true, 1, "t1", TrackSource::Yandex, Some(5_000), || true),
            WatchdogAction::SwitchToCache {
                track_id: "t1".to_string(),
                resume_pos_ms: 5_000,
                stalled_ms: 4_000,
            }
        );
    }

    #[test]
    fn test_watchdog_ignores_non_http_uris() {
        assert!(!PlaybackWatchdog::is_http_uri(None));
        assert!(!PlaybackWatchdog::is_http_uri(Some("file:///home/user/cache/1.mp3")));
        assert!(PlaybackWatchdog::is_http_uri(Some("http://example.com/a.mp3")));
        assert!(PlaybackWatchdog::is_http_uri(Some("https://example.com/a.opus")));

        let mut wd = PlaybackWatchdog::new();
        for t in [0, 4_000, 12_000, 30_000] {
            assert_eq!(
                wd.poll(t, false, 1, "local_1", TrackSource::Vk, Some(1_000), || true),
                WatchdogAction::None
            );
        }
    }

    #[test]
    fn test_watchdog_switches_to_cache_once_at_4000ms_and_skips_early_fs_check() {
        let mut wd = PlaybackWatchdog::new();
        let fs_checks = AtomicUsize::new(0);
        let check_cache = || {
            fs_checks.fetch_add(1, Ordering::SeqCst);
            true
        };

        assert_eq!(
            wd.poll(0, true, 7, "t_cache", TrackSource::Vk, Some(10_000), check_cache),
            WatchdogAction::None
        );
        assert_eq!(
            wd.poll(3_999, true, 7, "t_cache", TrackSource::Vk, Some(10_000), check_cache),
            WatchdogAction::None
        );
        assert_eq!(
            fs_checks.load(Ordering::SeqCst),
            0,
            "до порога 4000 мс проверка диска выполняться не должна"
        );

        assert_eq!(
            wd.poll(4_000, true, 7, "t_cache", TrackSource::Vk, Some(10_000), check_cache),
            WatchdogAction::SwitchToCache {
                track_id: "t_cache".to_string(),
                resume_pos_ms: 10_000,
                stalled_ms: 4_000,
            }
        );
        assert_eq!(fs_checks.load(Ordering::SeqCst), 1);

        // Повторный тик в той же эпохе не должен повторно триггерить SwitchToCache
        assert_eq!(
            wd.poll(4_500, true, 7, "t_cache", TrackSource::Vk, Some(10_000), check_cache),
            WatchdogAction::None
        );
        assert_eq!(fs_checks.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_watchdog_retries_stream_once_at_12000ms_and_unlocks_after_1000ms_progress() {
        let mut wd = PlaybackWatchdog::new();

        // Старт и прогресс до 15_000 мс
        assert_eq!(
            wd.poll(1_000, true, 3, "yt_1", TrackSource::YouTubeMusic, Some(14_000), || false),
            WatchdogAction::None
        );
        assert_eq!(
            wd.poll(2_000, true, 3, "yt_1", TrackSource::YouTubeMusic, Some(15_000), || false),
            WatchdogAction::None
        );

        // Первый тик без прогресса фиксирует stalled_since_ms = 2_100
        assert_eq!(
            wd.poll(2_100, true, 3, "yt_1", TrackSource::YouTubeMusic, Some(15_000), || false),
            WatchdogAction::None
        );
        // В 14_099 (11_999 мс простоя) ещё рано
        assert_eq!(
            wd.poll(14_099, true, 3, "yt_1", TrackSource::YouTubeMusic, Some(15_000), || false),
            WatchdogAction::None
        );
        // Ровно 12_000 мс простоя (14_100 - 2_100) -> RetryStream
        assert_eq!(
            wd.poll(14_100, true, 3, "yt_1", TrackSource::YouTubeMusic, Some(15_000), || false),
            WatchdogAction::RetryStream {
                track_id: "yt_1".to_string(),
                track_source: TrackSource::YouTubeMusic,
                resume_pos_ms: 15_000,
                stalled_ms: 12_000,
            }
        );

        // Повторный retry без прогресса в той же эпохе заблокирован
        assert_eq!(
            wd.poll(20_000, true, 3, "yt_1", TrackSource::YouTubeMusic, Some(15_000), || false),
            WatchdogAction::None
        );

        // Прогресс +500 мс (<= 1000 мс от anchor 15_000) сбрасывает таймер, но ещё не снимает блокировку retry
        assert_eq!(
            wd.poll(21_000, true, 3, "yt_1", TrackSource::YouTubeMusic, Some(15_500), || false),
            WatchdogAction::None
        );
        assert_eq!(
            wd.poll(21_100, true, 3, "yt_1", TrackSource::YouTubeMusic, Some(15_500), || false),
            WatchdogAction::None
        );
        assert_eq!(
            wd.poll(34_000, true, 3, "yt_1", TrackSource::YouTubeMusic, Some(15_500), || false),
            WatchdogAction::None
        );

        // Прогресс +1050 мс (> 1000 мс от anchor 15_000) снимает блокировку retry
        assert_eq!(
            wd.poll(35_000, true, 3, "yt_1", TrackSource::YouTubeMusic, Some(16_050), || false),
            WatchdogAction::None
        );
        assert_eq!(
            wd.poll(35_100, true, 3, "yt_1", TrackSource::YouTubeMusic, Some(16_050), || false),
            WatchdogAction::None
        );
        assert_eq!(
            wd.poll(47_100, true, 3, "yt_1", TrackSource::YouTubeMusic, Some(16_050), || false),
            WatchdogAction::RetryStream {
                track_id: "yt_1".to_string(),
                track_source: TrackSource::YouTubeMusic,
                resume_pos_ms: 16_050,
                stalled_ms: 12_000,
            }
        );
    }

    #[test]
    fn test_watchdog_resets_on_epoch_change_and_handles_none_position() {
        let mut wd = PlaybackWatchdog::new();

        // Эпоха 1: набираем позицию 18_500, затем position_ms становится None (буферизация)
        assert_eq!(
            wd.poll(1_000, true, 1, "t1", TrackSource::Yandex, Some(18_500), || false),
            WatchdogAction::None
        );
        assert_eq!(
            wd.poll(13_000, true, 1, "t1", TrackSource::Yandex, None, || false),
            WatchdogAction::RetryStream {
                track_id: "t1".to_string(),
                track_source: TrackSource::Yandex,
                resume_pos_ms: 18_500,
                stalled_ms: 12_000,
            }
        );

        // Смена эпохи (новый трек) полностью сбрасывает таймер, позицию и блокировки
        assert_eq!(
            wd.poll(20_000, true, 2, "t2", TrackSource::Yandex, Some(0), || true),
            WatchdogAction::None
        );
        assert_eq!(
            wd.poll(24_000, true, 2, "t2", TrackSource::Yandex, Some(0), || true),
            WatchdogAction::SwitchToCache {
                track_id: "t2".to_string(),
                resume_pos_ms: 0,
                stalled_ms: 4_000,
            }
        );
    }

    #[test]
    fn test_watchdog_note_seek_backwards_prevents_false_stall() {
        let mut wd = PlaybackWatchdog::new();

        assert_eq!(
            wd.poll(1_000, true, 1, "t1", TrackSource::Yandex, Some(90_000), || false),
            WatchdogAction::None
        );

        // Пользователь перематывает назад на 10_000 мс
        wd.note_seek(10_000);
        assert_eq!(
            wd.poll(1_100, true, 1, "t1", TrackSource::Yandex, Some(10_000), || false),
            WatchdogAction::None
        );
        // Дальнейший прогресс (10_100 > 10_050) корректно сбрасывает таймер зависания
        assert_eq!(
            wd.poll(2_000, true, 1, "t1", TrackSource::Yandex, Some(10_100), || false),
            WatchdogAction::None
        );
        assert_eq!(
            wd.poll(13_500, true, 1, "t1", TrackSource::Yandex, Some(10_200), || false),
            WatchdogAction::None
        );
    }

    #[test]
    fn test_on_playback_error_resets_loading_and_emits_error() {
        let client = YandexMusicClient::new(None);
        let audio = Arc::new(AudioBackend::new().unwrap());
        let client_mutex = Arc::new(Mutex::new(client.clone()));
        let state_arc = Arc::new(Mutex::new(PlayerState::default()));
        {
            let mut st = state_arc.lock().unwrap();
            st.is_transitioning = true;
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
            watchdog: Arc::new(Mutex::new(PlaybackWatchdog::new())),
            artist_registry: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        };

        let rx = manager.subscribe();
        manager.on_playback_error("HTTP 403 Forbidden".to_string());

        assert!(
            !state_arc.lock().unwrap().is_transitioning,
            "is_transitioning должен сбрасываться при ошибке воспроизведения"
        );

        let mut events = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }

        assert!(
            events.iter().any(|e| matches!(e, PlayerUiEvent::LoadingState(false))),
            "ожидалось LoadingState(false), получено: {:?}",
            events
        );
        assert!(
            events.iter().any(|e| matches!(e, PlayerUiEvent::LoadingTrack(None))),
            "ожидалось LoadingTrack(None), получено: {:?}",
            events
        );
        assert!(
            events.iter().any(|e| matches!(e, PlayerUiEvent::BufferingProgress(None))),
            "ожидалось BufferingProgress(None), получено: {:?}",
            events
        );
        assert!(
            events.iter().any(|e| matches!(e, PlayerUiEvent::PlaybackToggled(false))),
            "ожидалось PlaybackToggled(false) при неактивном аудио, получено: {:?}",
            events
        );
        assert!(
            !events.iter().any(|e| matches!(e, PlayerUiEvent::PlaybackToggled(true))),
            "не должно быть ложного PlaybackToggled(true)"
        );
        assert!(
            events.iter().any(|e| matches!(e, PlayerUiEvent::Error(msg) if msg.contains("HTTP 403 Forbidden"))),
            "ожидалось PlayerUiEvent::Error с текстом ошибки, получено: {:?}",
            events
        );
    }

    #[test]
    fn test_buffering_progress_dedup_and_reset() {
        let senders: Arc<Mutex<Vec<Sender<PlayerUiEvent>>>> = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = async_channel::unbounded();
        senders.lock().unwrap().push(tx);
        let last = std::sync::atomic::AtomicU8::new(255);

        emit_buffering_if_changed(&last, &senders, 15);
        emit_buffering_if_changed(&last, &senders, 15); // дубль не шлется
        emit_buffering_if_changed(&last, &senders, 72);
        emit_buffering_if_changed(&last, &senders, 100);
        emit_buffering_if_changed(&last, &senders, 100); // повторный 100% не шлется
        emit_buffering_if_changed(&last, &senders, 72); // после 100% то же значение снова шлется

        let mut received = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let PlayerUiEvent::BufferingProgress(p) = ev {
                received.push(p);
            }
        }
        assert_eq!(received, vec![Some(15), Some(72), None, Some(72)]);
    }

    #[test]
    fn test_resolve_yt_lyrics_rejects_poisoned_and_prefers_verified_or_yt() {
        let yt_crosses = "\
This is a trick of the mind\n\
You have been watching me slide\n\
Into the place where you hide\n\
Tell me the things you decide";
        let bts_lrc = "\
[00:20.37] 매번 같은 하루들 중에 (들 중에)\n\
[00:24.52] 너를 만날 때 가장 난 행복해 (행복해)\n\
[00:29.89] 매번 다른 일상들 속에 (들 속에)\n\
[00:34.34] 너란 사람은 내게 가장 특별해 (특별해)";
        let crosses_lrc = "\
[00:12.10] This is a trick of the mind\n\
[00:16.40] You have been watching me slide\n\
[00:20.80] Into the place where you hide\n\
[00:25.10] Tell me the things you decide";

        // 1. Отравленный BTS LRC отбрасывается, `stale_plain` не перебивает официальный `yt_text`
        let res = resolve_yt_lyrics_outcome(
            Some(bts_lrc),
            None,
            Some(yt_crosses),
            Some("old stale cache text"),
            false,
            Some(185),
        );
        assert_eq!(
            res,
            Some(ResolvedYtLyrics {
                text: yt_crosses.to_string(),
                cache_text: true,
                is_yt_official: true,
            })
        );

        // При `had_stale_synced = true` возвращается `yt_text`, но без перезаписи кэша
        let res_stale = resolve_yt_lyrics_outcome(
            Some(bts_lrc),
            None,
            Some(yt_crosses),
            Some("old stale cache text"),
            true,
            Some(185),
        );
        assert_eq!(
            res_stale,
            Some(ResolvedYtLyrics {
                text: yt_crosses.to_string(),
                cache_text: false,
                is_yt_official: true,
            })
        );

        // 2. Если LRCLIB отброшен (или даже передан отравленным), а Яндекс дал валидный Crosses LRC — берётся он
        let res_yandex = resolve_yt_lyrics_outcome(
            Some(bts_lrc),
            Some(crosses_lrc),
            Some(yt_crosses),
            None,
            false,
            Some(185),
        );
        assert_eq!(
            res_yandex,
            Some(ResolvedYtLyrics {
                text: crosses_lrc.to_string(),
                cache_text: true,
                is_yt_official: false,
            })
        );

        // 3. Отравлены оба источника — оба отбрасываются в пользу официального `yt_text`
        let res_both_poisoned = resolve_yt_lyrics_outcome(
            Some(bts_lrc),
            Some(bts_lrc),
            Some(yt_crosses),
            None,
            false,
            Some(185),
        );
        assert_eq!(
            res_both_poisoned,
            Some(ResolvedYtLyrics {
                text: yt_crosses.to_string(),
                cache_text: true,
                is_yt_official: true,
            })
        );

        // 4. Подтверждённый Crosses LRC от LRCLIB имеет приоритет над статическим `yt_text`
        let res_lrclib = resolve_yt_lyrics_outcome(
            Some(crosses_lrc),
            None,
            Some(yt_crosses),
            None,
            true,
            Some(185),
        );
        assert_eq!(
            res_lrclib,
            Some(ResolvedYtLyrics {
                text: crosses_lrc.to_string(),
                cache_text: true,
                is_yt_official: false,
            })
        );
    }

    fn build_test_manager_with_state(state: PlayerState) -> PlayerManager {
        let client = YandexMusicClient::new(None);
        let audio = Arc::new(AudioBackend::new().unwrap());
        let client_mutex = Arc::new(Mutex::new(client.clone()));
        let state_arc = Arc::new(Mutex::new(state));
        let yandex_provider = Arc::new(client) as Arc<dyn MusicProvider>;
        let ytmusic_provider = Arc::new(YtMusicClient::new()) as Arc<dyn MusicProvider>;
        let provider_manager =
            ProviderManager::with_yandex_and_ytmusic(yandex_provider, ytmusic_provider);

        PlayerManager {
            audio,
            client: client_mutex,
            providers: Arc::new(provider_manager),
            yt_client: Arc::new(YtMusicClient::new()),
            clips: crate::player::clip::ClipBackground::new(
                Arc::new(YtMusicClient::new()),
                Arc::new(crate::player::clip_telemetry::ClipTelemetry::new()),
                Arc::new(Mutex::new(Vec::new())),
            ),
            state: state_arc,
            wave_engine: Arc::new(Mutex::new(WaveEngine::new())),
            event_senders: Arc::new(Mutex::new(Vec::new())),
            cast: Arc::default(),
            prefetch: Arc::default(),
            prefetch_inflight: Arc::default(),
            generation: Arc::new(AtomicU64::new(0)),
            tail_swap_epoch: Arc::new(AtomicU64::new(0)),
            crossfade: Arc::new(Mutex::new((false, 3))),
            watchdog: Arc::new(Mutex::new(PlaybackWatchdog::new())),
            artist_registry: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }

    #[test]
    fn test_on_playback_error_rolls_back_index_when_audio_still_playing() {
        let t0 = Track {
            id: "t0".to_string(),
            title: "Playing Track".to_string(),
            source: TrackSource::YouTubeMusic,
            ..Default::default()
        };
        let t1 = Track {
            id: "t1".to_string(),
            title: "Failed Next Track".to_string(),
            source: TrackSource::YouTubeMusic,
            ..Default::default()
        };
        let t2 = Track {
            id: "t2".to_string(),
            title: "Third Track".to_string(),
            source: TrackSource::YouTubeMusic,
            ..Default::default()
        };

        let mut state = PlayerState::default();
        state.wave = true;
        state.queue = vec![t0.clone(), t1.clone(), t2];
        state.current_index = 1;
        state.current_track = Some(t1);
        state.liked_track_ids.insert("t0".to_string());
        state.is_liked = false;
        state.is_transitioning = true;
        state.rollback_track = Some((0, t0));

        let manager = build_test_manager_with_state(state);
        // Имитируем живой активный слот (например, слот 0 продолжает играть во время failed crossfade)
        manager.audio.play_url("file:///tmp/active_slot_volna_test.mp3");
        assert!(manager.audio.is_playing());

        let rx = manager.subscribe();
        manager.on_playback_error("GstSoupHTTPSrc timeout".to_string());

        {
            let st = manager.state.lock().unwrap();
            assert_eq!(st.current_index, 0, "индекс должен откатиться к реально играющему треку");
            assert_eq!(
                st.current_track.as_ref().map(|t| t.id.as_str()),
                Some("t0"),
                "current_track должен восстановиться на t0"
            );
            assert!(st.is_liked, "is_liked должен пересчитаться для t0");
            assert!(st.rollback_track.is_none(), "rollback_track должен очиститься");
            assert!(!st.is_transitioning, "is_transitioning должен сброситься");
        }

        let mut events = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }

        assert!(
            events.iter().any(|e| matches!(e, PlayerUiEvent::LikeStateChanged(true))),
            "ожидалось LikeStateChanged(true), получено: {:?}",
            events
        );
        assert!(
            !events.iter().any(|e| matches!(e, PlayerUiEvent::TrackStarted { .. })),
            "TrackStarted не должен отправляться при откате, чтобы не сбрасывать активный клип/позицию"
        );
        assert!(
            !events.iter().any(|e| matches!(e, PlayerUiEvent::PlaybackToggled(false))),
            "PlaybackToggled(false) не должен отправляться, пока аудио продолжает играть"
        );
        assert!(
            events.iter().any(|e| matches!(e, PlayerUiEvent::Error(msg) if msg.contains("GstSoupHTTPSrc timeout"))),
            "ожидалось PlayerUiEvent::Error, получено: {:?}",
            events
        );

        std::mem::forget(manager);
    }

    #[test]
    fn test_save_rollback_preserves_initial_playing_track_on_rapid_double_skip() {
        let t0 = Track {
            id: "t0".to_string(),
            title: "Initial".to_string(),
            ..Default::default()
        };
        let t1 = Track {
            id: "t1".to_string(),
            title: "Skipped".to_string(),
            ..Default::default()
        };

        let mut state = PlayerState::default();
        state.queue = vec![t0.clone(), t1.clone()];
        state.current_index = 0;
        state.current_track = Some(t0);

        // При остановленном аудио откат не сохраняется
        state.save_rollback_if_needed(false);
        assert!(state.rollback_track.is_none());

        // Первый переход при живом аудио сохраняет (0, t0)
        state.save_rollback_if_needed(true);
        state.current_index = 1;
        state.current_track = Some(t1);
        state.is_transitioning = true;

        // Быстрый второй переход до старта t1 НЕ перезаписывает rollback_track на t1
        state.save_rollback_if_needed(true);
        let (idx, tr) = state.rollback_track.expect("rollback должен сохраниться");
        assert_eq!(idx, 0);
        assert_eq!(tr.id, "t0");
    }

    #[test]
    fn test_should_predownload_segmented_predicate() {
        assert!(should_predownload_segmented(
            TrackSource::YouTubeMusic,
            "https://rr2---sn-u15hn5-cc.googlevideo.com/videoplayback?itag=251&clen=4000000"
        ));
        assert!(should_predownload_segmented(
            TrackSource::YouTubeMusic,
            "https://rr2---sn-u15hn5-cc.googlevideo.com/videoplayback?itag=251"
        ));
        assert!(!should_predownload_segmented(
            TrackSource::YouTubeMusic,
            "https://rr2---sn-u15hn5-cc.googlevideo.com/videoplayback?itag=251&clen=20000000"
        ));
        assert!(!should_predownload_segmented(
            TrackSource::YouTubeMusic,
            "file:///tmp/cached.opus"
        ));
        assert!(!should_predownload_segmented(
            TrackSource::Yandex,
            "https://storage.yandex.net/track.mp3"
        ));
        assert!(should_predownload_segmented(
            TrackSource::Yandex,
            "https://rr1---sn-u15hn5-cc.googlevideo.com/videoplayback?clen=3000000"
        ));
    }

    #[tokio::test]
    async fn test_download_audio_range_segmented_cancels_between_segments() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local test server");
        let addr = listener.local_addr().unwrap();

        let total_size: usize = 2 * 1024 * 1024 + 512 * 1024;
        let payload: Vec<u8> = vec![0x5Au8; total_size];
        let request_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let request_count_srv = request_count.clone();
        let cancel_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancel_flag_srv = cancel_flag.clone();

        let server_task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                let Ok(n) = socket.read(&mut buf).await else {
                    break;
                };
                if n == 0 {
                    break;
                }
                request_count_srv.fetch_add(1, Ordering::SeqCst);
                let req_str = String::from_utf8_lossy(&buf[..n]);
                let range_line = req_str
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("range:"))
                    .unwrap_or("");
                let (start, end) = range_line
                    .split_once("bytes=")
                    .and_then(|(_, r)| r.split_once('-'))
                    .map(|(s, e)| {
                        (
                            s.trim().parse::<usize>().unwrap_or(0),
                            e.trim().parse::<usize>().unwrap_or(total_size - 1),
                        )
                    })
                    .unwrap_or((0, total_size - 1));
                let actual_end = end.min(total_size - 1);
                let slice = &payload[start..=actual_end];
                let header = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Type: audio/webm\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                    slice.len(),
                    start,
                    actual_end,
                    total_size
                );
                let _ = socket.write_all(header.as_bytes()).await;
                let _ = socket.write_all(slice).await;
                let _ = socket.shutdown().await;
                // После первого сегмента имитируем переключение трека (stale epoch)
                cancel_flag_srv.store(true, Ordering::SeqCst);
            }
        });

        let tmp_path = std::env::temp_dir().join(format!(
            "volna_range_cancel_test_{}.opus.part",
            std::process::id()
        ));
        let _ = std::fs::write(&tmp_path, b"stale-partial");
        let url = format!("http://{addr}/videoplayback?itag=251");
        let res = download_audio_range_segmented_cancellable(&url, &tmp_path, &|| {
            cancel_flag.load(Ordering::SeqCst)
        })
        .await;

        assert!(res.is_none(), "отменённая загрузка должна вернуть None");
        assert!(
            !tmp_path.exists(),
            "временный файл должен удаляться при отмене"
        );
        assert_eq!(
            request_count.load(Ordering::SeqCst),
            1,
            "второй сегмент не должен запрашиваться после отмены"
        );
        server_task.abort();
    }
}
