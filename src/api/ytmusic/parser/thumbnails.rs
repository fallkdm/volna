//! Обложки: поиск массивов thumbnails, выбор лучшего, отсев баннеров.

/// Поиск массива `thumbnails` по всем известным путям и через рекурсивный обход
fn find_thumbnails_array(val: &serde_json::Value) -> Option<&Vec<serde_json::Value>> {
    if let Some(arr) = val.as_array()
        && arr.first().and_then(|item| item.get("url")).is_some()
    {
        return Some(arr);
    }
    if let Some(arr) = val.get("thumbnails").and_then(|v| v.as_array()) {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("thumbnail")
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("thumbnail")
        .and_then(|t| t.get("musicThumbnailRenderer"))
        .and_then(|m| m.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("thumbnail")
        .and_then(|t| t.get("croppedSquareThumbnailRenderer"))
        .and_then(|m| m.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("musicResponsiveHeaderRenderer")
        .and_then(|h| h.get("thumbnail"))
        .and_then(|t| t.get("musicThumbnailRenderer"))
        .and_then(|m| m.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("musicResponsiveHeaderRenderer")
        .and_then(|h| h.get("thumbnail"))
        .and_then(|t| t.get("croppedSquareThumbnailRenderer"))
        .and_then(|m| m.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("musicDetailHeaderRenderer")
        .and_then(|h| h.get("thumbnail"))
        .and_then(|t| t.get("musicThumbnailRenderer"))
        .and_then(|m| m.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("musicDetailHeaderRenderer")
        .and_then(|h| h.get("thumbnail"))
        .and_then(|t| t.get("croppedSquareThumbnailRenderer"))
        .and_then(|m| m.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("musicThumbnailRenderer")
        .and_then(|m| m.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("croppedSquareThumbnailRenderer")
        .and_then(|m| m.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("thumbnailRenderer")
        .and_then(|t| t.get("musicThumbnailRenderer"))
        .and_then(|m| m.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("thumbnailRenderer")
        .and_then(|m| m.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("thumbnailRenderer")
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("musicTwoRowItemRenderer")
        .and_then(|r| r.get("thumbnailRenderer"))
        .and_then(|t| t.get("musicThumbnailRenderer"))
        .and_then(|m| m.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("musicTwoRowItemRenderer")
        .and_then(|r| r.get("thumbnailRenderer"))
        .and_then(|m| m.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("musicTwoRowItemRenderer")
        .and_then(|r| r.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("foregroundThumbnail")
        .and_then(|t| t.get("musicThumbnailRenderer"))
        .and_then(|m| m.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("foregroundThumbnail")
        .and_then(|t| t.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }
    if let Some(arr) = val
        .get("foregroundThumbnail")
        .and_then(|t| t.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        return Some(arr);
    }

    // Рекурсивный поиск массива thumbnails с элементами, содержащими url
    fn deep_search(v: &serde_json::Value) -> Option<&Vec<serde_json::Value>> {
        if let Some(obj) = v.as_object() {
            if let Some(arr) = obj.get("thumbnails").and_then(|a| a.as_array())
                && arr.iter().any(|item| item.get("url").is_some())
            {
                return Some(arr);
            }
            for sub in obj.values() {
                if let Some(res) = deep_search(sub) {
                    return Some(res);
                }
            }
        } else if let Some(arr) = v.as_array() {
            for sub in arr {
                if let Some(res) = deep_search(sub) {
                    return Some(res);
                }
            }
        }
        None
    }
    deep_search(val)
}

fn normalize_thumbnail_url(url_str: &str) -> String {
    if url_str.starts_with("//") {
        format!("https:{}", url_str)
    } else {
        url_str.to_string()
    }
}

/// Извлекает размеры из полей JSON (`width`/`height`) либо из параметров Google CDN (`=w...-h...`)
fn parse_url_dimensions(url: &str) -> Option<(f64, f64)> {
    let idx = url.find("=w")?;
    let tail = &url[idx + 2..];
    let h_idx = tail.find("-h")?;
    let w_str = &tail[..h_idx];
    let rest = &tail[h_idx + 2..];
    let h_end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    let h_str = &rest[..h_end];
    let width = w_str.parse::<f64>().ok()?;
    let height = h_str.parse::<f64>().ok()?;
    Some((width, height))
}

fn thumbnail_area(item: &serde_json::Value) -> u64 {
    let w = item.get("width").and_then(|w| w.as_u64()).unwrap_or(0);
    let h = item.get("height").and_then(|h| h.as_u64()).unwrap_or(0);
    if w > 0 || h > 0 {
        return w * h.max(1);
    }
    if let Some(url) = item.get("url").and_then(|u| u.as_str())
        && let Some((uw, uh)) = parse_url_dimensions(url)
    {
        return (uw as u64) * (uh as u64).max(1);
    }
    0
}

fn select_thumbnail(
    val: &serde_json::Value,
    predicate: fn(&serde_json::Value) -> bool,
) -> Option<String> {
    if let Some(list) = find_thumbnails_array(val)
        && let Some(best) = list
            .iter()
            .filter(|item| predicate(item))
            .max_by_key(|item| thumbnail_area(item))
        && let Some(url_str) = best.get("url").and_then(|u| u.as_str())
    {
        return Some(normalize_thumbnail_url(url_str));
    }

    if let Some(url_str) = val.get("url").and_then(|u| u.as_str())
        && predicate(val)
    {
        return Some(normalize_thumbnail_url(url_str));
    }

    None
}

/// Извлечение лучшего качества URL обложки из структуры `thumbnails`
pub fn extract_best_thumbnail(val: &serde_json::Value) -> Option<String> {
    select_thumbnail(val, |_| true)
}

/// Проверяет, является ли thumbnail широкоформатным баннером (соотношение сторон w > h * 1.3)
pub fn is_banner_thumbnail(item: &serde_json::Value) -> bool {
    let w = item.get("width").and_then(|w| w.as_f64()).unwrap_or(0.0);
    let h = item.get("height").and_then(|h| h.as_f64()).unwrap_or(0.0);
    if w > 0.0 && h > 0.0 && w > h * 1.3 {
        return true;
    }
    if let Some(url) = item.get("url").and_then(|u| u.as_str())
        && let Some((width, height)) = parse_url_dimensions(url)
        && width > height * 1.3
    {
        return true;
    }
    false
}

/// Извлечение лучшего качества URL квадратного thumbnail (отсекая широкоформатные баннеры)
pub fn extract_square_thumbnail(val: &serde_json::Value) -> Option<String> {
    select_thumbnail(val, |item| !is_banner_thumbnail(item))
}

/// Извлечение наибольшего широкоформатного баннера (`w > h * 1.3`), отсекая квадратные аватары
pub fn extract_banner_thumbnail(val: &serde_json::Value) -> Option<String> {
    select_thumbnail(val, is_banner_thumbnail)
}
