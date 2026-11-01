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

/// Чипы над очередью YouTube Music (`chipCloudChipRenderer`)
///
/// Чип — это не фильтр поверх текущего микса: в нём лежит собственный
/// `playlistId` (обычно `RDAT…`), и подстановка его в запрос `next`
/// меняет саму выдачу. `params` из `browse FEmusic_moods_and_genres` для этого
/// бесполезны (выдача совпадает на 50 из 50 треков), поэтому чипы берём только отсюда.
pub fn parse_station_chips(json: &serde_json::Value) -> Vec<crate::api::provider::StationChip> {
    let mut chips = Vec::new();
    collect_station_chips(json, &mut chips);
    let mut seen = std::collections::HashSet::new();
    chips.retain(|chip: &crate::api::provider::StationChip| seen.insert(chip.playlist_id.clone()));
    chips
}

fn collect_station_chips(
    json: &serde_json::Value,
    out: &mut Vec<crate::api::provider::StationChip>,
) {
    match json {
        serde_json::Value::Object(map) => {
            if let Some(renderer) = map.get("chipCloudChipRenderer") {
                if let (Some(label), Some(playlist_id)) =
                    (chip_label(renderer), chip_playlist_id(renderer))
                {
                    out.push(crate::api::provider::StationChip { label, playlist_id });
                }
                return;
            }
            for value in map.values() {
                collect_station_chips(value, out);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_station_chips(item, out);
            }
        }
        _ => {}
    }
}

/// Подпись чипа: `simpleText` либо склеенные `runs`
fn chip_label(chip: &serde_json::Value) -> Option<String> {
    let text = chip.get("text")?;
    if let Some(simple) = text.get("simpleText").and_then(|v| v.as_str()) {
        let label = simple.trim();
        return (!label.is_empty()).then(|| label.to_string());
    }
    let runs = text.get("runs")?.as_array()?;
    let joined: String = runs
        .iter()
        .filter_map(|run| run.get("text").and_then(|v| v.as_str()))
        .collect();
    let label = joined.trim();
    (!label.is_empty()).then(|| label.to_string())
}

/// `playlistId` варианта станции из навигации чипа
///
/// В живом ответе `next` идентификатор лежит не прямо в `watchEndpoint`,
/// а глубже — в `queueUpdateCommand.fetchContentsCommand.watchEndpoint`
/// (проверено на ответе 2026-11-01). Поэтому после прямых ключей обходим
/// эндпоинт рекурсивно: форма ответа у YouTube меняется чаще, чем смысл чипа.
fn chip_playlist_id(chip: &serde_json::Value) -> Option<String> {
    let endpoint = chip.get("navigationEndpoint")?;
    for key in ["watchEndpoint", "watchPlaylistEndpoint"] {
        if let Some(id) = endpoint
            .get(key)
            .and_then(|e| e.get("playlistId"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            return Some(id.to_string());
        }
    }
    find_playlist_id(endpoint)
}

/// Первый непустой `playlistId` в произвольном поддереве
fn find_playlist_id(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(id) = map
                .get("playlistId")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|id| !id.is_empty())
            {
                return Some(id.to_string());
            }
            map.values().find_map(find_playlist_id)
        }
        serde_json::Value::Array(items) => items.iter().find_map(find_playlist_id),
        _ => None,
    }
}
