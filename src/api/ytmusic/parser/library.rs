//! Библиотека пользователя: треки «Мне нравится».

use super::*;

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
        && let Some(contents) = shelf.get("contents").and_then(|c| c.as_array())
    {
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
        let needs_ytimg = t
            .cover_uri
            .as_deref()
            .is_none_or(|u| u.contains("googleusercontent.com"));
        if needs_ytimg && !t.id.is_empty() {
            t.cover_uri = Some(format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", t.id));
        }
    }

    tracks
}

/// Треки из плиток TV-клиента (`tileRenderer`).
///
/// Так медиатека приходит при входе по OAuth: Bearer-токен Google принимает
/// только TV-клиент (`WEB_REMIX` отвечает 400), а разметка у него плиточная,
/// без `musicResponsiveListItemRenderer`.
pub fn parse_tv_tile_tracks(json: &serde_json::Value) -> Vec<Track> {
    fn collect<'a>(value: &'a serde_json::Value, out: &mut Vec<&'a serde_json::Value>) {
        if let Some(object) = value.as_object() {
            if let Some(tile) = object.get("tileRenderer") {
                out.push(tile);
            }
            for child in object.values() {
                collect(child, out);
            }
        } else if let Some(array) = value.as_array() {
            for item in array {
                collect(item, out);
            }
        }
    }

    /// Первая строка метаданных плитки: исполнитель
    fn tile_line(tile: &serde_json::Value, index: usize) -> Option<String> {
        let lines = tile
            .get("metadata")?
            .get("tileMetadataRenderer")?
            .get("lines")?
            .as_array()?;
        let items = lines
            .get(index)?
            .get("lineRenderer")?
            .get("items")?
            .as_array()?;
        let text: Vec<String> = items
            .iter()
            .filter_map(|item| {
                item.get("lineItemRenderer")
                    .and_then(|renderer| renderer.get("text"))
                    .and_then(super::common::extract_text)
            })
            .filter(|text| !text.trim().is_empty())
            .collect();
        (!text.is_empty()).then(|| text.join(", "))
    }

    let mut tiles = Vec::new();
    collect(json, &mut tiles);

    let mut tracks: Vec<Track> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for tile in tiles {
        let Some(meta) = tile
            .get("metadata")
            .and_then(|metadata| metadata.get("tileMetadataRenderer"))
        else {
            continue;
        };
        let title = meta
            .get("title")
            .and_then(super::common::extract_text)
            .unwrap_or_default();
        if title.is_empty() {
            continue;
        }

        let video_id = meta
            .get("title")
            .and_then(|title| title.get("runs"))
            .and_then(|runs| runs.as_array())
            .and_then(|runs| {
                runs.iter().find_map(|run| {
                    run.get("navigationEndpoint")
                        .and_then(|endpoint| endpoint.get("watchEndpoint"))
                        .and_then(|endpoint| endpoint.get("videoId"))
                        .and_then(|id| id.as_str())
                })
            })
            .map(str::to_string)
            .or_else(|| {
                // запасной путь: id внутри ссылки на превью
                super::thumbnails::extract_best_thumbnail(tile)
                    .and_then(|url| url.split("/vi/").nth(1).map(str::to_string))
                    .and_then(|rest| rest.split('/').next().map(str::to_string))
            });
        let Some(video_id) = video_id.filter(|id| id.len() == 11) else {
            continue;
        };
        if !seen.insert(video_id.clone()) {
            continue;
        }

        let artist = tile_line(tile, 0).unwrap_or_default();
        let artists = if artist.is_empty() {
            Vec::new()
        } else {
            vec![crate::api::models::Artist {
                id: String::new(),
                name: artist,
                cover: None,
            }]
        };

        let cover_uri = super::thumbnails::extract_best_thumbnail(tile)
            .or_else(|| Some(format!("https://i.ytimg.com/vi/{video_id}/hqdefault.jpg")));

        tracks.push(Track {
            id: video_id,
            title,
            version: None,
            artists,
            albums: Vec::new(),
            duration_ms: None,
            cover_uri,
            og_image: None,
            available: Some(true),
            content_warning: None,
            derived_colors: None,
            source: TrackSource::YouTubeMusic,
        });
    }
    tracks
}
