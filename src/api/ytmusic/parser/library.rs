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
