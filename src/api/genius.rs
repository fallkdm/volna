//! Краткая справка об исполнителе из Genius (официальный API, нужен клиентский токен).

use serde_json::Value;

use super::client::urlencoding_encode;
use super::wiki::{
    clean_html_entities_and_tags, find_ascii_ci, normalize_paragraphs, same_artist, trim_extract, ArtistSummary,
    USER_AGENT,
};

/// Описание в Genius часто пустое или «?»
fn usable(description: &str) -> bool {
    let d = description.trim();
    d.chars().count() > 20 && d != "?"
}

/// Очищает HTML-описание артиста из Genius: отсекает служебный подвал с `<hr>`,
/// убирает теги, нормализует переводы строк и декодирует HTML-сущности
pub(crate) fn clean_genius_html(html: &str) -> String {
    let clean = |s: &str| normalize_paragraphs(&clean_html_entities_and_tags(s, false));
    if let Some(pos) = find_ascii_ci(html, "<hr")
        && html
            .as_bytes()
            .get(pos + 3)
            .is_some_and(|b| matches!(b, b'>' | b'/' | b' ' | b'\t' | b'\n' | b'\r'))
    {
        let before = clean(&html[..pos]);
        if usable(&before) {
            return before;
        }
    }
    clean(html)
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

    let page = get_json(&client, &token, &format!("https://api.genius.com/artists/{artist_id}?text_format=html")).await?;
    let artist = &page["response"]["artist"];
    let raw = page["response"]["artist"]["description"]["html"].as_str().unwrap_or_default();
    let description = clean_genius_html(raw);
    if !usable(&description) {
        return None;
    }
    Some(ArtistSummary {
        extract: trim_extract(&description),
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

    #[test]
    fn html_description_cuts_footer_and_decodes_entities() {
        let html = "<p>&laquo;Твоя волна&raquo; — проект &quot;артиста&quot; <a href=\"https://genius.com/artists/X\">Ивана</a> &amp; Co.<br/>Вторая строка с&nbsp;&#39;апострофом&#x27; &lt;2024&gt;.</p>\n\n\n<p>Третий абзац описания.</p><hr><center><h3>Дискография</h3></center><hr><ul><li>Альбом 1 (2022)</li><li>Сообщество ВКонтакте / Телеграм-канал / YouTube</li></ul>";
        assert_eq!(
            clean_genius_html(html),
            "«Твоя волна» — проект \"артиста\" Ивана & Co.\nВторая строка с 'апострофом' <2024>.\n\nТретий абзац описания."
        );
    }

    #[test]
    fn short_prefix_before_hr_keeps_full_cleaned_html() {
        let html = "<p>?</p><HR /><ul><li>Основное описание артиста начинается после разделителя и достаточно длинное.</li><li>Второй пункт списка.</li></ul>";
        assert_eq!(
            clean_genius_html(html),
            "?\n\nОсновное описание артиста начинается после разделителя и достаточно длинное.\nВторой пункт списка."
        );
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
