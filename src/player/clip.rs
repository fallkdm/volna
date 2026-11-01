//! Фоновый клип Волны: резолв источника, загрузка в кэш, negative cache.
//!
//! Этап 1: всё, что не требует UI — решение «клип или арт-трек», обход GGC-нод,
//! потоковая загрузка в `.part` с бюджетами, LRU и телеметрия для `volna cmd clip`.
//! Показ клипа (`ClipController`) подключается отдельным этапом; здесь же живёт
//! единая точка правды о том, что с клипом трека.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::clip_telemetry::{ClipSource, ClipTelemetry};
use super::manager::{ClipKind, ClipRequest, ClipUpdate, PlayerUiEvent};
use crate::api::models::{Track, TrackSource};
use crate::api::ytmusic::{VideoClipError, VideoClipResolution, YtMusicClient};
use crate::cache::{self, ClipDownloadError, YoutubeClipDownload};

/// Временные неудачи (сеть, таймаут, 429/5xx) подавляют повторные попытки 5 минут.
pub const TRANSIENT_TTL: Duration = Duration::from_secs(5 * 60);

/// Запомненный отрицательный результат по треку.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipNegative {
    ArtTrack,
    NoH264,
    TooLarge,
    HttpStatus(u16),
    PlaybackFailed,
    DownloadTimeout,
    DownloadStalled,
    InvalidContentType,
    ResolveFailed,
    Io,
}

impl ClipNegative {
    /// Строка для `clip_error`.
    pub fn code(&self) -> String {
        match self {
            Self::ArtTrack => "art-track".into(),
            Self::NoH264 => "no-h264".into(),
            Self::TooLarge => "too-large".into(),
            Self::HttpStatus(code) => format!("http-{code}"),
            Self::PlaybackFailed => "playback-failed".into(),
            Self::DownloadTimeout => "download-timeout".into(),
            Self::DownloadStalled => "download-stalled".into(),
            Self::InvalidContentType => "invalid-content-type".into(),
            Self::ResolveFailed => "resolve-failed".into(),
            Self::Io => "io".into(),
        }
    }

    /// Терминальные результаты живут до конца процесса, временные — [`TRANSIENT_TTL`].
    pub fn is_terminal(&self) -> bool {
        match self {
            Self::ArtTrack
            | Self::NoH264
            | Self::TooLarge
            | Self::PlaybackFailed => true,
            // 403 и прочие «постоянные» 4xx; 429 — временный
            Self::HttpStatus(code) => *code != 429 && *code < 500,
            Self::DownloadTimeout
            | Self::DownloadStalled
            | Self::InvalidContentType
            | Self::ResolveFailed
            | Self::Io => false,
        }
    }

    fn from_resolve(error: &VideoClipError) -> Self {
        match error {
            VideoClipError::NoH264 => Self::NoH264,
            // playability — временный диагноз: бот-проверка/сессия проходят сами
            VideoClipError::Player(_) => Self::ResolveFailed,
            VideoClipError::Resolve(_) => Self::ResolveFailed,
        }
    }

    fn from_download(error: &ClipDownloadError) -> Self {
        match error {
            ClipDownloadError::DownloadTimeout => Self::DownloadTimeout,
            ClipDownloadError::DownloadStalled => Self::DownloadStalled,
            ClipDownloadError::HttpStatus(code) => Self::HttpStatus(*code),
            ClipDownloadError::InvalidContentType => Self::InvalidContentType,
            ClipDownloadError::TooLarge => Self::TooLarge,
            ClipDownloadError::Io(_) => Self::Io,
            // отмена не является неудачей трека и сюда не попадает
            ClipDownloadError::Cancelled => Self::Io,
        }
    }
}

struct NegativeEntry {
    result: ClipNegative,
    recorded_at: tokio::time::Instant,
}

