use std::fs::{create_dir_all, File};
use std::io::Write;
use std::path::PathBuf;
use md5::{Digest, Md5};

pub mod cover_resolver;
pub use cover_resolver::{resolve_artist_avatar, resolve_studio_cover};

/// Возвращает путь к директории кэша обложек (~/.cache/volna/covers/)
pub fn covers_cache_dir() -> PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".cache")
        });
    let dir = base.join("volna").join("covers");
    let _ = create_dir_all(&dir);
    dir
}

/// Путь к кэшу синхронизированного текста песни (~/.cache/volna/lyrics/ID.lrc)
pub fn lyrics_cache_path(track_id: &str) -> PathBuf {
    let dir = covers_cache_dir().parent().map(|p| p.join("lyrics")).unwrap_or_else(|| PathBuf::from("lyrics"));
    let _ = create_dir_all(&dir);
    let safe: String = track_id.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    dir.join(format!("{}.lrc", safe))
}

/// Директория кэша текстов (~/.cache/volna/lyrics)
fn lyrics_dir() -> PathBuf {
    let dir = covers_cache_dir().parent().map(|p| p.join("lyrics")).unwrap_or_else(|| PathBuf::from("lyrics"));
    let _ = create_dir_all(&dir);
    dir
}

/// Путь к терминальному маркеру «текста нет» (~/.cache/volna/lyrics/ID.no_lyrics)
///
/// В отличие от `.lrc`-кэша, дефис и подчёркивание сохраняются: YouTube-ID может
/// содержать оба символа, и замена их на `_` давала бы коллизии (`A-B` vs `A_B`).
pub fn lyrics_negative_cache_path(track_id: &str) -> PathBuf {
    let safe: String = track_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    lyrics_dir().join(format!("{}.no_lyrics", safe))
}

/// Есть ли терминальный маркер «у трека нет текста»
pub fn has_negative_lyrics_marker(track_id: &str) -> bool {
    lyrics_negative_cache_path(track_id).exists()
}

/// Записать терминальный маркер «у трека нет текста»
pub fn write_negative_lyrics_marker(track_id: &str) {
    let _ = std::fs::write(lyrics_negative_cache_path(track_id), b"");
}

/// Снять маркер «текста нет» — вызывается, когда текст всё-таки нашёлся
pub fn clear_negative_lyrics_marker(track_id: &str) {
    let path = lyrics_negative_cache_path(track_id);
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
}

/// Вычисляет хэш для имени файла обложки
pub fn cover_cache_path(url: &str) -> PathBuf {
    let mut hasher = Md5::new();
    hasher.update(url.as_bytes());
    let res = hasher.finalize();
    let hash: String = res.iter().map(|b| format!("{:02x}", b)).collect();
    covers_cache_dir().join(format!("{}.jpg", hash))
}

/// Загружает обложку с диска, либо асинхронно скачивает и сохраняет в кэш
pub async fn fetch_cover_cached(url: &str) -> Option<PathBuf> {
    let path = cover_cache_path(url);
    if path.exists() {
        return Some(path);
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;

    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }

    let bytes = resp.bytes().await.ok()?;
    let mut file = File::create(&path).ok()?;
    file.write_all(&bytes).ok()?;

    Some(path)
}

/// Размер кэша обложек в байтах
pub fn covers_cache_size() -> u64 {
    std::fs::read_dir(covers_cache_dir())
        .map(|d| d.flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum())
        .unwrap_or(0)
}

/// Удаляет файлы кэша обложек, возвращает освобождённые байты
pub fn clear_covers_cache() -> u64 {
    let freed = covers_cache_size();
    if let Ok(dir) = std::fs::read_dir(covers_cache_dir()) {
        for entry in dir.flatten().filter(|e| e.path().is_file()) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    freed
}

/// Папка кэша клипов (~/.cache/volna/clips/)
pub fn clips_cache_dir() -> PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string())).join(".cache"));
    let dir = base.join("volna").join("clips");
    let _ = create_dir_all(&dir);
    dir
}

