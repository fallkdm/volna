use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Вспомогательная функция для десериализации ID как строки из числа или строки
pub fn deserialize_id<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(deserializer)?;
    match v {
        serde_json::Value::String(s) => Ok(s),
        serde_json::Value::Number(n) => Ok(n.to_string()),
        _ => Ok(v.to_string()),
    }
}

/// Стандартный ответ Яндекс Музыки с полем `result`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YandexApiResponse<T> {
    pub result: T,
}

/// Артист
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Artist {
    #[serde(deserialize_with = "deserialize_id")]
    pub id: String,
    pub name: String,
    pub cover: Option<ArtistCover>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ArtistCover {
    pub uri: Option<String>,
}

/// Альбом
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Album {
    #[serde(deserialize_with = "deserialize_id")]
    pub id: String,
    pub title: Option<String>,
    pub year: Option<u32>,
    #[serde(rename = "coverUri")]
    pub cover_uri: Option<String>,
}

/// Трек
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Track {
    #[serde(deserialize_with = "deserialize_id")]
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub artists: Vec<Artist>,
    #[serde(default)]
    pub albums: Vec<Album>,
    #[serde(rename = "durationMs")]
    pub duration_ms: Option<u64>,
    #[serde(rename = "coverUri")]
    pub cover_uri: Option<String>,
    #[serde(rename = "ogImage")]
    pub og_image: Option<String>,
    pub available: Option<bool>,
    #[serde(rename = "derivedColors")]
    pub derived_colors: Option<DerivedColors>,
}

/// Производные акцентные цвета трека из Яндекс Музыки
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DerivedColors {
    pub average: Option<String>,
    pub accent: Option<String>,
    #[serde(rename = "miniPlayer")]
    pub mini_player: Option<String>,
    #[serde(rename = "waveText")]
    pub wave_text: Option<String>,
}

/// Парсинг HEX цвета вида #RRGGBB или RRGGBB в нормализованные [f32; 3]
pub fn parse_hex_color(hex: &str) -> Option<[f32; 3]> {
    let s = hex.trim().trim_start_matches('#');
    if s.len() == 6 {
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        Some([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0])
    } else {
        None
    }
}

impl DerivedColors {
    /// Основной акцентный цвет (по умолчанию янтарный [0.95, 0.42, 0.02])
    pub fn core_rgb(&self) -> [f32; 3] {
        self.accent
            .as_deref()
            .and_then(parse_hex_color)
            .unwrap_or([0.95, 0.42, 0.02])
    }

    /// Глубокий тёмный фоновый цвет (по умолчанию тёмно-терракотовый [0.36, 0.10, 0.00])
    pub fn deep_rgb(&self) -> [f32; 3] {
        self.average
            .as_deref()
            .and_then(parse_hex_color)
            .unwrap_or([0.36, 0.10, 0.00])
    }

    /// Осветлённый акцент складок (по умолчанию кремово-золотистый [1.0, 0.84, 0.42])
    pub fn ridge_rgb(&self) -> [f32; 3] {
        if let Some(accent) = self.accent.as_deref().and_then(parse_hex_color) {
            [
                (accent[0] * 0.6 + 0.4).min(1.0),
                (accent[1] * 0.6 + 0.4).min(1.0),
                (accent[2] * 0.6 + 0.4).min(1.0),
            ]
        } else {
            [1.0, 0.84, 0.42]
        }
    }
}

impl Track {
    /// Возвращает имена артистов через запятую
    pub fn artists_string(&self) -> String {
        if self.artists.is_empty() {
            "Неизвестный исполнитель".to_string()
        } else {
            self.artists
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        }
    }

    /// Формирует URL обложки с заданным размером (например, 200, 400)
    pub fn cover_url(&self, size: u32) -> Option<String> {
        let uri = self.cover_uri.as_ref().or(self.og_image.as_ref())?;
        let size_str = format!("{}x{}", size, size);
        let path = uri.replace("%%", &size_str);
        if path.starts_with("http://") || path.starts_with("https://") {
            Some(path)
        } else {
            Some(format!("https://{}", path))
        }
    }

    /// Возвращает длительность в формате "MM:SS"
    pub fn formatted_duration(&self) -> String {
        let total_secs = self.duration_ms.unwrap_or(0) / 1000;
        let mins = total_secs / 60;
        let secs = total_secs % 60;
        format!("{:02}:{:02}", mins, secs)
    }

    /// Основной акцентный цвет шейдера Волны
    pub fn core_rgb(&self) -> [f32; 3] {
        self.derived_colors
            .as_ref()
            .map(|c| c.core_rgb())
            .unwrap_or([0.95, 0.42, 0.02])
    }

    /// Глубокий тёмный цвет шейдера Волны
    pub fn deep_rgb(&self) -> [f32; 3] {
        self.derived_colors
            .as_ref()
            .map(|c| c.deep_rgb())
            .unwrap_or([0.36, 0.10, 0.00])
    }

    /// Осветленный акцентный цвет гребней шейдера Волны
    pub fn ridge_rgb(&self) -> [f32; 3] {
        self.derived_colors
            .as_ref()
            .map(|c| c.ridge_rgb())
            .unwrap_or([1.0, 0.84, 0.42])
    }
}

/// Идентификатор станции Ротора
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RotorStationId {
    #[serde(rename = "type")]
    pub station_type: String,
    pub tag: String,
}

impl RotorStationId {
    pub fn new(station_type: &str, tag: &str) -> Self {
        Self {
            station_type: station_type.to_string(),
            tag: tag.to_string(),
        }
    }

    pub fn to_station_string(&self) -> String {
        format!("{}:{}", self.station_type, self.tag)
    }
}

/// Иконка станции
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RotorIcon {
    #[serde(rename = "backgroundColor")]
    pub background_color: Option<String>,
    #[serde(rename = "imageUrl")]
    pub image_url: Option<String>,
}

/// Станция Ротора («Моя волна» и др.)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RotorStation {
    pub id: RotorStationId,
    pub name: Option<String>,
    pub icon: Option<RotorIcon>,
}

