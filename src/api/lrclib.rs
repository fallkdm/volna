use crate::api::models::{LrclibResponse, parse_lrc};

/// Допуск по длительности при сверке записи LRCLIB с треком (±4 с).
///
/// Один на все сравнения (`/api/get`, `/api/search` и запасной поиск), чтобы «±N с»
/// в документации не разъезжалось с кодом.
pub const DURATION_TOLERANCE_SECS: f64 = 4.0;

/// Статус ответа LRCLIB
pub enum LrclibFetch {
    Found(reqwest::Response),
    NotFound,
    Failed,
}

/// Итог поиска текста с сохранением терминальности.
///
/// Нужен алгоритмическим фильтрам Волны: негативный кэш можно писать только при
/// `NotFound`, а `WrongTrack`/`Failed` — транзиентные состояния, их кэшировать нельзя.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LyricsLookup {
    /// Текст найден (синхронизированный или обычный)
    Found(String),
    /// Терминально: 404 по обоим запросам и поиск ничего не дал
    NotFound,
    /// Запись есть, но это другой трек / не сошлась длительность — не терминально
    WrongTrack,
    /// Сеть, таймаут или битый JSON — не терминально
    Failed,
}

/// GET к LRCLIB с повтором: сервис нередко отвечает 429 (лимит на общий IP) или 5xx, а 404 означает «текста нет»
pub async fn lrclib_get(http: &reqwest::Client, url: &str) -> LrclibFetch {
    for attempt in 0..3u64 {
        let sent = http
            .get(url)
            .header(
                "User-Agent",
                "VolnaMusicPlayer/1.0 (https://github.com/fallkdm/volna)",
            )
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

/// Таймкоды не вылезают за длительность трека.
///
/// В LRCLIB лежат записи, размеченные под другой монтаж (расширенная версия, «lyric video» с длинным
/// вступлением): текст правильный, а таймлайн длиннее самого трека, поэтому караоке уезжает всё сильнее
/// и к концу песни подсвечена уже финальная строка. Сдвигом такое не лечится — такую разметку надо
/// отбрасывать и искать заново. Допуск +15 с — на концовку/аплодисменты в оригинальной записи.
pub fn timeline_fits(text: &str, duration_secs: Option<u64>) -> bool {
    let Some(secs) = duration_secs else {
        return true;
    };
    let Some(last) = parse_lrc(text).last().map(|l| l.time_ms) else {
        return false;
    };
    last <= secs.saturating_mul(1000).saturating_add(15_000)
}

/// Насколько разметка может вылезать за длительность трека, прежде чем это будет считаться
/// не «другой монтаж», а чужим текстом.
const FOREIGN_TIMELINE_FACTOR: f64 = 1.25;
/// Допуск поверх множителя: короткие треки и длинные концовки не должны считаться чужими.
const FOREIGN_TIMELINE_SLACK_MS: u64 = 30_000;

/// Тайминги заведомо длиннее трека: скорее всего, в записи лежит текст совсем другой песни.
///
/// Отличать это от «другого монтажа» приходится по масштабу: если последняя строка заходит за
/// длительность больше чем на четверть (и больше чем на полминуты), это не расширенная версия,
/// а чужая песня — её нельзя показывать даже без таймкодов.
pub fn timeline_too_long(last_time_ms: u64, duration_secs: u64) -> bool {
    if duration_secs == 0 {
        return false;
    }
    let limit = (duration_secs as f64 * 1000.0 * FOREIGN_TIMELINE_FACTOR) as u64;
    last_time_ms > limit.saturating_add(FOREIGN_TIMELINE_SLACK_MS)
}

/// Текст размечен под другой трек целиком (а не под другой монтаж этого же)
pub fn looks_like_foreign_song(text: &str, duration_secs: Option<u64>) -> bool {
    let Some(secs) = duration_secs.filter(|s| *s > 0) else {
        return false;
    };
    parse_lrc(text)
        .last()
        .is_some_and(|l| timeline_too_long(l.time_ms, secs))
}

/// Совпадают ли названия треков с точностью до скобок, регистра и клиповых хвостов
pub fn same_song_title(a: &str, b: &str) -> bool {
    let (x, y) = (normalize_name(a), normalize_name(b));
    if x.is_empty() || y.is_empty() {
        // сравнивать нечего — не мешаем поиску
        return true;
    }
    x == y
        || (x.chars().count() >= 4 && y.chars().count() >= 4 && (x.contains(&y) || y.contains(&x)))
}

/// Совпадают ли исполнители: у нас они через запятую, у сервиса бывает один из них
pub fn same_artist_name(a: &str, b: &str) -> bool {
    let (x, y) = (normalize_name(a), normalize_name(b));
    if x.is_empty() || y.is_empty() {
        return true;
    }
    let first = |s: &str| s.split(',').next().map(normalize_name).unwrap_or_default();
    let (fx, fy) = (first(a), first(b));
    x == y
        || (fx.chars().count() >= 4 && y.contains(&fx))
        || (fy.chars().count() >= 4 && x.contains(&fy))
        || (x.chars().count() >= 4 && y.contains(&x))
        || (y.chars().count() >= 4 && x.contains(&y))
}

/// Синхронизированный текст годится и по числу строк, и по таймлайну
pub fn usable_synced_at(text: &str, duration_secs: Option<u64>) -> bool {
    usable_synced(text) && timeline_fits(text, duration_secs)
}

/// Годится ли текст, уже лежащий в дисковом кэше.
///
/// Кэш ключуется одним id трека, поэтому разметка от другой версии живёт там годами. Статический
/// текст без таймкодов принимаем как есть, а синхронизированный — только если таймлайн попадает
/// в длительность текущего трека.
pub fn cached_usable(text: &str, duration_secs: Option<u64>) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    if !usable_synced(trimmed) {
        return true;
    }
    timeline_fits(trimmed, duration_secs)
}

/// Убирает таймкоды у разметки, которая не подходит этому треку.
///
/// Такую разметку нельзя показывать как караоке: подсветка поедет от музыки. Но текст в ней
/// правильный, поэтому оставляем его как статический — лучше текст без караоке, чем уехавшая строка.
pub fn demote_stale_synced(text: &str, duration_secs: Option<u64>) -> String {
    let trimmed = text.trim();
    if usable_synced(trimmed) && !timeline_fits(trimmed, duration_secs) {
        return strip_lrc_tags(trimmed);
    }
    trimmed.to_string()
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
        "(official video)",
        "(official music video)",
        "(official audio)",
        "(lyric video)",
        "(lyrics)",
        "(audio)",
        "(visualizer)",
        "[official video]",
        "[music video]",
        "[audio]",
        "[lyrics]",
        "[visualizer]",
        "(clip)",
        "(клип)",
        "(премьера клипа)",
        "(hd)",
        "[hd]",
        "(4k)",
        "[4k]",
    ];
    for pat in noise_patterns {
        if let Some(pos) = find_ascii_case_insensitive(&t, pat) {
            t = t[..pos].to_string();
            break;
        }
    }
    t.trim().to_string()
}

