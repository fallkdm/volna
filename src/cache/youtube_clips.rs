//! Кэш полноценных YouTube-клипов: download-first без `tee`.
//!
//! Один потоковый запрос пишет `.part`, после успешного конца тела и проверок файл
//! атомарно переименовывается в `.mp4` и становится и кэшем, и источником показа.
//! Требования и обоснование — `волна-видео-и-карточка-аккаунта-ревизия-7-финальная.md` §5.

use std::fs::File;
#[cfg(test)]
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::AsyncWriteExt;

/// Один файл больше этого не качаем: 480p настоящего клипа — 8–30 МБ.
pub const YOUTUBE_CLIP_MAX_BYTES: u64 = 200 * 1024 * 1024;
/// Общий лимит каталога YouTube-клипов.
pub const YOUTUBE_CACHE_LIMIT_BYTES: u64 = 512 * 1024 * 1024;
/// До какого размера чистим при превышении лимита.
pub const YOUTUBE_CACHE_TARGET_BYTES: u64 = 384 * 1024 * 1024;
/// Ни одного нового байта дольше этого — `DownloadStalled`.
pub const DOWNLOAD_STALL_TIMEOUT: Duration = Duration::from_secs(10);
/// Общий бюджет одной загрузки.
pub const DOWNLOAD_TOTAL_BUDGET: Duration = Duration::from_secs(30);
/// `.part` старше этого при старте приложения считается осиротевшим.
pub const STALE_PART_AGE: Duration = Duration::from_secs(10 * 60);
/// Сегмент Range: CDN отдаёт длинный прогрессивный поток рывками, а короткие
/// диапазоны — стабильно быстро (замер: 3 МБ ≈ 1.2 с против 2 МБ за 30 с).
const SEGMENT_BYTES: u64 = 4 * 1024 * 1024;
/// Бюджет одного сегмента: недописанный сегмент повторяем с той же позиции.
const SEGMENT_BUDGET: Duration = Duration::from_secs(15);
const SEGMENT_ATTEMPTS: u32 = 3;

/// Порог «статичный ролик»: байт на секунду. Совпадает с metadata-порогом 80 кбит/с.
pub fn art_track_min_bytes_per_sec() -> u64 {
    crate::api::ytmusic::ART_TRACK_BITRATE_BPS / 8
}

/// Результат загрузки: готовый файл или отсеянный арт-трек.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum YoutubeClipDownload {
    Completed(PathBuf),
    ArtTrack,
}

/// Типизированные причины неудачи. `Cancelled` — не ошибка трека.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipDownloadError {
    DownloadTimeout,
    DownloadStalled,
    HttpStatus(u16),
    InvalidContentType,
    TooLarge,
    Io(String),
    Cancelled,
}

impl ClipDownloadError {
    /// Строка для `clip_error` в телеметрии.
    pub fn code(&self) -> String {
        match self {
            Self::DownloadTimeout => "download-timeout".into(),
            Self::DownloadStalled => "download-stalled".into(),
            Self::HttpStatus(code) => format!("http-{code}"),
            Self::InvalidContentType => "invalid-content-type".into(),
            Self::TooLarge => "too-large".into(),
            Self::Io(_) => "io".into(),
            Self::Cancelled => "cancelled".into(),
        }
    }
}

/// Каталог YouTube-клипов внутри общего кэша клипов.
pub fn youtube_clips_dir() -> PathBuf {
    let dir = crate::cache::clips_cache_dir().join("youtube");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// `safe_id` сохраняет дефис и подчёркивание: `A-B` и `A_B` — разные файлы.
fn safe_id(track_id: &str) -> String {
    let id: String = track_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if id.is_empty() { "unknown".to_string() } else { id }
}

/// Готовый файл трека (только финализированный `.mp4`).
pub fn youtube_clip_cache_path(track_id: &str) -> PathBuf {
    youtube_clips_dir().join(format!("{}.mp4", safe_id(track_id)))
}

/// Уникальный `.part` конкретной попытки: две быстрые попытки не пишут один файл.
pub fn youtube_clip_part_path(track_id: &str, attempt_id: u64) -> PathBuf {
    youtube_clips_dir().join(format!("{}.{}.part", safe_id(track_id), attempt_id))
}

/// Ранее финализированный клип, если он есть.
pub fn cached_youtube_clip(track_id: &str) -> Option<PathBuf> {
    let path = youtube_clip_cache_path(track_id);
    path.is_file().then_some(path)
}

/// Суммарный размер готовых YouTube-клипов.
pub fn youtube_clips_size() -> u64 {
    youtube_clips_size_in(&youtube_clips_dir())
}

fn youtube_clips_size_in(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "mp4"))
                .filter_map(|e| e.metadata().ok())
                .filter(|m| m.is_file())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

