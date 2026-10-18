use std::fs::{create_dir_all, File};
use std::io::Write;
use std::path::PathBuf;
use md5::{Digest, Md5};

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

// ================= Офлайн-треки =================
// Лежат в данных приложения (~/.local/share/volna/offline), а не в кэше: «Очистить память» их не трогает.

use crate::api::models::Track;

fn offline_dir() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(".local").join("share")
    });
    let dir = base.join("volna").join("offline");
    let _ = create_dir_all(&dir);
    dir
}

fn safe_id(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect()
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
