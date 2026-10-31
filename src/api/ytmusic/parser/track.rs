//! Треки: разбор responsive-элементов, метаданных и панели плейлиста.

use super::*;

/// Стоп-слова для отсечения подкастов, разговоров и видео-мусора
pub const MUSIC_STOP_WORDS: &[&str] = &[
    "подкаст",
    "podcast",
    "выпуск #",
    "выпуск №",
    "эпизод ",
    "episode ",
    "интервью",
    "шоу ",
    "talk show",
    "стрим",
    "stream",
    "реакция ",
    "reaction",
    "review",
    "обзор ",
    "full album",
    "тизер",
    "teaser",
    "аудиокнига",
    "audiobook",
];

/// Проверяет, является ли трек чистым музыкальным треком:
/// - Длительность: от 30 секунд (30_000 мс) до 20 минут (1_200_000 мс). Если длительность известна и выходит за пределы — возвращает `false`.
/// - Проверяет стоп-слова в названии и имени артиста (в нижнем регистре). Если найдено — возвращает `false`.
/// - Иначе `true`.
pub fn is_clean_music_track(track: &Track) -> bool {
    if let Some(dur_ms) = track.duration_ms
        && (!(30_000..=1_200_000).contains(&dur_ms))
    {
        return false;
    }
    let title_lower = track.title.to_lowercase();
    let artists_lower = track.artists_string().to_lowercase();
    for &stop_word in MUSIC_STOP_WORDS {
        if title_lower.contains(stop_word) || artists_lower.contains(stop_word) {
            return false;
        }
    }
    true
}

/// Извлечение метаданных трека (артисты, альбом, длительность) из массива `runs`
fn parse_track_meta_runs(
    runs: &[serde_json::Value],
    artists: &mut Vec<Artist>,
    albums: &mut Vec<Album>,
    duration_ms: &mut Option<u64>,
) {
    let mut explicit_found_artist = false;
    let mut explicit_found_album = false;

    for r in runs {
        let text = match r.get("text").and_then(|t| t.as_str()) {
            Some(t) => t.trim(),
            None => continue,
        };
        if text.is_empty() || text == "•" || text == "," {
            continue;
        }

        let browse_id = r
            .get("navigationEndpoint")
            .and_then(|n| n.get("browseEndpoint"))
            .and_then(|b| b.get("browseId"))
            .and_then(|id| id.as_str());

        if let Some(bid) = browse_id {
            if bid.starts_with("UC") || bid.contains("artist") {
                artists.push(Artist {
                    id: bid.to_string(),
                    name: text.to_string(),
                    cover: None,
                });
                explicit_found_artist = true;
                continue;
            } else if bid.starts_with("MPRE") || bid.starts_with("FEmusic_library_album") {
                albums.push(Album {
                    id: bid.to_string(),
                    title: Some(text.to_string()),
                    year: None,
                    cover_uri: None,
                    og_image: None,
                    artists: Vec::new(),
                    track_count: None,
                });
                explicit_found_album = true;
                continue;
            }
        }

        if duration_ms.is_none()
            && text.contains(':')
            && let Some(d) = parse_duration_ms(text)
        {
            *duration_ms = Some(d);
        }
    }

    if !explicit_found_artist {
        let mut sections: Vec<Vec<String>> = Vec::new();
        let mut current_section: Vec<String> = Vec::new();
        for r in runs {
            let t = r.get("text").and_then(|t| t.as_str()).unwrap_or("").trim();
            if t == "•" {
                if !current_section.is_empty() {
                    sections.push(std::mem::take(&mut current_section));
                }
            } else if !t.is_empty() && t != "," {
                current_section.push(t.to_string());
            }
        }
        if !current_section.is_empty() {
            sections.push(current_section);
        }

        let mut sec_iter = sections.into_iter();
        if let Some(first_sec) = sec_iter.next() {
            let combined = first_sec.join(" ");
            let is_type = matches!(
                combined.to_lowercase().as_str(),
                "песня" | "видео" | "song" | "video" | "сингл" | "single"
            );
            let artist_sec = if is_type {
                sec_iter.next()
            } else {
                Some(first_sec)
            };

            if let Some(art_sec) = artist_sec {
                for name in art_sec {
                    if !name.trim().is_empty() {
                        artists.push(Artist {
                            id: String::new(),
                            name,
                            cover: None,
                        });
                    }
                }
            }

            if !explicit_found_album && let Some(next_sec) = sec_iter.next() {
                let album_title = next_sec.join(" ");
                if parse_duration_ms(&album_title).is_none() {
                    albums.push(Album {
                        id: String::new(),
                        title: Some(album_title),
                        year: None,
                        cover_uri: None,
                        og_image: None,
                        artists: Vec::new(),
                        track_count: None,
                    });
                }
            }
        }
    }
}

