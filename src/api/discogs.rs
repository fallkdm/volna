//! Краткая справка об исполнителе из открытого каталога Discogs (без API-ключа, данные под CC0).

use serde_json::Value;

use super::client::urlencoding_encode;
use super::wiki::{normalize_paragraphs, same_artist, trim_extract, ArtistSummary, USER_AGENT};

fn is_discogs_named_tag(inner: &str) -> bool {
    let bytes = inner.as_bytes();
    bytes.get(1) == Some(&b'=')
        && matches!(
            bytes.first(),
            Some(b'a' | b'A' | b'l' | b'L' | b'm' | b'M' | b'r' | b'R')
        )
}

fn is_discogs_numeric_ref(inner: &str) -> bool {
    let bytes = inner.as_bytes();
    bytes.len() > 1
        && matches!(
            bytes.first(),
            Some(b'a' | b'A' | b'l' | b'L' | b'm' | b'M' | b'r' | b'R')
        )
        && bytes[1..].iter().all(u8::is_ascii_digit)
}

fn is_stripped_bbcode_tag(inner: &str) -> bool {
    (inner.len() >= 4 && inner.as_bytes()[..4].eq_ignore_ascii_case(b"url="))
        || inner.eq_ignore_ascii_case("b")
        || inner.eq_ignore_ascii_case("/b")
        || inner.eq_ignore_ascii_case("i")
        || inner.eq_ignore_ascii_case("/i")
        || inner.eq_ignore_ascii_case("u")
        || inner.eq_ignore_ascii_case("/u")
        || inner.eq_ignore_ascii_case("url")
        || inner.eq_ignore_ascii_case("/url")
}

/// Очищает BBCode-разметку профиля Discogs и нормализует переводы строк и пробелы
pub(crate) fn clean_discogs_bbcode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find('[') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let Some(end) = tail.find(']') else {
            out.push_str(tail);
            rest = "";
            break;
        };
        let inner = &tail[1..end];
        if is_discogs_named_tag(inner) {
            out.push_str(&inner[2..]);
            rest = &tail[end + 1..];
        } else if is_stripped_bbcode_tag(inner) || is_discogs_numeric_ref(inner) {
            rest = &tail[end + 1..];
        } else {
            out.push('[');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    normalize_paragraphs(&out)
}

/// Отбрасывает слишком короткие тексты и служебные заглушки Discogs
pub(crate) fn usable_profile(text: &str) -> bool {
    let t = text.trim();
    let len = t.chars().count();
    if len < 25 {
        return false;
    }
    let lower = t.to_lowercase();
    if lower.contains("do not use") {
        return false;
    }
    if len < 160 && (lower.starts_with("please use ") || lower.starts_with("incorrect artist")) {
        return false;
    }
    true
}

async fn get_json(client: &reqwest::Client, url: &str) -> Option<Value> {
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json().await.ok()
}

/// Ищет справку об исполнителе в Discogs по точному совпадению имени
pub async fn artist_summary(name: &str) -> Option<ArtistSummary> {
    if name.trim().is_empty() {
        return None;
    }
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .ok()?;

    let search = format!(
        "https://api.discogs.com/database/search?q={}&type=artist",
        urlencoding_encode(name)
    );
    let found = get_json(&client, &search).await?;
    let ids: Vec<u64> = found["results"]
        .as_array()?
        .iter()
        .filter(|r| r["title"].as_str().is_some_and(|t| same_artist(t, name)))
        .filter_map(|r| r["id"].as_u64())
        .take(3)
        .collect();

    for id in ids {
        let url = format!("https://api.discogs.com/artists/{id}");
        let Some(artist) = get_json(&client, &url).await else { continue };
        let profile = artist["profile"].as_str().unwrap_or_default();
        let cleaned = clean_discogs_bbcode(profile);
        if !usable_profile(&cleaned) {
            continue;
        }
        let page_url = artist["uri"]
            .as_str()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .unwrap_or("https://www.discogs.com")
            .to_string();
        return Some(ArtistSummary {
            extract: trim_extract(&cleaned),
            url: page_url,
            source: "Discogs",
            license: "CC0",
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bbcode_is_cleaned() {
        let raw = "[b]Deftones[/b] formed with [a=Chino Moreno] on [l=Maverick].\r\n\r\n\r\nAlbums like [m=White Pony] and [r=Single] ([2000]) [i]text[/i] [u]under[/u] [a12345] — [url=https://deftones.com]Text[/url] / [url]https://example.com[/url].";
        assert_eq!(
            clean_discogs_bbcode(raw),
            "Deftones formed with Chino Moreno on Maverick.\n\nAlbums like White Pony and Single ([2000]) text under — Text / https://example.com."
        );
        assert_eq!(clean_discogs_bbcode("[b]text[/b]"), "text");
    }

    #[test]
    fn profile_filtering_rejects_stubs_and_short_text() {
        let stub = clean_discogs_bbcode("[b]DO NOT USE.[/b]");
        assert!(!usable_profile(&stub));
        assert!(!usable_profile("DO NOT USE. Duplicate profile created by mistake."));
        assert!(!usable_profile("Please use [a=Another Artist] instead for releases."));
        assert!(!usable_profile("Incorrect artist entry, see the primary page."));
        assert!(!usable_profile("Too short text."));
        assert!(usable_profile(
            "American alternative metal band formed in Sacramento, California in 1988."
        ));
    }

    /// Ходит в сеть: `cargo test discogs -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_lookup() {
        for name in ["Deftones", "Noisecream", "Kira Voss"] {
            match artist_summary(name).await {
                Some(s) => println!("LIVE {name}: {} | {}", s.url, s.extract.chars().take(80).collect::<String>()),
                None => println!("LIVE {name}: —"),
            }
        }
    }
}
