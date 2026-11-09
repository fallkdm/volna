use crate::api::models::{Album, Playlist, Track, TrackSource};

/// Куда ведёт клик по плашке источника
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum SourceTarget {
    Album(Album),
    Playlist(Playlist),
    Artist(String),
    Track(Track),
    /// Вкладка «Медиатека» («Мне нравится», «Загруженные»)
    Library,
}

/// Откуда играет очередь: подпись на плашке и страница, куда ведёт клик по ней
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PlaySource {
    pub label: String,
    pub target: Option<SourceTarget>,
    #[serde(default)]
    pub source: Option<TrackSource>,
    #[serde(default)]
    pub artist_name: Option<String>,
}

impl PlaySource {
    pub fn new(label: impl Into<String>, target: Option<SourceTarget>) -> Self {
        Self {
            label: label.into(),
            target,
            source: None,
            artist_name: None,
        }
    }
}

/// Разбирает идентификатор артиста, очищает префиксы (`ya:`, `yt:`, `vk:name:`) и определяет источник.
pub fn parse_artist_id(raw_id: &str, fallback_source: Option<TrackSource>) -> Result<(String, TrackSource), String> {
    let clean = raw_id.trim();
    if let Some(rest) = clean.strip_prefix("vk:name:") {
        let decoded = glib::Uri::unescape_string(rest, None)
            .map(|s| s.to_string())
            .unwrap_or_else(|| rest.to_string());
        return Ok((decoded, TrackSource::Vk));
    }
    if let Some(rest) = clean.strip_prefix("vk:") {
        return Ok((rest.to_string(), TrackSource::Vk));
    }
    if let Some(rest) = clean.strip_prefix("yt:") {
        return Ok((rest.to_string(), TrackSource::YouTubeMusic));
    }
    if clean.starts_with("UC") {
        return Ok((clean.to_string(), TrackSource::YouTubeMusic));
    }
    if let Some(rest) = clean.strip_prefix("ya:") {
        return Ok((rest.to_string(), TrackSource::Yandex));
    }
    if let Some(src) = fallback_source {
        return Ok((clean.to_string(), src));
    }
    Err("Источник артиста не определён".to_string())
}

/// Ищет артиста в реестре только по `id` (без знания источника).
/// Если совпадение ровно одно — возвращает источник и имя.
/// При коллизии (найдено >1 источника) или отсутствии — возвращает `None`.
pub fn lookup_registered_artist_in_map(
    registry: &std::collections::HashMap<(TrackSource, String), String>,
    raw_id: &str,
) -> Option<(TrackSource, String)> {
    let clean_id = raw_id.trim();
    if clean_id.is_empty() {
        return None;
    }
    let matches: Vec<(TrackSource, String)> = registry
        .iter()
        .filter(|((_, id), _)| id == clean_id)
        .map(|(&(source, _), name)| (source, name.clone()))
        .collect();
    if matches.len() == 1 {
        Some(matches[0].clone())
    } else {
        None
    }
}