/// Общая для всех задач часть клипа: телеметрия, negative cache, номер попытки.
#[derive(Clone)]
pub struct ClipBackground {
    telemetry: Arc<ClipTelemetry>,
    yt: Arc<YtMusicClient>,
    negative: Arc<Mutex<HashMap<String, NegativeEntry>>>,
    attempt: Arc<AtomicU64>,
    /// Канал событий в UI: показ/снятие клипа
    events: Arc<Mutex<Vec<async_channel::Sender<PlayerUiEvent>>>>,
    /// Поколение запросов: защищает от повторного запуска той же задачи на тот же трек
    generation: Arc<AtomicU64>,
    /// Треки, для которых задача уже запускалась в текущем поколении:
    /// один и тот же трек не должен стартовать клип дважды.
    handled: Arc<Mutex<HashMap<String, u64>>>,
}

impl ClipBackground {
    pub fn new(
        yt: Arc<YtMusicClient>,
        telemetry: Arc<ClipTelemetry>,
        events: Arc<Mutex<Vec<async_channel::Sender<PlayerUiEvent>>>>,
    ) -> Self {
        Self {
            telemetry,
            yt,
            events,
            negative: Arc::default(),
            attempt: Arc::new(AtomicU64::new(0)),
            generation: Arc::new(AtomicU64::new(0)),
            handled: Arc::default(),
        }
    }

    pub fn telemetry(&self) -> &Arc<ClipTelemetry> {
        &self.telemetry
    }

    /// Текущий (последний выданный) номер попытки.
    pub fn current_attempt(&self) -> u64 {
        self.attempt.load(Ordering::SeqCst)
    }

    /// Инвалидирует текущую попытку: незавершённая загрузка увидит это между чанками.
    pub fn invalidate_attempt(&self) {
        self.attempt.fetch_add(1, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut handled) = self.handled.lock() {
            handled.clear();
        }
    }

    /// Включает/выключает клипы: выключение отменяет текущую загрузку.
    pub fn set_enabled(&self, enabled: bool) {
        self.telemetry.set_enabled(enabled);
        if !enabled {
            self.invalidate_attempt();
            self.telemetry.no_clip();
        }
    }

    /// Действующий negative-результат; истёкшую временную запись удаляет.
    pub fn negative_result(&self, track_id: &str) -> Option<ClipNegative> {
        let mut map = self.negative.lock().ok()?;
        let Some(entry) = map.get(track_id) else {
            return None;
        };
        let expired = !entry.result.is_terminal() && entry.recorded_at.elapsed() > TRANSIENT_TTL;
        if expired {
            map.remove(track_id);
            return None;
        }
        Some(entry.result.clone())
    }

    fn record_negative(&self, track_id: &str, result: ClipNegative) {
        if let Ok(mut map) = self.negative.lock() {
            map.insert(
                track_id.to_string(),
                NegativeEntry {
                    result,
                    recorded_at: tokio::time::Instant::now(),
                },
            );
        }
    }

    /// Сброс negative cache (IPC `clip reset`). Положительный MP4-кэш не трогаем.
    pub fn reset_negative(&self) -> usize {
        self.invalidate_attempt();
        match self.negative.lock() {
            Ok(mut map) => {
                let removed = map.len();
                map.clear();
                removed
            }
            Err(_) => 0,
        }
    }

    /// Решает судьбу клипа трека в фоне. Сеть не блокирует звук и UI.
    fn emit(&self, event: PlayerUiEvent) {
        let Ok(mut senders) = self.events.lock() else {
            return;
        };
        senders.retain(|sender| sender.send_blocking(event.clone()).is_ok());
    }

    fn show(&self, track_id: &str, epoch: u64, attempt_id: u64, kind: ClipKind, path: &std::path::Path) {
        self.emit(PlayerUiEvent::ClipUpdated(ClipUpdate::Show(ClipRequest {
            track_id: track_id.to_string(),
            epoch,
            attempt_id,
            kind,
            path: path.to_string_lossy().to_string(),
        })));
    }

    fn clear(&self, track_id: &str, epoch: u64) {
        self.emit(PlayerUiEvent::ClipUpdated(ClipUpdate::Clear {
            track_id: track_id.to_string(),
            epoch,
        }));
    }

