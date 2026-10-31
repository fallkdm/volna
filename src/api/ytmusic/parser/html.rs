//! Разбор HTML-страниц: JS-строки, поля, счётчики и списки треков.

use super::*;

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
pub(super) fn extract_js_string_literal(slice: &str) -> Option<&str> {
    let bytes = slice.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    while i < n && (bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\r' || bytes[i] == b'\n')
    {
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
pub(super) fn extract_field_value(chunk: &str, field: &str) -> Option<String> {
    for prefix in &[
        format!("\\x22{field}\\x22:\\x22"),
        format!("\"{field}\":\""),
        format!("\"{field}\": \""),
        format!("{field}\\x22:\\x22"),
    ] {
        if let Some(pos) = chunk.find(prefix.as_str()) {
            let start = pos + prefix.len();
            let tail = &chunk[start..];
            let end = tail.find(['"', '\\', '\'']).unwrap_or(tail.len());
            let val = &tail[..end];
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

/// Извлечение названия трека и исполнителя из строки accessibility label
pub(super) fn extract_track_label(chunk: &str) -> Option<(String, String)> {
    for prefix in &[
        "\\x22label\\x22:\\x22",
        "\"label\":\"",
        "\"label\": \"",
        "label\\x22:\\x22",
    ] {
        if let Some(pos) = chunk.find(prefix) {
            let start = pos + prefix.len();
            let tail = &chunk[start..];
            let end = tail.find(['"', '\\', '\'']).unwrap_or(tail.len());
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
pub(super) fn extract_continuation_from_raw_html(html: &str) -> Option<String> {
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
                    && let Some(runs) = sub.get("runs").and_then(|r| r.as_array())
                {
                    for r in runs {
                        if let Some(text) = r.get("text").and_then(|t| t.as_str()) {
                            let clean = text.replace('\u{a0}', " ");
                            let num_str: String =
                                clean.chars().take_while(|c| c.is_ascii_digit()).collect();
                            if let Ok(n) = num_str.parse::<u32>()
                                && n > 0
                                && (clean.contains("трек")
                                    || clean.contains("песн")
                                    || clean.contains("track")
                                    || clean.contains("song"))
                            {
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
            && path_pos < 120
        {
            let path_tail = &tail[path_pos + 5..];
            if let Some(path_str) = extract_js_string_literal(path_tail)
                && (path_str == "/browse" || path_str == "\\/browse")
                && let Some(data_pos) = tail.find("data:")
            {
                let data_tail = &tail[data_pos + 5..];
                if let Some(raw_data) = extract_js_string_literal(data_tail) {
                    let unescaped = unescape_js_string(raw_data);
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&unescaped)
                        && let Some(cnt) = parse_header_track_count(&json)
                    {
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
                        && n > 0
                        && n < 100_000
                    {
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
            && path_pos < 120
        {
            let path_tail = &tail[path_pos + 5..];
            if let Some(path_str) = extract_js_string_literal(path_tail)
                && (path_str == "/browse" || path_str == "\\/browse")
                && let Some(data_pos) = tail.find("data:")
            {
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
    for delimiter in &[
        "musicResponsiveListItemRenderer",
        "\\x22musicResponsiveListItemRenderer\\x22",
    ] {
        let chunks: Vec<&str> = html.split(delimiter).collect();
        if chunks.len() > 1 {
            for chunk in &chunks[1..] {
                if let Some(id) = extract_field_value(chunk, "videoId")
                    && id.len() == 11
                    && !seen_ids.contains(&id)
                    && let Some((title, artist)) = extract_track_label(chunk)
                {
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
        let needs_ytimg = t
            .cover_uri
            .as_deref()
            .is_none_or(|u| u.contains("googleusercontent.com"));
        if needs_ytimg && !t.id.is_empty() {
            t.cover_uri = Some(format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", t.id));
        }
    }

    (tracks, continuation_token)
}
