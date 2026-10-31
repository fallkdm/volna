use crate::api::models::{
    Album, Artist, ArtistBrief, ArtistCover, Playlist, SearchResult, SearchSection, Track,
    TrackSource,
};

/// Парсинг длительности вида "3:45", "01:23:45", "45" в миллисекунды
pub fn parse_duration_ms(duration_str: &str) -> Option<u64> {
    let s = duration_str.trim();
    if s.is_empty() {
        return None;
    }
    let parts: Vec<&str> = s.split(':').collect();
    let ms = match parts.len() {
        1 => parts[0].trim().parse::<u64>().ok().map(|secs| secs * 1000),
        2 => {
            let mins = parts[0].trim().parse::<u64>().ok()?;
            let secs = parts[1].trim().parse::<u64>().ok()?;
            Some((mins * 60 + secs) * 1000)
        }
        3 => {
            let hours = parts[0].trim().parse::<u64>().ok()?;
            let mins = parts[1].trim().parse::<u64>().ok()?;
            let secs = parts[2].trim().parse::<u64>().ok()?;
            Some((hours * 3600 + mins * 60 + secs) * 1000)
        }
        _ => None,
    };
    match ms {
        Some(0) => None,
        other => other,
    }
}

/// Поиск массива `thumbnails` по всем известным путям и через рекурсивный обход
fn find_thumbnails_array(val: &serde_json::Value) -> Option<&Vec<serde_json::Value>> {
    if let Some(arr) = val.as_array()
        && arr.first().and_then(|item| item.get("url")).is_some() {
            return Some(arr);
        }
    if let Some(arr) = val.get("thumbnails").and_then(|v| v.as_array()) {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("thumbnail")
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("thumbnail")
        .and_then(|t| t.get("musicThumbnailRenderer"))
        .and_then(|m| m.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("thumbnail")
        .and_then(|t| t.get("croppedSquareThumbnailRenderer"))
        .and_then(|m| m.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("musicResponsiveHeaderRenderer")
        .and_then(|h| h.get("thumbnail"))
        .and_then(|t| t.get("musicThumbnailRenderer"))
        .and_then(|m| m.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("musicResponsiveHeaderRenderer")
        .and_then(|h| h.get("thumbnail"))
        .and_then(|t| t.get("croppedSquareThumbnailRenderer"))
        .and_then(|m| m.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("musicDetailHeaderRenderer")
        .and_then(|h| h.get("thumbnail"))
        .and_then(|t| t.get("musicThumbnailRenderer"))
        .and_then(|m| m.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("musicDetailHeaderRenderer")
        .and_then(|h| h.get("thumbnail"))
        .and_then(|t| t.get("croppedSquareThumbnailRenderer"))
        .and_then(|m| m.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("musicThumbnailRenderer")
        .and_then(|m| m.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("croppedSquareThumbnailRenderer")
        .and_then(|m| m.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("thumbnailRenderer")
        .and_then(|t| t.get("musicThumbnailRenderer"))
        .and_then(|m| m.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("thumbnailRenderer")
        .and_then(|m| m.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("thumbnailRenderer")
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("musicTwoRowItemRenderer")
        .and_then(|r| r.get("thumbnailRenderer"))
        .and_then(|t| t.get("musicThumbnailRenderer"))
        .and_then(|m| m.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("musicTwoRowItemRenderer")
        .and_then(|r| r.get("thumbnailRenderer"))
        .and_then(|m| m.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("musicTwoRowItemRenderer")
        .and_then(|r| r.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("foregroundThumbnail")
        .and_then(|t| t.get("musicThumbnailRenderer"))
        .and_then(|m| m.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("foregroundThumbnail")
        .and_then(|t| t.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("foregroundThumbnail")
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }

    // Рекурсивный поиск массива thumbnails с элементами, содержащими url
    fn deep_search(v: &serde_json::Value) -> Option<&Vec<serde_json::Value>> {
        if let Some(obj) = v.as_object() {
            if let Some(arr) = obj.get("thumbnails").and_then(|a| a.as_array())
                && arr.iter().any(|item| item.get("url").is_some()) {
                    return Some(arr);
                }
            for sub in obj.values() {
                if let Some(res) = deep_search(sub) {
                    return Some(res);
                }
            }
        } else if let Some(arr) = v.as_array() {
            for sub in arr {
                if let Some(res) = deep_search(sub) {
                    return Some(res);
                }
            }
        }
        None
    }
    deep_search(val)
}

/// Извлечение лучшего качества URL обложки из структуры `thumbnails`
pub fn extract_best_thumbnail(val: &serde_json::Value) -> Option<String> {
    if let Some(list) = find_thumbnails_array(val)
        && let Some(best) = list.iter().max_by_key(|item| {
            let w = item.get("width").and_then(|w| w.as_u64()).unwrap_or(0);
            let h = item.get("height").and_then(|h| h.as_u64()).unwrap_or(0);
            w * h.max(1)
        })
            && let Some(url_str) = best.get("url").and_then(|u| u.as_str()) {
                let mut url = url_str.to_string();
                if url.starts_with("//") {
                    url = format!("https:{}", url);
                }
                return Some(url);
            }

    if let Some(url_str) = val.get("url").and_then(|u| u.as_str()) {
        let mut url = url_str.to_string();
        if url.starts_with("//") {
            url = format!("https:{}", url);
        }
        return Some(url);
    }

    None
}

/// Проверяет, является ли thumbnail широкоформатным баннером (соотношение сторон w > h * 1.3)
pub fn is_banner_thumbnail(item: &serde_json::Value) -> bool {
    let w = item.get("width").and_then(|w| w.as_f64()).unwrap_or(0.0);
    let h = item.get("height").and_then(|h| h.as_f64()).unwrap_or(0.0);
    if w > 0.0 && h > 0.0 && w > h * 1.3 {
        return true;
    }
    if let Some(url) = item.get("url").and_then(|u| u.as_str())
        && let Some(idx) = url.find("=w") {
            let tail = &url[idx + 2..];
            if let Some(h_idx) = tail.find("-h") {
                let w_str = &tail[..h_idx];
                let rest = &tail[h_idx + 2..];
                let h_end = rest.find('-').unwrap_or(rest.len());
                let h_str = &rest[..h_end];
                if let (Ok(width), Ok(height)) = (w_str.parse::<f64>(), h_str.parse::<f64>())
                    && width > height * 1.3 {
                        return true;
                    }
            }
        }
    false
}

/// Извлечение лучшего качества URL квадратного thumbnail (отсекая широкоформатные баннеры)
pub fn extract_square_thumbnail(val: &serde_json::Value) -> Option<String> {
    if let Some(list) = find_thumbnails_array(val) {
        let square_items: Vec<_> = list.iter().filter(|item| !is_banner_thumbnail(item)).collect();
        if let Some(best) = square_items.into_iter().max_by_key(|item| {
            let w = item.get("width").and_then(|w| w.as_u64()).unwrap_or(0);
            let h = item.get("height").and_then(|h| h.as_u64()).unwrap_or(0);
            w * h.max(1)
        })
            && let Some(url_str) = best.get("url").and_then(|u| u.as_str()) {
                let mut url = url_str.to_string();
                if url.starts_with("//") {
                    url = format!("https:{}", url);
                }
                return Some(url);
            }
    }

    if let Some(url_str) = val.get("url").and_then(|u| u.as_str())
        && !is_banner_thumbnail(val) {
            let mut url = url_str.to_string();
            if url.starts_with("//") {
                url = format!("https:{}", url);
            }
            return Some(url);
        }

    None
}

/// Извлечение плоского текста из объектов InnerTube (`runs` или `simpleText`)
pub fn extract_text(val: &serde_json::Value) -> Option<String> {
    if let Some(s) = val.as_str() {
        return Some(s.to_string());
    }
    if let Some(s) = val.get("simpleText").and_then(|t| t.as_str()) {
        return Some(s.to_string());
    }
    if let Some(runs) = val.get("runs").and_then(|r| r.as_array()) {
        let mut res = String::new();
        for r in runs {
            if let Some(t) = r.get("text").and_then(|t| t.as_str()) {
                res.push_str(t);
            }
        }
        if !res.is_empty() {
            return Some(res);
        }
    }
    None
}

/// Стоп-слова для отсечения подкастов, разговоров и видео-мусора
pub const MUSIC_STOP_WORDS: &[&str] = &[
    "подкаст",
    "podcast",
    "выпуск #",
    "выпуск №",
    "эпизод ",
    "episode ",
    "интервью",
    "шоу ",
    "talk show",
    "стрим",
    "stream",
    "реакция ",
    "reaction",
    "review",
    "обзор ",
    "full album",
    "тизер",
    "teaser",
    "аудиокнига",
    "audiobook",
];

/// Проверяет, является ли трек чистым музыкальным треком:
/// - Длительность: от 30 секунд (30_000 мс) до 20 минут (1_200_000 мс). Если длительность известна и выходит за пределы — возвращает `false`.
/// - Проверяет стоп-слова в названии и имени артиста (в нижнем регистре). Если найдено — возвращает `false`.
/// - Иначе `true`.
pub fn is_clean_music_track(track: &Track) -> bool {
    if let Some(dur_ms) = track.duration_ms
        && (!(30_000..=1_200_000).contains(&dur_ms)) {
            return false;
        }
    let title_lower = track.title.to_lowercase();
    let artists_lower = track.artists_string().to_lowercase();
    for &stop_word in MUSIC_STOP_WORDS {
        if title_lower.contains(stop_word) || artists_lower.contains(stop_word) {
            return false;
        }
    }
    true
}

/// Рекурсивный поиск videoId в объекте
fn find_video_id(val: &serde_json::Value) -> Option<String> {
    if let Some(id) = val.get("videoId").and_then(|v| v.as_str())
        && !id.is_empty() {
            return Some(id.to_string());
        }
    if let Some(id) = val
        .get("playlistItemData")
        .and_then(|p| p.get("videoId"))
        .and_then(|v| v.as_str())
        && !id.is_empty() {
            return Some(id.to_string());
        }
    if let Some(id) = val
        .get("watchEndpoint")
        .and_then(|w| w.get("videoId"))
        .and_then(|v| v.as_str())
        && !id.is_empty() {
            return Some(id.to_string());
        }
    if let Some(id) = val
        .get("navigationEndpoint")
        .and_then(|n| n.get("watchEndpoint"))
        .and_then(|w| w.get("videoId"))
        .and_then(|v| v.as_str())
        && !id.is_empty() {
            return Some(id.to_string());
        }
    if let Some(id) = val
        .get("onTap")
        .and_then(|n| n.get("watchEndpoint"))
        .and_then(|w| w.get("videoId"))
        .and_then(|v| v.as_str())
        && !id.is_empty() {
            return Some(id.to_string());
        }
    None
}

/// Рекурсивный поиск browseId в объекте
fn find_browse_id(val: &serde_json::Value) -> Option<String> {
    if let Some(id) = val.get("browseId").and_then(|v| v.as_str())
        && !id.is_empty() {
            return Some(id.to_string());
        }
    if let Some(id) = val
        .get("browseEndpoint")
        .and_then(|b| b.get("browseId"))
        .and_then(|v| v.as_str())
        && !id.is_empty() {
            return Some(id.to_string());
        }
    if let Some(id) = val
        .get("navigationEndpoint")
        .and_then(|n| n.get("browseEndpoint"))
        .and_then(|b| b.get("browseId"))
        .and_then(|v| v.as_str())
        && !id.is_empty() {
            return Some(id.to_string());
        }
    if let Some(id) = val
        .get("onTap")
        .and_then(|n| n.get("browseEndpoint"))
        .and_then(|b| b.get("browseId"))
        .and_then(|v| v.as_str())
        && !id.is_empty() {
            return Some(id.to_string());
        }
    if let Some(id) = val
        .get("onTap")
        .and_then(|n| n.get("navigationEndpoint"))
        .and_then(|n| n.get("browseEndpoint"))
        .and_then(|b| b.get("browseId"))
        .and_then(|v| v.as_str())
        && !id.is_empty() {
            return Some(id.to_string());
        }
    if let Some(runs) = val
        .get("flexColumns")
        .and_then(|fc| fc.get(0))
        .and_then(|c| c.get("musicResponsiveListItemFlexColumnRenderer"))
        .and_then(|c| c.get("text"))
        .and_then(|t| t.get("runs"))
        .and_then(|r| r.as_array())
    {
        for r in runs {
            if let Some(id) = find_browse_id(r) {
                return Some(id);
            }
        }
    }
    if let Some(runs) = val.get("title").and_then(|t| t.get("runs")).and_then(|r| r.as_array()) {
        for r in runs {
            if let Some(id) = find_browse_id(r) {
                return Some(id);
            }
        }
    }
    if let Some(header) = val.get("header").and_then(|h| h.get("musicCardShelfHeaderBasicRenderer"))
        && let Some(id) = find_browse_id(header) {
            return Some(id);
        }
    None
}

/// Извлечение метаданных трека (артисты, альбом, длительность) из массива `runs`
fn parse_track_meta_runs(
    runs: &[serde_json::Value],
    artists: &mut Vec<Artist>,
    albums: &mut Vec<Album>,
    duration_ms: &mut Option<u64>,
) {
    let mut explicit_found_artist = false;
    let mut explicit_found_album = false;

    for r in runs {
        let text = match r.get("text").and_then(|t| t.as_str()) {
            Some(t) => t.trim(),
            None => continue,
        };
        if text.is_empty() || text == "•" || text == "," {
            continue;
        }

        let browse_id = r
            .get("navigationEndpoint")
            .and_then(|n| n.get("browseEndpoint"))
            .and_then(|b| b.get("browseId"))
            .and_then(|id| id.as_str());

        if let Some(bid) = browse_id {
            if bid.starts_with("UC") || bid.contains("artist") {
                artists.push(Artist {
                    id: bid.to_string(),
                    name: text.to_string(),
                    cover: None,
                });
                explicit_found_artist = true;
                continue;
            } else if bid.starts_with("MPRE") || bid.starts_with("FEmusic_library_album") {
                albums.push(Album {
                    id: bid.to_string(),
                    title: Some(text.to_string()),
                    year: None,
                    cover_uri: None,
                    og_image: None,
                    artists: Vec::new(),
                    track_count: None,
                });
                explicit_found_album = true;
                continue;
            }
        }

        if duration_ms.is_none() && text.contains(':')
            && let Some(d) = parse_duration_ms(text) {
                *duration_ms = Some(d);
            }
    }

    if !explicit_found_artist {
        let mut sections: Vec<Vec<String>> = Vec::new();
        let mut current_section: Vec<String> = Vec::new();
        for r in runs {
            let t = r.get("text").and_then(|t| t.as_str()).unwrap_or("").trim();
            if t == "•" {
                if !current_section.is_empty() {
                    sections.push(std::mem::take(&mut current_section));
                }
            } else if !t.is_empty() && t != "," {
                current_section.push(t.to_string());
            }
        }
        if !current_section.is_empty() {
            sections.push(current_section);
        }

        let mut sec_iter = sections.into_iter();
        if let Some(first_sec) = sec_iter.next() {
            let combined = first_sec.join(" ");
            let is_type = matches!(
                combined.to_lowercase().as_str(),
                "песня" | "видео" | "song" | "video" | "сингл" | "single"
            );
            let artist_sec = if is_type {
                sec_iter.next()
            } else {
                Some(first_sec)
            };

            if let Some(art_sec) = artist_sec {
                for name in art_sec {
                    if !name.trim().is_empty() {
                        artists.push(Artist {
                            id: String::new(),
                            name,
                            cover: None,
                        });
                    }
                }
            }

            if !explicit_found_album
                && let Some(next_sec) = sec_iter.next() {
                    let album_title = next_sec.join(" ");
                    if parse_duration_ms(&album_title).is_none() {
                        albums.push(Album {
                            id: String::new(),
                            title: Some(album_title),
                            year: None,
                            cover_uri: None,
                            og_image: None,
                            artists: Vec::new(),
                            track_count: None,
                        });
                    }
                }
        }
    }
}

/// Извлечение explicit признака из badges
fn is_explicit(val: &serde_json::Value) -> bool {
    if let Some(badges) = val.get("badges").and_then(|b| b.as_array()) {
        for b in badges {
            let icon = b
                .get("musicInlineBadgeRenderer")
                .and_then(|m| m.get("icon"))
                .and_then(|i| i.get("iconType"))
                .and_then(|t| t.as_str());
            if icon == Some("MUSIC_EXPLICIT_BADGE") {
                return true;
            }
        }
    }
    false
}

/// Парсинг `musicResponsiveListItemRenderer` в `Track`
pub fn parse_responsive_track(item: &serde_json::Value) -> Option<Track> {
    let renderer = item.get("musicResponsiveListItemRenderer").unwrap_or(item);

    let video_id = find_video_id(renderer)?;

    let flex_columns = renderer.get("flexColumns").and_then(|fc| fc.as_array())?;
    let col0 = flex_columns.first()?;
    let title_val = col0
        .get("musicResponsiveListItemFlexColumnRenderer")
        .and_then(|c| c.get("text"));
    let title = title_val.and_then(extract_text).unwrap_or_default();
    if title.is_empty() {
        return None;
    }

    let mut cover_uri = extract_best_thumbnail(renderer).or_else(|| extract_best_thumbnail(item));
    if cover_uri.as_deref().is_none_or(|u| u.contains("googleusercontent.com")) {
        cover_uri = Some(format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", video_id));
    }

    let mut artists = Vec::new();
    let mut albums = Vec::new();
    let mut duration_ms = None;

    if let Some(col1) = flex_columns.get(1)
        && let Some(runs) = col1
            .get("musicResponsiveListItemFlexColumnRenderer")
            .and_then(|c| c.get("text"))
            .and_then(|t| t.get("runs"))
            .and_then(|r| r.as_array())
        {
            parse_track_meta_runs(runs, &mut artists, &mut albums, &mut duration_ms);
        }

    // Дополнительный поиск длительности в fixedColumns
    if duration_ms.is_none()
        && let Some(fixed) = renderer.get("fixedColumns").and_then(|fc| fc.as_array()) {
            for col in fixed {
                if let Some(text) = col
                    .get("musicResponsiveListItemFixedColumnRenderer")
                    .and_then(|c| c.get("text"))
                    .and_then(extract_text)
                    && let Some(d) = parse_duration_ms(&text) {
                        duration_ms = Some(d);
                        break;
                    }
            }
        }

    let content_warning = if is_explicit(renderer) {
        Some("explicit".to_string())
    } else {
        None
    };

    Some(Track {
        id: video_id,
        title,
        version: None,
        artists,
        albums,
        duration_ms,
        cover_uri,
        og_image: None,
        available: Some(true),
        content_warning,
        derived_colors: None,
        source: TrackSource::YouTubeMusic,
    })
}

/// Парсинг `playlistPanelVideoRenderer` (из очереди радио `/next`)
pub fn parse_playlist_panel_video(item: &serde_json::Value) -> Option<Track> {
    let renderer = item.get("playlistPanelVideoRenderer").unwrap_or(item);

    let video_id = renderer.get("videoId").and_then(|v| v.as_str())?.to_string();
    let title = renderer.get("title").and_then(extract_text).unwrap_or_default();
    let mut cover_uri = extract_best_thumbnail(renderer);
    if cover_uri.as_deref().is_none_or(|u| u.contains("googleusercontent.com")) {
        cover_uri = Some(format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", video_id));
    }

    let mut duration_ms = renderer
        .get("lengthText")
        .and_then(extract_text)
        .and_then(|s| parse_duration_ms(&s));

    let mut artists = Vec::new();
    let mut albums = Vec::new();

    let byline = renderer
        .get("longBylineText")
        .or_else(|| renderer.get("shortBylineText"));

    if let Some(runs) = byline.and_then(|b| b.get("runs")).and_then(|r| r.as_array()) {
        parse_track_meta_runs(runs, &mut artists, &mut albums, &mut duration_ms);
    }

    let content_warning = if is_explicit(renderer) {
        Some("explicit".to_string())
    } else {
        None
    };

    Some(Track {
        id: video_id,
        title,
        version: None,
        artists,
        albums,
        duration_ms,
        cover_uri,
        og_image: None,
        available: Some(true),
        content_warning,
        derived_colors: None,
        source: TrackSource::YouTubeMusic,
    })
}

/// Парсинг альбома из `musicResponsiveListItemRenderer` или `musicTwoRowItemRenderer`
pub fn parse_album_item(item: &serde_json::Value) -> Option<Album> {
    let renderer = if let Some(r) = item.get("musicTwoRowItemRenderer") {
        r
    } else if let Some(r) = item.get("musicResponsiveListItemRenderer") {
        r
    } else {
        item
    };

    let browse_id = find_browse_id(renderer).unwrap_or_default();
    let title = renderer
        .get("title")
        .and_then(extract_text)
        .or_else(|| {
            renderer
                .get("flexColumns")
                .and_then(|fc| fc.get(0))
                .and_then(|c| c.get("musicResponsiveListItemFlexColumnRenderer"))
                .and_then(|c| c.get("text"))
                .and_then(extract_text)
        });

    let mut cover_uri = extract_best_thumbnail(renderer);
    if cover_uri.is_none() {
        cover_uri = extract_best_thumbnail(item);
    }

    let mut artists = Vec::new();
    let mut year = None;

    let subtitle_runs = renderer
        .get("subtitle")
        .and_then(|s| s.get("runs"))
        .and_then(|r| r.as_array())
        .or_else(|| {
            renderer
                .get("flexColumns")
                .and_then(|fc| fc.get(1))
                .and_then(|c| c.get("musicResponsiveListItemFlexColumnRenderer"))
                .and_then(|c| c.get("text"))
                .and_then(|t| t.get("runs"))
                .and_then(|r| r.as_array())
        });

    if let Some(runs) = subtitle_runs {
        for r in runs {
            let t = r.get("text").and_then(|t| t.as_str()).unwrap_or("").trim();
            if t.len() == 4 && (t.starts_with("19") || t.starts_with("20"))
                && let Ok(y) = t.parse::<u32>() {
                    year = Some(y);
                }
            if let Some(bid) = r
                .get("navigationEndpoint")
                .and_then(|n| n.get("browseEndpoint"))
                .and_then(|b| b.get("browseId"))
                .and_then(|id| id.as_str())
                && (bid.starts_with("UC") || bid.contains("artist")) {
                    artists.push(Artist {
                        id: bid.to_string(),
                        name: t.to_string(),
                        cover: None,
                    });
                }
        }
    }

    Some(Album {
        id: browse_id,
        title,
        year,
        cover_uri,
        og_image: None,
        artists,
        track_count: None,
    })
}

/// Парсинг артиста из `musicResponsiveListItemRenderer` или `musicTwoRowItemRenderer`
pub fn parse_artist_item(item: &serde_json::Value) -> Option<Artist> {
    let renderer = if let Some(r) = item.get("musicTwoRowItemRenderer") {
        r
    } else if let Some(r) = item.get("musicResponsiveListItemRenderer") {
        r
    } else {
        item
    };

    let browse_id = find_browse_id(renderer).unwrap_or_default();
    let name = renderer
        .get("title")
        .and_then(extract_text)
        .or_else(|| {
            renderer
                .get("flexColumns")
                .and_then(|fc| fc.get(0))
                .and_then(|c| c.get("musicResponsiveListItemFlexColumnRenderer"))
                .and_then(|c| c.get("text"))
                .and_then(extract_text)
        })?;

    let cover_uri = extract_best_thumbnail(renderer);

    Some(Artist {
        id: browse_id,
        name,
        cover: cover_uri.map(|uri| ArtistCover { uri: Some(uri) }),
    })
}

/// Парсинг плейлиста из `musicResponsiveListItemRenderer` или `musicTwoRowItemRenderer`
pub fn parse_playlist_item(item: &serde_json::Value) -> Option<Playlist> {
    let renderer = if let Some(r) = item.get("musicTwoRowItemRenderer") {
        r
    } else if let Some(r) = item.get("musicResponsiveListItemRenderer") {
        r
    } else {
        item
    };

    let browse_id = find_browse_id(renderer).unwrap_or_default();
    let title = renderer
        .get("title")
        .and_then(extract_text)
        .or_else(|| {
            renderer
                .get("flexColumns")
                .and_then(|fc| fc.get(0))
                .and_then(|c| c.get("musicResponsiveListItemFlexColumnRenderer"))
                .and_then(|c| c.get("text"))
                .and_then(extract_text)
        })
        .unwrap_or_default();

    let og_image = extract_best_thumbnail(renderer);

    let mut track_count = None;
    let subtitle_runs = renderer
        .get("subtitle")
        .and_then(|s| s.get("runs"))
        .and_then(|r| r.as_array());

    if let Some(runs) = subtitle_runs {
        for r in runs {
            let t = r.get("text").and_then(|t| t.as_str()).unwrap_or("").trim();
            // Пример: "100 треков", "25 песен", "50 tracks"
            let num_str: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(cnt) = num_str.parse::<u32>() {
                track_count = Some(cnt);
            }
        }
    }

    Some(Playlist {
        uid: browse_id.clone(),
        kind: browse_id,
        title,
        track_count,
        og_image,
    })
}

/// Извлечение прямого аудиопотока из ответа `/youtubei/v1/player`
pub fn parse_stream_url(json: &serde_json::Value) -> Result<String, String> {
    if let Some(status) = json
        .get("playabilityStatus")
        .and_then(|s| s.get("status"))
        .and_then(|s| s.as_str())
        && status != "OK" {
            let reason = json
                .get("playabilityStatus")
                .and_then(|s| s.get("reason"))
                .and_then(|r| r.as_str())
                .unwrap_or(status);
            return Err(format!("Воспроизведение недоступно: {}", reason));
        }

    let empty_vec = Vec::new();
    let streaming_data = json.get("streamingData");
    let adaptive_formats = streaming_data
        .and_then(|sd| sd.get("adaptiveFormats"))
        .and_then(|af| af.as_array())
        .unwrap_or(&empty_vec);

    let formats = streaming_data
        .and_then(|sd| sd.get("formats"))
        .and_then(|f| f.as_array())
        .unwrap_or(&empty_vec);

    struct Candidate<'a> {
        url: &'a str,
        itag: u64,
        mime_type: &'a str,
        bitrate: u64,
    }

    let mut candidates: Vec<Candidate> = Vec::new();
    for f in adaptive_formats.iter().chain(formats.iter()) {
        if let Some(url) = f.get("url").and_then(|u| u.as_str())
            && !url.is_empty() {
                let itag = f.get("itag").and_then(|i| i.as_u64()).unwrap_or(0);
                let mime_type = f.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
                let bitrate = f
                    .get("bitrate")
                    .and_then(|b| b.as_u64())
                    .or_else(|| f.get("averageBitrate").and_then(|b| b.as_u64()))
                    .unwrap_or(0);
                candidates.push(Candidate {
                    url,
                    itag,
                    mime_type,
                    bitrate,
                });
            }
    }

    if candidates.is_empty() {
        return Err("Не найдено прямых аудиопотоков в ответе InnerTube".to_string());
    }

    // 1. Приоритет: Opus 160 kbps (audio/webm; codecs="opus" / itag 251)
    if let Some(c) = candidates
        .iter()
        .filter(|c| c.itag == 251 || (c.mime_type.contains("audio/webm") && c.mime_type.contains("opus")))
        .max_by_key(|c| c.bitrate)
    {
        return Ok(c.url.to_string());
    }

    // 2. Фолбэк: AAC 128 kbps (audio/mp4; codecs="mp4a.40.2" / itag 140)
    if let Some(c) = candidates
        .iter()
        .filter(|c| c.itag == 140 || (c.mime_type.contains("audio/mp4") && c.mime_type.contains("mp4a")))
        .max_by_key(|c| c.bitrate)
    {
        return Ok(c.url.to_string());
    }

    // 3. Фолбэк: любой аудиопоток
    if let Some(c) = candidates
        .iter()
        .filter(|c| c.mime_type.starts_with("audio/"))
        .max_by_key(|c| c.bitrate)
    {
        return Ok(c.url.to_string());
    }

    // 4. Фолбэк: любой формат с максимальным битрейтом
    if let Some(c) = candidates.iter().max_by_key(|c| c.bitrate) {
        return Ok(c.url.to_string());
    }

    Err("Не удалось выбрать подходящий аудиопоток".to_string())
}

/// Рекурсивный сбор треков очереди радио `playlistPanelVideoRenderer`
fn collect_playlist_panel_videos(json: &serde_json::Value, out: &mut Vec<Track>) {
    if let Some(obj) = json.as_object() {
        if let Some(video_renderer) = obj.get("playlistPanelVideoRenderer") {
            if let Some(track) = parse_playlist_panel_video(video_renderer) {
                out.push(track);
            }
        } else {
            for v in obj.values() {
                collect_playlist_panel_videos(v, out);
            }
        }
    } else if let Some(arr) = json.as_array() {
        for item in arr {
            collect_playlist_panel_videos(item, out);
        }
    }
}

/// Рекурсивный поиск continuation токена для очереди радио или плейлиста
pub fn find_continuation_token(json: &serde_json::Value) -> Option<String> {
    if let Some(obj) = json.as_object() {
        if let Some(token) = obj
            .get("nextContinuationData")
            .and_then(|c| c.get("continuation"))
            .and_then(|t| t.as_str())
        {
            return Some(token.to_string());
        }
        if let Some(token) = obj
            .get("nextRadioContinuationData")
            .and_then(|c| c.get("continuation"))
            .and_then(|t| t.as_str())
        {
            return Some(token.to_string());
        }
        if let Some(token) = obj
            .get("continuationCommand")
            .and_then(|c| c.get("token"))
            .and_then(|t| t.as_str())
        {
            return Some(token.to_string());
        }
        if let Some(token) = obj
            .get("continuationItemRenderer")
            .and_then(|r| r.get("continuationEndpoint"))
            .and_then(|e| e.get("continuationCommand"))
            .and_then(|cmd| cmd.get("token"))
            .and_then(|t| t.as_str())
        {
            return Some(token.to_string());
        }
        for v in obj.values() {
            if let Some(tok) = find_continuation_token(v) {
                return Some(tok);
            }
        }
    } else if let Some(arr) = json.as_array() {
        for item in arr {
            if let Some(tok) = find_continuation_token(item) {
                return Some(tok);
            }
        }
    }
    None
}

/// Парсинг порции треков радио и continuation-токена из ответа `/youtubei/v1/next`
pub fn parse_radio_batch(json: &serde_json::Value) -> (Vec<Track>, Option<String>) {
    let mut tracks = Vec::new();
    collect_playlist_panel_videos(json, &mut tracks);
    tracks.retain(is_clean_music_track);
    let continuation = find_continuation_token(json);
    (tracks, continuation)
}

/// Рекурсивный сбор всех треков из `musicResponsiveListItemRenderer`
fn collect_responsive_tracks(json: &serde_json::Value, out: &mut Vec<Track>) {
    if let Some(obj) = json.as_object() {
        if let Some(item_renderer) = obj.get("musicResponsiveListItemRenderer") {
            if let Some(track) = parse_responsive_track(item_renderer) {
                out.push(track);
            }
        } else {
            for v in obj.values() {
                collect_responsive_tracks(v, out);
            }
        }
    } else if let Some(arr) = json.as_array() {
        for item in arr {
            collect_responsive_tracks(item, out);
        }
    }
}

/// Парсинг понравившихся треков (плейлист `LM` / `VLLM` / `VLSE` / `FEmusic_liked_videos`)
pub fn parse_liked_tracks(json: &serde_json::Value) -> Vec<Track> {
    let mut tracks = Vec::new();

    fn find_playlist_shelf(val: &serde_json::Value) -> Option<&serde_json::Value> {
        if let Some(obj) = val.as_object() {
            if let Some(shelf) = obj
                .get("musicPlaylistShelfRenderer")
                .or_else(|| obj.get("musicPlaylistShelfContinuation"))
                .or_else(|| obj.get("musicShelfContinuation"))
            {
                return Some(shelf);
            }
            for v in obj.values() {
                if let Some(s) = find_playlist_shelf(v) {
                    return Some(s);
                }
            }
        } else if let Some(arr) = val.as_array() {
            for item in arr {
                if let Some(s) = find_playlist_shelf(item) {
                    return Some(s);
                }
            }
        }
        None
    }

    if let Some(shelf) = find_playlist_shelf(json)
        && let Some(contents) = shelf.get("contents").and_then(|c| c.as_array()) {
            for item in contents {
                if let Some(t) = parse_responsive_track(item) {
                    tracks.push(t);
                }
            }
        }

    if tracks.is_empty() {
        collect_responsive_tracks(json, &mut tracks);
    }

    let mut seen = std::collections::HashSet::new();
    tracks.retain(|t| {
        if t.id.is_empty() {
            false
        } else {
            seen.insert(t.id.clone())
        }
    });

    for t in &mut tracks {
        let needs_ytimg = t.cover_uri.as_deref().is_none_or(|u| u.contains("googleusercontent.com"));
        if needs_ytimg && !t.id.is_empty() {
            t.cover_uri = Some(format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", t.id));
        }
    }

    tracks
}

/// Проверяет, является ли элемент системной кнопкой создания нового плейлиста или служебным элементом
fn is_system_playlist_button(title: &str, browse_id: &str) -> bool {
    let t = title.trim().to_lowercase();
    if t == "новый"
        || t == "новый плейлист"
        || t == "new"
        || t == "new playlist"
        || t == "создать плейлист"
        || t == "create playlist"
    {
        return true;
    }
    if browse_id.is_empty()
        || browse_id == "FEmusic_new_playlist"
        || browse_id.contains("create")
        || (!browse_id.starts_with("VL")
            && !browse_id.starts_with("PL")
            && !browse_id.starts_with("RD"))
    {
        return true;
    }
    false
}

/// Рекурсивный сбор плейлистов
fn collect_playlists(json: &serde_json::Value, out: &mut Vec<Playlist>) {
    if let Some(obj) = json.as_object() {
        if let Some(item) = obj.get("musicTwoRowItemRenderer") {
            let bid = find_browse_id(item).unwrap_or_default();
            let title = item.get("title").and_then(extract_text).unwrap_or_default();
            if !is_system_playlist_button(&title, &bid)
                && let Some(pl) = parse_playlist_item(item) {
                    out.push(pl);
                }
        } else if let Some(item) = obj.get("musicResponsiveListItemRenderer") {
            let bid = find_browse_id(item).unwrap_or_default();
            let title = item
                .get("flexColumns")
                .and_then(|fc| fc.get(0))
                .and_then(|c| c.get("musicResponsiveListItemFlexColumnRenderer"))
                .and_then(|c| c.get("text"))
                .and_then(extract_text)
                .unwrap_or_default();
            if !is_system_playlist_button(&title, &bid)
                && let Some(pl) = parse_playlist_item(item) {
                    out.push(pl);
                }
        } else {
            for v in obj.values() {
                collect_playlists(v, out);
            }
        }
    } else if let Some(arr) = json.as_array() {
        for item in arr {
            collect_playlists(item, out);
        }
    }
}

/// Парсинг списка плейлистов из ответа browse (`FEmusic_liked_playlists` / `FEmusic_library_playlists`)
pub fn parse_playlists(json: &serde_json::Value) -> Vec<Playlist> {
    let mut playlists = Vec::new();
    collect_playlists(json, &mut playlists);

    // Дедупликация плейлистов по uid
    let mut seen = std::collections::HashSet::new();
    playlists.retain(|p| {
        if p.uid.is_empty() {
            false
        } else {
            seen.insert(p.uid.clone())
        }
    });

    playlists
}

/// Декодирует JavaScript строковый литерал из `data: '...'` (unescape hex-кодов `\xHH`, экранированных слэшей и кавычек)
pub fn unescape_js_string(s: &str) -> String {
    let bytes = s.as_bytes();
    let n = bytes.len();
    let mut out = Vec::with_capacity(n);
    let mut i = 0;

    let parse_hex = |b: u8| -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    };

    while i < n {
        if bytes[i] == b'\\' && i + 1 < n {
            match bytes[i + 1] {
                b'x' if i + 3 < n => {
                    let h1 = bytes[i + 2];
                    let h2 = bytes[i + 3];
                    if let (Some(v1), Some(v2)) = (parse_hex(h1), parse_hex(h2)) {
                        out.push((v1 << 4) | v2);
                        i += 4;
                        continue;
                    }
                }
                b'\'' => {
                    out.push(b'\'');
                    i += 2;
                    continue;
                }
                b'\\' => {
                    out.push(b'\\');
                    i += 2;
                    continue;
                }
                b'/' => {
                    out.push(b'/');
                    i += 2;
                    continue;
                }
                b'n' => {
                    out.push(b'\n');
                    i += 2;
                    continue;
                }
                b'r' => {
                    out.push(b'\r');
                    i += 2;
                    continue;
                }
                b't' => {
                    out.push(b'\t');
                    i += 2;
                    continue;
                }
                b'"' => {
                    out.push(b'"');
                    i += 2;
                    continue;
                }
                _ => {
                    out.push(b'\\');
                    out.push(bytes[i + 1]);
                    i += 2;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }

    String::from_utf8_lossy(&out).into_owned()
}

/// Извлечение строкового литерала JavaScript в одинарных или двойных кавычках
fn extract_js_string_literal(slice: &str) -> Option<&str> {
    let bytes = slice.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    while i < n && (bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\r' || bytes[i] == b'\n') {
        i += 1;
    }
    if i >= n {
        return None;
    }
    let quote = bytes[i];
    if quote != b'\'' && quote != b'"' {
        return None;
    }
    let start = i + 1;
    let mut j = start;
    let mut escaped = false;
    while j < n {
        if escaped {
            escaped = false;
        } else if bytes[j] == b'\\' {
            escaped = true;
        } else if bytes[j] == quote {
            return Some(&slice[start..j]);
        }
        j += 1;
    }
    None
}

/// Извлечение значения поля (например videoId) из HTML фрагмента
fn extract_field_value(chunk: &str, field: &str) -> Option<String> {
    for prefix in &[
        format!("\\x22{field}\\x22:\\x22"),
        format!("\"{field}\":\""),
        format!("\"{field}\": \""),
        format!("{field}\\x22:\\x22"),
    ] {
        if let Some(pos) = chunk.find(prefix.as_str()) {
            let start = pos + prefix.len();
            let tail = &chunk[start..];
            let end = tail.find(['"', '\\', '\''])
                .unwrap_or(tail.len());
            let val = &tail[..end];
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

/// Извлечение названия трека и исполнителя из строки accessibility label
fn extract_track_label(chunk: &str) -> Option<(String, String)> {
    for prefix in &[
        "\\x22label\\x22:\\x22",
        "\"label\":\"",
        "\"label\": \"",
        "label\\x22:\\x22",
    ] {
        if let Some(pos) = chunk.find(prefix) {
            let start = pos + prefix.len();
            let tail = &chunk[start..];
            let end = tail.find(['"', '\\', '\''])
                .unwrap_or(tail.len());
            let label = &tail[..end];

            // Формат label: "Happy Song (Official Audio) - Bring Me The Horizon - 3 минуты 57 секунд – воспроизвести"
            let text_before = if let Some(p) = label.find("– воспроизвести") {
                &label[..p]
            } else if let Some(p) = label.find("- воспроизвести") {
                &label[..p]
            } else if let Some(p) = label.find("– play") {
                &label[..p]
            } else if let Some(p) = label.find("- play") {
                &label[..p]
            } else {
                label
            };

            let parts: Vec<&str> = text_before.split(" - ").collect();
            if parts.len() >= 2 {
                let title = parts[0].trim().to_string();
                let artist = parts[1].trim().to_string();
                if !title.is_empty() && !artist.is_empty() {
                    return Some((title, artist));
                }
            } else {
                let parts: Vec<&str> = text_before.split('-').collect();
                if parts.len() >= 2 {
                    let title = parts[0].trim().to_string();
                    let artist = parts[1].trim().to_string();
                    if !title.is_empty() && !artist.is_empty() {
                        return Some((title, artist));
                    }
                }
            }
        }
    }
    None
}

/// Извлечение continuation токена из сырого HTML в качестве фолбэка
fn extract_continuation_from_raw_html(html: &str) -> Option<String> {
    for needle in &["nextContinuationData", "\\x22nextContinuationData\\x22"] {
        if let Some(pos) = html.find(needle) {
            let tail = &html[pos..];
            if let Some(cont_pos) = tail.find("continuation") {
                let after = &tail[cont_pos + 12..];
                let mut chars = after.chars().skip_while(|c| *c != '"' && *c != '\'');
                if let Some(quote) = chars.next() {
                    let val: String = chars.take_while(|c| *c != quote && *c != '\\').collect();
                    if !val.is_empty() && val.len() > 10 {
                        return Some(val);
                    }
                }
            }
        }
    }
    None
}

/// Извлекает общее количество треков плейлиста из заголовка (например, "325 треков" -> 325)
pub fn parse_header_track_count(json: &serde_json::Value) -> Option<u32> {
    fn search_subtitle(val: &serde_json::Value) -> Option<u32> {
        if let Some(obj) = val.as_object() {
            for key in ["secondSubtitle", "subtitle"] {
                if let Some(sub) = obj.get(key)
                    && let Some(runs) = sub.get("runs").and_then(|r| r.as_array()) {
                        for r in runs {
                            if let Some(text) = r.get("text").and_then(|t| t.as_str()) {
                                let clean = text.replace('\u{a0}', " ");
                                let num_str: String = clean.chars().take_while(|c| c.is_ascii_digit()).collect();
                                if let Ok(n) = num_str.parse::<u32>()
                                    && n > 0 && (clean.contains("трек") || clean.contains("песн") || clean.contains("track") || clean.contains("song")) {
                                        return Some(n);
                                    }
                            }
                        }
                    }
            }
            for v in obj.values() {
                if let Some(n) = search_subtitle(v) {
                    return Some(n);
                }
            }
        } else if let Some(arr) = val.as_array() {
            for it in arr {
                if let Some(n) = search_subtitle(it) {
                    return Some(n);
                }
            }
        }
        None
    }

    search_subtitle(json)
}

/// Извлекает общее количество треков плейлиста из HTML страницы
pub fn parse_header_track_count_from_html(html: &str) -> Option<u32> {
    // 1. Поиск в initialData JSON
    let push_needle = "initialData.push(";
    let mut search_pos = 0;
    while let Some(pos) = html[search_pos..].find(push_needle) {
        let abs_pos = search_pos + pos + push_needle.len();
        let tail = &html[abs_pos..];
        if let Some(path_pos) = tail.find("path:")
            && path_pos < 120 {
                let path_tail = &tail[path_pos + 5..];
                if let Some(path_str) = extract_js_string_literal(path_tail)
                    && (path_str == "/browse" || path_str == "\\/browse")
                        && let Some(data_pos) = tail.find("data:") {
                            let data_tail = &tail[data_pos + 5..];
                            if let Some(raw_data) = extract_js_string_literal(data_tail) {
                                let unescaped = unescape_js_string(raw_data);
                                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&unescaped)
                                    && let Some(cnt) = parse_header_track_count(&json) {
                                        return Some(cnt);
                                    }
                            }
                        }
            }
        search_pos = abs_pos;
    }

    // 2. Fallback поиск по строкам заголовка в HTML
    for needle in &["secondSubtitle", "musicResponsiveHeaderRenderer", "header"] {
        if let Some(pos) = html.find(needle) {
            let chunk = &html[pos..html.len().min(pos + 3000)];
            let clean_chunk = chunk
                .replace("\\x22", "\"")
                .replace("\\\"", "\"")
                .replace('\u{a0}', " ");
            for word in ["трек", "песн", "track", "song"] {
                if let Some(w_pos) = clean_chunk.find(word) {
                    let prefix = &clean_chunk[..w_pos];
                    let tokens: Vec<&str> = prefix
                        .split(|c: char| !c.is_ascii_digit())
                        .filter(|s| !s.is_empty())
                        .collect();
                    if let Some(last_digits) = tokens.last()
                        && let Ok(n) = last_digits.parse::<u32>()
                            && n > 0 && n < 100_000 {
                                return Some(n);
                            }
                }
            }
        }
    }

    None
}

/// Парсинг понравившихся треков напрямую из HTML страницы YouTube Music (`https://music.youtube.com/playlist?list=LM`)
pub fn parse_tracks_from_html(html: &str) -> Vec<Track> {
    parse_tracks_and_continuation_from_html(html).0
}

/// Парсинг понравившихся треков и continuation токена напрямую из HTML страницы YouTube Music
pub fn parse_tracks_and_continuation_from_html(html: &str) -> (Vec<Track>, Option<String>) {
    let mut tracks = Vec::new();
    let mut continuation_token = None;
    let mut seen_ids = std::collections::HashSet::new();

    // 1. Поиск initialData.push с path: '/browse'
    let push_needle = "initialData.push(";
    let mut search_pos = 0;
    while let Some(pos) = html[search_pos..].find(push_needle) {
        let abs_pos = search_pos + pos + push_needle.len();
        let tail = &html[abs_pos..];
        if let Some(path_pos) = tail.find("path:")
            && path_pos < 120 {
                let path_tail = &tail[path_pos + 5..];
                if let Some(path_str) = extract_js_string_literal(path_tail)
                    && (path_str == "/browse" || path_str == "\\/browse")
                        && let Some(data_pos) = tail.find("data:") {
                            let data_tail = &tail[data_pos + 5..];
                            if let Some(raw_data) = extract_js_string_literal(data_tail) {
                                let unescaped = unescape_js_string(raw_data);
                                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&unescaped) {
                                    if continuation_token.is_none() {
                                        continuation_token = find_continuation_token(&json);
                                    }
                                    let parsed_tracks = parse_liked_tracks(&json);
                                    for t in parsed_tracks {
                                        if !t.id.is_empty() && seen_ids.insert(t.id.clone()) {
                                            tracks.push(t);
                                        }
                                    }
                                }
                            }
                        }
            }
        search_pos = abs_pos;
    }

    // 2. Дополнительный / fallback regex парсер HTML
    // Ищем блоки musicResponsiveListItemRenderer
    for delimiter in &["musicResponsiveListItemRenderer", "\\x22musicResponsiveListItemRenderer\\x22"] {
        let chunks: Vec<&str> = html.split(delimiter).collect();
        if chunks.len() > 1 {
            for chunk in &chunks[1..] {
                if let Some(id) = extract_field_value(chunk, "videoId")
                    && id.len() == 11 && !seen_ids.contains(&id)
                        && let Some((title, artist)) = extract_track_label(chunk) {
                            seen_ids.insert(id.clone());
                            tracks.push(Track {
                                id: id.clone(),
                                title,
                                version: None,
                                artists: vec![Artist {
                                    id: String::new(),
                                    name: artist,
                                    cover: None,
                                }],
                                albums: Vec::new(),
                                duration_ms: None,
                                cover_uri: Some(format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg")),
                                og_image: None,
                                available: Some(true),
                                content_warning: None,
                                derived_colors: None,
                                source: TrackSource::YouTubeMusic,
                            });
                        }
            }
            if !tracks.is_empty() {
                break;
            }
        }
    }

    // 3. Если continuation токен не найден в parsed JSON, пробуем извлечь из HTML
    if continuation_token.is_none() {
        continuation_token = extract_continuation_from_raw_html(html);
    }

    // Гарантируем качественные обложки для всех найденных треков
    for t in &mut tracks {
        let needs_ytimg = t.cover_uri.as_deref().is_none_or(|u| u.contains("googleusercontent.com"));
        if needs_ytimg && !t.id.is_empty() {
            t.cover_uri = Some(format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", t.id));
        }
    }

    (tracks, continuation_token)
}

/// Рекурсивный сбор плейлистов из guide или browse JSON
fn collect_playlists_from_json(
    val: &serde_json::Value,
    out: &mut Vec<Playlist>,
    seen: &mut std::collections::HashSet<String>,
) {
    if let Some(obj) = val.as_object() {
        if let Some(r) = obj.get("guideEntryRenderer") {
            let title = r
                .get("formattedTitle")
                .and_then(|ft| ft.get("runs"))
                .and_then(|runs| runs.as_array())
                .and_then(|arr| arr.first())
                .and_then(|item| item.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .trim();

            let browse_id = r
                .get("navigationEndpoint")
                .and_then(|ne| ne.get("browseEndpoint"))
                .and_then(|be| be.get("browseId"))
                .and_then(|b| b.as_str())
                .unwrap_or("");

            if (browse_id.starts_with("VLPL") || browse_id.starts_with("PL") || (browse_id.starts_with("VL") && browse_id != "VLLM" && browse_id != "VLSE"))
                && !is_system_playlist_button(title, browse_id)
                && !title.is_empty()
                && seen.insert(browse_id.to_string())
            {
                out.push(Playlist {
                    uid: browse_id.to_string(),
                    kind: browse_id.to_string(),
                    title: title.to_string(),
                    track_count: None,
                    og_image: None,
                });
            }
        }

        if let Some(item) = obj.get("musicTwoRowItemRenderer") {
            let bid = find_browse_id(item).unwrap_or_default();
            let title = item.get("title").and_then(extract_text).unwrap_or_default();
            if (bid.starts_with("VLPL") || bid.starts_with("PL") || (bid.starts_with("VL") && bid != "VLLM" && bid != "VLSE"))
                && !is_system_playlist_button(&title, &bid)
                && seen.insert(bid.clone())
            {
                if let Some(pl) = parse_playlist_item(item) {
                    out.push(pl);
                } else if !title.is_empty() {
                    out.push(Playlist {
                        uid: bid.clone(),
                        kind: bid,
                        title,
                        track_count: None,
                        og_image: None,
                    });
                }
            }
        }

        for v in obj.values() {
            collect_playlists_from_json(v, out, seen);
        }
    } else if let Some(arr) = val.as_array() {
        for item in arr {
            collect_playlists_from_json(item, out, seen);
        }
    }
}

/// Парсинг плейлистов пользователя из HTML страницы YouTube Music (`/library/playlists`, `/feed/library`, `/playlist?list=LM`)
pub fn parse_playlists_from_html(html: &str) -> Vec<Playlist> {
    let mut playlists = Vec::new();
    let mut seen_bids = std::collections::HashSet::new();

    // 1. Поиск initialData.push (включая /guide и /browse)
    let push_needle = "initialData.push(";
    let mut search_pos = 0;
    while let Some(pos) = html[search_pos..].find(push_needle) {
        let abs_pos = search_pos + pos + push_needle.len();
        let tail = &html[abs_pos..];
        if let Some(data_pos) = tail.find("data:") {
            let data_tail = &tail[data_pos + 5..];
            if let Some(raw_data) = extract_js_string_literal(data_tail) {
                let unescaped = unescape_js_string(raw_data);
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&unescaped) {
                    collect_playlists_from_json(&json, &mut playlists, &mut seen_bids);
                }
            }
        }
        search_pos = abs_pos;
    }

    // 2. Fallback парсер по строкам HTML для поиска плейлистов VLPL... / PL...
    if playlists.is_empty() {
        let mut pos = 0;
        while let Some(idx) = html[pos..].find("VLPL") {
            let abs_idx = pos + idx;
            let tail = &html[abs_idx..];
            let end = tail.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
                .unwrap_or(tail.len());
            let bid = &tail[..end];
            if bid.len() >= 10 && seen_bids.insert(bid.to_string()) {
                // Ищем заголовок в окрестности 300 символов до bid
                let window_start = abs_idx.saturating_sub(300);
                let window = &html[window_start..abs_idx];
                let title = extract_field_value(window, "text")
                    .or_else(|| extract_field_value(window, "title"))
                    .unwrap_or_else(|| "Плейлист".to_string());
                if !is_system_playlist_button(&title, bid) {
                    playlists.push(Playlist {
                        uid: bid.to_string(),
                        kind: bid.to_string(),
                        title,
                        track_count: None,
                        og_image: None,
                    });
                }
            }
            pos = abs_idx + 4;
        }
    }

    playlists
}

/// Рекурсивный сбор альбомов
fn collect_albums(json: &serde_json::Value, out: &mut Vec<Album>) {
    if let Some(obj) = json.as_object() {
        if let Some(item) = obj.get("musicTwoRowItemRenderer") {
            let bid = find_browse_id(item).unwrap_or_default();
            if (bid.starts_with("MPRE") || bid.starts_with("FEmusic_library_album"))
                && let Some(al) = parse_album_item(item) {
                    out.push(al);
                }
        } else if let Some(item) = obj.get("musicResponsiveListItemRenderer") {
            let bid = find_browse_id(item).unwrap_or_default();
            if (bid.starts_with("MPRE") || bid.starts_with("FEmusic_library_album"))
                && let Some(al) = parse_album_item(item) {
                    out.push(al);
                }
        } else {
            for v in obj.values() {
                collect_albums(v, out);
            }
        }
    } else if let Some(arr) = json.as_array() {
        for item in arr {
            collect_albums(item, out);
        }
    }
}

/// Парсинг списка альбомов из ответа browse (`FEmusic_liked_albums`)
pub fn parse_albums(json: &serde_json::Value) -> Vec<Album> {
    let mut albums = Vec::new();
    collect_albums(json, &mut albums);
    albums
}

/// Извлечение текста подписи (subtitle) из объекта элемента или карточки
fn extract_subtitle_text(val: &serde_json::Value) -> String {
    if let Some(sub) = val.get("subtitle")
        && let Some(t) = extract_text(sub) {
            return t;
        }
    if let Some(fc) = val.get("flexColumns").and_then(|f| f.as_array())
        && let Some(col1) = fc.get(1)
            && let Some(t) = col1
                .get("musicResponsiveListItemFlexColumnRenderer")
                .and_then(|c| c.get("text"))
                .and_then(extract_text)
            {
                return t;
            }
    if let Some(header) = val.get("header").and_then(|h| h.get("musicCardShelfHeaderBasicRenderer"))
        && let Some(sub) = header.get("subtitle")
            && let Some(t) = extract_text(sub) {
                return t;
            }
    String::new()
}

/// Парсинг карточки `musicCardShelfRenderer`
fn parse_card_shelf(
    card: &serde_json::Value,
    artists: &mut Vec<Artist>,
    albums: &mut Vec<Album>,
    playlists: &mut Vec<Playlist>,
    tracks: &mut Vec<Track>,
) {
    // 1. Извлекаем прикрепленные треки из card.get("contents")
    if let Some(contents) = card.get("contents").and_then(|c| c.as_array()) {
        for it in contents {
            if let Some(t) = parse_responsive_track(it) {
                tracks.push(t);
            }
        }
    }

    // 2. Извлекаем артиста/альбом/плейлист из карточки (заголовок, browseId, обложка)
    let title = card
        .get("title")
        .and_then(extract_text)
        .or_else(|| {
            card.get("header")
                .and_then(|h| h.get("musicCardShelfHeaderBasicRenderer"))
                .and_then(|h| h.get("title"))
                .and_then(extract_text)
        });

    let title = match title {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => return,
    };

    let browse_id = find_browse_id(card).unwrap_or_default();
    let cover_uri = extract_best_thumbnail(card);
    let subtitle = extract_subtitle_text(card).to_lowercase();

    if subtitle.contains("исполнитель") || subtitle.contains("artist") || browse_id.starts_with("UC") {
        artists.push(Artist {
            id: browse_id,
            name: title,
            cover: cover_uri.map(|uri| ArtistCover { uri: Some(uri) }),
        });
    } else if subtitle.contains("альбом")
        || subtitle.contains("album")
        || subtitle.contains("сингл")
        || subtitle.contains("single")
        || subtitle.contains("ep")
        || browse_id.starts_with("MPRE")
    {
        let year = subtitle.split_whitespace().find_map(|w| {
            let digits: String = w.chars().filter(|c| c.is_ascii_digit()).collect();
            if digits.len() == 4 && (digits.starts_with("19") || digits.starts_with("20")) {
                digits.parse::<u32>().ok()
            } else {
                None
            }
        });
        albums.push(Album {
            id: browse_id,
            title: Some(title),
            year,
            cover_uri,
            og_image: None,
            artists: Vec::new(),
            track_count: None,
        });
    } else if subtitle.contains("плейлист")
        || subtitle.contains("playlist")
        || browse_id.starts_with("VL")
        || browse_id.starts_with("PL")
    {
        playlists.push(Playlist {
            uid: browse_id.clone(),
            kind: browse_id,
            title,
            track_count: None,
            og_image: cover_uri,
        });
    } else if let Some(t) = parse_responsive_track(card) {
        tracks.push(t);
    }
}

/// Парсинг `musicResponsiveListItemRenderer` в результатах поиска
fn parse_responsive_search_item(
    item: &serde_json::Value,
    tracks: &mut Vec<Track>,
    albums: &mut Vec<Album>,
    artists: &mut Vec<Artist>,
    playlists: &mut Vec<Playlist>,
) {
    let renderer = item.get("musicResponsiveListItemRenderer").unwrap_or(item);

    // Если есть videoId -> парси как Track
    if find_video_id(renderer).is_some()
        && let Some(track) = parse_responsive_track(renderer) {
            tracks.push(track);
            return;
        }

    // Если videoId нет -> анализируй subtitle и browseId
    let browse_id = find_browse_id(renderer).unwrap_or_default();
    let sub = extract_subtitle_text(renderer).to_lowercase();

    if (sub.contains("исполнитель") || sub.contains("artist") || browse_id.starts_with("UC"))
        && let Some(art) = parse_artist_item(renderer) {
            artists.push(art);
            return;
        }

    if (sub.contains("альбом")
        || sub.contains("album")
        || sub.contains("сингл")
        || sub.contains("single")
        || sub.contains("ep")
        || browse_id.starts_with("MPRE"))
        && let Some(alb) = parse_album_item(renderer) {
            albums.push(alb);
            return;
        }

    if (sub.contains("плейлист")
        || sub.contains("playlist")
        || browse_id.starts_with("VL")
        || browse_id.starts_with("PL"))
        && let Some(pl) = parse_playlist_item(renderer) {
            playlists.push(pl);
        }
}

/// Парсинг результатов поиска InnerTube (`/youtubei/v1/search`)
pub fn parse_search_results(json: &serde_json::Value) -> SearchResult {
    let mut tracks = Vec::new();
    let mut albums = Vec::new();
    let mut artists = Vec::new();
    let mut playlists = Vec::new();

    fn collect_search_elements(
        val: &serde_json::Value,
        tracks: &mut Vec<Track>,
        albums: &mut Vec<Album>,
        artists: &mut Vec<Artist>,
        playlists: &mut Vec<Playlist>,
    ) {
        if let Some(obj) = val.as_object() {
            if let Some(card) = obj.get("musicCardShelfRenderer") {
                parse_card_shelf(card, artists, albums, playlists, tracks);
                return;
            }
            if let Some(item) = obj.get("musicResponsiveListItemRenderer") {
                parse_responsive_search_item(item, tracks, albums, artists, playlists);
                return;
            }
            for v in obj.values() {
                collect_search_elements(v, tracks, albums, artists, playlists);
            }
        } else if let Some(arr) = val.as_array() {
            for item in arr {
                collect_search_elements(item, tracks, albums, artists, playlists);
            }
        }
    }

    collect_search_elements(json, &mut tracks, &mut albums, &mut artists, &mut playlists);

    // Фильтрация треков: отсечение подкастов, разговоров и видео-мусора
    tracks.retain(is_clean_music_track);

    // Дедупликация результатов по id трека, альбома, артиста, плейлиста
    let mut seen_tracks = std::collections::HashSet::new();
    tracks.retain(|t| {
        if t.id.is_empty() {
            true
        } else {
            seen_tracks.insert(t.id.clone())
        }
    });

    let mut seen_albums = std::collections::HashSet::new();
    albums.retain(|a| {
        let key = if !a.id.is_empty() {
            a.id.clone()
        } else {
            a.title.clone().unwrap_or_default()
        };
        if key.is_empty() {
            true
        } else {
            seen_albums.insert(key)
        }
    });

    let mut seen_artists = std::collections::HashSet::new();
    artists.retain(|a| {
        let key = if !a.id.is_empty() {
            a.id.clone()
        } else {
            a.name.clone()
        };
        if key.is_empty() {
            true
        } else {
            seen_artists.insert(key)
        }
    });

    let mut seen_playlists = std::collections::HashSet::new();
    playlists.retain(|p| {
        let key = if !p.uid.is_empty() {
            p.uid.clone()
        } else {
            p.title.clone()
        };
        if key.is_empty() {
            true
        } else {
            seen_playlists.insert(key)
        }
    });

    SearchResult {
        tracks: if tracks.is_empty() {
            None
        } else {
            Some(SearchSection { results: tracks })
        },
        albums: if albums.is_empty() {
            None
        } else {
            Some(SearchSection { results: albums })
        },
        playlists: if playlists.is_empty() {
            None
        } else {
            Some(SearchSection { results: playlists })
        },
        artists: if artists.is_empty() {
            None
        } else {
            Some(SearchSection { results: artists })
        },
    }
}

/// Поиск заголовка артиста (musicImmersiveHeaderRenderer, musicVisualHeaderRenderer, musicResponsiveHeaderRenderer и др.)
fn find_artist_header(val: &serde_json::Value) -> Option<&serde_json::Value> {
    if let Some(h) = val.get("header") {
        if let Some(r) = h
            .get("musicImmersiveHeaderRenderer")
            .or_else(|| h.get("musicVisualHeaderRenderer"))
            .or_else(|| h.get("musicResponsiveHeaderRenderer"))
            .or_else(|| h.get("musicDetailHeaderRenderer"))
            .or_else(|| h.get("musicHeaderRenderer"))
        {
            return Some(r);
        }
        return Some(h);
    }
    fn deep_search(v: &serde_json::Value) -> Option<&serde_json::Value> {
        if let Some(obj) = v.as_object() {
            if let Some(r) = obj
                .get("musicImmersiveHeaderRenderer")
                .or_else(|| obj.get("musicVisualHeaderRenderer"))
                .or_else(|| obj.get("musicResponsiveHeaderRenderer"))
                .or_else(|| obj.get("musicDetailHeaderRenderer"))
            {
                return Some(r);
            }
            for val in obj.values() {
                if let Some(res) = deep_search(val) {
                    return Some(res);
                }
            }
        } else if let Some(arr) = v.as_array() {
            for val in arr {
                if let Some(res) = deep_search(val) {
                    return Some(res);
                }
            }
        }
        None
    }
    deep_search(val)
}

/// Парсинг страницы артиста из ответа `/youtubei/v1/browse`
pub fn parse_artist_browse(
    json: &serde_json::Value,
    browse_id: &str,
) -> Result<ArtistBrief, Box<dyn std::error::Error + Send + Sync>> {
    let header_obj = find_artist_header(json);

    let mut artist_name = header_obj
        .and_then(|h| h.get("title"))
        .and_then(extract_text)
        .unwrap_or_default();

    // Извлекаем аватар артиста: ищем строго квадратный thumbnail, отсекаем широкоформатные баннеры (w > h * 1.3)
    let mut cover_uri = None;
    if let Some(h) = header_obj
        && let Some(fg) = h.get("foregroundThumbnail") {
            cover_uri = extract_square_thumbnail(fg);
        }
    if cover_uri.is_none()
        && let Some(h) = json.get("header")
            && let Some(fg) = h.get("foregroundThumbnail") {
                cover_uri = extract_square_thumbnail(fg);
            }
    if cover_uri.is_none() {
        cover_uri = header_obj.and_then(extract_square_thumbnail);
    }
    if cover_uri.is_none()
        && let Some(h) = json.get("header") {
            cover_uri = extract_square_thumbnail(h);
        }

    // Извлекаем треки из первой полки `musicShelfRenderer` («Топ треков» / «Songs»)
    let mut popular_tracks = Vec::new();
    fn find_first_music_shelf(val: &serde_json::Value) -> Option<&serde_json::Value> {
        if let Some(obj) = val.as_object() {
            if let Some(shelf) = obj.get("musicShelfRenderer") {
                return Some(shelf);
            }
            for v in obj.values() {
                if let Some(shelf) = find_first_music_shelf(v) {
                    return Some(shelf);
                }
            }
        } else if let Some(arr) = val.as_array() {
            for item in arr {
                if let Some(shelf) = find_first_music_shelf(item) {
                    return Some(shelf);
                }
            }
        }
        None
    }

    if let Some(shelf) = find_first_music_shelf(json)
        && let Some(contents) = shelf.get("contents").and_then(|c| c.as_array()) {
            for it in contents {
                if let Some(mut track) = parse_responsive_track(it) {
                    let is_ytimg = track.cover_uri.as_deref().is_some_and(|u| u.starts_with("https://i.ytimg.com/vi/"));
                    if (track.cover_uri.is_none() || is_ytimg) && cover_uri.is_some() {
                        track.cover_uri = cover_uri.clone();
                    }
                    popular_tracks.push(track);
                }
            }
        }

    // Если имя артиста не нашлось в header, берем первого исполнителя из первого трека
    if artist_name.is_empty()
        && let Some(first_track) = popular_tracks.first()
            && let Some(first_artist) = first_track.artists.first() {
                artist_name = first_artist.name.clone();
            }

    // Извлекаем альбомы из полок `musicCarouselShelfRenderer`
    let mut albums = Vec::new();
    fn collect_carousel_albums(val: &serde_json::Value, out: &mut Vec<Album>) {
        if let Some(obj) = val.as_object() {
            if let Some(carousel) = obj.get("musicCarouselShelfRenderer") {
                if let Some(contents) = carousel.get("contents").and_then(|c| c.as_array()) {
                    for it in contents {
                        if let Some(album) = parse_album_item(it)
                            && (!album.id.is_empty() || album.title.is_some()) {
                                out.push(album);
                            }
                    }
                }
            } else {
                for v in obj.values() {
                    collect_carousel_albums(v, out);
                }
            }
        } else if let Some(arr) = val.as_array() {
            for item in arr {
                collect_carousel_albums(item, out);
            }
        }
    }
    collect_carousel_albums(json, &mut albums);

    // Дедупликация альбомов
    let mut seen_albums = std::collections::HashSet::new();
    albums.retain(|a| {
        let key = if !a.id.is_empty() {
            a.id.clone()
        } else {
            a.title.clone().unwrap_or_default()
        };
        if key.is_empty() {
            true
        } else {
            seen_albums.insert(key)
        }
    });

    let artist_cover = cover_uri.as_ref().map(|u| ArtistCover { uri: Some(u.clone()) });
    let all_covers = cover_uri.into_iter().map(|u| ArtistCover { uri: Some(u) }).collect();

    Ok(ArtistBrief {
        artist: Artist {
            id: browse_id.to_string(),
            name: artist_name,
            cover: artist_cover,
        },
        albums,
        popular_tracks,
        stats: None,
        all_covers,
        background_video_url: None,
    })
}

/// Поиск заголовка альбома (musicResponsiveHeaderRenderer, musicDetailHeaderRenderer и др.)
fn find_album_header(val: &serde_json::Value) -> Option<&serde_json::Value> {
    if let Some(h) = val.get("header")
        && let Some(r) = h
            .get("musicResponsiveHeaderRenderer")
            .or_else(|| h.get("musicDetailHeaderRenderer"))
            .or_else(|| h.get("musicImmersiveHeaderRenderer"))
            .or_else(|| h.get("musicVisualHeaderRenderer"))
            .or_else(|| h.get("musicHeaderRenderer"))
        {
            return Some(r);
        }
    fn deep_search(v: &serde_json::Value) -> Option<&serde_json::Value> {
        if let Some(obj) = v.as_object() {
            if let Some(r) = obj
                .get("musicResponsiveHeaderRenderer")
                .or_else(|| obj.get("musicDetailHeaderRenderer"))
                .or_else(|| obj.get("musicImmersiveHeaderRenderer"))
                .or_else(|| obj.get("musicVisualHeaderRenderer"))
            {
                return Some(r);
            }
            for val in obj.values() {
                if let Some(res) = deep_search(val) {
                    return Some(res);
                }
            }
        } else if let Some(arr) = v.as_array() {
            for val in arr {
                if let Some(res) = deep_search(val) {
                    return Some(res);
                }
            }
        }
        None
    }
    if let Some(res) = deep_search(val) {
        return Some(res);
    }
    val.get("header")
}

/// Парсинг треков альбома из ответа `/youtubei/v1/browse`
pub fn parse_album_tracks(json: &serde_json::Value) -> Vec<Track> {
    let header_obj = find_album_header(json);
    let album_cover = header_obj.and_then(extract_best_thumbnail);

    let mut tracks = Vec::new();
    collect_responsive_tracks(json, &mut tracks);

    if let Some(cover) = album_cover {
        for t in &mut tracks {
            let is_ytimg = t.cover_uri.as_deref().is_some_and(|u| u.starts_with("https://i.ytimg.com/vi/"));
            if t.cover_uri.is_none() || is_ytimg {
                t.cover_uri = Some(cover.clone());
            }
        }
    }

    // Дедупликация треков по id
    let mut seen = std::collections::HashSet::new();
    tracks.retain(|t| {
        if t.id.is_empty() {
            true
        } else {
            seen.insert(t.id.clone())
        }
    });

    tracks
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_duration_ms() {
        assert_eq!(parse_duration_ms("3:45"), Some(225_000));
        assert_eq!(parse_duration_ms("0:45"), Some(45_000));
        assert_eq!(parse_duration_ms("1:02:30"), Some(3_750_000));
        assert_eq!(parse_duration_ms("45"), Some(45_000));
        assert_eq!(parse_duration_ms("invalid"), None);
    }

    #[test]
    fn test_extract_best_thumbnail() {
        let val = json!({
            "thumbnails": [
                { "url": "//example.com/small.jpg", "width": 60, "height": 60 },
                { "url": "https://example.com/large.jpg", "width": 540, "height": 540 },
                { "url": "https://example.com/med.jpg", "width": 240, "height": 240 }
            ]
        });
        assert_eq!(
            extract_best_thumbnail(&val),
            Some("https://example.com/large.jpg".to_string())
        );

        let proto_val = json!([
            { "url": "//example.com/test.jpg", "width": 100 }
        ]);
        assert_eq!(
            extract_best_thumbnail(&proto_val),
            Some("https://example.com/test.jpg".to_string())
        );

        // Вложенный путь musicResponsiveListItemRenderer -> thumbnail -> musicThumbnailRenderer -> thumbnail -> thumbnails
        let nested_responsive = json!({
            "thumbnail": {
                "musicThumbnailRenderer": {
                    "thumbnail": {
                        "thumbnails": [
                            { "url": "https://lh3.googleusercontent.com/small.jpg", "width": 60, "height": 60 },
                            { "url": "https://lh3.googleusercontent.com/best.jpg", "width": 544, "height": 544 }
                        ]
                    }
                }
            }
        });
        assert_eq!(
            extract_best_thumbnail(&nested_responsive),
            Some("https://lh3.googleusercontent.com/best.jpg".to_string())
        );

        // Прямой musicThumbnailRenderer
        let direct_mtr = json!({
            "thumbnail": {
                "thumbnails": [
                    { "url": "https://yt3.googleusercontent.com/avatar.jpg", "width": 300, "height": 300 }
                ]
            }
        });
        assert_eq!(
            extract_best_thumbnail(&direct_mtr),
            Some("https://yt3.googleusercontent.com/avatar.jpg".to_string())
        );
    }

    #[test]
    fn test_parse_artist_browse() {
        let json = json!({
            "header": {
                "musicImmersiveHeaderRenderer": {
                    "title": { "runs": [{ "text": "Король и Шут" }] },
                    "thumbnail": {
                        "musicThumbnailRenderer": {
                            "thumbnail": {
                                "thumbnails": [
                                    { "url": "https://lh3.googleusercontent.com/artist.jpg", "width": 500 }
                                ]
                            }
                        }
                    }
                }
            },
            "contents": {
                "singleColumnBrowseResultsRenderer": {
                    "tabs": [
                        {
                            "tabRenderer": {
                                "content": {
                                    "sectionListRenderer": {
                                        "contents": [
                                            {
                                                "musicShelfRenderer": {
                                                    "title": { "runs": [{ "text": "Песни" }] },
                                                    "contents": [
                                                        {
                                                            "musicResponsiveListItemRenderer": {
                                                                "playlistItemData": { "videoId": "kish_track_1" },
                                                                "flexColumns": [
                                                                    {
                                                                        "musicResponsiveListItemFlexColumnRenderer": {
                                                                            "text": { "runs": [{ "text": "Лесник" }] }
                                                                        }
                                                                    },
                                                                    {
                                                                        "musicResponsiveListItemFlexColumnRenderer": {
                                                                            "text": {
                                                                                "runs": [
                                                                                    { "text": "Король и Шут" },
                                                                                    { "text": " • " },
                                                                                    { "text": "3:12" }
                                                                                ]
                                                                            }
                                                                        }
                                                                    }
                                                                ]
                                                            }
                                                        }
                                                    ]
                                                }
                                            },
                                            {
                                                "musicCarouselShelfRenderer": {
                                                    "header": {
                                                        "musicCarouselShelfBasicHeaderRenderer": {
                                                            "title": { "runs": [{ "text": "Альбомы" }] }
                                                        }
                                                    },
                                                    "contents": [
                                                        {
                                                            "musicTwoRowItemRenderer": {
                                                                "title": { "runs": [{ "text": "Камнем по голове" }] },
                                                                "subtitle": { "runs": [{ "text": "1996" }] },
                                                                "navigationEndpoint": {
                                                                    "browseEndpoint": { "browseId": "MPREb_album1" }
                                                                },
                                                                "thumbnailRenderer": {
                                                                    "musicThumbnailRenderer": {
                                                                        "thumbnail": {
                                                                            "thumbnails": [
                                                                                { "url": "https://lh3.googleusercontent.com/album.jpg", "width": 300 }
                                                                            ]
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    ]
                                                }
                                            }
                                        ]
                                    }
                                }
                            }
                        }
                    ]
                }
            }
        });

        let brief = parse_artist_browse(&json, "UC_kish").expect("Should parse artist brief");
        assert_eq!(brief.artist.id, "UC_kish");
        assert_eq!(brief.artist.name, "Король и Шут");
        assert_eq!(brief.artist.cover_url(400), Some("https://lh3.googleusercontent.com/artist.jpg".to_string()));
        assert_eq!(brief.popular_tracks.len(), 1);
        assert_eq!(brief.popular_tracks[0].id, "kish_track_1");
        assert_eq!(brief.popular_tracks[0].title, "Лесник");
        assert_eq!(brief.albums.len(), 1);
        assert_eq!(brief.albums[0].id, "MPREb_album1");
        assert_eq!(brief.albums[0].title.as_deref(), Some("Камнем по голове"));
        assert_eq!(brief.albums[0].year, Some(1996));
    }

    #[test]
    fn test_parse_artist_browse_ignores_banner() {
        let banner_json = json!({
            "header": {
                "musicImmersiveHeaderRenderer": {
                    "title": { "runs": [{ "text": "Deftones" }] },
                    "thumbnail": {
                        "musicThumbnailRenderer": {
                            "thumbnail": {
                                "thumbnails": [
                                    { "url": "https://lh3.googleusercontent.com/banner=w540-h225", "width": 540, "height": 225 },
                                    { "url": "https://lh3.googleusercontent.com/banner=w1080-h450", "width": 1080, "height": 450 }
                                ]
                            }
                        }
                    }
                }
            }
        });

        let brief = parse_artist_browse(&banner_json, "UC_deftones").expect("Should parse artist brief");
        assert_eq!(brief.artist.name, "Deftones");
        assert_eq!(brief.artist.cover_url(400), None, "Wide header banner should not be used as artist avatar");
    }

    #[test]
    fn test_parse_album_tracks() {
        let json = json!({
            "header": {
                "musicDetailHeaderRenderer": {
                    "title": { "runs": [{ "text": "Акустический альбом" }] },
                    "thumbnail": {
                        "croppedSquareThumbnailRenderer": {
                            "thumbnail": {
                                "thumbnails": [
                                    { "url": "https://lh3.googleusercontent.com/album_acoustic.jpg", "width": 500 }
                                ]
                            }
                        }
                    }
                }
            },
            "contents": {
                "singleColumnBrowseResultsRenderer": {
                    "tabs": [
                        {
                            "tabRenderer": {
                                "content": {
                                    "sectionListRenderer": {
                                        "contents": [
                                            {
                                                "musicShelfRenderer": {
                                                    "contents": [
                                                        {
                                                            "musicResponsiveListItemRenderer": {
                                                                "playlistItemData": { "videoId": "track_jump" },
                                                                "flexColumns": [
                                                                    {
                                                                        "musicResponsiveListItemFlexColumnRenderer": {
                                                                            "text": { "runs": [{ "text": "Прыгну со скалы" }] }
                                                                        }
                                                                    },
                                                                    {
                                                                        "musicResponsiveListItemFlexColumnRenderer": {
                                                                            "text": {
                                                                                "runs": [
                                                                                    { "text": "Король и Шут" },
                                                                                    { "text": " • " },
                                                                                    { "text": "3:15" }
                                                                                ]
                                                                            }
                                                                        }
                                                                    }
                                                                ]
                                                            }
                                                        }
                                                    ]
                                                }
                                            }
                                        ]
                                    }
                                }
                            }
                        }
                    ]
                }
            }
        });

        let tracks = parse_album_tracks(&json);
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].id, "track_jump");
        assert_eq!(tracks[0].title, "Прыгну со скалы");
        assert_eq!(tracks[0].cover_uri, Some("https://lh3.googleusercontent.com/album_acoustic.jpg".to_string()));
    }

    #[test]
    fn test_parse_stream_url_priority() {
        let json = json!({
            "playabilityStatus": {
                "status": "OK"
            },
            "streamingData": {
                "adaptiveFormats": [
                    {
                        "itag": 140,
                        "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"",
                        "bitrate": 128000,
                        "url": "https://googlevideo.com/aac"
                    },
                    {
                        "itag": 251,
                        "mimeType": "audio/webm; codecs=\"opus\"",
                        "bitrate": 160000,
                        "url": "https://googlevideo.com/opus"
                    }
                ]
            }
        });

        let url = parse_stream_url(&json).expect("Stream url should be found");
        assert_eq!(url, "https://googlevideo.com/opus");
    }

    #[test]
    fn test_parse_stream_url_fallback_aac() {
        let json = json!({
            "playabilityStatus": {
                "status": "OK"
            },
            "streamingData": {
                "adaptiveFormats": [
                    {
                        "itag": 140,
                        "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"",
                        "bitrate": 128000,
                        "url": "https://googlevideo.com/aac"
                    }
                ]
            }
        });

        let url = parse_stream_url(&json).expect("Stream url should be found");
        assert_eq!(url, "https://googlevideo.com/aac");
    }

    #[test]
    fn test_parse_radio_batch() {
        let json = json!({
            "contents": {
                "singleColumnMusicWatchNextResultsRenderer": {
                    "tabbedRenderer": {
                        "watchNextTabbedResultsRenderer": {
                            "tabs": [
                                {
                                    "tabRenderer": {
                                        "content": {
                                            "musicQueueRenderer": {
                                                "content": {
                                                    "playlistPanelRenderer": {
                                                        "contents": [
                                                            {
                                                                "playlistPanelVideoRenderer": {
                                                                    "videoId": "vid123",
                                                                    "title": { "runs": [{ "text": "Radio Track 1" }] },
                                                                    "lengthText": { "runs": [{ "text": "2:30" }] },
                                                                    "longBylineText": {
                                                                        "runs": [
                                                                            {
                                                                                "text": "Artist Radio",
                                                                                "navigationEndpoint": {
                                                                                    "browseEndpoint": { "browseId": "UC1234" }
                                                                                }
                                                                            }
                                                                        ]
                                                                    },
                                                                    "thumbnail": {
                                                                        "thumbnails": [{ "url": "https://img.com/cover.jpg", "width": 300 }]
                                                                    }
                                                                }
                                                            }
                                                        ],
                                                        "continuations": [
                                                            {
                                                                "nextRadioContinuationData": {
                                                                    "continuation": "cont_token_xyz"
                                                                }
                                                            }
                                                        ]
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            ]
                        }
                    }
                }
            }
        });

        let (tracks, continuation) = parse_radio_batch(&json);
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].id, "vid123");
        assert_eq!(tracks[0].title, "Radio Track 1");
        assert_eq!(tracks[0].duration_ms, Some(150_000));
        assert_eq!(tracks[0].artists.len(), 1);
        assert_eq!(tracks[0].artists[0].name, "Artist Radio");
        assert_eq!(tracks[0].source, TrackSource::YouTubeMusic);
        assert_eq!(continuation, Some("cont_token_xyz".to_string()));
    }

    #[test]
    fn test_parse_search_results() {
        let json = json!({
            "contents": {
                "tabbedSearchResultsRenderer": {
                    "tabs": [
                        {
                            "tabRenderer": {
                                "content": {
                                    "sectionListRenderer": {
                                        "contents": [
                                            {
                                                "musicShelfRenderer": {
                                                    "title": { "runs": [{ "text": "Песни" }] },
                                                    "contents": [
                                                        {
                                                            "musicResponsiveListItemRenderer": {
                                                                "playlistItemData": { "videoId": "search_vid_1" },
                                                                "flexColumns": [
                                                                    {
                                                                        "musicResponsiveListItemFlexColumnRenderer": {
                                                                            "text": { "runs": [{ "text": "Found Song" }] }
                                                                        }
                                                                    },
                                                                    {
                                                                        "musicResponsiveListItemFlexColumnRenderer": {
                                                                            "text": {
                                                                                "runs": [
                                                                                    { "text": "Песня" },
                                                                                    { "text": " • " },
                                                                                    {
                                                                                        "text": "Singer",
                                                                                        "navigationEndpoint": {
                                                                                            "browseEndpoint": { "browseId": "UCsinger" }
                                                                                        }
                                                                                    },
                                                                                    { "text": " • " },
                                                                                    { "text": "3:10" }
                                                                                ]
                                                                            }
                                                                        }
                                                                    }
                                                                ]
                                                            }
                                                        }
                                                    ]
                                                }
                                            }
                                        ]
                                    }
                                }
                            }
                        }
                    ]
                }
            }
        });

        let res = parse_search_results(&json);
        assert!(res.tracks.is_some());
        let tr_list = res.tracks.unwrap().results;
        assert_eq!(tr_list.len(), 1);
        assert_eq!(tr_list[0].id, "search_vid_1");
        assert_eq!(tr_list[0].title, "Found Song");
        assert_eq!(tr_list[0].artists[0].name, "Singer");
        assert_eq!(tr_list[0].duration_ms, Some(190_000));
        assert_eq!(tr_list[0].source, TrackSource::YouTubeMusic);
    }

    #[test]
    fn test_parse_liked_tracks() {
        let json = json!({
            "contents": {
                "singleColumnBrowseResultsRenderer": {
                    "tabs": [
                        {
                            "tabRenderer": {
                                "content": {
                                    "sectionListRenderer": {
                                        "contents": [
                                            {
                                                "musicPlaylistShelfRenderer": {
                                                    "contents": [
                                                        {
                                                            "musicResponsiveListItemRenderer": {
                                                                "playlistItemData": { "videoId": "liked_track_1" },
                                                                "flexColumns": [
                                                                    {
                                                                        "musicResponsiveListItemFlexColumnRenderer": {
                                                                            "text": { "runs": [{ "text": "Liked Song" }] }
                                                                        }
                                                                    },
                                                                    {
                                                                        "musicResponsiveListItemFlexColumnRenderer": {
                                                                            "text": {
                                                                                "runs": [
                                                                                    {
                                                                                        "text": "Favorite Artist",
                                                                                        "navigationEndpoint": {
                                                                                            "browseEndpoint": { "browseId": "UCfav" }
                                                                                        }
                                                                                    },
                                                                                    { "text": " • " },
                                                                                    { "text": "2:45" }
                                                                                ]
                                                                            }
                                                                        }
                                                                    }
                                                                ]
                                                            }
                                                        }
                                                    ]
                                                }
                                            }
                                        ]
                                    }
                                }
                            }
                        }
                    ]
                }
            }
        });

        let liked = parse_liked_tracks(&json);
        assert_eq!(liked.len(), 1);
        assert_eq!(liked[0].id, "liked_track_1");
        assert_eq!(liked[0].title, "Liked Song");
        assert_eq!(liked[0].artists[0].name, "Favorite Artist");
        assert_eq!(liked[0].duration_ms, Some(165_000));
        assert_eq!(liked[0].source, TrackSource::YouTubeMusic);
    }

    #[test]
    fn test_parse_playlists() {
        let json = json!({
            "contents": {
                "sectionListRenderer": {
                    "contents": [
                        {
                            "gridRenderer": {
                                "items": [
                                    {
                                        "musicTwoRowItemRenderer": {
                                            "title": { "runs": [{ "text": "My Workout Mix" }] },
                                            "subtitle": { "runs": [{ "text": "Плейлист • 50 треков" }] },
                                            "navigationEndpoint": {
                                                "browseEndpoint": { "browseId": "VLPL_workout_123" }
                                            },
                                            "thumbnailRenderer": {
                                                "musicThumbnailRenderer": {
                                                    "thumbnail": {
                                                        "thumbnails": [{ "url": "https://img.com/pl.jpg", "width": 400 }]
                                                    }
                                                }
                                            }
                                        }
                                    }
                                ]
                            }
                        }
                    ]
                }
            }
        });

        let pls = parse_playlists(&json);
        assert_eq!(pls.len(), 1);
        assert_eq!(pls[0].uid, "VLPL_workout_123");
        assert_eq!(pls[0].title, "My Workout Mix");
        assert_eq!(pls[0].og_image, Some("https://img.com/pl.jpg".to_string()));
    }

    #[test]
    fn test_parse_albums() {
        let json = json!({
            "contents": {
                "sectionListRenderer": {
                    "contents": [
                        {
                            "gridRenderer": {
                                "items": [
                                    {
                                        "musicTwoRowItemRenderer": {
                                            "title": { "runs": [{ "text": "Random Access Memories" }] },
                                            "subtitle": {
                                                "runs": [
                                                    { "text": "Альбом" },
                                                    { "text": " • " },
                                                    {
                                                        "text": "Daft Punk",
                                                        "navigationEndpoint": {
                                                            "browseEndpoint": { "browseId": "UCdaft" }
                                                        }
                                                    },
                                                    { "text": " • " },
                                                    { "text": "2013" }
                                                ]
                                            },
                                            "navigationEndpoint": {
                                                "browseEndpoint": { "browseId": "MPREb_daft_ram" }
                                            },
                                            "thumbnailRenderer": {
                                                "musicThumbnailRenderer": {
                                                    "thumbnail": {
                                                        "thumbnails": [{ "url": "https://img.com/ram.jpg", "width": 500 }]
                                                    }
                                                }
                                            }
                                        }
                                    }
                                ]
                            }
                        }
                    ]
                }
            }
        });

        let albums = parse_albums(&json);
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].id, "MPREb_daft_ram");
        assert_eq!(albums[0].title, Some("Random Access Memories".to_string()));
        assert_eq!(albums[0].year, Some(2013));
        assert_eq!(albums[0].artists.len(), 1);
        assert_eq!(albums[0].artists[0].name, "Daft Punk");
        assert_eq!(albums[0].cover_uri, Some("https://img.com/ram.jpg".to_string()));
    }

    #[test]
    fn test_parse_search_card_shelf_and_dedup() {
        let json = json!({
            "contents": {
                "sectionListRenderer": {
                    "contents": [
                        {
                            "musicCardShelfRenderer": {
                                "title": { "runs": [{ "text": "Queen", "navigationEndpoint": { "browseEndpoint": { "browseId": "UCqueen" } } }] },
                                "subtitle": { "runs": [{ "text": "Исполнитель • 40M подписчиков" }] },
                                "thumbnail": { "thumbnails": [{ "url": "https://img.com/queen.jpg", "width": 500 }] },
                                "contents": [
                                    {
                                        "musicResponsiveListItemRenderer": {
                                            "playlistItemData": { "videoId": "bohemian_rhapsody" },
                                            "flexColumns": [
                                                {
                                                    "musicResponsiveListItemFlexColumnRenderer": {
                                                        "text": { "runs": [{ "text": "Bohemian Rhapsody" }] }
                                                    }
                                                },
                                                {
                                                    "musicResponsiveListItemFlexColumnRenderer": {
                                                        "text": { "runs": [{ "text": "Queen" }, { "text": " • " }, { "text": "5:55" }] }
                                                    }
                                                }
                                            ]
                                        }
                                    }
                                ]
                            }
                        },
                        {
                            "itemSectionRenderer": {
                                "contents": [
                                    // Дубликат того же трека
                                    {
                                        "musicResponsiveListItemRenderer": {
                                            "playlistItemData": { "videoId": "bohemian_rhapsody" },
                                            "flexColumns": [
                                                {
                                                    "musicResponsiveListItemFlexColumnRenderer": {
                                                        "text": { "runs": [{ "text": "Bohemian Rhapsody" }] }
                                                    }
                                                }
                                            ]
                                        }
                                    },
                                    // Альбом без videoId
                                    {
                                        "musicResponsiveListItemRenderer": {
                                            "navigationEndpoint": {
                                                "browseEndpoint": { "browseId": "MPRE_night_at_opera" }
                                            },
                                            "flexColumns": [
                                                {
                                                    "musicResponsiveListItemFlexColumnRenderer": {
                                                        "text": { "runs": [{ "text": "A Night at the Opera" }] }
                                                    }
                                                },
                                                {
                                                    "musicResponsiveListItemFlexColumnRenderer": {
                                                        "text": { "runs": [{ "text": "Альбом • Queen • 1975" }] }
                                                    }
                                                }
                                            ]
                                        }
                                    },
                                    // Плейлист без videoId
                                    {
                                        "musicResponsiveListItemRenderer": {
                                            "navigationEndpoint": {
                                                "browseEndpoint": { "browseId": "VLPL_queen_best" }
                                            },
                                            "flexColumns": [
                                                {
                                                    "musicResponsiveListItemFlexColumnRenderer": {
                                                        "text": { "runs": [{ "text": "Best of Queen" }] }
                                                    }
                                                },
                                                {
                                                    "musicResponsiveListItemFlexColumnRenderer": {
                                                        "text": { "runs": [{ "text": "Плейлист • 30 треков" }] }
                                                    }
                                                }
                                            ]
                                        }
                                    }
                                ]
                            }
                        }
                    ]
                }
            }
        });

        let res = parse_search_results(&json);

        // Проверяем артиста из карточки
        assert!(res.artists.is_some());
        let artists = res.artists.unwrap().results;
        assert_eq!(artists.len(), 1);
        assert_eq!(artists[0].id, "UCqueen");
        assert_eq!(artists[0].name, "Queen");
        assert_eq!(artists[0].cover.as_ref().unwrap().uri.as_deref(), Some("https://img.com/queen.jpg"));

        // Проверяем треки с дедупликацией
        assert!(res.tracks.is_some());
        let tracks = res.tracks.unwrap().results;
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].id, "bohemian_rhapsody");
        assert_eq!(tracks[0].title, "Bohemian Rhapsody");

        // Проверяем альбом
        assert!(res.albums.is_some());
        let albums = res.albums.unwrap().results;
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].id, "MPRE_night_at_opera");
        assert_eq!(albums[0].title.as_deref(), Some("A Night at the Opera"));

        // Проверяем плейлист
        assert!(res.playlists.is_some());
        let playlists = res.playlists.unwrap().results;
        assert_eq!(playlists.len(), 1);
        assert_eq!(playlists[0].uid, "VLPL_queen_best");
        assert_eq!(playlists[0].title, "Best of Queen");
    }

    #[test]
    fn test_is_clean_music_track() {
        // Обычный трек 3 мин 20 сек
        let normal_track = Track {
            id: "track1".to_string(),
            title: "Shape of You".to_string(),
            artists: vec![Artist { id: "a1".to_string(), name: "Ed Sheeran".to_string(), cover: None }],
            duration_ms: Some(200_000),
            ..Default::default()
        };
        assert!(is_clean_music_track(&normal_track));

        // Слишком короткий (звуковой эффект 15 сек)
        let short_track = Track {
            id: "short".to_string(),
            title: "Sound Effect".to_string(),
            duration_ms: Some(15_000),
            ..Default::default()
        };
        assert!(!is_clean_music_track(&short_track));

        // Слишком длинный (подкаст/аудиокнига 1 час)
        let long_track = Track {
            id: "long".to_string(),
            title: "Audio recording".to_string(),
            duration_ms: Some(3_600_000),
            ..Default::default()
        };
        assert!(!is_clean_music_track(&long_track));

        // Стоп-слова в названии
        for word in &[
            "Музыкальный Подкаст #1",
            "Joe Rogan Podcast",
            "Выпуск #42 с гостем",
            "Выпуск №5",
            "Эпизод 10",
            "Episode 4",
            "Интервью со звездой",
            "Утреннее Шоу на радио",
            "Talk show 2024",
            "Стрим игры",
            "Stream replay",
            "Реакция на клип",
            "Reaction to song",
            "Review of album",
            "Обзор техники",
            "Metallica - Full Album",
            "Тизер нового альбома",
            "Official Teaser",
            "Аудиокнига Война и мир",
            "Audiobook chapter 1",
        ] {
            let t = Track {
                id: "stop".to_string(),
                title: word.to_string(),
                duration_ms: Some(240_000),
                ..Default::default()
            };
            assert!(!is_clean_music_track(&t), "Должен был отсечься трек со стоп-словом: {}", word);
        }

        // Стоп-слово в артисте
        let artist_stop = Track {
            id: "artist_stop".to_string(),
            title: "Интересный выпуск".to_string(),
            artists: vec![Artist { id: "a2".to_string(), name: "Подкаст Клуб".to_string(), cover: None }],
            duration_ms: Some(300_000),
            ..Default::default()
        };
        assert!(!is_clean_music_track(&artist_stop));
    }

    #[test]
    fn test_parse_album_tracks_with_deep_header_and_cover_fallback() {
        let json = json!({
            "contents": {
                "twoColumnBrowseResultsRenderer": {
                    "secondaryContents": {
                        "sectionListRenderer": {
                            "contents": [
                                {
                                    "musicResponsiveHeaderRenderer": {
                                        "title": { "runs": [{ "text": "Deep Album" }] },
                                        "thumbnail": {
                                            "musicThumbnailRenderer": {
                                                "thumbnail": {
                                                    "thumbnails": [
                                                        { "url": "//lh3.googleusercontent.com/album_cover.jpg", "width": 544, "height": 544 }
                                                    ]
                                                }
                                            }
                                        }
                                    }
                                }
                            ]
                        }
                    }
                }
            },
            "musicShelfRenderer": {
                "contents": [
                    {
                        "musicResponsiveListItemRenderer": {
                            "playlistItemData": { "videoId": "vid_1" },
                            "flexColumns": [
                                {
                                    "musicResponsiveListItemFlexColumnRenderer": {
                                        "text": { "runs": [{ "text": "Track 1" }] }
                                    }
                                }
                            ]
                        }
                    },
                    {
                        "musicResponsiveListItemRenderer": {
                            "playlistItemData": { "videoId": "vid_2" },
                            "flexColumns": [
                                {
                                    "musicResponsiveListItemFlexColumnRenderer": {
                                        "text": { "runs": [{ "text": "Track 2" }] }
                                    }
                                }
                            ]
                        }
                    },
                    // Дубликат первого трека
                    {
                        "musicResponsiveListItemRenderer": {
                            "playlistItemData": { "videoId": "vid_1" },
                            "flexColumns": [
                                {
                                    "musicResponsiveListItemFlexColumnRenderer": {
                                        "text": { "runs": [{ "text": "Track 1" }] }
                                    }
                                }
                            ]
                        }
                    }
                ]
            }
        });

        let tracks = parse_album_tracks(&json);
        assert_eq!(tracks.len(), 2, "Дубликат трека должен быть отсечен");
        assert_eq!(tracks[0].id, "vid_1");
        assert_eq!(tracks[0].title, "Track 1");
        assert_eq!(
            tracks[0].cover_uri.as_deref(),
            Some("https://lh3.googleusercontent.com/album_cover.jpg"),
            "Обложка альбома должна быть унаследована треком"
        );
        assert_eq!(
            tracks[1].cover_uri.as_deref(),
            Some("https://lh3.googleusercontent.com/album_cover.jpg")
        );
    }

    #[test]
    fn test_parse_playlists_filters_system_buttons_and_extracts_meta() {
        let json = json!({
            "sectionListRenderer": {
                "contents": [
                    {
                        "gridRenderer": {
                            "items": [
                                // Системная кнопка создания плейлиста
                                {
                                    "musicTwoRowItemRenderer": {
                                        "title": { "runs": [{ "text": "Новый плейлист" }] },
                                        "navigationEndpoint": {
                                            "browseEndpoint": { "browseId": "FEmusic_new_playlist" }
                                        }
                                    }
                                },
                                // Настоящий плейлист пользователя
                                {
                                    "musicTwoRowItemRenderer": {
                                        "title": { "runs": [{ "text": "Мой Рок-плейлист" }] },
                                        "navigationEndpoint": {
                                            "browseEndpoint": { "browseId": "VLPL_my_rock" }
                                        },
                                        "thumbnailRenderer": {
                                            "musicThumbnailRenderer": {
                                                "thumbnail": {
                                                    "thumbnails": [
                                                        { "url": "https://example.com/rock.jpg", "width": 400, "height": 400 }
                                                    ]
                                                }
                                            }
                                        },
                                        "subtitle": {
                                            "runs": [{ "text": "45 треков" }]
                                        }
                                    }
                                }
                            ]
                        }
                    }
                ]
            }
        });

        let playlists = parse_playlists(&json);
        assert_eq!(playlists.len(), 1, "Системная кнопка должна быть отсечена");
        assert_eq!(playlists[0].uid, "VLPL_my_rock");
        assert_eq!(playlists[0].title, "Мой Рок-плейлист");
        assert_eq!(playlists[0].track_count, Some(45));
        assert_eq!(playlists[0].og_image.as_deref(), Some("https://example.com/rock.jpg"));
    }

    #[test]
    fn test_parse_liked_tracks_from_shelf() {
        let json = json!({
            "musicPlaylistShelfRenderer": {
                "contents": [
                    {
                        "musicResponsiveListItemRenderer": {
                            "playlistItemData": { "videoId": "liked_1" },
                            "flexColumns": [
                                {
                                    "musicResponsiveListItemFlexColumnRenderer": {
                                        "text": { "runs": [{ "text": "Liked Track 1" }] }
                                    }
                                },
                                {
                                    "musicResponsiveListItemFlexColumnRenderer": {
                                        "text": { "runs": [{ "text": "Singer Name" }, { "text": " • " }, { "text": "3:15" }] }
                                    }
                                }
                            ],
                            "thumbnail": {
                                "musicThumbnailRenderer": {
                                    "thumbnail": {
                                        "thumbnails": [
                                            { "url": "https://img.com/liked1.jpg", "width": 200 }
                                        ]
                                    }
                                }
                            }
                        }
                    }
                ]
            }
        });

        let tracks = parse_liked_tracks(&json);
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].id, "liked_1");
        assert_eq!(tracks[0].title, "Liked Track 1");
        assert_eq!(tracks[0].cover_uri.as_deref(), Some("https://img.com/liked1.jpg"));
        assert_eq!(tracks[0].duration_ms, Some(195_000));
        assert_eq!(tracks[0].artists.len(), 1);
        assert_eq!(tracks[0].artists[0].name, "Singer Name");
    }

    #[test]
    fn test_unescape_js_string() {
        let raw = r#"{"text":"\x48\x65\x6c\x6c\x6f \\\x22World\\\x22"}"#;
        let unescaped = unescape_js_string(raw);
        assert_eq!(unescaped, r#"{"text":"Hello \"World\""}"#);
    }

    #[test]
    fn test_parse_tracks_from_html_regex_and_data() {
        let sample_html = r#"
        <script>
        initialData.push({path: '/browse', params: JSON.parse('{}'), data: '{\x22musicPlaylistShelfRenderer\x22:{\x22contents\x22:[{\x22musicResponsiveListItemRenderer\x22:{\x22playlistItemData\x22:{\x22videoId\x22:\x22GBRAnuT48qo\x22},\x22flexColumns\x22:[{\x22musicResponsiveListItemFlexColumnRenderer\x22:{\x22text\x22:{\x22runs\x22:[{\x22text\x22:\x22Happy Song\x22}]}}},{\x22musicResponsiveListItemFlexColumnRenderer\x22:{\x22text\x22:{\x22runs\x22:[{\x22text\x22:\x22Bring Me The Horizon\x22}]}}}]}}]}}'});
        </script>
        <!-- Regex fallback block -->
        <div>musicResponsiveListItemRenderer \x22videoId\x22:\x22hzRltUL5M3k\x22, \x22label\x22:\x22Change (In the House of Flies) - Deftones - 4 минуты 59 секунд – воспроизвести\x22</div>
        "#;

        let tracks = parse_tracks_from_html(sample_html);
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].id, "GBRAnuT48qo");
        assert_eq!(tracks[0].title, "Happy Song");
        assert_eq!(tracks[0].artists[0].name, "Bring Me The Horizon");
        assert_eq!(tracks[0].cover_uri.as_deref(), Some("https://i.ytimg.com/vi/GBRAnuT48qo/hqdefault.jpg"));

        assert_eq!(tracks[1].id, "hzRltUL5M3k");
        assert_eq!(tracks[1].title, "Change (In the House of Flies)");
        assert_eq!(tracks[1].artists[0].name, "Deftones");
        assert_eq!(tracks[1].cover_uri.as_deref(), Some("https://i.ytimg.com/vi/hzRltUL5M3k/hqdefault.jpg"));
    }

    #[test]
    fn test_parse_playlists_from_html_guide() {
        let sample_html = r#"
        <script>
        initialData.push({path: '/guide', params: JSON.parse('{}'), data: '{\x22items\x22:[{\x22guideEntryRenderer\x22:{\x22formattedTitle\x22:{\x22runs\x22:[{\x22text\x22:\x22TikTok Songs\x22}]},\x22navigationEndpoint\x22:{\x22browseEndpoint\x22:{\x22browseId\x22:\x22VLPLcHCtI7UKhDqwiMLD3A_nTUx7DJxKQt2c\x22}}}},{\x22guideEntryRenderer\x22:{\x22formattedTitle\x22:{\x22runs\x22:[{\x22text\x22:\x22Новый плейлист\x22}]},\x22navigationEndpoint\x22:{\x22browseEndpoint\x22:{\x22browseId\x22:\x22FEmusic_new_playlist\x22}}}}]}}'});
        </script>
        "#;

        let playlists = parse_playlists_from_html(sample_html);
        assert_eq!(playlists.len(), 1);
        assert_eq!(playlists[0].uid, "VLPLcHCtI7UKhDqwiMLD3A_nTUx7DJxKQt2c");
        assert_eq!(playlists[0].title, "TikTok Songs");
    }

    #[test]
    fn test_parse_album_item_thumbnail_renderer() {
        let item = json!({
            "musicTwoRowItemRenderer": {
                "title": { "runs": [{ "text": "Hybrid Theory" }] },
                "subtitle": { "runs": [{ "text": "2000" }] },
                "navigationEndpoint": {
                    "browseEndpoint": { "browseId": "MPREb_album_ht" }
                },
                "thumbnailRenderer": {
                    "musicThumbnailRenderer": {
                        "thumbnail": {
                            "thumbnails": [
                                { "url": "https://lh3.googleusercontent.com/album_ht.jpg", "width": 544, "height": 544 }
                            ]
                        }
                    }
                }
            }
        });

        let album = parse_album_item(&item).expect("Should parse album");
        assert_eq!(album.id, "MPREb_album_ht");
        assert_eq!(album.title.as_deref(), Some("Hybrid Theory"));
        assert_eq!(album.year, Some(2000));
        assert_eq!(album.cover_url(300), Some("https://lh3.googleusercontent.com/album_ht.jpg".to_string()));
    }

    #[test]
    fn test_artist_browse_empty_duration_and_album_cover() {
        let json = json!({
            "header": {
                "musicImmersiveHeaderRenderer": {
                    "title": { "runs": [{ "text": "Deftones" }] },
                    "thumbnail": {
                        "musicThumbnailRenderer": {
                            "thumbnail": {
                                "thumbnails": [
                                    { "url": "https://lh3.googleusercontent.com/deftones_artist.jpg", "width": 800 }
                                ]
                            }
                        }
                    }
                }
            },
            "contents": {
                "singleColumnBrowseResultsRenderer": {
                    "tabs": [
                        {
                            "tabRenderer": {
                                "content": {
                                    "sectionListRenderer": {
                                        "contents": [
                                            {
                                                "musicShelfRenderer": {
                                                    "title": { "runs": [{ "text": "Песни" }] },
                                                    "contents": [
                                                        {
                                                            "musicResponsiveListItemRenderer": {
                                                                "playlistItemData": { "videoId": "deftones_song_1" },
                                                                "flexColumns": [
                                                                    {
                                                                        "musicResponsiveListItemFlexColumnRenderer": {
                                                                            "text": { "runs": [{ "text": "Be Quiet and Drive" }] }
                                                                        }
                                                                    },
                                                                    {
                                                                        "musicResponsiveListItemFlexColumnRenderer": {
                                                                            "text": {
                                                                                "runs": [
                                                                                    { "text": "Deftones" }
                                                                                ]
                                                                            }
                                                                        }
                                                                    }
                                                                ]
                                                            }
                                                        }
                                                    ]
                                                }
                                            },
                                            {
                                                "musicCarouselShelfRenderer": {
                                                    "header": {
                                                        "musicCarouselShelfBasicHeaderRenderer": {
                                                            "title": { "runs": [{ "text": "Альбомы" }] }
                                                        }
                                                    },
                                                    "contents": [
                                                        {
                                                            "musicTwoRowItemRenderer": {
                                                                "title": { "runs": [{ "text": "Around the Fur" }] },
                                                                "subtitle": { "runs": [{ "text": "1997" }] },
                                                                "navigationEndpoint": {
                                                                    "browseEndpoint": { "browseId": "MPREb_atf" }
                                                                },
                                                                "thumbnailRenderer": {
                                                                    "musicThumbnailRenderer": {
                                                                        "thumbnail": {
                                                                            "thumbnails": [
                                                                                { "url": "https://lh3.googleusercontent.com/album_atf.jpg", "width": 500 }
                                                                            ]
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    ]
                                                }
                                            }
                                        ]
                                    }
                                }
                            }
                        }
                    ]
                }
            }
        });

        let brief = parse_artist_browse(&json, "UC_deftones").expect("Should parse artist brief");
        assert_eq!(brief.artist.id, "UC_deftones");
        assert_eq!(brief.artist.name, "Deftones");
        assert_eq!(brief.artist.cover_url(400), Some("https://lh3.googleusercontent.com/deftones_artist.jpg".to_string()));

        // Альбом из карусели получил обложку через thumbnailRenderer
        assert_eq!(brief.albums.len(), 1);
        assert_eq!(brief.albums[0].id, "MPREb_atf");
        assert_eq!(brief.albums[0].cover_url(300), Some("https://lh3.googleusercontent.com/album_atf.jpg".to_string()));

        // Популярный трек без указанной длительности в runs
        assert_eq!(brief.popular_tracks.len(), 1);
        assert_eq!(brief.popular_tracks[0].id, "deftones_song_1");
        assert_eq!(brief.popular_tracks[0].duration_ms, None);
        // Не должен возвращать "00:00"!
        assert_eq!(brief.popular_tracks[0].formatted_duration(), "");
        // Обложка трека унаследована от артиста
        assert_eq!(brief.popular_tracks[0].cover_url(300), Some("https://lh3.googleusercontent.com/deftones_artist.jpg".to_string()));
    }

    #[test]
    fn test_parse_header_track_count() {
        let json = serde_json::json!({
            "contents": {
                "twoColumnBrowseResultsRenderer": {
                    "tabs": [{
                        "tabRenderer": {
                            "content": {
                                "sectionListRenderer": {
                                    "contents": [{
                                        "musicResponsiveHeaderRenderer": {
                                            "secondSubtitle": {
                                                "runs": [
                                                    { "text": "325\u{00a0}треков" },
                                                    { "text": " • " },
                                                    { "text": "Больше 6 часов" }
                                                ]
                                            }
                                        }
                                    }]
                                }
                            }
                        }
                    }]
                }
            }
        });

        assert_eq!(parse_header_track_count(&json), Some(325));
    }

    #[test]
    fn test_parse_header_track_count_from_html() {
        let html = r#"<script>initialData.push({path: '/browse', params: JSON.parse("null"), data: '{\x22contents\x22:{\x22twoColumnBrowseResultsRenderer\x22:{\x22tabs\x22:[{\x22tabRenderer\x22:{\x22content\x22:{\x22sectionListRenderer\x22:{\x22contents\x22:[{\x22musicResponsiveHeaderRenderer\x22:{\x22secondSubtitle\x22:{\x22runs\x22:[{\x22text\x22:\x22325 треков\x22}]}}}]}}}]}}}}'});</script>"#;
        assert_eq!(parse_header_track_count_from_html(html), Some(325));

        let html_fallback = r#"<div>musicResponsiveHeaderRenderer ... secondSubtitle: {"runs":[{"text":"325 треков"}]}</div>"#;
        assert_eq!(parse_header_track_count_from_html(html_fallback), Some(325));
    }
}

