//! Краткая справка об исполнителе из Last.fm (официальный API, нужен бесплатный ключ).
//! Тексты биографий на Last.fm доступны на условиях CC BY-SA; ссылка на страницу артиста обязательна.

use serde_json::Value;

use super::client::urlencoding_encode;
use super::wiki::{trim_extract, ArtistSummary};

const USER_AGENT: &str = "Volna/0.1 (unofficial Yandex Music client; GPL-3.0)";

/// Убирает HTML из биографии: ссылки «Read more on Last.fm» вместе с текстом, остальные теги и сущности
fn strip_html(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let Some(end) = tail.find('>') else { break };
        let tag = tail[..=end].to_lowercase();
        if tag.starts_with("<a ") {
            // текст внутри ссылки (в биографии это только «Read more on Last.fm») отбрасываем
            match tail.to_lowercase().find("</a>") {
                Some(close) => {
                    rest = &tail[close + 4..];
                    continue;
                }
                None => break,
            }
        }
        if tag.starts_with("<br") || tag.starts_with("</p") {
            out.push(' ');
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    let decoded = out
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Имя в ответе должно совпадать с искомым (без учёта регистра)
fn same_artist(found: &str, name: &str) -> bool {
    !name.trim().is_empty() && found.trim().to_lowercase() == name.trim().to_lowercase()
}

/// Ищет справку об исполнителе в Last.fm; без ключа ничего не делает
pub async fn artist_summary(name: &str) -> Option<ArtistSummary> {
    let key = super::auth::load_lastfm_key()?;
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .ok()?;
    let url = format!(
        "https://ws.audioscrobbler.com/2.0/?method=artist.getinfo&autocorrect=0&lang=ru&format=json&api_key={}&artist={}",
        urlencoding_encode(&key),
        urlencoding_encode(name)
    );
    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let data: Value = resp.json().await.ok()?;
    let artist = &data["artist"];
    if !same_artist(artist["name"].as_str()?, name) {
        return None;
    }
    let text = strip_html(artist["bio"]["summary"].as_str().unwrap_or_default());
    if text.chars().count() < 40 {
        return None;
    }
    Some(ArtistSummary {
        extract: trim_extract(&text),
        url: artist["url"].as_str().unwrap_or_default().to_string(),
        source: "Last.fm",
        license: "CC BY-SA",
    })
}

/// Справка об альбоме из Last.fm (`wiki` у альбома часто пустой); без ключа ничего не делает
pub async fn album_summary(title: &str, artist: &str) -> Option<ArtistSummary> {
    let key = super::auth::load_lastfm_key()?;
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .ok()?;
    let url = format!(
        "https://ws.audioscrobbler.com/2.0/?method=album.getinfo&autocorrect=0&lang=ru&format=json&api_key={}&artist={}&album={}",
        urlencoding_encode(&key),
        urlencoding_encode(artist),
        urlencoding_encode(title)
    );
    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let data: Value = resp.json().await.ok()?;
    let album = &data["album"];
    if !same_artist(album["name"].as_str()?, title) || !same_artist(album["artist"].as_str()?, artist) {
        return None;
    }
    let text = strip_html(album["wiki"]["summary"].as_str().unwrap_or_default());
    if text.chars().count() < 40 {
        return None;
    }
    Some(ArtistSummary {
        extract: trim_extract(&text),
        url: album["url"].as_str().unwrap_or_default().to_string(),
        source: "Last.fm",
        license: "CC BY-SA",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bio_html_is_cleaned() {
        let html = "Deftones &amp; <b>друзья</b> — группа.<br />Вторая &quot;строка&quot;. <a href=\"https://www.last.fm/music/Deftones\">Read more on Last.fm</a>";
        assert_eq!(strip_html(html), "Deftones & друзья — группа. Вторая \"строка\".");
    }

    #[test]
    fn only_the_link_means_empty_bio() {
        assert_eq!(strip_html("<a href=\"x\">Read more on Last.fm</a>"), "");
    }

    #[test]
    fn artist_name_must_match() {
        assert!(same_artist("Deftones", "deftones"));
        assert!(!same_artist("Deftones Tribute", "Deftones"));
    }

    /// Ходит в сеть и берёт ключ из конфига: `cargo test lastfm -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_lookup() {
        for name in ["Deftones", "Три дня дождя", "Noisecream", "Kira Voss"] {
            match artist_summary(name).await {
                Some(s) => println!("LIVE {name}: {} | {}", s.url, s.extract.chars().take(80).collect::<String>()),
                None => println!("LIVE {name}: —"),
            }
        }
    }
}