/// Элемент последовательности треков в станции
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RotorSequenceItem {
    pub track: Track,
    pub liked: Option<bool>,
}

/// Ответ списка треков станции
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RotorStationTracks {
    #[serde(rename = "batchId")]
    pub batch_id: Option<String>,
    pub sequence: Vec<RotorSequenceItem>,
}

/// Тип события обратной связи для станции
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotorFeedbackType {
    TrackStarted,
    TrackFinished,
    Skip,
    Like,
    Dislike,
}

impl RotorFeedbackType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TrackStarted => "trackStarted",
            Self::TrackFinished => "trackFinished",
            Self::Skip => "skip",
            Self::Like => "like",
            Self::Dislike => "dislike",
        }
    }
}

impl Serialize for RotorFeedbackType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for RotorFeedbackType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "trackStarted" => Ok(Self::TrackStarted),
            "trackFinished" => Ok(Self::TrackFinished),
            "skip" => Ok(Self::Skip),
            "like" => Ok(Self::Like),
            "dislike" => Ok(Self::Dislike),
            _ => Err(serde::de::Error::custom(format!("Unknown feedback type: {}", s))),
        }
    }
}

/// Запрос обратной связи (Feedback)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RotorFeedback {
    #[serde(rename = "type")]
    pub feedback_type: RotorFeedbackType,
    #[serde(rename = "trackId")]
    pub track_id: String,
    pub timestamp: String,
    #[serde(rename = "totalPlayedSeconds", skip_serializing_if = "Option::is_none")]
    pub total_played_seconds: Option<f64>,
    #[serde(rename = "batchId", skip_serializing_if = "Option::is_none")]
    pub batch_id: Option<String>,
    #[serde(rename = "from", skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

/// Информация о варианте скачивания трека
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadInfo {
    pub codec: String,
    #[serde(rename = "bitrateInKbps")]
    pub bitrate_in_kbps: u32,
    #[serde(rename = "downloadInfoUrl")]
    pub download_info_url: String,
    #[serde(default)]
    pub direct: bool,
}

// ================= Модели Поиска =================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SearchResultTracks {
    #[serde(default)]
    pub items: Vec<Track>,
    pub total: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SearchResult {
    pub tracks: Option<SearchResultTracks>,
}

