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
    #[serde(default)]
    pub artists: Vec<Artist>,
    #[serde(rename = "trackCount")]
    pub track_count: Option<u32>,
}

/// Плейлист (в поиске и в списке плейлистов пользователя)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Playlist {
    #[serde(deserialize_with = "deserialize_id")]
    pub uid: String,
    #[serde(deserialize_with = "deserialize_id")]
    pub kind: String,
    pub title: String,
    #[serde(rename = "trackCount")]
    pub track_count: Option<u32>,
    #[serde(rename = "ogImage")]
    pub og_image: Option<String>,
}

/// Формирует URL обложки из uri вида `avatars.yandex.net/.../%%` (например, 200, 400)
pub fn cover_url(uri: &str, size: u32) -> String {
    let path = uri.replace("%%", &format!("{}x{}", size, size));
    if path.starts_with("http://") || path.starts_with("https://") {
        path
    } else {
        format!("https://{}", path)
    }
}

impl Artist {
    pub fn cover_url(&self, size: u32) -> Option<String> {
        self.cover.as_ref()?.uri.as_deref().map(|u| cover_url(u, size))
    }
}

impl Album {
    pub fn cover_url(&self, size: u32) -> Option<String> {
        self.cover_uri.as_deref().map(|u| cover_url(u, size))
    }

    /// Имена артистов через запятую
    pub fn artists_string(&self) -> String {
        self.artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ")
    }
}

impl Playlist {
    pub fn cover_url(&self, size: u32) -> Option<String> {
        self.og_image.as_deref().map(|u| cover_url(u, size))
    }
}

/// Трек
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Track {
    #[serde(deserialize_with = "deserialize_id")]
    pub id: String,
    pub title: String,
    /// Пометка версии рядом с названием: «Slowed», «Remix», «Live»
    #[serde(default)]
    pub version: Option<String>,
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
    #[serde(rename = "contentWarning")]
    pub content_warning: Option<String>,
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
    /// Название для Pango-разметки: версия («Slowed») идёт следом приглушённым цветом
    pub fn title_markup(&self) -> String {
        let t = gtk4::glib::markup_escape_text(&self.title);
        match self.version.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            Some(v) => format!("{} <span alpha=\"55%\">{}</span>", t, gtk4::glib::markup_escape_text(v)),
            None => t.to_string(),
        }
    }

    /// Трек с пометкой «explicit» (нецензурная лексика)
    pub fn is_explicit(&self) -> bool {
        self.content_warning.as_deref() == Some("explicit")
    }

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
        self.cover_uri.as_ref().or(self.og_image.as_ref()).map(|u| cover_url(u, size))
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

/// Станция Ротора («Волна» и др.)
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
pub struct SearchSection<T> {
    #[serde(default = "Vec::new")]
    pub results: Vec<T>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SearchResult {
    pub tracks: Option<SearchSection<Track>>,
    pub albums: Option<SearchSection<Album>>,
    pub playlists: Option<SearchSection<Playlist>>,
    pub artists: Option<SearchSection<Artist>>,
}

/// Краткая страница артиста (`/artists/{id}/brief-info`)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtistBrief {
    pub artist: Artist,
    #[serde(default)]
    pub albums: Vec<Album>,
    #[serde(default, rename = "popularTracks")]
    pub popular_tracks: Vec<Track>,
}

/// Похожие треки (`/tracks/{id}/similar`)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SimilarTracks {
    #[serde(default, rename = "similarTracks")]
    pub similar_tracks: Vec<Track>,
}

/// Альбом с треками (`/albums/{id}/with-tracks`)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AlbumWithTracks {
    #[serde(default)]
    pub volumes: Vec<Vec<Track>>,
}

/// Плейлист с треками (`/users/{uid}/playlists/{kind}`)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlaylistWithTracks {
    #[serde(default)]
    pub tracks: Vec<PlaylistTrackItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlaylistTrackItem {
    pub track: Option<Track>,
}

