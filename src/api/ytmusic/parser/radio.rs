//! Радио и миксы: батчи, continuation-токены, видео из панели.

use super::*;

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