/// Скачивает превью-клип в кэш (несколько МБ), повторно берёт с диска. Лимит размера ответа — защита от подмены ссылки
pub async fn fetch_clip_cached(url: &str) -> Option<PathBuf> {
    let mut hasher = Md5::new();
    hasher.update(url.as_bytes());
    let name: String = hasher.finalize().iter().map(|b| format!("{:02x}", b)).collect();
    let path = clips_cache_dir().join(format!("{name}.mp4"));
    if path.exists() {
        return Some(path);
    }
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build().ok()?;
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() || resp.content_length().is_some_and(|n| n > 30_000_000) {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    if bytes.len() > 30_000_000 {
        return None;
    }
    // во временный файл и переименование: оборванная загрузка не должна остаться «готовым» клипом
    let tmp = path.with_extension("part");
    File::create(&tmp).ok()?.write_all(&bytes).ok()?;
    std::fs::rename(&tmp, &path).ok()?;
    Some(path)
}

/// Размер кэша клипов в байтах
pub fn clips_cache_size() -> u64 {
    std::fs::read_dir(clips_cache_dir())
        .map(|d| d.flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum())
        .unwrap_or(0)
}

/// Удаляет кэш клипов, возвращает освобождённые байты
pub fn clear_clips_cache() -> u64 {
    let freed = clips_cache_size();
    if let Ok(dir) = std::fs::read_dir(clips_cache_dir()) {
        for entry in dir.flatten().filter(|e| e.path().is_file()) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    freed
}

// ================= Последняя очередь (альбом, плейлист…) =================
// Нужна, чтобы после перезапуска «Play» продолжал альбом или плейлист, а не включал Волну по треку.

#[derive(serde::Serialize, serde::Deserialize)]
struct Session {
    source: Option<crate::player::PlaySource>,
    queue: Vec<Track>,
}

fn session_dir() -> PathBuf {
    let dir = data_dir().join("session");
    let _ = create_dir_all(&dir);
    dir
}

/// Запоминает очередь и её источник (запись в фоне: очередь может быть в сотни треков)
pub fn save_session_queue(queue: &[Track], source: Option<crate::player::PlaySource>) {
    let session = Session { source, queue: queue.iter().take(500).cloned().collect() };
    let path = session_dir().join("queue.json");
    std::thread::spawn(move || {
        if let Ok(json) = serde_json::to_vec(&session) {
            let _ = std::fs::write(path, json);
        }
    });
}

/// Позиция в очереди: меняется на каждом треке, поэтому лежит отдельно от самой очереди
pub fn save_session_index(index: usize) {
    let _ = std::fs::write(session_dir().join("index"), index.to_string());
}

fn load_session_in(dir: &std::path::Path) -> Option<(Vec<Track>, usize, Option<crate::player::PlaySource>)> {
    let session: Session = serde_json::from_slice(&std::fs::read(dir.join("queue.json")).ok()?).ok()?;
    if session.queue.is_empty() {
        return None;
    }
    let index = std::fs::read_to_string(dir.join("index")).ok().and_then(|s| s.trim().parse::<usize>().ok()).unwrap_or(0);
    let index = index.min(session.queue.len() - 1);
    Some((session.queue, index, session.source))
}

/// Забывает очередь (включена Волна: после перезапуска она продолжается по треку)
pub fn clear_session() {
    let _ = std::fs::remove_file(session_dir().join("queue.json"));
}

/// Сохранённая очередь: (треки, индекс, источник); индекс приводится к границам очереди
pub fn load_session() -> Option<(Vec<Track>, usize, Option<crate::player::PlaySource>)> {
    load_session_in(&session_dir())
}

// ================= Свои клипы для фона =================
// ~/.local/share/volna/custom_clips/<id альбома>/*.mp4: для треков этого альбома фон берёт их вместо клипов Яндекса.
// Файлы кладёт пользователь, в репозиторий они не входят.

/// Каталог данных приложения без glib: `$XDG_DATA_HOME/volna` или `~/.local/share/volna`
fn data_dir() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(".local").join("share")
    });
    base.join("volna")
}

/// Случайный свой клип для любого из альбомов трека
pub fn custom_clip_for(track: &Track) -> Option<PathBuf> {
    let pick = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos() as usize).unwrap_or(0);
    custom_clip_in(&data_dir(), track, pick)
}

fn custom_clip_in(data: &std::path::Path, track: &Track, pick: usize) -> Option<PathBuf> {
    let mut clips: Vec<PathBuf> = Vec::new();
    for album in &track.albums {
        let dir = data.join("custom_clips").join(safe_id(&album.id));
        clips.extend(std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "mp4")));
    }
    if clips.is_empty() {
        return None;
    }
    clips.sort();
    Some(clips.swap_remove(pick % clips.len()))
}

// ================= Офлайн-треки =================
// Лежат в данных приложения (~/.local/share/volna/offline), а не в кэше: «Очистить память» их не трогает.

use crate::api::models::Track;