/// Элемент `/users/{uid}/likes/albums?rich=true`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LikedAlbumItem {
    pub album: Album,
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
pub struct TrackLyricsResult {
    pub lyrics: Option<String>,
    #[serde(rename = "fullLyrics")]
    pub full_lyrics: Option<String>,
}

/// Ответ от открытой базы караоке LRCLIB (https://lrclib.net)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LrclibResponse {
    #[serde(rename = "syncedLyrics", alias = "synced_lyrics")]
    pub synced_lyrics: Option<String>,
    #[serde(rename = "plainLyrics", alias = "plain_lyrics")]
    pub plain_lyrics: Option<String>,
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
/// Чинит «кракозябры» в тексте песни: в LRCLIB встречаются записи, где UTF-8 прочитали как Windows-1252
/// (`can’t` превращается в `canâ€™t`). Строка чинится, только если получается целиком корректный UTF-8.
pub fn fix_mojibake(text: &str) -> String {
    // единицы cp1252 в диапазоне 0x80–0x9F, которых нет в Latin-1
    fn byte_of(c: char) -> Option<u8> {
        let b = match c {
            '€' => 0x80, '‚' => 0x82, 'ƒ' => 0x83, '„' => 0x84, '…' => 0x85, '†' => 0x86, '‡' => 0x87,
            'ˆ' => 0x88, '‰' => 0x89, 'Š' => 0x8A, '‹' => 0x8B, 'Œ' => 0x8C, 'Ž' => 0x8E, '‘' => 0x91,
            '’' => 0x92, '“' => 0x93, '”' => 0x94, '•' => 0x95, '–' => 0x96, '—' => 0x97, '˜' => 0x98,
            '™' => 0x99, 'š' => 0x9A, '›' => 0x9B, 'œ' => 0x9C, 'ž' => 0x9E, 'Ÿ' => 0x9F,
            c if (c as u32) <= 0xFF => c as u32 as u8,
            _ => return None,
        };
        Some(b)
    }
    // типичное начало испорченной последовательности: â, Ã, Â перед символом из 0x80–0xBF
    if !text.chars().any(|c| matches!(c, 'â' | 'Ã' | 'Â')) {
        return text.to_string();
    }
    let bytes: Option<Vec<u8>> = text.chars().map(byte_of).collect();
    match bytes.and_then(|b| String::from_utf8(b).ok()) {
        Some(fixed) if fixed.chars().count() < text.chars().count() => fixed,
        _ => text.to_string(),
    }
}

pub fn parse_lrc(lrc_text: &str) -> Vec<LrcLine> {
    let mut lines = Vec::new();
    for raw_line in lrc_text.lines() {
        let trimmed = raw_line.trim();
        if !trimmed.starts_with('[') {
            continue;
        }

        if let Some(close_bracket) = trimmed.find(']') {
            let time_part = &trimmed[1..close_bracket];
            let text = fix_mojibake(trimmed[close_bracket + 1..].trim());

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
    #[test]
    fn mojibake_is_repaired_per_line() {
        use super::fix_mojibake;
        assert_eq!(fix_mojibake("But you can\u{e2}\u{20ac}\u{2122}t put your finger"), "But you can’t put your finger");
        assert_eq!(fix_mojibake("But you can't put your finger"), "But you can't put your finger");
        // нормальные «â» и «Ã» не портим
        assert_eq!(fix_mojibake("Voilà, âge"), "Voilà, âge");
        assert_eq!(fix_mojibake("Привет"), "Привет");
    }

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

    #[test]
    fn test_search_duplicate_keys() {
        // Яндекс отдаёт `albums` дважды: через Value (как в client.rs) разбор должен проходить
        let v: serde_json::Value =
            serde_json::from_str(r#"{"result":{"albums":{"results":[]},"albums":{"results":[]}}}"#).unwrap();
        let r: YandexApiResponse<SearchResult> = serde_json::from_value(v).unwrap();
        assert!(r.result.albums.is_some());
    }
}
