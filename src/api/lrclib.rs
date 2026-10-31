use crate::api::models::{parse_lrc, LrclibResponse};

/// Статус ответа LRCLIB
pub enum LrclibFetch {
    Found(reqwest::Response),
    NotFound,
    Failed,
}

/// GET к LRCLIB с повтором: сервис нередко отвечает 429 (лимит на общий IP) или 5xx, а 404 означает «текста нет»
pub async fn lrclib_get(http: &reqwest::Client, url: &str) -> LrclibFetch {
    for attempt in 0..3u64 {
        let sent = http
            .get(url)
            .header("User-Agent", "VolnaMusicPlayer/1.0 (https://github.com/fallkdm/volna)")
            .send()
            .await;
        match sent {
            Ok(r) if r.status().is_success() => return LrclibFetch::Found(r),
            Ok(r) if r.status() == reqwest::StatusCode::NOT_FOUND => return LrclibFetch::NotFound,
            _ => tokio::time::sleep(std::time::Duration::from_millis(700 * (attempt + 1))).await,
        }
    }
    LrclibFetch::Failed
}

/// Синхронизированный текст годится, только если в нём хотя бы несколько строк: в базах встречаются
/// огрызки из одной строки, и тогда весь экран показывал одну фразу
pub fn usable_synced(text: &str) -> bool {
    parse_lrc(text).len() >= 4
}