fn offline_dir() -> PathBuf {
    let dir = data_dir().join("offline");
    let _ = create_dir_all(&dir);
    dir
}

fn safe_id(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect()
}

/// Папка дискового кэша потокового аудио (~/.cache/volna/audio/)
pub fn audio_cache_dir() -> PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".cache")
        });
    let dir = base.join("volna").join("audio");
    let _ = create_dir_all(&dir);
    dir
}

/// Проверяет наличие файлов mp3, opus или m4a в кэше размером > 128 КБ
pub fn cached_audio_path(track_id: &str) -> Option<PathBuf> {
    let dir = audio_cache_dir();
    let s_id = safe_id(track_id);
    for ext in ["mp3", "opus", "m4a"] {
        let p = dir.join(format!("{}.{}", s_id, ext));
        if let Ok(meta) = p.metadata() {
            if meta.is_file() && meta.len() > 128 * 1024 {
                return Some(p);
            }
        }
    }
    None
}

/// Временный путь для сохранения скачиваемого трека
pub fn temp_audio_cache_path(track_id: &str, ext: &str) -> PathBuf {
    audio_cache_dir().join(format!("{}.{}.part", safe_id(track_id), ext))
}

/// Финальный путь сохранённого в кэше трека
pub fn final_audio_cache_path(track_id: &str, ext: &str) -> PathBuf {
    audio_cache_dir().join(format!("{}.{}", safe_id(track_id), ext))
}

/// Удаляет старые файлы кэша аудио по алгоритму LRU, пока общий размер не станет <= 75% от max_bytes
pub fn clean_audio_cache_lru(max_bytes: u64) {
    let dir = audio_cache_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
    let mut total_bytes: u64 = 0;
    for entry in entries.flatten() {
        if let Ok(meta) = entry.metadata() {
            if meta.is_file() {
                let size = meta.len();
                let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                total_bytes = total_bytes.saturating_add(size);
                files.push((mtime, size, entry.path()));
            }
        }
    }

    if total_bytes > max_bytes {
        files.sort_by_key(|&(mtime, _, _)| mtime);
        let target_bytes = (max_bytes * 3) / 4;
        for (_, size, path) in files {
            if total_bytes <= target_bytes {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total_bytes = total_bytes.saturating_sub(size);
            }
        }
    }
}

/// Путь к сохранённому mp3, если трек есть офлайн
pub fn offline_audio_path(track_id: &str) -> Option<PathBuf> {
    let p = offline_dir().join(format!("{}.mp3", safe_id(track_id)));
    p.exists().then_some(p)
}

/// Сохраняет аудио и описание трека; описание пишется последним, по нему трек считается сохранённым
pub fn save_offline(track: &Track, audio: &[u8]) -> std::io::Result<()> {
    let base = offline_dir().join(safe_id(&track.id));
    let part = base.with_extension("part");
    std::fs::write(&part, audio)?;
    std::fs::rename(&part, base.with_extension("mp3"))?;
    std::fs::write(base.with_extension("json"), serde_json::to_vec(track).map_err(std::io::Error::other)?)
}

pub fn remove_offline(track_id: &str) {
    let base = offline_dir().join(safe_id(track_id));
    for ext in ["json", "mp3", "part"] {
        let _ = std::fs::remove_file(base.with_extension(ext));
    }
}

/// Все сохранённые треки (новые первыми)
pub fn list_offline() -> Vec<Track> {
    let mut items: Vec<(std::time::SystemTime, Track)> = std::fs::read_dir(offline_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| {
            let track: Track = serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok()?;
            offline_audio_path(&track.id)?;
            Some((e.metadata().ok()?.modified().ok()?, track))
        })
        .collect();
    items.sort_by(|a, b| b.0.cmp(&a.0));
    items.into_iter().map(|(_, t)| t).collect()
}

/// Размер офлайн-треков в байтах
pub fn offline_size() -> u64 {
    std::fs::read_dir(offline_dir()).into_iter().flatten().flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()
}

#[cfg(test)]
mod offline_tests {
    use super::*;

    #[test]
    fn offline_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("volna-offline-{}", std::process::id()));
        // SAFETY: единственный тест, трогающий окружение, и он выполняется до первого обращения к каталогу
        unsafe { std::env::set_var("XDG_DATA_HOME", &tmp) };
        let track: Track = serde_json::from_value(serde_json::json!({"id": "7", "title": "T", "artists": []})).unwrap();
        assert!(offline_audio_path("7").is_none());
        save_offline(&track, b"abc").unwrap();
        assert!(offline_audio_path("7").is_some());
        assert_eq!(list_offline().len(), 1);
        remove_offline("7");
        assert!(list_offline().is_empty());
        let _ = std::fs::remove_dir_all(tmp);
    }
}