/// Поиск ASCII-подстроки без учёта регистра с байтовым индексом в ИСХОДНОЙ строке.
///
/// Раньше индекс искали в `to_lowercase()` и резали исходную строку по нему: если lowercase менял
/// длину в байтах (например «İ»), срез попадал внутрь символа и паниковал.
fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    for (idx, _) in haystack.char_indices() {
        let mut chars = haystack[idx..].chars().map(|c| c.to_ascii_lowercase());
        let matched = needle.chars().all(|n| chars.next() == Some(n));
        if matched {
            return Some(idx);
        }
    }
    None
}

/// Название для сравнения: без регистра, без скобок («(feat. …)», «[Remix]»), хвоста « - Remastered» и пунктуации
pub fn normalize_name(s: &str) -> String {
    let mut depth = 0u32;
    let no_brackets: String = s
        .chars()
        .filter(|c| match c {
            '(' | '[' => {
                depth += 1;
                false
            }
            ')' | ']' => {
                depth = depth.saturating_sub(1);
                false
            }
            _ => depth == 0,
        })
        .collect();
    let head = no_brackets.split(" - ").next().unwrap_or("");
    head.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
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
///
/// Обёртка над статусной версией: сетевой сбой и «ничего не нашли» здесь
/// неразличимы — вызывающим, которым важна терминальность, нужен
/// `lrclib_search_synced_status`.
pub async fn lrclib_search_synced(
    http: &reqwest::Client,
    artist: &str,
    title: &str,
    duration_secs: Option<u64>,
) -> Option<String> {
    lrclib_search_synced_status(http, artist, title, duration_secs)
        .await
        .unwrap_or(None)
}

/// То же, но с сохранением терминальности: `Err(())` — сетевой сбой или битый JSON.
async fn lrclib_search_synced_status(
    http: &reqwest::Client,
    artist: &str,
    title: &str,
    duration_secs: Option<u64>,
) -> Result<Option<String>, ()> {
    let clean_t = clean_title_for_lyrics(title);
    let url = format!(
        "https://lrclib.net/api/search?q={}",
        urlencoding_encode(&format!("{clean_t} {artist}"))
    );
    let resp = match lrclib_get(http, &url).await {
        LrclibFetch::Found(resp) => resp,
        LrclibFetch::Failed => return Err(()),
        LrclibFetch::NotFound => return Ok(None),
    };
    let items: Vec<serde_json::Value> = resp.json().await.map_err(|_| ())?;
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
        (t == title_n && same_artist(&a, &raw))
            || (!artist_n.is_empty() && t == artist_n && a == title_n)
    };
    Ok(items
        .iter()
        .filter(|v| names_match(v))
        .filter(|v| match (duration_secs, v["duration"].as_f64()) {
            (Some(want), Some(got)) => (got - want as f64).abs() <= DURATION_TOLERANCE_SECS,
            _ => true,
        })
        .find_map(|v| {
            v["syncedLyrics"]
                .as_str()
                .map(str::trim)
                .filter(|t| usable_synced_at(t, duration_secs))
                .map(String::from)
        }))
}

