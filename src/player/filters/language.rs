//! Языковые фильтры Волны: вердикты, маркеры инструментала и матрица решений.
//!
//! Все функции чистые: на вход только текст (метаданные трека или его текст песни),
//! на выходе вердикт. Сеть и состояние плеера — забота вызывающего кода.

/// Активный фильтр языка из кнопок окна «Настроить волну»
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageFilter {
    Russian,
    NotRussian,
    WithoutWords,
}

/// Вердикт по треку (после Фазы 1 или Фазы 2)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageVerdict {
    Russian,
    Foreign,
    Instrumental,
    Unknown,
}

/// Результат Фазы 1: то, что видно по метаданным без обращения к сети
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseOne {
    /// Найден маркер инструментала — принимать/отклонять сразу
    Instrumental,
    /// В названии/артистах есть кириллица — вердикт `Russian` без сети
    Russian,
    /// Метаданные ничего не говорят — нужна Фаза 2
    Ambiguous,
}

/// Разбор значения кнопки языка (`apply_rotor_filter` передаёт именно эти строки)
pub fn parse_filter(value: &str) -> Option<LanguageFilter> {
    match value {
        "russian" => Some(LanguageFilter::Russian),
        "not-russian" => Some(LanguageFilter::NotRussian),
        "without-words" => Some(LanguageFilter::WithoutWords),
        _ => None,
    }
}

/// Кириллические буквы (основной блок и дополнение)
pub fn is_cyrillic_char(c: char) -> bool {
    matches!(c, '\u{0400}'..='\u{04FF}' | '\u{0500}'..='\u{052F}')
}

/// Есть ли в строке хотя бы одна кириллическая буква
pub fn has_cyrillic(text: &str) -> bool {
    text.chars().any(is_cyrillic_char)
}

/// Слова строки без пунктуации, в нижнем регистре.
///
/// Дефис считается разделителем: `post-rock` → `["post", "rock"]`,
/// `lo-fi` → `["lo", "fi"]`.
fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// Маркеры инструментала по границам слов.
///
/// `\b(instrumental|ambient|post-rock|soundtrack|ost|synthwave|score)\b`
/// плюс варианты `lo-fi` / `lofi` / `lo fi`.
///
/// Границы обязательны: «Ghost», «Lost», «Coast» не должны срабатывать на `ost`.
pub fn has_instrumental_marker(text: &str) -> bool {
    const SINGLE: [&str; 6] = ["instrumental", "ambient", "soundtrack", "ost", "synthwave", "score"];
    let ws = words(text);
    if ws.is_empty() {
        return false;
    }
    for w in &ws {
        if SINGLE.contains(&w.as_str()) || w == "lofi" {
            return true;
        }
    }
    for pair in ws.windows(2) {
        match (pair[0].as_str(), pair[1].as_str()) {
            ("post", "rock") => return true,
            ("lo", "fi") => return true,
            _ => {}
        }
    }
    false
}

/// Фаза 1: вердикт по метаданным (`title`, `version`, `artists`).
///
/// Порядок важен: маркеры инструментала проверяются раньше кириллицы, поэтому
/// инструментал с русским названием получает `Instrumental` и под фильтром
/// `russian` отклоняется — в точности по матрице решений.
pub fn phase_one(title: &str, version: &str, artists: &str) -> PhaseOne {
    let text = format!("{} {} {}", title, version, artists);
    if has_instrumental_marker(&text) {
        PhaseOne::Instrumental
    } else if has_cyrillic(&text) {
        PhaseOne::Russian
    } else {
        PhaseOne::Ambiguous
    }
}

/// Классификация текста песни (Фаза 2).
///
/// Текст должен быть уже очищен от LRC-директив (`strip_lrc_tags`), иначе
/// `[ar:…]`, `[ti:…]`, `[mm:ss.xx]` раздувают знаменатель латиницей.
pub fn classify_lyrics(text: &str) -> LanguageVerdict {
    if has_instrumental_marker(text) {
        return LanguageVerdict::Instrumental;
    }
    let mut letters = 0usize;
    let mut cyrillic = 0usize;
    for c in text.chars() {
        if c.is_alphabetic() {
            letters += 1;
            if is_cyrillic_char(c) {
                cyrillic += 1;
            }
        }
    }
    if letters == 0 {
        return LanguageVerdict::Unknown;
    }
    // Порог 20 % при достаточной длине текста: одна случайная кириллическая
    // буква в англоязычной песне не должна переключать вердикт.
    if letters >= 10 && cyrillic * 100 >= letters * 20 {
        LanguageVerdict::Russian
    } else if cyrillic * 100 < letters * 5 {
        LanguageVerdict::Foreign
    } else {
        LanguageVerdict::Unknown
    }
}