#[cfg(test)]
mod custom_clip_tests {
    use super::*;

    #[test]
    fn custom_clip_is_taken_from_track_album_folder() {
        let tmp = std::env::temp_dir().join(format!("volna-clips-{}", std::process::id()));
        let dir = tmp.join("custom_clips").join("22045");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.mp4"), b"x").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        let white_pony: Track = serde_json::from_value(serde_json::json!({"id": "1", "title": "T", "albums": [{"id": "22045"}]})).unwrap();
        let other: Track = serde_json::from_value(serde_json::json!({"id": "2", "title": "T", "albums": [{"id": "1"}]})).unwrap();
        assert_eq!(custom_clip_in(&tmp, &white_pony, 7), Some(dir.join("a.mp4")));
        assert_eq!(custom_clip_in(&tmp, &other, 0), None);
        let _ = std::fs::remove_dir_all(tmp);
    }
}

#[cfg(test)]
mod session_tests {
    use super::*;

    #[test]
    fn session_is_loaded_and_index_is_clamped() {
        let dir = std::env::temp_dir().join(format!("volna-session-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let t = |id: &str| -> Track { serde_json::from_value(serde_json::json!({"id": id, "title": id})).unwrap() };
        assert!(load_session_in(&dir).is_none());
        let json = serde_json::to_vec(&Session { source: Some(crate::player::PlaySource::new("Альбом: X", None)), queue: vec![t("1"), t("2")] }).unwrap();
        std::fs::write(dir.join("queue.json"), json).unwrap();
        std::fs::write(dir.join("index"), "9").unwrap();
        let (queue, index, source) = load_session_in(&dir).unwrap();
        assert_eq!((queue.len(), index, source.map(|s| s.label).as_deref()), (2, 1, Some("Альбом: X")));
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod audio_cache_tests {
    use super::*;

    #[test]
    fn test_audio_cache_paths_and_clean() {
        let _dir = audio_cache_dir();
        let track_id = "test_audio_track_999";
        let final_path = final_audio_cache_path(track_id, "mp3");
        let temp_path = temp_audio_cache_path(track_id, "mp3");
        assert!(temp_path.to_string_lossy().ends_with(".part"));
        assert!(final_path.to_string_lossy().ends_with(".mp3"));

        // Удалим перед тестом если остался
        let _ = std::fs::remove_file(&final_path);
        let _ = std::fs::remove_file(&temp_path);

        assert!(cached_audio_path(track_id).is_none());

        // Файл слишком маленький (меньше 128 КБ) -> cached_audio_path должен вернуть None
        let small_data = vec![0u8; 100 * 1024];
        std::fs::write(&final_path, &small_data).unwrap();
        assert!(cached_audio_path(track_id).is_none());

        // Файл больше 128 КБ -> Some(path)
        let valid_data = vec![0u8; 150 * 1024];
        std::fs::write(&final_path, &valid_data).unwrap();
        assert_eq!(cached_audio_path(track_id), Some(final_path.clone()));

        // Проверим clean_audio_cache_lru
        clean_audio_cache_lru(100 * 1024); // лимит 100 КБ, наш файл 150 КБ -> должен очиститься
        assert!(cached_audio_path(track_id).is_none());

        let _ = std::fs::remove_file(&final_path);
    }
}


#[cfg(test)]
mod negative_cache_tests {
    use super::*;

    #[test]
    fn marker_paths_do_not_collide() {
        let a = lyrics_negative_cache_path("A-B");
        let b = lyrics_negative_cache_path("A_B");
        assert_ne!(a, b, "дефис и подчёркивание должны различаться");
        assert!(a.to_string_lossy().ends_with("A-B.no_lyrics"));
        assert!(b.to_string_lossy().ends_with("A_B.no_lyrics"));
    }

    #[test]
    fn marker_write_and_clear() {
        let id = "test-marker-track_1";
        let _ = std::fs::remove_file(lyrics_negative_cache_path(id));
        assert!(!has_negative_lyrics_marker(id));
        write_negative_lyrics_marker(id);
        assert!(has_negative_lyrics_marker(id));
        clear_negative_lyrics_marker(id);
        assert!(!has_negative_lyrics_marker(id));
    }
}
