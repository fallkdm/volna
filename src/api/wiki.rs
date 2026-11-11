//! Краткая справка об исполнителе из Википедии (публичный REST API, текст под CC BY-SA).

use serde_json::Value;

use super::client::urlencoding_encode;

pub(crate) const USER_AGENT: &str = "Volna/0.1 (unofficial Yandex Music client; GPL-3.0)";
/// Подстроки описания страницы, по которым понятно, что это музыкант или коллектив
const MUSIC_WORDS: [&str; 17] = [
    "групп", "музык", "певец", "певиц", "исполнител", "рэпер", "композитор", "диджей", "дуэт", "вокал",
    "band", "singer", "rapper", "musician", "composer", "producer", "dj",
];
const MAX_CHARS: usize = 600;

pub struct ArtistSummary {
    pub extract: String,
    pub url: String,
    /// Название источника и условия использования для подписи под текстом
    pub source: &'static str,
    pub license: &'static str,
}

/// Заголовок страницы подходит, если совпадает с именем или это «Имя (уточнение)»
fn title_matches(title: &str, name: &str) -> bool {
    let (t, n) = (title.to_lowercase(), name.trim().to_lowercase());
    !n.is_empty() && (t == n || t.starts_with(&format!("{} (", n)))
}

/// Имя артиста в ответе должно совпадать с искомым (без учёта регистра)
pub(crate) fn same_artist(found: &str, name: &str) -> bool {
    !name.trim().is_empty() && found.trim().to_lowercase() == name.trim().to_lowercase()
}

/// Ищет ASCII-подстроку без учёта регистра и возвращает байтовое смещение на границе UTF-8 символов
pub(crate) fn find_ascii_ci(haystack: &str, needle: &str) -> Option<usize> {
    debug_assert!(needle.is_ascii());
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}

fn decode_html_entities(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&laquo;", "«")
        .replace("&raquo;", "»")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&#X27;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

fn is_html_tag_named(tag_lower: &str, prefix: &str) -> bool {
    let Some(rest) = tag_lower.strip_prefix(prefix) else {
        return false;
    };
    rest.starts_with('>') || rest.starts_with('/') || rest.starts_with(char::is_whitespace)
}

fn is_closing_heading_tag(tag_lower: &str) -> bool {
    let Some(rest) = tag_lower.strip_prefix("</h") else {
        return false;
    };
    let bytes = rest.as_bytes();
    bytes.first().is_some_and(u8::is_ascii_digit)
        && bytes.get(1).is_some_and(|b| *b == b'>' || b.is_ascii_whitespace())
}

/// Удаляет HTML-теги и декодирует основные HTML-сущности.
/// При `drop_links == true` вырезает ссылки `<a>...</a>` вместе с текстом (например, «Read more on Last.fm»).
pub(crate) fn clean_html_entities_and_tags(html: &str, drop_links: bool) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let Some(end) = tail.find('>') else { break };
        let tag = tail[..=end].to_lowercase();
        if drop_links && is_html_tag_named(&tag, "<a") {
            match find_ascii_ci(tail, "</a>") {
                Some(close) => {
                    rest = &tail[close + 4..];
                    continue;
                }
                None => break,
            }
        }
        if is_html_tag_named(&tag, "<br")
            || is_html_tag_named(&tag, "</li")
            || is_html_tag_named(&tag, "<hr")
            || is_html_tag_named(&tag, "</div")
            || is_closing_heading_tag(&tag)
        {
            out.push('\n');
        } else if is_html_tag_named(&tag, "</p") {
            out.push_str("\n\n");
        } else if (is_html_tag_named(&tag, "<p") || is_html_tag_named(&tag, "<li"))
            && !out.is_empty()
            && !out.ends_with('\n')
        {
            out.push('\n');
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    decode_html_entities(&out)
}

/// Нормализует `\r\n` -> `\n`, сжимает пробелы внутри строк и оставляет максимум две `\n` подряд
pub(crate) fn normalize_paragraphs(text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut empty_run = 0usize;
    for raw_line in text.replace("\r\n", "\n").lines() {
        let line = raw_line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            if !lines.is_empty() && empty_run == 0 {
                lines.push(String::new());
            }
            empty_run += 1;
        } else {
            empty_run = 0;
            lines.push(line);
        }
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.join("\n")
}

fn looks_musical(description: &str) -> bool {
    let d = description.to_lowercase();
    MUSIC_WORDS.iter().any(|w| d.contains(w))
}

/// Обрезает текст до `MAX_CHARS` по границе предложения, если оно есть в первой половине
pub(crate) fn trim_extract(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= MAX_CHARS {
        return text.to_string();
    }
    let cut: String = text.chars().take(MAX_CHARS).collect();
    match cut.rfind(". ") {
        Some(i) if i > cut.len() / 2 => cut[..=i].to_string(),
        _ => format!("{}…", cut.trim_end()),
    }
}

async fn get_json(client: &reqwest::Client, url: &str) -> Option<Value> {
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json().await.ok()
}

/// Ищет страницу по `query`; заголовок должен совпадать с `name`, а `accept(описание, текст)` подтверждает тип страницы
async fn summary_in(
    client: &reqwest::Client,
    lang: &str,
    query: &str,
    name: &str,
    accept: &dyn Fn(&str, &str) -> bool,
) -> Option<ArtistSummary> {
    let search = format!(
        "https://{lang}.wikipedia.org/w/api.php?action=query&list=search&srlimit=5&format=json&srsearch={}",
        urlencoding_encode(query)
    );
    let found = get_json(client, &search).await?;
    let titles: Vec<String> = found["query"]["search"]
        .as_array()?
        .iter()
        .filter_map(|r| r["title"].as_str().map(str::to_string))
        .filter(|t| title_matches(t, name))
        .collect();

    for title in titles {
        let url = format!(
            "https://{lang}.wikipedia.org/api/rest_v1/page/summary/{}",
            urlencoding_encode(&title.replace(' ', "_"))
        );
        let Some(page) = get_json(client, &url).await else { continue };
        let description = page["description"].as_str().unwrap_or_default();
        let extract = page["extract"].as_str().unwrap_or_default();
        if page["type"] == "standard" && !extract.is_empty() && accept(description, extract) {
            let link = page["content_urls"]["desktop"]["page"].as_str().unwrap_or_default();
            return Some(ArtistSummary {
                extract: trim_extract(extract),
                url: link.to_string(),
                source: "Википедия",
                license: "CC BY-SA",
            });
        }
    }
    None
}

/// Ищет справку об исполнителе: сначала русская Википедия, затем английская.
/// Лучше ничего не показать, чем чужую страницу, поэтому нужны точное имя и музыкальное описание.
pub async fn artist_summary(name: &str) -> Option<ArtistSummary> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .ok()?;
    let accept = |description: &str, _: &str| looks_musical(description);
    match summary_in(&client, "ru", name, name, &accept).await {
        Some(s) => Some(s),
        None => summary_in(&client, "en", name, name, &accept).await,
    }
}

