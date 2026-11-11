//! Поиск: карточки, полки, секции и их классификация.

use super::*;

/// Извлечение текста подписи (subtitle) из объекта элемента или карточки
fn extract_subtitle_text(val: &serde_json::Value) -> String {
    if let Some(sub) = val.get("subtitle")
        && let Some(t) = extract_text(sub)
    {
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
    if let Some(header) = val
        .get("header")
        .and_then(|h| h.get("musicCardShelfHeaderBasicRenderer"))
        && let Some(sub) = header.get("subtitle")
        && let Some(t) = extract_text(sub)
    {
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
    let title = card.get("title").and_then(extract_text).or_else(|| {
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

    if subtitle.contains("исполнитель")
        || subtitle.contains("artist")
        || browse_id.starts_with("UC")
    {
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
            ..Default::default()
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
        && let Some(track) = parse_responsive_track(renderer)
    {
        tracks.push(track);
        return;
    }

    // Если videoId нет -> анализируй subtitle и browseId
    let browse_id = find_browse_id(renderer).unwrap_or_default();
    let sub = extract_subtitle_text(renderer).to_lowercase();

    if (sub.contains("исполнитель") || sub.contains("artist") || browse_id.starts_with("UC"))
        && let Some(art) = parse_artist_item(renderer)
    {
        artists.push(art);
        return;
    }

    if (sub.contains("альбом")
        || sub.contains("album")
        || sub.contains("сингл")
        || sub.contains("single")
        || sub.contains("ep")
        || browse_id.starts_with("MPRE"))
        && let Some(alb) = parse_album_item(renderer)
    {
        albums.push(alb);
        return;
    }

    if (sub.contains("плейлист")
        || sub.contains("playlist")
        || browse_id.starts_with("VL")
        || browse_id.starts_with("PL"))
        && let Some(pl) = parse_playlist_item(renderer)
    {
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
