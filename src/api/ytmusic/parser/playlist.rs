//! Плейлисты: элементы, системные кнопки, сбор из JSON и HTML.

use super::*;

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
                && let Some(pl) = parse_playlist_item(item)
            {
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
                && let Some(pl) = parse_playlist_item(item)
            {
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

            if (browse_id.starts_with("VLPL")
                || browse_id.starts_with("PL")
                || (browse_id.starts_with("VL") && browse_id != "VLLM" && browse_id != "VLSE"))
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
            if (bid.starts_with("VLPL")
                || bid.starts_with("PL")
                || (bid.starts_with("VL") && bid != "VLLM" && bid != "VLSE"))
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
            let end = tail
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
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
