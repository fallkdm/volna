//! Общие утилиты: длительность, текст, id-шники видео и browse.

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

/// Рекурсивный поиск videoId в объекте
pub(super) fn find_video_id(val: &serde_json::Value) -> Option<String> {
    if let Some(id) = val.get("videoId").and_then(|v| v.as_str())
        && !id.is_empty()
    {
        return Some(id.to_string());
    }
    if let Some(id) = val
        .get("playlistItemData")
        .and_then(|p| p.get("videoId"))
        .and_then(|v| v.as_str())
        && !id.is_empty()
    {
        return Some(id.to_string());
    }
    if let Some(id) = val
        .get("watchEndpoint")
        .and_then(|w| w.get("videoId"))
        .and_then(|v| v.as_str())
        && !id.is_empty()
    {
        return Some(id.to_string());
    }
    if let Some(id) = val
        .get("navigationEndpoint")
        .and_then(|n| n.get("watchEndpoint"))
        .and_then(|w| w.get("videoId"))
        .and_then(|v| v.as_str())
        && !id.is_empty()
    {
        return Some(id.to_string());
    }
    if let Some(id) = val
        .get("onTap")
        .and_then(|n| n.get("watchEndpoint"))
        .and_then(|w| w.get("videoId"))
        .and_then(|v| v.as_str())
        && !id.is_empty()
    {
        return Some(id.to_string());
    }
    None
}

/// Рекурсивный поиск browseId в объекте
pub(super) fn find_browse_id(val: &serde_json::Value) -> Option<String> {
    if let Some(id) = val.get("browseId").and_then(|v| v.as_str())
        && !id.is_empty()
    {
        return Some(id.to_string());
    }
    if let Some(id) = val
        .get("browseEndpoint")
        .and_then(|b| b.get("browseId"))
        .and_then(|v| v.as_str())
        && !id.is_empty()
    {
        return Some(id.to_string());
    }
    if let Some(id) = val
        .get("navigationEndpoint")
        .and_then(|n| n.get("browseEndpoint"))
        .and_then(|b| b.get("browseId"))
        .and_then(|v| v.as_str())
        && !id.is_empty()
    {
        return Some(id.to_string());
    }
    if let Some(id) = val
        .get("onTap")
        .and_then(|n| n.get("browseEndpoint"))
        .and_then(|b| b.get("browseId"))
        .and_then(|v| v.as_str())
        && !id.is_empty()
    {
        return Some(id.to_string());
    }
    if let Some(id) = val
        .get("onTap")
        .and_then(|n| n.get("navigationEndpoint"))
        .and_then(|n| n.get("browseEndpoint"))
        .and_then(|b| b.get("browseId"))
        .and_then(|v| v.as_str())
        && !id.is_empty()
    {
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
    if let Some(runs) = val
        .get("title")
        .and_then(|t| t.get("runs"))
        .and_then(|r| r.as_array())
    {
        for r in runs {
            if let Some(id) = find_browse_id(r) {
                return Some(id);
            }
        }
    }
    if let Some(header) = val
        .get("header")
        .and_then(|h| h.get("musicCardShelfHeaderBasicRenderer"))
        && let Some(id) = find_browse_id(header)
    {
        return Some(id);
    }
    None
}
