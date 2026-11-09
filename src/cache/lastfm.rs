use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

static LISTENERS_CACHE: LazyLock<RwLock<HashMap<String, u64>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

static HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent("Volna/0.1 (unofficial Yandex Music client; GPL-3.0)")
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default()
});

/// Получение количества слушателей артиста в месяц.
/// Сначала проверяет in-memory кэш, затем запрашивает Last.fm (если есть ключ) или Deezer.
pub async fn get_artist_listeners(
    artist_name: &str,
) -> Result<Option<u64>, Box<dyn std::error::Error + Send + Sync>> {
    let name_trimmed = artist_name.trim();
    if name_trimmed.is_empty() {
        return Ok(None);
    }
    let key = name_trimmed.to_lowercase();

    if let Ok(guard) = LISTENERS_CACHE.read() {
        if let Some(&count) = guard.get(&key) {
            return Ok(Some(count));
        }
    }

    let mut count_opt = None;

    // 1. Попытка запроса через Last.fm API
    if let Some(api_key) = crate::api::auth::load_lastfm_key() {
        let url = format!(
            "https://ws.audioscrobbler.com/2.0/?method=artist.getinfo&autocorrect=0&format=json&api_key={}&artist={}",
            crate::api::client::urlencoding_encode(&api_key),
            crate::api::client::urlencoding_encode(name_trimmed)
        );
        if let Ok(resp) = HTTP_CLIENT.get(&url).send().await {
            if resp.status().is_success() {
                if let Ok(json) = resp.json::<serde_json::Value>().await {
                    if let Some(listeners_str) = json["artist"]["stats"]["listeners"].as_str() {
                        if let Ok(num) = listeners_str.parse::<u64>() {
                            count_opt = Some(num);
                        }
                    } else if let Some(num) = json["artist"]["stats"]["listeners"].as_u64() {
                        count_opt = Some(num);
                    }
                }
            }
        }
    }

    // 2. Fallback через Deezer artist search API (nb_fan)
    if count_opt.is_none() {
        let escaped = glib::Uri::escape_string(name_trimmed, None, true);
        let url = format!("https://api.deezer.com/search/artist?q={}&limit=1", escaped);
        if let Ok(resp) = HTTP_CLIENT.get(&url).send().await {
            if resp.status().is_success() {
                if let Ok(json) = resp.json::<serde_json::Value>().await {
                    if let Some(first) = json.get("data").and_then(|d| d.as_array()).and_then(|a| a.first()) {
                        if let Some(fans) = first.get("nb_fan").and_then(|f| f.as_u64()) {
                            count_opt = Some(fans);
                        }
                    }
                }
            }
        }
    }

    if let Some(count) = count_opt {
        if let Ok(mut guard) = LISTENERS_CACHE.write() {
            guard.insert(key, count);
        }
        Ok(Some(count))
    } else {
        Ok(None)
    }
}
