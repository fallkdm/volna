//! Исполнители: элементы карусели, заголовок и страница исполнителя.

use super::*;

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
    let name = renderer.get("title").and_then(extract_text).or_else(|| {
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
        && let Some(fg) = h.get("foregroundThumbnail")
    {
        cover_uri = extract_square_thumbnail(fg);
    }
    if cover_uri.is_none()
        && let Some(h) = json.get("header")
        && let Some(fg) = h.get("foregroundThumbnail")
    {
        cover_uri = extract_square_thumbnail(fg);
    }
    if cover_uri.is_none() {
        cover_uri = header_obj.and_then(extract_square_thumbnail);
    }
    if cover_uri.is_none()
        && let Some(h) = json.get("header")
    {
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
        && let Some(contents) = shelf.get("contents").and_then(|c| c.as_array())
    {
        for it in contents {
            if let Some(mut track) = parse_responsive_track(it) {
                let is_ytimg = track
                    .cover_uri
                    .as_deref()
                    .is_some_and(|u| u.starts_with("https://i.ytimg.com/vi/"));
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
        && let Some(first_artist) = first_track.artists.first()
    {
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
                            && (!album.id.is_empty() || album.title.is_some())
                        {
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

    let artist_cover = cover_uri.as_ref().map(|u| ArtistCover {
        uri: Some(u.clone()),
    });
    let all_covers = cover_uri
        .into_iter()
        .map(|u| ArtistCover { uri: Some(u) })
        .collect();

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