/// Удаляет осиротевшие `.part` старше [`STALE_PART_AGE`].
///
/// Свежие `.part` не трогаем: второй экземпляр приложения может качать их прямо сейчас.
/// Ошибка чтения метаданных означает пропуск, а не удаление.
pub fn cleanup_stale_youtube_parts() -> usize {
    cleanup_stale_parts_in(&youtube_clips_dir(), STALE_PART_AGE)
}

fn cleanup_stale_parts_in(dir: &Path, max_age: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "part") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let stale = meta
            .modified()
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > max_age);
        if stale && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// LRU-очистка: удаляет самые давно неиспользуемые `.mp4`, пока размер не станет ≤ цели.
///
/// `protect_track` — трек, чей файл сейчас играет; его не трогаем.
pub fn clean_youtube_clips_lru(protect_track: Option<&str>) -> u64 {
    clean_youtube_clips_lru_in(&youtube_clips_dir(), protect_track)
}

fn clean_youtube_clips_lru_in(dir: &Path, protect_track: Option<&str>) -> u64 {
    let protected = protect_track.map(|id| dir.join(format!("{}.mp4", safe_id(id))));
    let protected = protected.as_deref();
    let mut files: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
    let mut protected_size: u64 = 0;
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "mp4") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        if protected.is_some_and(|p| p == path) {
            // текущий клип занимает место в лимите, но не может быть удалён
            protected_size = protected_size.saturating_add(meta.len());
            continue;
        }
        let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
        files.push((path, meta.len(), modified));
    }

    let mut total: u64 = files.iter().map(|(_, size, _)| *size).sum::<u64>() + protected_size;
    if total <= YOUTUBE_CACHE_LIMIT_BYTES {
        return 0;
    }
    files.sort_by_key(|(_, _, modified)| *modified);
    let mut freed = 0;
    for (path, size, _) in files {
        if total <= YOUTUBE_CACHE_TARGET_BYTES {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(size);
            freed += size;
        }
    }
    freed
}

/// Обновляет отметку последнего использования готового клипа (LRU читает `modified`).
pub fn touch_youtube_clip(track_id: &str) {
    let path = youtube_clip_cache_path(track_id);
    let Ok(file) = File::options().write(true).open(&path) else {
        return;
    };
    let now = std::time::SystemTime::now();
    let _ = file.set_times(std::fs::FileTimes::new().set_accessed(now).set_modified(now));
}

/// Статичен ли файл по среднему битрейту — без декодирования и `is_static()`.
///
/// Неизвестная или нулевая длительность не даёт вердикта: неизвестность ≠ арт-трек.
pub fn looks_like_art_track(file_size: u64, duration_secs: Option<u64>) -> bool {
    duration_secs
        .filter(|secs| *secs > 0)
        .is_some_and(|secs| file_size / secs < art_track_min_bytes_per_sec())
}

/// RAII-страж `.part`: удаляет недокачанный файл даже если future отменили посередине.
///
/// Хранит только путь `.part` — финальный `.mp4` ему не передаётся никогда.
struct PartFileGuard {
    path: PathBuf,
    armed: bool,
}