    pub fn spawn_clip_for_track(&self, track: &Track, epoch: u64) {
        // Один трек — одна задача: трек стартует из нескольких мест, и второй
        // заход затирал бы метрики реальной загрузки кэш-хитом.
        let generation = self.generation.load(Ordering::SeqCst);
        {
            let Ok(mut handled) = self.handled.lock() else {
                return;
            };
            if handled.get(&track.id) == Some(&generation) {
                return;
            }
            // новый трек: записи о прежних больше не нужны
            if !handled.is_empty() && !handled.contains_key(&track.id) {
                handled.clear();
            }
            handled.insert(track.id.clone(), generation);
        }
        let this = self.clone();
        let track = track.clone();
        tokio::spawn(async move {
            this.run(track, epoch).await;
        });
    }

    async fn run(&self, track: Track, epoch: u64) {
        let track_id = track.id.clone();
        let attempt_id = self.attempt.fetch_add(1, Ordering::SeqCst) + 1;

        // Состояние включённости берём из телеметрии, а не с диска: настройку
        // меняет менеджер, и лишнее чтение конфига в фоне не нужно.
        if !self.telemetry.snapshot().enabled {
            self.telemetry.begin_track(&track_id, attempt_id);
            self.telemetry.no_clip();
            self.clear(&track_id, epoch);
            return;
        }

        self.telemetry.begin_track(&track_id, attempt_id);

        // Свои клипы альбома важнее любого сервисного
        if let Some(path) = crate::cache::custom_clip_for(&track) {
            self.telemetry.set_source(ClipSource::Custom);
            self.show(&track_id, epoch, attempt_id, ClipKind::Custom, &path);
            return;
        }

        if track.source != TrackSource::YouTubeMusic {
            // Превью Яндекса и свои клипы приходят своим путём из spawn_cover_and_lyrics
            self.telemetry.no_clip();
            return;
        }

        if let Some(path) = crate::cache::cached_youtube_clip(&track_id) {
            crate::cache::touch_youtube_clip(&track_id);
            self.telemetry.set_source(ClipSource::Mv);
            self.telemetry.set_download(Some(0), Some(0));
            self.show(&track_id, epoch, attempt_id, ClipKind::YouTubeFull, &path);
            return;
        }

        if let Some(result) = self.negative_result(&track_id) {
            self.apply_negative(&track_id, epoch, result);
            return;
        }

        let resolved = match self.yt.resolve_video_clip(&track_id).await {
            Ok(resolved) => resolved,
            Err(error) => {
                let negative = ClipNegative::from_resolve(&error);
                self.record_negative(&track_id, negative.clone());
                self.apply_negative(&track_id, epoch, negative);
                return;
            }
        };

        let (url, duration_secs, format) = match resolved {
            VideoClipResolution::ArtTrack => {
                self.record_negative(&track_id, ClipNegative::ArtTrack);
                self.apply_negative(&track_id, epoch, ClipNegative::ArtTrack);
                return;
            }
            VideoClipResolution::MusicVideo {
                url,
                duration_secs,
                format,
            } => (url, duration_secs, format),
        };

        self.telemetry.set_source(ClipSource::Mv);
        self.telemetry.set_format(format.as_ref().map(|f| f.label()));

        let progress = Arc::new(AtomicU64::new(0));
        let started = tokio::time::Instant::now();
        let cancel = {
            let attempts = self.attempt.clone();
            move || attempts.load(Ordering::SeqCst) != attempt_id
        };
        let result = cache::download_youtube_clip(
            &url,
            &track_id,
            attempt_id,
            duration_secs,
            &cancel,
            Some(progress.as_ref()),
        )
        .await;
        let elapsed_ms = started.elapsed().as_millis() as u64;
        let written = progress.load(Ordering::Relaxed);

        match result {
            Ok(YoutubeClipDownload::Completed(path)) => {
                let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(written);
                self.telemetry.set_download(Some(elapsed_ms), Some(bytes));
                self.telemetry.set_source(ClipSource::Mv);
                self.telemetry.set_error(None);
                crate::cache::clean_youtube_clips_lru(Some(&track_id));
                self.show(&track_id, epoch, attempt_id, ClipKind::YouTubeFull, &path);
                println!(
                    "[clip] {track_id}: клип готов за {elapsed_ms} мс, {} байт",
                    bytes
                );
            }
            Ok(YoutubeClipDownload::ArtTrack) => {
                self.telemetry.set_download(Some(elapsed_ms), Some(written));
                self.record_negative(&track_id, ClipNegative::ArtTrack);
                self.apply_negative(&track_id, epoch, ClipNegative::ArtTrack);
            }
            Err(ClipDownloadError::Cancelled) => {
                // устаревшая или выключенная попытка: не ошибка трека
            }
            Err(error) => {
                let negative = ClipNegative::from_download(&error);
                self.telemetry.set_download(Some(elapsed_ms), Some(written));
                self.record_negative(&track_id, negative.clone());
                self.apply_negative(&track_id, epoch, negative);
                if let ClipDownloadError::Io(details) = &error {
                    println!("[clip] {track_id}: {details}");
                }
            }
        }
    }