const ALBUM_WORDS: [&str; 10] = ["альбом", "album", "сингл", "single", "ep", "сборник", "compilation", "саундтрек", "soundtrack", "mixtape"];

/// Страница об альбоме: в описании тип записи, а в описании или тексте назван исполнитель
fn looks_like_album_by(description: &str, extract: &str, artist: &str) -> bool {
    let d = description.to_lowercase();
    let is_album = d.split(|c: char| !c.is_alphanumeric()).any(|w| ALBUM_WORDS.contains(&w));
    let artist = artist.trim().to_lowercase();
    is_album && !artist.is_empty() && (d.contains(&artist) || extract.to_lowercase().contains(&artist))
}

/// Справка об альбоме: русская Википедия, затем английская; нужны точное название и исполнитель
pub async fn album_summary(title: &str, artist: &str) -> Option<ArtistSummary> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .ok()?;
    let query = format!("{title} {artist}");
    let accept = |description: &str, extract: &str| looks_like_album_by(description, extract, artist);
    match summary_in(&client, "ru", &query, title, &accept).await {
        Some(s) => Some(s),
        None => summary_in(&client, "en", &query, title, &accept).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_must_match_name() {
        assert!(title_matches("Deftones", "deftones"));
        assert!(title_matches("Три дня дождя (группа)", "Три дня дождя"));
        assert!(!title_matches("Грязь (группа)", "Noisecream"));
        assert!(!title_matches("Дискография Deftones", "Deftones"));
    }

    #[test]
    fn album_page_must_name_the_artist() {
        assert!(looks_like_album_by("студийный альбом группы Deftones", "", "Deftones"));
        assert!(looks_like_album_by("2000 studio album", "White Pony is the third album by Deftones.", "Deftones"));
        assert!(!looks_like_album_by("студийный альбом группы Linkin Park", "", "Deftones"));
        assert!(!looks_like_album_by("американская метал-группа", "Deftones — группа.", "Deftones"));
    }

    #[test]
    fn music_description_filter() {
        assert!(looks_musical("американская альтернативная метал-группа"));
        assert!(!looks_musical("российский фильм 2016 года"));
    }

    #[test]
    fn long_extract_is_trimmed_at_sentence() {
        let text = format!("{} Конец.", "Слово. ".repeat(200));
        let t = trim_extract(&text);
        assert!(t.chars().count() <= MAX_CHARS && t.ends_with('.'));
        assert_eq!(trim_extract("Коротко."), "Коротко.");
    }

    #[test]
    fn html_cleaner_and_unicode_ascii_search_work() {
        let html = "<p>İstanbul &amp; <a href=\"x\">İzmir</a></p>\n\n\n<p>Test</p>";
        let cleaned = normalize_paragraphs(&clean_html_entities_and_tags(html, true));
        assert_eq!(cleaned, "İstanbul &\n\nTest");
        assert_eq!(find_ascii_ci("İstanbul<HR>", "<hr"), Some("İstanbul".len()));
    }

    /// Ходит в сеть: `cargo test wiki -- --ignored --nocapture`
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

    /// Ходит в сеть: `cargo test wiki::tests::live_album -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_album() {
        for (title, artist) in [("White Pony", "Deftones"), ("Deftones", "Deftones"), ("Ripe For Revenge", "Noisecream"), ("Hours", "The Weeknd")] {
            match album_summary(title, artist).await {
                Some(s) => println!("LIVE {title}/{artist}: {} | {}", s.url, s.extract.chars().take(70).collect::<String>()),
                None => println!("LIVE {title}/{artist}: —"),
            }
        }
    }
}