impl PartFileGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PartFileGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Скачивает video-only MP4 в `.part` и атомарно финализирует его.
///
/// `cancel` вызывается между чанками: устаревшая попытка прекращается с `Cancelled`,
/// её `.part` удаляет страж.
pub async fn download_youtube_clip(
    url: &str,
    track_id: &str,
    attempt_id: u64,
    duration_secs: Option<u64>,
    cancel: &(dyn Fn() -> bool + Send + Sync),
    progress: Option<&std::sync::atomic::AtomicU64>,
) -> Result<YoutubeClipDownload, ClipDownloadError> {
    let final_path = youtube_clip_cache_path(track_id);
    if final_path.is_file() {
        return Ok(YoutubeClipDownload::Completed(final_path));
    }
    download_youtube_clip_to(&youtube_clips_dir(), url, track_id, attempt_id, duration_secs, cancel, progress).await
}

async fn download_youtube_clip_to(
    dir: &Path,
    url: &str,
    track_id: &str,
    attempt_id: u64,
    duration_secs: Option<u64>,
    cancel: &(dyn Fn() -> bool + Send + Sync),
    progress: Option<&std::sync::atomic::AtomicU64>,
) -> Result<YoutubeClipDownload, ClipDownloadError> {
    let final_path = dir.join(format!("{}.mp4", safe_id(track_id)));
    let part_path = dir.join(format!("{}.{}.part", safe_id(track_id), attempt_id));
    let mut guard = PartFileGuard::new(part_path.clone());

    if cancel() {
        return Err(ClipDownloadError::Cancelled);
    }

    // Полуживые keep-alive сокеты к googlevideo уже ломали аудио — большой GET
    // идёт своим клиентом без переиспользования соединений.
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .build()
        .map_err(|e| ClipDownloadError::Io(e.to_string()))?;

    let deadline = tokio::time::Instant::now() + DOWNLOAD_TOTAL_BUDGET;
    let body = async {
        let mut file = tokio::fs::File::create(&part_path)
            .await
            .map_err(|e| ClipDownloadError::Io(e.to_string()))?;

        let mut written: u64 = 0;
        let mut total: Option<u64> = None;
        let mut first_response = true;

        loop {
            if cancel() {
                return Err(ClipDownloadError::Cancelled);
            }
            if total.is_some_and(|total| written >= total) {
                break;
            }

            let segment_start = written;
            let mut attempt = 0;
            loop {
                attempt += 1;
                let result = fetch_range_segment(
                    &client,
                    url,
                    segment_start,
                    SEGMENT_BYTES,
                    &mut file,
                    &mut written,
                    &mut total,
                    first_response,
                    cancel,
                    progress,
                    segment_start,
                )
                .await;
                first_response = false;
                match result {
                    Ok(()) => break,
                    // Медленный, но живой сегмент: откатываем хвост и берём новый Range
                    Err(ClipDownloadError::DownloadStalled) if attempt < SEGMENT_ATTEMPTS => {
                        written = segment_start;
                        let _ = file.set_len(segment_start).await;
                        if let Some(progress) = progress {
                            progress.store(segment_start, std::sync::atomic::Ordering::Relaxed);
                        }
                        continue;
                    }
                    Err(error) => return Err(error),
                }
            }

            // Сервер без поддержки Range отдал весь файл одним ответом
            if total.is_none() {
                break;
            }
        }

        file.flush()
            .await
            .map_err(|e| ClipDownloadError::Io(e.to_string()))?;
        drop(file);

        if let Some(total) = total
            && written != total
        {
            return Err(ClipDownloadError::Io(format!(
                "неполное тело: {written} из {total} байт"
            )));
        }

        // Послезагрузочный отсев статичных роликов: тот же порог, что metadata-битрейт
        if looks_like_art_track(written, duration_secs) {
            let _ = std::fs::remove_file(&part_path);
            return Ok(YoutubeClipDownload::ArtTrack);
        }

        std::fs::rename(&part_path, &final_path)
            .map_err(|e| ClipDownloadError::Io(e.to_string()))?;
        guard.disarm();
        Ok(YoutubeClipDownload::Completed(final_path))
    };

    match tokio::time::timeout_at(deadline, body).await {
        Ok(result) => result,
        Err(_) => Err(ClipDownloadError::DownloadTimeout),
    }
}

