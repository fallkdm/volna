//! Краткая справка об исполнителе из Википедии (публичный REST API, текст под CC BY-SA).

use serde_json::Value;

use super::client::urlencoding_encode;

pub(crate) const USER_AGENT: &str = "Volna/0.1 (unofficial Yandex Music client; GPL-3.0)";
/// Подстроки описания страницы, по которым понятно, что это музыкант или коллектив
const MUSIC_WORDS: &[&str] = &[
    "групп",
    "музык",
    "певец",
    "певиц",
    "исполнител",
    "рэпер",
    "композитор",
    "диджей",
    "дуэт",
    "вокал",
    "битмейкер",
    "мультиинструменталист",
    "песенник",
    "band",
    "singer",
    "rapper",
    "musician",
    "composer",
    "producer",
    "dj",
    "multi-instrumentalist",
    "songwriter",
    "beatmaker",
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

fn page_slug(title: &str) -> String {
    title.split_whitespace().collect::<Vec<_>>().join("_")
}

fn summary_url(lang: &str, slug: &str) -> String {
    format!(
        "https://{lang}.wikipedia.org/api/rest_v1/page/summary/{}",
        urlencoding_encode(slug)
    )
}

fn extract_summary_from_page(
    page: &Value,
    fallback_title: &str,
    accept: Option<&dyn Fn(&str, &str) -> bool>,
) -> Option<(ArtistSummary, String)> {
    let description = page["description"].as_str().unwrap_or_default();
    let extract = page["extract"].as_str().unwrap_or_default();
    if page["type"] != "standard" || extract.is_empty() {
        return None;
    }
    if let Some(check) = accept
        && !check(description, extract)
    {
        return None;
    }
    let link = page["content_urls"]["desktop"]["page"]
        .as_str()
        .unwrap_or_default();
    let canonical_title = page["titles"]["normalized"]
        .as_str()
        .or_else(|| page["title"].as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback_title);
    Some((
        ArtistSummary {
            extract: trim_extract(extract),
            url: link.to_string(),
            source: "Википедия",
            license: "CC BY-SA",
        },
        canonical_title.to_string(),
    ))
}

/// Извлекает точный заголовок русской статьи из ответа Wikipedia `prop=langlinks&lllang=ru`,
/// игнорируя отсутствующие страницы с отрицательным `pageid`
pub(crate) fn extract_ru_langlink(json: &Value) -> Option<String> {
    let pages = json["query"]["pages"].as_object()?;
    for (page_id, page) in pages {
        let is_negative_id = page_id.trim().starts_with('-')
            || page_id.trim().parse::<i64>().is_ok_and(|id| id < 0)
            || page["pageid"].as_i64().is_some_and(|id| id < 0);
        if is_negative_id {
            continue;
        }
        let Some(links) = page["langlinks"].as_array() else {
            continue;
        };
        if let Some(title) = links
            .iter()
            .find(|link| link["lang"].as_str().is_none_or(|lang| lang == "ru"))
            .and_then(|link| link["*"].as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Some(title.to_string());
        }
    }
    None
}

/// Ищет страницу по `name` (сначала прямой REST-запрос, затем поиск по `query`);
/// возвращает сводку и канонический заголовок страницы
async fn summary_in_with_title(
    client: &reqwest::Client,
    lang: &str,
    query: &str,
    name: &str,
    accept: &dyn Fn(&str, &str) -> bool,
) -> Option<(ArtistSummary, String)> {
    let clean_name = name.trim();
    if clean_name.is_empty() {
        return None;
    }

    // Быстрый путь: прямой REST-запрос мгновенно резолвит редиректы вроде Flo_Rida -> Флоу Райда
    let direct_slug = page_slug(clean_name);
    let direct_url = summary_url(lang, &direct_slug);
    let mut skip_direct = false;
    if let Some(page) = get_json(client, &direct_url).await {
        skip_direct = true;
        if let Some(res) = extract_summary_from_page(&page, clean_name, Some(accept)) {
            return Some(res);
        }
    }

    let search = format!(
        "https://{lang}.wikipedia.org/w/api.php?action=query&list=search&srlimit=5&format=json&srsearch={}",
        urlencoding_encode(query)
    );
    let found = get_json(client, &search).await?;
    let titles: Vec<String> = found["query"]["search"]
        .as_array()?
        .iter()
        .filter_map(|r| r["title"].as_str().map(str::to_string))
        .filter(|t| title_matches(t, clean_name))
        .collect();

    for title in titles {
        let slug = page_slug(&title);
        if skip_direct && slug == direct_slug {
            continue;
        }
        let url = summary_url(lang, &slug);
        let Some(page) = get_json(client, &url).await else {
            continue;
        };
        if let Some(res) = extract_summary_from_page(&page, &title, Some(accept)) {
            return Some(res);
        }
    }
    None
}

/// Ищет страницу по `query`; заголовок должен совпадать с `name`, а `accept(описание, текст)` подтверждает тип страницы
async fn summary_in(
    client: &reqwest::Client,
    lang: &str,
    query: &str,
    name: &str,
    accept: &dyn Fn(&str, &str) -> bool,
) -> Option<ArtistSummary> {
    summary_in_with_title(client, lang, query, name, accept)
        .await
        .map(|(summary, _)| summary)
}

/// По каноническому английскому заголовку пытается загрузить русскую статью через межъязыковую ссылку
async fn fetch_ru_via_en_langlink(
    client: &reqwest::Client,
    en_title: &str,
) -> Option<ArtistSummary> {
    let langlinks_url = format!(
        "https://en.wikipedia.org/w/api.php?action=query&titles={}&redirects=1&prop=langlinks&lllang=ru&format=json",
        urlencoding_encode(en_title)
    );
    let json = get_json(client, &langlinks_url).await?;
    let ru_title = extract_ru_langlink(&json)?;
    let ru_url = summary_url("ru", &page_slug(&ru_title));
    let page = get_json(client, &ru_url).await?;
    // Английская статья уже прошла строгий accept и связана официальной интервики,
    // поэтому достаточно проверить type == "standard" и непустой текст.
    extract_summary_from_page(&page, &ru_title, None).map(|(summary, _)| summary)
}

async fn summary_ru_or_en(
    query: &str,
    name: &str,
    accept: &dyn Fn(&str, &str) -> bool,
) -> Option<ArtistSummary> {
    if name.trim().is_empty() {
        return None;
    }
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .ok()?;

    if let Some(ru) = summary_in(&client, "ru", query, name, accept).await {
        return Some(ru);
    }

    let (en_summary, canonical_en_title) =
        summary_in_with_title(&client, "en", query, name, accept).await?;

    if let Some(ru_summary) = fetch_ru_via_en_langlink(&client, &canonical_en_title).await {
        return Some(ru_summary);
    }

    Some(en_summary)
}

/// Ищет справку об исполнителе: сначала русская Википедия, затем английская (с переходом на русскую по `langlinks`).
/// Лучше ничего не показать, чем чужую страницу, поэтому нужны точное имя и музыкальное описание.
pub async fn artist_summary(name: &str) -> Option<ArtistSummary> {
    let accept = |description: &str, _: &str| looks_musical(description);
    summary_ru_or_en(name, name, &accept).await
}

const ALBUM_WORDS: [&str; 10] = ["альбом", "album", "сингл", "single", "ep", "сборник", "compilation", "саундтрек", "soundtrack", "mixtape"];

/// Страница об альбоме: в описании тип записи, а в описании или тексте назван исполнитель
fn looks_like_album_by(description: &str, extract: &str, artist: &str) -> bool {
    let d = description.to_lowercase();
    let is_album = d.split(|c: char| !c.is_alphanumeric()).any(|w| ALBUM_WORDS.contains(&w));
    let artist = artist.trim().to_lowercase();
    is_album && !artist.is_empty() && (d.contains(&artist) || extract.to_lowercase().contains(&artist))
}

/// Справка об альбоме: русская Википедия, затем английская (с переходом на русскую по `langlinks`); нужны точное название и исполнитель
pub async fn album_summary(title: &str, artist: &str) -> Option<ArtistSummary> {
    if title.trim().is_empty() || artist.trim().is_empty() {
        return None;
    }
    let query = format!("{title} {artist}");
    let accept = |description: &str, extract: &str| looks_like_album_by(description, extract, artist);
    summary_ru_or_en(&query, title, &accept).await
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
        assert!(looks_musical("американский рэпер и битмейкер"));
        assert!(looks_musical("британский мультиинструменталист и песенник"));
        assert!(looks_musical("American beatmaker, songwriter and multi-instrumentalist"));
        assert!(!looks_musical("российский фильм 2016 года"));
        assert!(!looks_musical("российский актёр и кинопродюсер"));
    }

    #[test]
    fn extracts_russian_langlink_and_ignores_missing_pages() {
        let valid: Value = serde_json::json!({
            "batchcomplete": "",
            "query": {
                "pages": {
                    "10287594": {
                        "pageid": 10287594,
                        "ns": 0,
                        "title": "Flo Rida",
                        "langlinks": [
                            {
                                "lang": "ru",
                                "*": "Флоу Райда"
                            }
                        ]
                    }
                }
            }
        });
        assert_eq!(extract_ru_langlink(&valid).as_deref(), Some("Флоу Райда"));

        let empty_langlinks: Value = serde_json::json!({
            "batchcomplete": "",
            "query": {
                "pages": {
                    "42": {
                        "pageid": 42,
                        "ns": 0,
                        "title": "Some Artist",
                        "langlinks": []
                    }
                }
            }
        });
        assert_eq!(extract_ru_langlink(&empty_langlinks), None);

        let no_langlinks: Value = serde_json::json!({
            "batchcomplete": "",
            "query": {
                "pages": {
                    "42": {
                        "pageid": 42,
                        "ns": 0,
                        "title": "Some Artist"
                    }
                }
            }
        });
        assert_eq!(extract_ru_langlink(&no_langlinks), None);

        let negative_key: Value = serde_json::json!({
            "batchcomplete": "",
            "query": {
                "pages": {
                    "-1": {
                        "ns": 0,
                        "title": "Missing Artist",
                        "missing": "",
                        "langlinks": [
                            { "lang": "ru", "*": "Игнорируемый заголовок" }
                        ]
                    }
                }
            }
        });
        assert_eq!(extract_ru_langlink(&negative_key), None);

        let negative_pageid: Value = serde_json::json!({
            "batchcomplete": "",
            "query": {
                "pages": {
                    "1": {
                        "pageid": -1,
                        "ns": 0,
                        "title": "Missing Artist",
                        "langlinks": [
                            { "lang": "ru", "*": "Игнорируемый заголовок" }
                        ]
                    }
                }
            }
        });
        assert_eq!(extract_ru_langlink(&negative_pageid), None);
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
        for name in [
            "Deftones",
            "Три дня дождя",
            "Noisecream",
            "Kira Voss",
            "Flo Rida",
            "David Guetta",
            "Nicki Minaj",
        ] {
            let summary = artist_summary(name).await;
            match &summary {
                Some(s) => println!("LIVE {name}: {} | {}", s.url, s.extract.chars().take(80).collect::<String>()),
                None => println!("LIVE {name}: —"),
            }
            if matches!(name, "Flo Rida" | "David Guetta" | "Nicki Minaj")
                && let Some(s) = summary
            {
                assert!(
                    s.url.contains("ru.wikipedia.org"),
                    "ожидался русский URL для {name}, получено: {}",
                    s.url
                );
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