/// Один запрос к LRCLIB без повторов и с коротким таймаутом.
///
/// Нужен фильтрам: полный `lrclib_get` при 429/5xx спит 0.7 + 1.4 с, и на пачке
/// кандидатов это выедает весь бюджет Фазы 2.
pub async fn lrclib_get_once(http: &reqwest::Client, url: &str) -> LrclibFetch {
    match http
        .get(url)
        .header(
            "User-Agent",
            "VolnaMusicPlayer/1.0 (https://github.com/fallkdm/volna)",
        )
        .timeout(std::time::Duration::from_millis(1500))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => LrclibFetch::Found(r),
        Ok(r) if r.status() == reqwest::StatusCode::NOT_FOUND => LrclibFetch::NotFound,
        _ => LrclibFetch::Failed,
    }
}

/// Быстрая проба текста для фильтров: без поиска и без повторов.
pub async fn lookup_lyrics_fast(
    http: &reqwest::Client,
    artist: &str,
    title: &str,
    duration_secs: Option<u64>,
) -> LyricsLookup {
    let clean_artist = artist.trim();
    let clean_title = clean_title_for_lyrics(title);
    if clean_artist.is_empty() || clean_title.is_empty() {
        return LyricsLookup::WrongTrack;
    }
    let url = match duration_secs {
        Some(secs) => format!(
            "https://lrclib.net/api/get?artist_name={}&track_name={}&duration={}",
            urlencoding_encode(clean_artist),
            urlencoding_encode(&clean_title),
            secs
        ),
        None => format!(
            "https://lrclib.net/api/get?artist_name={}&track_name={}",
            urlencoding_encode(clean_artist),
            urlencoding_encode(&clean_title)
        ),
    };
    let response = match lrclib_get_once(http, &url).await {
        LrclibFetch::Found(r) => r,
        LrclibFetch::NotFound => return LyricsLookup::NotFound,
        LrclibFetch::Failed => return LyricsLookup::Failed,
    };
    let Ok(data) = response.json::<LrclibResponse>().await else {
        return LyricsLookup::Failed;
    };
    let matches_request = |name: &Option<String>| {
        name.as_deref()
            .is_none_or(|n| same_song_title(n, &clean_title))
    };
    let artist_matches = data
        .artist_name
        .as_deref()
        .is_none_or(|n| same_artist_name(n, clean_artist));
    let duration_matches = match (data.duration, duration_secs) {
        (Some(got), Some(want)) => (got - want as f64).abs() <= DURATION_TOLERANCE_SECS,
        _ => true,
    };
    let record_matches = matches_request(&data.track_name)
        && artist_matches
        && duration_matches
        && !looks_like_foreign_song(
            data.synced_lyrics.as_deref().unwrap_or(""),
            duration_secs,
        );
    if !record_matches {
        return LyricsLookup::WrongTrack;
    }
    if let Some(synced) = data.synced_lyrics {
        let trimmed = synced.trim();
        if usable_synced_at(trimmed, duration_secs) {
            return LyricsLookup::Found(trimmed.to_string());
        }
    }
    if let Some(plain) = data.plain_lyrics {
        let trimmed = plain.trim();
        if !trimmed.is_empty() {
            return LyricsLookup::Found(trimmed.to_string());
        }
    }
    LyricsLookup::NotFound
}