/// Один Range-сегмент: пишет байты в файл и обновляет `written`/`total`.
#[allow(clippy::too_many_arguments)]
async fn fetch_range_segment(
    client: &reqwest::Client,
    url: &str,
    start: u64,
    segment_bytes: u64,
    file: &mut tokio::fs::File,
    written: &mut u64,
    total: &mut Option<u64>,
    check_content_type: bool,
    cancel: &(dyn Fn() -> bool + Send + Sync),
    progress: Option<&std::sync::atomic::AtomicU64>,
    rollback_to: u64,
) -> Result<(), ClipDownloadError> {
    let end = start.saturating_add(segment_bytes).saturating_sub(1);
    let request = client
        .get(url)
        .header("User-Agent", crate::api::ytmusic::UA_SAFARI_26)
        .header("Referer", "https://www.youtube.com/")
        .header("Origin", "https://www.youtube.com");
    // Первый сегмент просим без Range: CDN всё равно отвечает 200/206 одинаково,
    // а явный Range нужен только для продолжений.
    // Range нужен и на первом сегменте: без него CDN отдаёт длинный прогрессивный
    // поток, который троттлится (2 МБ за 30 с вместо 2.6 МБ/с).
    let request = request.header(reqwest::header::RANGE, format!("bytes={start}-{end}"));

    let response = tokio::time::timeout(SEGMENT_BUDGET, request.send())
        .await
        .map_err(|_| ClipDownloadError::DownloadStalled)?
        .map_err(|e| {
            if e.is_timeout() {
                ClipDownloadError::DownloadStalled
            } else {
                ClipDownloadError::Io(e.to_string())
            }
        })?;

    let status = response.status();
    if !status.is_success() {
        return Err(ClipDownloadError::HttpStatus(status.as_u16()));
    }
    if start > 0 && status == reqwest::StatusCode::OK {
        // Сервер проигнорировал Range и отдаёт файл с нуля — дописывать нельзя
        return Err(ClipDownloadError::Io(
            "сервер не поддержал Range для продолжения".to_string(),
        ));
    }

    if check_content_type {
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let main_type = content_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if !main_type.starts_with("video/") {
            return Err(ClipDownloadError::InvalidContentType);
        }
    }

    if total.is_none() {
        *total = parse_total_size(&response);
        if total.is_some_and(|total| total > YOUTUBE_CLIP_MAX_BYTES) {
            return Err(ClipDownloadError::TooLarge);
        }
    }

    let mut response = response;
    loop {
        if cancel() {
            return Err(ClipDownloadError::Cancelled);
        }
        let chunk = match tokio::time::timeout(DOWNLOAD_STALL_TIMEOUT, response.chunk()).await {
            Err(_) => return Err(ClipDownloadError::DownloadStalled),
            Ok(Err(e)) => {
                return Err(if e.is_timeout() {
                    ClipDownloadError::DownloadStalled
                } else {
                    ClipDownloadError::Io(e.to_string())
                });
            }
            Ok(Ok(None)) => break,
            Ok(Ok(Some(chunk))) => chunk,
        };
        if chunk.is_empty() {
            continue;
        }
        *written = written.saturating_add(chunk.len() as u64);
        if *written > YOUTUBE_CLIP_MAX_BYTES {
            return Err(ClipDownloadError::TooLarge);
        }
        file.write_all(&chunk)
            .await
            .map_err(|e| ClipDownloadError::Io(e.to_string()))?;
        if let Some(progress) = progress {
            progress.store(*written, std::sync::atomic::Ordering::Relaxed);
        }
    }

    if *written == rollback_to {
        return Err(ClipDownloadError::Io(
            "сегмент не отдал ни одного байта".to_string(),
        ));
    }
    Ok(())
}

/// Полный размер файла из `Content-Range: bytes 0-4194303/15299542` или `Content-Length`.
fn parse_total_size(response: &reqwest::Response) -> Option<u64> {
    if let Some(content_range) = response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        && let Some(total) = content_range.rsplit('/').next()
        && let Ok(total) = total.trim().parse::<u64>()
    {
        return Some(total);
    }
    response.content_length()
}