/// Убирает таймкоды `[mm:ss.xx]` в начале строк, оставляя сам текст
pub fn strip_lrc_tags(text: &str) -> String {
    text.lines()
        .map(|l| {
            let mut l = l.trim_start();
            while l.starts_with('[') {
                match l.find(']') {
                    Some(end) => l = l[end + 1..].trim_start(),
                    None => break,
                }
            }
            l
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Очистка названия трека от клиповых хвостов YouTube (например, "(Official Video)", "(Lyric Video)", "[HD]")
pub fn clean_title_for_lyrics(title: &str) -> String {
    let mut t = title.to_string();
    let noise_patterns = [
        "(official video)", "(official music video)", "(official audio)",
        "(lyric video)", "(lyrics)", "(audio)", "(visualizer)", "[official video]",
        "[music video]", "[audio]", "[lyrics]", "[visualizer]", "(clip)", "(клип)",
        "(премьера клипа)", "(hd)", "[hd]", "(4k)", "[4k]"
    ];
    let lower = t.to_lowercase();
    for pat in noise_patterns {
        if let Some(pos) = lower.find(pat) {
            t = t[..pos].to_string();
            break;
        }
    }
    t.trim().to_string()
}

/// Название для сравнения: без регистра, без скобок («(feat. …)», «[Remix]»), хвоста « - Remastered» и пунктуации
pub fn normalize_name(s: &str) -> String {
    let mut depth = 0u32;
    let no_brackets: String = s
        .chars()
        .filter(|c| match c {
            '(' | '[' => { depth += 1; false }
            ')' | ']' => { depth = depth.saturating_sub(1); false }
            _ => depth == 0,
        })
        .collect();
    let head = no_brackets.split(" - ").next().unwrap_or("");
    head.to_lowercase().chars().filter(|c| c.is_alphanumeric() || c.is_whitespace()).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn urlencoding_encode(s: &str) -> String {
    let mut encoded = String::new();
    for b in s.as_bytes() {
        match *b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*b as char);
            }
            b' ' => encoded.push('+'),
            _ => {
                encoded.push_str(&format!("%{:02X}", b));
            }
        }
    }
    encoded
}

/// Поиск в LRCLIB записи с синхронизированным текстом той же длительности (±4 с)
pub async fn lrclib_search_synced(
    http: &reqwest::Client,
    artist: &str,
    title: &str,
    duration_secs: Option<u64>,
) -> Option<String> {
    let clean_t = clean_title_for_lyrics(title);
    let url = format!("https://lrclib.net/api/search?q={}", urlencoding_encode(&format!("{clean_t} {artist}")));
    let LrclibFetch::Found(resp) = lrclib_get(http, &url).await else { return None };
    let items: Vec<serde_json::Value> = resp.json().await.ok()?;
    let (title_n, artist_n) = (normalize_name(&clean_t), normalize_name(artist));
    let first_artist = normalize_name(artist.split(',').next().unwrap_or(artist));
    let raw_artist = artist.trim().to_lowercase();
    let same_artist = |a: &str, raw: &str| {
        if artist_n.is_empty() {
            return raw.trim() == raw_artist;
        }
        let long = |x: &str| x.chars().count() >= 4;
        !a.is_empty()
            && (a == artist_n
                || (long(&first_artist) && a.contains(&first_artist))
                || (long(a) && artist_n.contains(a)))
    };
    let names_match = |v: &serde_json::Value| {
        let field = |k: &str| normalize_name(v[k].as_str().unwrap_or(""));
        let (t, a) = (field("trackName"), field("artistName"));
        let raw = v["artistName"].as_str().unwrap_or("").to_lowercase();
        (t == title_n && same_artist(&a, &raw)) || (!artist_n.is_empty() && t == artist_n && a == title_n)
    };
    items
        .iter()
        .filter(|v| names_match(v))
        .filter(|v| match (duration_secs, v["duration"].as_f64()) {
            (Some(want), Some(got)) => (got - want as f64).abs() <= 4.0,
            _ => true,
        })
        .find_map(|v| v["syncedLyrics"].as_str().map(str::trim).filter(|t| usable_synced(t)).map(String::from))
}

/// Запрос текста песни из открытой глобальной базы LRCLIB (https://lrclib.net)
pub async fn fetch_lrclib_lyrics(
    http: &reqwest::Client,
    artist: &str,
    title: &str,
    duration_secs: Option<u64>,
) -> Option<String> {
    let clean_artist = artist.trim();
    let clean_title = clean_title_for_lyrics(title);
    if clean_artist.is_empty() || clean_title.is_empty() {
        return None;
    }

    let encoded_artist = urlencoding_encode(clean_artist);
    let encoded_title = urlencoding_encode(&clean_title);

    let make_url = |dur: Option<u64>| {
        if let Some(secs) = dur {
            format!(
                "https://lrclib.net/api/get?artist_name={}&track_name={}&duration={}",
                encoded_artist, encoded_title, secs
            )
        } else {
            format!(
                "https://lrclib.net/api/get?artist_name={}&track_name={}",
                encoded_artist, encoded_title
            )
        }
    };

    let mut plain_fallback: Option<String> = None;
    let mut resp = lrclib_get(http, &make_url(duration_secs)).await;
    // Если запрос с точной длительностью вернул 404, повторяем поиск без ограничения длительности
    if matches!(resp, LrclibFetch::NotFound) && duration_secs.is_some() {
        resp = lrclib_get(http, &make_url(None)).await;
    }
    let resp = match resp {
        LrclibFetch::Found(r) => Some(r),
        LrclibFetch::Failed => return None,
        LrclibFetch::NotFound => None,
    };

    if let Some(response) = resp {
        if let Ok(data) = response.json::<LrclibResponse>().await {
            if let Some(synced) = data.synced_lyrics {
                let trimmed = synced.trim();
                if usable_synced(trimmed) {
                    return Some(trimmed.to_string());
                }
            }
            if let Some(plain) = data.plain_lyrics {
                let trimmed = plain.trim();
                if !trimmed.is_empty() {
                    plain_fallback = Some(trimmed.to_string());
                }
            }
        }
    }

    // Точное совпадение дало только обычный текст или 404: ищем запись с таймингами через поиск
    if let Some(synced) = lrclib_search_synced(http, clean_artist, &clean_title, duration_secs).await {
        return Some(synced);
    }

    plain_fallback
}

/// Получение текста песни с дисковым кэшированием для мгновенного доступа
pub async fn get_cached_or_fetch_lrclib(
    http: &reqwest::Client,
    track_id: &str,
    artist: &str,
    title: &str,
    duration_secs: Option<u64>,
) -> Option<String> {
    let cache_path = crate::cache::lyrics_cache_path(track_id);
    if let Ok(cached) = std::fs::read_to_string(&cache_path) {
        if usable_synced(&cached) || !cached.trim().is_empty() {
            return Some(cached);
        }
    }

    let lyrics = fetch_lrclib_lyrics(http, artist, title, duration_secs).await;
    if let Some(text) = &lyrics {
        let _ = std::fs::write(&cache_path, text);
    }
    lyrics
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_title_for_lyrics() {
        assert_eq!(clean_title_for_lyrics("Dear Diary, (Lyric Video)"), "Dear Diary,");
        assert_eq!(clean_title_for_lyrics("Happy Song [Official Video]"), "Happy Song");
        assert_eq!(clean_title_for_lyrics("Change (In the House of Flies)"), "Change (In the House of Flies)");
    }

    #[test]
    fn test_usable_synced() {
        let sample_good = "[00:10.00] Line 1\n[00:15.00] Line 2\n[00:20.00] Line 3\n[00:25.00] Line 4";
        assert!(usable_synced(sample_good));

        let sample_short = "[00:10.00] Only one line";
        assert!(!usable_synced(sample_short));
    }
}