// ================= Модели Аккаунта и Медиатеки =================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AccountStatus {
    pub account: Option<AccountInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AccountInfo {
    #[serde(deserialize_with = "deserialize_id")]
    pub uid: String,
    pub login: Option<String>,
    #[serde(rename = "fullName")]
    pub full_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LikedTrackItem {
    #[serde(deserialize_with = "deserialize_id")]
    pub id: String,
    #[serde(rename = "albumId")]
    pub album_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LikedTracksResult {
    pub library: Option<LikedTracksLibrary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LikedTracksLibrary {
    #[serde(default)]
    pub tracks: Vec<LikedTrackItem>,
}

// ================= Модели Синхронизированных Текстов (Lyrics) =================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TrackSupplement {
    pub lyrics: Option<LyricsInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LyricsInfo {
    #[serde(rename = "syncType")]
    pub sync_type: Option<String>,
    #[serde(rename = "fullLyrics")]
    pub full_lyrics: Option<String>,
}

/// Строка синхронизированного караоке
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LrcLine {
    pub time_ms: u64,
    pub text: String,
}

/// Парсит LRC текст формата `[mm:ss.xx] Текст`
pub fn parse_lrc(lrc_text: &str) -> Vec<LrcLine> {
    let mut lines = Vec::new();
    for raw_line in lrc_text.lines() {
        let trimmed = raw_line.trim();
        if !trimmed.starts_with('[') {
            continue;
        }

        if let Some(close_bracket) = trimmed.find(']') {
            let time_part = &trimmed[1..close_bracket];
            let text = trimmed[close_bracket + 1..].trim().to_string();

            // mm:ss.xx
            if let Some((min_str, sec_str)) = time_part.split_once(':') {
                if let (Ok(mins), Ok(secs_f)) = (min_str.parse::<u64>(), sec_str.parse::<f64>()) {
                    let time_ms = (mins * 60 * 1000) + (secs_f * 1000.0) as u64;
                    lines.push(LrcLine { time_ms, text });
                }
            }
        }
    }
    lines.sort_by_key(|l| l.time_ms);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_track_deserialization_and_helpers() {
        let json_data = r#"{
            "id": 1234567,
            "title": "Спокойная ночь",
            "artists": [{"id": "41126", "name": "Кино"}],
            "albums": [{"id": 123, "title": "Группа крови", "year": 1988}],
            "durationMs": 367000,
            "coverUri": "avatars.yandex.net/get-music-content/12345/%%"
        }"#;

        let track: Track = serde_json::from_str(json_data).expect("Must deserialize track");
        assert_eq!(track.id, "1234567");
        assert_eq!(track.title, "Спокойная ночь");
        assert_eq!(track.artists_string(), "Кино");
        assert_eq!(track.formatted_duration(), "06:07");
        assert_eq!(
            track.cover_url(400),
            Some("https://avatars.yandex.net/get-music-content/12345/400x400".to_string())
        );
    }

    #[test]
    fn test_feedback_serialization() {
        let fb = RotorFeedback {
            feedback_type: RotorFeedbackType::TrackStarted,
            track_id: "123".to_string(),
            timestamp: "2026-10-13T19:00:00Z".to_string(),
            total_played_seconds: Some(42.5),
            batch_id: Some("b1".to_string()),
            from: Some("volna".to_string()),
        };

        let json = serde_json::to_string(&fb).unwrap();
        assert!(json.contains("\"type\":\"trackStarted\""));
        assert!(json.contains("\"trackId\":\"123\""));
        assert!(json.contains("\"totalPlayedSeconds\":42.5"));
    }

    #[test]
    fn test_parse_lrc() {
        let sample = "[00:04.20] Waiting in a car\n[00:08.50] Waiting for a ride in the dark\n[00:15.10] The city is my church";
        let parsed = parse_lrc(sample);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].time_ms, 4200);
        assert_eq!(parsed[0].text, "Waiting in a car");
        assert_eq!(parsed[1].time_ms, 8500);
        assert_eq!(parsed[2].time_ms, 15100);
    }

    #[test]
    fn test_derived_colors() {
        let json_data = r##"{
            "id": 999,
            "title": "Звезда по имени Солнце",
            "derivedColors": {
                "average": "#1A0A00",
                "accent": "#FF6B00",
                "miniPlayer": "#FF6B00",
                "waveText": "#FFFFFF"
            }
        }"##;

        let track: Track = serde_json::from_str(json_data).unwrap();
        let colors = track.derived_colors.as_ref().unwrap();
        assert_eq!(colors.accent.as_deref(), Some("#FF6B00"));
        assert_eq!(colors.average.as_deref(), Some("#1A0A00"));

        let core = track.core_rgb();
        assert!((core[0] - 1.0).abs() < 0.01);
        assert!((core[1] - (107.0 / 255.0)).abs() < 0.01);
        assert!((core[2] - 0.0).abs() < 0.01);

        let deep = track.deep_rgb();
        assert!((deep[0] - (26.0 / 255.0)).abs() < 0.01);
        assert!((deep[1] - (10.0 / 255.0)).abs() < 0.01);

        let ridge = track.ridge_rgb();
        assert!(ridge[0] > 0.9);
    }
}
