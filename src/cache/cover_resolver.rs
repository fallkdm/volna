use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

static COVER_CACHE: LazyLock<RwLock<HashMap<String, Option<String>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

static ARTIST_CACHE: LazyLock<RwLock<HashMap<String, Option<String>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

static HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36")
        .timeout(std::time::Duration::from_millis(3500))
        .build()
        .unwrap_or_default()
});

/// Очистка названия трека от мусора YouTube: `(Official Video)`, `[Live]`, `(Audio)` и т.д.
pub fn clean_title_for_search(title: &str) -> String {
    let mut s = title.to_string();
    let patterns = [
        "(Official Video)", "(Official Music Video)", "(Official Audio)",
        "(Official Lyric Video)", "(Lyric Video)", "(Visualizer)",
        "(Audio)", "(Video)", "[Official Video]", "[Official Music Video]",
        "[Official Audio]", "[Lyric Video]", "[Audio]", "[Video]",
        "(Remastered)", "[Remastered]", "(Live)", "[Live]",
        "(Music Video)", "[Music Video]", "(Official)", "[Official]",
        "(Audio Only)", "[Audio Only]", "(Clip)", "[Clip]",
        "(4K)", "(HD)", "[4K]", "[HD]", "(HQ)", "[HQ]",
    ];
    for pat in &patterns {
        s = s.replace(pat, " ");
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Поиск студийной обложки высокого качества через Deezer и iTunes Search API
pub async fn resolve_studio_cover(artist: &str, title: &str) -> Option<String> {
    let artist_trimmed = artist.trim();
    let title_clean = clean_title_for_search(title);
    if artist_trimmed.is_empty() && title_clean.is_empty() {
        return None;
    }

    let cache_key = format!("{}:{}", artist_trimmed.to_lowercase(), title_clean.to_lowercase());

    // 1. Проверяем кэш в памяти
    {
        let guard = COVER_CACHE.read().ok()?;
        if let Some(cached) = guard.get(&cache_key) {
            return cached.clone();
        }
    }

    let query = if artist_trimmed.is_empty() {
        title_clean.clone()
    } else {
        format!("{artist_trimmed} {title_clean}")
    };

    let result = resolve_from_apis(&query).await;

    // Сохраняем в кэш
    if let Ok(mut guard) = COVER_CACHE.write() {
        guard.insert(cache_key, result.clone());
    }

    result
}

async fn resolve_from_apis(query: &str) -> Option<String> {
    // 1. Приоритет 1 — Deezer Search API (быстро, ~60 мс, 1000x1000)
    if let Some(url) = search_deezer(query).await {
        return Some(url);
    }

    // 2. Приоритет 2 — iTunes Search API (Apple Music, ~90 мс, 600x600)
    if let Some(url) = search_itunes(query).await {
        return Some(url);
    }

    None
}

async fn search_deezer(query: &str) -> Option<String> {
    let escaped = glib::Uri::escape_string(query, None, true);
    let url = format!("https://api.deezer.com/search?q={}&limit=1", escaped);
    let resp = HTTP_CLIENT
        .get(&url)
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        return None;
    }

    let json: serde_json::Value = resp.json().await.ok()?;
    let first = json.get("data")?.as_array()?.first()?;
    let album = first.get("album")?;

    album
        .get("cover_xl")
        .or_else(|| album.get("cover_big"))
        .or_else(|| album.get("cover_medium"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

async fn search_itunes(query: &str) -> Option<String> {
    let escaped = glib::Uri::escape_string(query, None, true);
    let url = format!("https://itunes.apple.com/search?term={}&entity=song&limit=1", escaped);
    let resp = HTTP_CLIENT
        .get(&url)
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        return None;
    }

    let json: serde_json::Value = resp.json().await.ok()?;
    let first = json.get("results")?.as_array()?.first()?;
    let artwork = first.get("artworkUrl100")?.as_str()?;

    // Заменяем 100x100bb на 600x600bb для получения студийного разрешения
    Some(artwork.replace("100x100bb", "600x600bb"))
}

/// Поиск студийного квадратного портрета артиста через Deezer и iTunes Search API
pub async fn resolve_artist_avatar(artist_name: &str) -> Option<String> {
    let name_trimmed = artist_name.trim();
    if name_trimmed.is_empty() {
        return None;
    }

    let cache_key = name_trimmed.to_lowercase();

    // 1. Проверяем кэш в памяти
    {
        if let Ok(guard) = ARTIST_CACHE.read() {
            if let Some(cached) = guard.get(&cache_key) {
                return cached.clone();
            }
        }
    }

    let result = resolve_artist_from_apis(name_trimmed).await;

    // Сохраняем результат в кэш
    if let Ok(mut guard) = ARTIST_CACHE.write() {
        guard.insert(cache_key, result.clone());
    }

    result
}

async fn resolve_artist_from_apis(artist_name: &str) -> Option<String> {
    // 1. Приоритет 1 — Deezer Artist Search API (1000x1000 picture_xl или 500x500 picture_big)
    if let Some(url) = search_deezer_artist(artist_name).await {
        return Some(url);
    }

    // 2. Приоритет 2 — iTunes Search API (Apple Music)
    if let Some(url) = search_itunes_artist(artist_name).await {
        return Some(url);
    }

    None
}

async fn search_deezer_artist(artist_name: &str) -> Option<String> {
    let escaped = glib::Uri::escape_string(artist_name, None, true);
    let url = format!("https://api.deezer.com/search/artist?q={}&limit=1", escaped);
    let resp = HTTP_CLIENT.get(&url).send().await.ok()?;

    if !resp.status().is_success() {
        return None;
    }

    let json: serde_json::Value = resp.json().await.ok()?;
    let first = json.get("data")?.as_array()?.first()?;

    first
        .get("picture_xl")
        .or_else(|| first.get("picture_big"))
        .or_else(|| first.get("picture_medium"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

async fn search_itunes_artist(artist_name: &str) -> Option<String> {
    let escaped = glib::Uri::escape_string(artist_name, None, true);
    let url = format!("https://itunes.apple.com/search?term={}&entity=musicArtist&limit=1", escaped);
    let resp = HTTP_CLIENT.get(&url).send().await.ok()?;

    if !resp.status().is_success() {
        return None;
    }

    let json: serde_json::Value = resp.json().await.ok()?;
    let first = json.get("results")?.as_array()?.first()?;

    if let Some(art) = first.get("artworkUrl100").or_else(|| first.get("artworkUrl60")).and_then(|v| v.as_str()) {
        return Some(art.replace("100x100bb", "600x600bb"));
    }

    if let Some(link) = first.get("artistLinkUrl").and_then(|v| v.as_str()) {
        if let Ok(html_resp) = HTTP_CLIENT.get(link).send().await {
            if html_resp.status().is_success() {
                if let Ok(html) = html_resp.text().await {
                    if let Some(og_image) = extract_meta_og_image(&html) {
                        return Some(og_image);
                    }
                }
            }
        }
    }

    None
}

fn extract_meta_og_image(html: &str) -> Option<String> {
    let marker = "property=\"og:image\"";
    let pos = html.find(marker)?;
    let slice = &html[pos..pos.saturating_add(300).min(html.len())];
    let content_idx = slice.find("content=\"")?;
    let start = content_idx + "content=\"".len();
    let end = slice[start..].find('"')?;
    let raw_url = &slice[start..start + end];
    if raw_url.starts_with("http") {
        let high_res = if raw_url.contains("/1200x630cw.") {
            raw_url.replace("/1200x630cw.png", "/600x600bb.jpg").replace("/1200x630cw.", "/600x600bb.")
        } else {
            raw_url.to_string()
        };
        Some(high_res)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_title() {
        assert_eq!(
            clean_title_for_search("Happy Song (Official Audio)"),
            "Happy Song"
        );
        assert_eq!(
            clean_title_for_search("My Own Summer (Shove It) [Music Video]"),
            "My Own Summer (Shove It)"
        );
    }

    #[tokio::test]
    async fn test_deezer_resolver_live() {
        let cover = resolve_studio_cover("Deftones", "Change (In the House of Flies)").await;
        println!("[test] Deftones Change cover: {:?}", cover);
        assert!(cover.is_some(), "Should find Deftones Change cover");
        let url = cover.unwrap();
        assert!(url.starts_with("https://"), "Url should start with https://");
    }

    #[tokio::test]
    async fn test_deezer_artist_resolver_live() {
        let avatar = resolve_artist_avatar("Deftones").await;
        println!("[test] Deftones avatar: {:?}", avatar);
        assert!(avatar.is_some(), "Should find Deftones avatar");
        let url = avatar.unwrap();
        assert!(url.starts_with("https://"), "Url should start with https://");
        assert!(url.contains("1000x1000") || url.contains("500x500") || url.contains("600x600"), "Should be high resolution");
    }

    #[tokio::test]
    async fn test_artist_avatar_russian() {
        let avatar = resolve_artist_avatar("Три дня дождя").await;
        println!("[test] Tri Dnya Dozhdya avatar: {:?}", avatar);
        assert!(avatar.is_some(), "Should find Tri Dnya Dozhdya avatar");
    }
}
