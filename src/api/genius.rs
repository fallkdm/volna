//! Краткая справка об исполнителе из Genius (официальный API, нужен клиентский токен).

use serde_json::Value;

use super::client::urlencoding_encode;
use super::wiki::{trim_extract, ArtistSummary};

const USER_AGENT: &str = "Volna/0.1 (unofficial Yandex Music client; GPL-3.0)";

/// Имя артиста в Genius должно совпадать с искомым (без учёта регистра): чужого артиста не показываем
fn same_artist(found: &str, name: &str) -> bool {
    !name.trim().is_empty() && found.trim().to_lowercase() == name.trim().to_lowercase()
}

/// Описание в Genius часто пустое или «?»
fn usable(description: &str) -> bool {
    let d = description.trim();
    d.chars().count() > 20 && d != "?"
}

async fn get_json(client: &reqwest::Client, token: &str, url: &str) -> Option<Value> {
    let resp = client.get(url).bearer_auth(token).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json().await.ok()
}

/// Ищет справку об исполнителе в Genius; без токена ничего не делает
pub async fn artist_summary(name: &str) -> Option<ArtistSummary> {
    let token = super::auth::load_genius_token()?;
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .ok()?;

    let search = format!("https://api.genius.com/search?q={}", urlencoding_encode(name));
    let found = get_json(&client, &token, &search).await?;
    let artist_id = found["response"]["hits"].as_array()?.iter().find_map(|h| {
        let a = &h["result"]["primary_artist"];
        same_artist(a["name"].as_str()?, name).then(|| a["id"].as_u64()).flatten()
    })?;

    let page = get_json(&client, &token, &format!("https://api.genius.com/artists/{artist_id}?text_format=plain")).await?;
    let artist = &page["response"]["artist"];
    let description = artist["description"]["plain"].as_str().unwrap_or_default();
    if !usable(description) {
        return None;
    }
    Some(ArtistSummary {
        extract: trim_extract(description),
        url: artist["url"].as_str().unwrap_or_default().to_string(),
        source: "Genius",
        license: "",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artist_name_must_match() {
        assert!(same_artist("Noisecream ", "noisecream"));
        assert!(!same_artist("Noisecream Jr", "Noisecream"));
    }

    #[test]
    fn empty_descriptions_are_skipped() {
        assert!(!usable("?"));
        assert!(!usable("  "));
        assert!(usable("Американская метал-группа из Сакраменто, основанная в 1988 году."));
    }

    /// Ходит в сеть и берёт токен из конфига: `cargo test genius -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_lookup() {
        for name in ["Noisecream", "Deftones", "Три дня дождя", "Kira Voss"] {
            match artist_summary(name).await {
                Some(s) => println!("LIVE {name}: {} | {}", s.url, s.extract.chars().take(80).collect::<String>()),
                None => println!("LIVE {name}: —"),
            }
        }
    }
}