/// Синхронная запись .part (для тестов и простых случаев без сети).
#[cfg(test)]
fn write_part(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut file = File::create(path).expect("part");
    file.write_all(bytes).expect("write");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "volna-clip-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn dash_and_underscore_ids_do_not_collide() {
        assert_ne!(safe_id("A-B"), safe_id("A_B"));
    }

    #[test]
    fn part_path_is_never_the_final_path() {
        let final_path = youtube_clip_cache_path("dQw4w9WgXcQ");
        let part = youtube_clip_part_path("dQw4w9WgXcQ", 7);
        assert_ne!(final_path, part);
        assert_eq!(final_path.extension().and_then(|e| e.to_str()), Some("mp4"));
        assert_eq!(part.extension().and_then(|e| e.to_str()), Some("part"));
        assert_eq!(youtube_clip_part_path("A-B", 1), youtube_clip_part_path("A-B", 1));
        assert_ne!(youtube_clip_part_path("A-B", 1), youtube_clip_part_path("A-B", 2));
    }

    #[test]
    fn guard_removes_part_on_drop_and_keeps_it_after_disarm() {
        let dir = temp_dir("guard");
        let armed_path = dir.join("a.1.part");
        write_part(&armed_path, b"x");
        {
            let _guard = PartFileGuard::new(armed_path.clone());
            assert!(armed_path.is_file());
        }
        assert!(!armed_path.exists(), "drop до rename должен удалить .part");

        let kept_path = dir.join("b.1.part");
        write_part(&kept_path, b"x");
        {
            let mut guard = PartFileGuard::new(kept_path.clone());
            guard.disarm();
        }
        assert!(kept_path.is_file(), "disarmed страж не трогает файл");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn art_track_threshold_uses_ten_kilobytes_per_second() {
        assert!(looks_like_art_track(600_000, Some(310))); // 1.9 КБ/с
        assert!(!looks_like_art_track(15_300_000, Some(278))); // 55 КБ/с
        assert!(!looks_like_art_track(3_100_000, Some(310))); // ровно 10 КБ/с
        assert!(!looks_like_art_track(100_000, None));
        assert!(!looks_like_art_track(100_000, Some(0)));
    }

    #[test]
    fn stale_parts_are_removed_but_fresh_ones_survive() {
        let dir = temp_dir("stale");
        write_part(&dir.join("old.1.part"), b"old");
        write_part(&dir.join("new.1.part"), b"new");
        write_part(&dir.join("done.mp4"), b"done");

        let old = dir.join("old.1.part");
        let file = File::options().write(true).open(&old).expect("open");
        let long_ago = std::time::SystemTime::now() - Duration::from_secs(11 * 60);
        file.set_times(
            std::fs::FileTimes::new()
                .set_accessed(long_ago)
                .set_modified(long_ago),
        )
        .expect("set times");
        drop(file);

        let removed = cleanup_stale_parts_in(&dir, STALE_PART_AGE);
        assert_eq!(removed, 1);
        assert!(!old.exists());
        assert!(dir.join("new.1.part").is_file(), "свежий .part трогать нельзя");
        assert!(dir.join("done.mp4").is_file(), "готовый MP4 трогать нельзя");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lru_removes_oldest_until_target_and_protects_current() {
        let dir = temp_dir("lru");
        let chunk = 100 * 1024 * 1024u64;
        let mut files = Vec::new();
        for (index, name) in ["a", "b", "c", "d", "e", "f"].iter().enumerate() {
            let path = dir.join(format!("{name}.mp4"));
            // sparse: длину задаём без реальных 100 МБ на диске
            let file = File::create(&path).expect("create");
            file.set_len(chunk).expect("set_len");
            let stamp = std::time::SystemTime::now() + Duration::from_secs(index as u64);
            let _ = file.set_times(
                std::fs::FileTimes::new()
                    .set_accessed(stamp)
                    .set_modified(stamp),
            );
            files.push(path);
        }
        // 6 × 100 МБ = 600 МБ > 512 МБ; защищаем самый старый файл — он должен выжить
        let freed = clean_youtube_clips_lru_in(&dir, Some("a"));
        assert!(freed >= 100 * 1024 * 1024);
        assert!(files[0].is_file(), "защищённый текущий клип нельзя удалять");
        let rest = youtube_clips_size_in(&dir);
        assert!(rest <= YOUTUBE_CACHE_TARGET_BYTES + 100 * 1024 * 1024);
        let _ = std::fs::remove_dir_all(dir);
    }
}
