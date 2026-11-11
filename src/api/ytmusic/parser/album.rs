//! Альбомы: элементы карусели, шельфы, заголовок и трек-лист альбома.

use super::*;

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
    let title = renderer.get("title").and_then(extract_text).or_else(|| {
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
            if t.len() == 4
                && (t.starts_with("19") || t.starts_with("20"))
                && let Ok(y) = t.parse::<u32>()
            {
                year = Some(y);
            }
            if let Some(bid) = r
                .get("navigationEndpoint")
                .and_then(|n| n.get("browseEndpoint"))
                .and_then(|b| b.get("browseId"))
                .and_then(|id| id.as_str())
                && (bid.starts_with("UC") || bid.contains("artist"))
            {
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
        ..Default::default()
    })
}

/// Рекурсивный сбор альбомов
fn collect_albums(json: &serde_json::Value, out: &mut Vec<Album>) {
    if let Some(obj) = json.as_object() {
        if let Some(item) = obj.get("musicTwoRowItemRenderer") {
            let bid = find_browse_id(item).unwrap_or_default();
            if (bid.starts_with("MPRE") || bid.starts_with("FEmusic_library_album"))
                && let Some(al) = parse_album_item(item)
            {
                out.push(al);
            }
        } else if let Some(item) = obj.get("musicResponsiveListItemRenderer") {
            let bid = find_browse_id(item).unwrap_or_default();
            if (bid.starts_with("MPRE") || bid.starts_with("FEmusic_library_album"))
                && let Some(al) = parse_album_item(item)
            {
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
            let is_ytimg = t
                .cover_uri
                .as_deref()
                .is_some_and(|u| u.starts_with("https://i.ytimg.com/vi/"));
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