    fn apply_negative(&self, track_id: &str, epoch: u64, negative: ClipNegative) {
        self.clear(track_id, epoch);
        if negative == ClipNegative::ArtTrack {
            self.telemetry.art_track();
            self.telemetry.set_format(None);
            return;
        }
        self.telemetry.set_source(ClipSource::None);
        self.telemetry.set_error(Some(&negative.code()));
        self.telemetry.set_position(None);
        self.telemetry.set_local_active(false);
        println!("[clip] {track_id}: {}", negative.code());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_and_transient_negative_results() {
        assert!(ClipNegative::ArtTrack.is_terminal());
        assert!(ClipNegative::NoH264.is_terminal());
        assert!(ClipNegative::TooLarge.is_terminal());
        assert!(ClipNegative::HttpStatus(403).is_terminal());
        assert!(ClipNegative::PlaybackFailed.is_terminal());

        assert!(!ClipNegative::DownloadTimeout.is_terminal());
        assert!(!ClipNegative::DownloadStalled.is_terminal());
        assert!(!ClipNegative::InvalidContentType.is_terminal());
        assert!(!ClipNegative::HttpStatus(429).is_terminal());
        assert!(!ClipNegative::HttpStatus(503).is_terminal());
        assert!(!ClipNegative::ResolveFailed.is_terminal());
        assert!(!ClipNegative::Io.is_terminal());
    }

    #[test]
    fn error_codes_are_distinct_for_diagnostics() {
        assert_eq!(ClipNegative::DownloadTimeout.code(), "download-timeout");
        assert_eq!(ClipNegative::DownloadStalled.code(), "download-stalled");
        assert_eq!(ClipNegative::NoH264.code(), "no-h264");
        assert_eq!(ClipNegative::HttpStatus(403).code(), "http-403");
        assert_eq!(ClipNegative::PlaybackFailed.code(), "playback-failed");
    }

    #[test]
    fn negative_cache_suppresses_then_expires() {
        let yt = Arc::new(YtMusicClient::new());
        let telemetry = Arc::new(ClipTelemetry::new());
        let clips = ClipBackground::new(yt, telemetry, Arc::new(Mutex::new(Vec::new())));

        clips.record_negative("a", ClipNegative::DownloadTimeout);
        assert_eq!(clips.negative_result("a"), Some(ClipNegative::DownloadTimeout));
        assert_eq!(clips.reset_negative(), 1);
        assert_eq!(clips.negative_result("a"), None);

        clips.record_negative("b", ClipNegative::NoH264);
        assert_eq!(clips.negative_result("b"), Some(ClipNegative::NoH264));
        clips.reset_negative();
        assert_eq!(clips.negative_result("b"), None);
    }
}
