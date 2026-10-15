//! Перевод коротких текстов на русский через публичный API MyMemory (без ключа, суточный лимит).

use serde_json::Value;

use super::client::urlencoding_encode;

const USER_AGENT: &str = "Volna/0.1 (unofficial Yandex Music client; GPL-3.0)";
/// MyMemory принимает до 500 байт в запросе; берём с запасом
const CHUNK_BYTES: usize = 450;

const ENGLISH_WORDS: [&str; 16] = [
    "the", "and", "of", "is", "was", "in", "with", "for", "as", "he", "she", "his", "her", "by", "an", "are",
];

/// Текст английский: почти одни латинские буквы и заметная доля частых английских слов
/// (так отсеиваются испанский, немецкий и т.п.; для них перевод en→ru был бы неверным)
pub fn is_english(text: &str) -> bool {
    let letters: Vec<char> = text.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.len() < 20 {
        return false;
    }
    let ascii = letters.iter().filter(|c| c.is_ascii_alphabetic()).count();
    if ascii * 100 < letters.len() * 90 {
        return false;
    }
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphabetic())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect();
    let hits = words.iter().filter(|w| ENGLISH_WORDS.contains(&w.as_str())).count();
    hits * 100 >= words.len() * 8
}

/// Режет текст на куски до `CHUNK_BYTES` по границам предложений (иначе по пробелам)
fn split_chunks(text: &str) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        // новый кусок начинаем на границе предложения, если места почти не осталось, либо при переполнении
        let would_be = current.len() + 1 + word.len();
        let sentence_end = current.ends_with('.') || current.ends_with('!') || current.ends_with('?');
        if !current.is_empty() && (would_be > CHUNK_BYTES || (sentence_end && would_be > CHUNK_BYTES * 2 / 3)) {
            chunks.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

async fn translate_chunk(client: &reqwest::Client, chunk: &str) -> Option<String> {
    let url = format!(
        "https://api.mymemory.translated.net/get?langpair=en%7Cru&q={}",
        urlencoding_encode(chunk)
    );
    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let data: Value = resp.json().await.ok()?;
    // при исчерпании лимита сервис возвращает предупреждение вместо перевода
    let status_ok = match &data["responseStatus"] {
        Value::Number(n) => n.as_u64() == Some(200),
        Value::String(s) => s == "200",
        _ => false,
    };
    let translated = data["responseData"]["translatedText"].as_str()?.trim();
    (status_ok && !translated.is_empty() && !translated.starts_with("MYMEMORY WARNING")).then(|| translated.to_string())
}

/// Переводит английский текст на русский; `None`, если сервис недоступен или лимит исчерпан
pub async fn to_russian(text: &str) -> Option<String> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;
    let mut parts = Vec::new();
    for chunk in split_chunks(text) {
        parts.push(translate_chunk(&client, &chunk).await?);
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_english_only() {
        assert!(is_english("Formed in Sacramento, CA in 1988, the band became one of the most influential metal acts."));
        assert!(!is_english("Американская метал-группа из Сакраменто, основанная в 1988 году."));
        assert!(!is_english("La banda se formó en Sacramento en 1988 y es una de las más influyentes del metal."));
        assert!(!is_english("バンドは1988年にサクラメントで結成された。"));
        assert!(!is_english("Hi"));
    }

    #[test]
    fn chunks_fit_limit_and_keep_all_words() {
        let text = "This is a sentence. ".repeat(60);
        let chunks = split_chunks(&text);
        assert!(chunks.len() > 1 && chunks.iter().all(|c| c.len() <= CHUNK_BYTES));
        assert_eq!(chunks.join(" "), text.trim());
    }

    /// Ходит в сеть: `cargo test translate -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_translate() {
        let r = to_russian("Formed in Sacramento, CA in 1988, the band became one of the most influential metal acts. They released many albums.").await;
        println!("LIVE {r:?}");
    }
}