/// Извлечение explicit признака из badges
fn is_explicit(val: &serde_json::Value) -> bool {
    if let Some(badges) = val.get("badges").and_then(|b| b.as_array()) {
        for b in badges {
            let icon = b
                .get("musicInlineBadgeRenderer")
                .and_then(|m| m.get("icon"))
                .and_then(|i| i.get("iconType"))
                .and_then(|t| t.as_str());
            if icon == Some("MUSIC_EXPLICIT_BADGE") {
                return true;
            }
        }
    }
    false
}

/// Парсинг `musicResponsiveListItemRenderer` в `Track`
pub fn parse_responsive_track(item: &serde_json::Value) -> Option<Track> {
    let renderer = item.get("musicResponsiveListItemRenderer").unwrap_or(item);

    let video_id = find_video_id(renderer)?;

    let flex_columns = renderer.get("flexColumns").and_then(|fc| fc.as_array())?;
    let col0 = flex_columns.first()?;
    let title_val = col0
        .get("musicResponsiveListItemFlexColumnRenderer")
        .and_then(|c| c.get("text"));
    let title = title_val.and_then(extract_text).unwrap_or_default();
    if title.is_empty() {
        return None;
    }

    let mut cover_uri = extract_best_thumbnail(renderer).or_else(|| extract_best_thumbnail(item));
    if cover_uri
        .as_deref()
        .is_none_or(|u| u.contains("googleusercontent.com"))
    {
        cover_uri = Some(format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", video_id));
    }

    let mut artists = Vec::new();
    let mut albums = Vec::new();
    let mut duration_ms = None;

    if let Some(col1) = flex_columns.get(1)
        && let Some(runs) = col1
            .get("musicResponsiveListItemFlexColumnRenderer")
            .and_then(|c| c.get("text"))
            .and_then(|t| t.get("runs"))
            .and_then(|r| r.as_array())
    {
        parse_track_meta_runs(runs, &mut artists, &mut albums, &mut duration_ms);
    }

    // Дополнительный поиск длительности в fixedColumns
    if duration_ms.is_none()
        && let Some(fixed) = renderer.get("fixedColumns").and_then(|fc| fc.as_array())
    {
        for col in fixed {
            if let Some(text) = col
                .get("musicResponsiveListItemFixedColumnRenderer")
                .and_then(|c| c.get("text"))
                .and_then(extract_text)
                && let Some(d) = parse_duration_ms(&text)
            {
                duration_ms = Some(d);
                break;
            }
        }
    }

    let content_warning = if is_explicit(renderer) {
        Some("explicit".to_string())
    } else {
        None
    };

    Some(Track {
        id: video_id,
        title,
        version: None,
        artists,
        albums,
        duration_ms,
        cover_uri,
        og_image: None,
        available: Some(true),
        content_warning,
        derived_colors: None,
        source: TrackSource::YouTubeMusic,
    })
}

/// Парсинг `playlistPanelVideoRenderer` (из очереди радио `/next`)
pub fn parse_playlist_panel_video(item: &serde_json::Value) -> Option<Track> {
    let renderer = item.get("playlistPanelVideoRenderer").unwrap_or(item);

    let video_id = renderer
        .get("videoId")
        .and_then(|v| v.as_str())?
        .to_string();
    let title = renderer
        .get("title")
        .and_then(extract_text)
        .unwrap_or_default();
    let mut cover_uri = extract_best_thumbnail(renderer);
    if cover_uri
        .as_deref()
        .is_none_or(|u| u.contains("googleusercontent.com"))
    {
        cover_uri = Some(format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", video_id));
    }

    let mut duration_ms = renderer
        .get("lengthText")
        .and_then(extract_text)
        .and_then(|s| parse_duration_ms(&s));

    let mut artists = Vec::new();
    let mut albums = Vec::new();

    let byline = renderer
        .get("longBylineText")
        .or_else(|| renderer.get("shortBylineText"));

    if let Some(runs) = byline
        .and_then(|b| b.get("runs"))
        .and_then(|r| r.as_array())
    {
        parse_track_meta_runs(runs, &mut artists, &mut albums, &mut duration_ms);
    }

    let content_warning = if is_explicit(renderer) {
        Some("explicit".to_string())
    } else {
        None
    };

    Some(Track {
        id: video_id,
        title,
        version: None,
        artists,
        albums,
        duration_ms,
        cover_uri,
        og_image: None,
        available: Some(true),
        content_warning,
        derived_colors: None,
        source: TrackSource::YouTubeMusic,
    })
}

/// Рекурсивный сбор всех треков из `musicResponsiveListItemRenderer`
pub(super) fn collect_responsive_tracks(json: &serde_json::Value, out: &mut Vec<Track>) {
    if let Some(obj) = json.as_object() {
        if let Some(item_renderer) = obj.get("musicResponsiveListItemRenderer") {
            if let Some(track) = parse_responsive_track(item_renderer) {
                out.push(track);
            }
        } else {
            for v in obj.values() {
                collect_responsive_tracks(v, out);
            }
        }
    } else if let Some(arr) = json.as_array() {
        for item in arr {
            collect_responsive_tracks(item, out);
        }
    }
}
