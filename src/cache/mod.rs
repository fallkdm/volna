use std::fs::{create_dir_all, File};
use std::io::Write;
use std::path::PathBuf;
use md5::{Digest, Md5};

/// Возвращает базовый каталог кэша приложения (~/.cache/volna)
pub fn volna_cache_dir() -> PathBuf {
    glib::user_cache_dir().join("volna")
}

/// Возвращает путь к директории кэша обложек (~/.cache/volna/covers/)
pub fn covers_cache_dir() -> PathBuf {
    let dir = volna_cache_dir().join("covers");
    let _ = create_dir_all(&dir);
    dir
}

/// Путь к кэшу синхронизированного текста песни (~/.cache/volna/lyrics/ID.lrc)
pub fn lyrics_cache_path(track_id: &str) -> PathBuf {
    let dir = volna_cache_dir().join("lyrics");
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
    let dir = volna_cache_dir().join("clips");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glib_cache_dir_without_gtk_init() {
        // Вызов glib::user_cache_dir() без gtk::init работает штатно и не падает
        let cache_dir = glib::user_cache_dir();
        assert!(!cache_dir.as_os_str().is_empty());

        if let Ok(expected) = std::env::var("XDG_CACHE_HOME") {
            assert_eq!(cache_dir, PathBuf::from(&expected));
        }

        let vdir = volna_cache_dir();
        assert!(vdir.ends_with("volna"));
        if let Ok(expected) = std::env::var("XDG_CACHE_HOME") {
            assert_eq!(vdir, PathBuf::from(expected).join("volna"));
        }
    }
}