/// Поиск текста с сохранением терминальности (для алгоритмических фильтров Волны).
///
/// Отличается от `fetch_lrclib_lyrics` тем, что не схлопывает «нет текста»,
/// «это чужой трек» и «сеть недоступна» в один `None`.
pub async fn lookup_lyrics(
    http: &reqwest::Client,
    artist: &str,
    title: &str,
    duration_secs: Option<u64>,
) -> LyricsLookup {
    let clean_artist = artist.trim();
    let clean_title = clean_title_for_lyrics(title);
    // Нечего искать: это не «нет текста», а кривые метаданные — кэшировать нельзя
    if clean_artist.is_empty() || clean_title.is_empty() {
        return LyricsLookup::WrongTrack;
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
    let mut mismatch = false;
    let mut saw_failure = false;

    let mut resp = lrclib_get(http, &make_url(duration_secs)).await;
    // Если запрос с точной длительностью вернул 404, повторяем поиск без ограничения длительности
    if matches!(resp, LrclibFetch::NotFound) && duration_secs.is_some() {
        resp = lrclib_get(http, &make_url(None)).await;
    }

    match resp {
        LrclibFetch::Failed => saw_failure = true,
        LrclibFetch::NotFound => {}
        LrclibFetch::Found(response) => match response.json::<LrclibResponse>().await {
            Err(_) => saw_failure = true,
            Ok(data) => {
                // `/api/get` отвечает и на нечёткий запрос: если в ответе явно другой трек,
                // его текст брать нельзя — иначе под одним названием окажется чужая песня
                let matches_request = |name: &Option<String>| {
                    name.as_deref()
                        .is_none_or(|n| same_song_title(n, &clean_title))
                };
                let artist_matches = data
                    .artist_name
                    .as_deref()
                    .is_none_or(|n| same_artist_name(n, clean_artist));
                let duration_matches = match (data.duration, duration_secs) {
                    (Some(got), Some(want)) => (got - want as f64).abs() <= DURATION_TOLERANCE_SECS,
                    _ => true,
                };
                let record_matches = matches_request(&data.track_name)
                    && artist_matches
                    && duration_matches
                    && !looks_like_foreign_song(
                        data.synced_lyrics.as_deref().unwrap_or(""),
                        duration_secs,
                    );
                if !record_matches {
                    mismatch = true;
                    println!(
                        "[lyrics] LRCLIB /api/get: «{}» / «{}» ({} с) — не тот трек",
                        data.track_name.as_deref().unwrap_or("?"),
                        data.artist_name.as_deref().unwrap_or("?"),
                        data.duration.unwrap_or(0.0)
                    );
                }
                if record_matches {
                    if let Some(synced) = data.synced_lyrics {
                        let trimmed = synced.trim();
                        if usable_synced_at(trimmed, duration_secs) {
                            return LyricsLookup::Found(trimmed.to_string());
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
        },
    }

    // Точное совпадение дало только обычный текст или 404: ищем запись с таймингами через поиск
    match lrclib_search_synced_status(http, clean_artist, &clean_title, duration_secs).await {
        Ok(Some(synced)) => return LyricsLookup::Found(synced),
        Ok(None) => {}
        Err(()) => saw_failure = true,
    }

    if let Some(plain) = plain_fallback {
        return LyricsLookup::Found(plain);
    }
    if saw_failure {
        LyricsLookup::Failed
    } else if mismatch {
        LyricsLookup::WrongTrack
    } else {
        LyricsLookup::NotFound
    }
}

/// Запрос текста песни из открытой глобальной базы LRCLIB (https://lrclib.net)
///
/// Обёртка над `lookup_lyrics`: три существующих вызова (UI-путь и кэш)
/// продолжают работать с `Option<String>`.
pub async fn fetch_lrclib_lyrics(
    http: &reqwest::Client,
    artist: &str,
    title: &str,
    duration_secs: Option<u64>,
) -> Option<String> {
    match lookup_lyrics(http, artist, title, duration_secs).await {
        LyricsLookup::Found(text) => Some(text),
        _ => None,
    }
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
    let mut stale_plain = None;
    if let Ok(cached) = std::fs::read_to_string(&cache_path) {
        let trimmed = cached.trim();
        if looks_like_foreign_song(trimmed, duration_secs) {
            // Текст чужой песни: держать его нельзя даже без таймкодов, а кэш по одному id
            // будет отдавать его вечно — удаляем и ищем заново
            println!(
                "[lyrics] Кэш {} отброшен: размечен под другой трек",
                track_id
            );
            let _ = std::fs::remove_file(&cache_path);
        } else if cached_usable(trimmed, duration_secs) {
            return Some(cached);
        } else if !trimmed.is_empty() {
            stale_plain = Some(strip_lrc_tags(trimmed));
        }
    }

    let lyrics = fetch_lrclib_lyrics(http, artist, title, duration_secs).await;
    if let Some(text) = &lyrics {
        let _ = std::fs::write(&cache_path, text);
    }
    // Разметка из кэша не от этого трека, а новой не нашлось: отдаём хотя бы текст,
    // но без таймкодов — уехавшая подсветка хуже, чем текст без караоке
    lyrics.or(stale_plain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_title_for_lyrics() {
        assert_eq!(
            clean_title_for_lyrics("Dear Diary, (Lyric Video)"),
            "Dear Diary,"
        );
        assert_eq!(
            clean_title_for_lyrics("Happy Song [Official Video]"),
            "Happy Song"
        );
        assert_eq!(
            clean_title_for_lyrics("Change (In the House of Flies)"),
            "Change (In the House of Flies)"
        );
    }

    #[test]
    fn test_usable_synced() {
        let sample_good =
            "[00:10.00] Line 1\n[00:15.00] Line 2\n[00:20.00] Line 3\n[00:25.00] Line 4";
        assert!(usable_synced(sample_good));

        let sample_short = "[00:10.00] Only one line";
        assert!(!usable_synced(sample_short));
    }

    #[test]
    fn test_timeline_fits() {
        let text = "[00:10.00] a\n[00:20.00] b\n[00:30.00] c\n[05:05.88] d";
        // Разметка под 5:06 в трек длиной 2:45 не влезает — это чужая версия
        assert!(!timeline_fits(text, Some(165)));
        // Неизвестная длительность — не мешаем
        assert!(timeline_fits(text, None));
        // Своя разметка проходит
        assert!(timeline_fits("[00:10.00] a\n[02:40.00] b", Some(165)));
        // Допуск на концовку
        assert!(timeline_fits("[00:10.00] a\n[02:55.00] b", Some(165)));
        assert!(!usable_synced_at(text, Some(165)));
        assert!(usable_synced(text));
    }

    #[test]
    fn test_looks_like_foreign_song() {
        // Текст «Blessed with a Curse» (5:06), ошибочно закэшированный под «Dear Diary,» (2:45)
        let wrong = "[01:01.06] Ever since this began\n[05:05.88] I was blessed with a curse";
        assert!(looks_like_foreign_song(wrong, Some(165)));
        // Та же разметка на своём треке — не чужая
        assert!(!looks_like_foreign_song(wrong, Some(306)));
        // Неизвестная длительность: судить не по чему
        assert!(!looks_like_foreign_song(wrong, None));
        // Другой монтаж своей песни: таймлайн длиннее, но в пределах четверти
        assert!(!looks_like_foreign_song(
            "[00:10.00] a\n[02:55.00] b",
            Some(165)
        ));
        // Текст без таймкодов не может быть «чужим по таймингам»
        assert!(!looks_like_foreign_song("просто текст", Some(165)));
        assert!(!timeline_too_long(305_880, 0));
    }

    #[test]
    fn test_same_song_title() {
        assert!(same_song_title("Dear Diary,", "Dear Diary"));
        assert!(same_song_title("Dear Diary, (Lyric Video)", "Dear Diary,"));
        assert!(!same_song_title("Dear Diary,", "Blessed with a Curse"));
        // Сравнивать нечего — не мешаем поиску
        assert!(same_song_title("", "Dear Diary,"));
        assert!(same_song_title("(Official Video)", "Dear Diary,"));
    }

    #[test]
    fn test_same_artist_name() {
        assert!(same_artist_name(
            "Bring Me The Horizon",
            "Bring Me The Horizon"
        ));
        assert!(same_artist_name(
            "Bring Me The Horizon, Someone",
            "Bring Me The Horizon"
        ));
        assert!(!same_artist_name("BMTH", "Bring Me The Horizon"));
        assert!(!same_artist_name("Linkin Park", "Bring Me The Horizon"));
        assert!(same_artist_name("", "Bring Me The Horizon"));
    }

    #[test]
    fn test_clean_title_multibyte_lowercase() {
        // «İ» при lowercase превращается в два символа — раньше срез по индексу из lower паниковал
        assert_eq!(clean_title_for_lyrics("İstanbul (Lyric Video)"), "İstanbul");
        assert_eq!(clean_title_for_lyrics("GRIEF (OFFICIAL VIDEO)"), "GRIEF");
    }

    #[test]
    fn test_cached_usable_and_demote() {
        let stale = "[00:10.00] a\n[00:20.00] b\n[00:30.00] c\n[05:05.88] d";
        // Чужая разметка из кэша не годится…
        assert!(!cached_usable(stale, Some(165)));
        // …но текст из неё сохраняем — уже без таймкодов
        assert_eq!(demote_stale_synced(stale, Some(165)), "a\nb\nc\nd");
        // Статический текст и своя разметка проходят без изменений
        assert!(cached_usable("просто текст песни", Some(165)));
        assert_eq!(
            demote_stale_synced("просто текст песни", Some(165)),
            "просто текст песни"
        );
        assert!(cached_usable(
            "[00:10.00] a\n[00:20.00] b\n[00:30.00] c\n[02:40.00] d",
            Some(165)
        ));
        // Пустой кэш бесполезен
        assert!(!cached_usable("   ", Some(165)));
    }
}