/// Матрица решений: принимает ли фильтр трек с данным вердиктом.
pub fn accepts(filter: LanguageFilter, verdict: LanguageVerdict) -> bool {
    matches!(
        (filter, verdict),
        (LanguageFilter::Russian, LanguageVerdict::Russian)
            | (LanguageFilter::NotRussian, LanguageVerdict::Foreign)
            | (LanguageFilter::WithoutWords, LanguageVerdict::Instrumental)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cyrillic_detection() {
        assert!(has_cyrillic("Привет"));
        assert!(has_cyrillic("Мельница — Дорога"));
        assert!(!has_cyrillic("Hello world"));
        assert!(!has_cyrillic(""));
    }

    #[test]
    fn instrumental_markers_match_whole_words() {
        // Не должны триггерить `ost` внутри слова
        assert!(!has_instrumental_marker("Ghost"));
        assert!(!has_instrumental_marker("Lost"));
        assert!(!has_instrumental_marker("Coast"));
        // Явные маркеры
        assert!(has_instrumental_marker("Instrumental version"));
        assert!(has_instrumental_marker("OST"));
        assert!(has_instrumental_marker("Post-Rock"));
        assert!(has_instrumental_marker("Soundtrack"));
    }

    #[test]
    fn lo_fi_variants() {
        assert!(has_instrumental_marker("Lo-Fi Beats"));
        assert!(has_instrumental_marker("lofi mix"));
        assert!(has_instrumental_marker("Lo Fi"));
        assert!(has_instrumental_marker("LO-FI"));
    }

    #[test]
    fn instrumental_has_priority_over_cyrillic() {
        // Инструментал с русским названием должен получить Instrumental,
        // а не Russian — иначе под фильтром «На русском» он пройдёт.
        assert_eq!(phase_one("Русский вокал", "Instrumental", ""), PhaseOne::Instrumental);
        assert_eq!(phase_one("Песня", "", "Артист"), PhaseOne::Russian);
        assert_eq!(phase_one("Song", "", "Artist"), PhaseOne::Ambiguous);
    }

    #[test]
    fn classification_after_lrc_strip() {
        let russian = "Привет, как дела?\nЯ иду домой";
        assert_eq!(classify_lyrics(russian), LanguageVerdict::Russian);

        let foreign = "Hello, how are you?\nI am going home";
        assert_eq!(classify_lyrics(foreign), LanguageVerdict::Foreign);

        // Транслит: кириллицы нет — иностранный
        assert_eq!(classify_lyrics("Privet, kak dela"), LanguageVerdict::Foreign);

        // Двуязычный припев с большим русским текстом остаётся русским
        let mixed = "Привет, как дела? Я иду домой\nHello, how are you";
        assert_eq!(classify_lyrics(mixed), LanguageVerdict::Russian);
    }

    #[test]
    fn dead_zone_and_empty() {
        assert_eq!(classify_lyrics(""), LanguageVerdict::Unknown);
        assert_eq!(classify_lyrics("12345"), LanguageVerdict::Unknown);
        // Мало букв — вердикта нет
        assert_eq!(classify_lyrics("Да"), LanguageVerdict::Unknown);
    }

    #[test]
    fn decision_matrix() {
        for verdict in [
            LanguageVerdict::Russian,
            LanguageVerdict::Foreign,
            LanguageVerdict::Instrumental,
            LanguageVerdict::Unknown,
        ] {
            assert_eq!(accepts(LanguageFilter::Russian, verdict), verdict == LanguageVerdict::Russian);
            assert_eq!(accepts(LanguageFilter::NotRussian, verdict), verdict == LanguageVerdict::Foreign);
            assert_eq!(
                accepts(LanguageFilter::WithoutWords, verdict),
                verdict == LanguageVerdict::Instrumental
            );
        }
    }

    #[test]
    fn filter_parsing() {
        assert_eq!(parse_filter("russian"), Some(LanguageFilter::Russian));
        assert_eq!(parse_filter("not-russian"), Some(LanguageFilter::NotRussian));
        assert_eq!(parse_filter("without-words"), Some(LanguageFilter::WithoutWords));
        assert_eq!(parse_filter("favorite"), None);
    }
}
