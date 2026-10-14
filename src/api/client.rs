use base64::prelude::*;
use chrono::Utc;
use hmac::{Hmac, KeyInit, Mac};
use md5::{Digest, Md5};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};
use sha2::Sha256;
use std::fmt;

use super::models::{
    parse_lrc, AccountStatus, DownloadInfo, LikedTracksResult, LrclibResponse, RotorFeedback,
    RotorFeedbackType, RotorStationTracks, SearchResult, Track, TrackLyricsResult, TrackSupplement,
    YandexApiResponse,
};

const BASE_API_URL: &str = "https://api.music.yandex.net";
const USER_AGENT_VALUE: &str = "YandexMusic/2024.1 (Linux; Volna Desktop)";
const MD5_SALT: &str = "XGRlBW9FXlekgb0rTaf0yN";
const LYRICS_SIGN_KEY: &[u8] = b"p93jhgh689SBReK6ghtw62";

type HmacSha256 = Hmac<Sha256>;

fn sign_lyrics_request(track_id: &str, timestamp: i64) -> String {
    let mut mac = HmacSha256::new_from_slice(LYRICS_SIGN_KEY).expect("HMAC supports any key size");
    let payload = format!("{}{}", track_id, timestamp);
    mac.update(payload.as_bytes());
    let code = mac.finalize();
    BASE64_STANDARD.encode(code.into_bytes())
}

#[derive(Debug)]
pub enum ApiError {
    Request(reqwest::Error),
    XmlParse(String),
    MissingData(String),
    InvalidResponse(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request(e) => write!(f, "Сетевая ошибка: {}", e),
            Self::XmlParse(e) => write!(f, "Ошибка разбора XML: {}", e),
            Self::MissingData(e) => write!(f, "Отсутствуют данные: {}", e),
            Self::InvalidResponse(e) => write!(f, "Некорректный ответ API: {}", e),
        }
    }
}

impl std::error::Error for ApiError {}

impl From<reqwest::Error> for ApiError {
    fn from(err: reqwest::Error) -> Self {
        Self::Request(err)
    }
}

/// Клиент для работы с API Яндекс Музыки
#[derive(Clone)]
pub struct YandexMusicClient {
    client: reqwest::Client,
    token: Option<String>,
    base_url: String,
}

impl YandexMusicClient {
    /// Создает новый экземпляр клиента с опциональным токеном
    pub fn new(token: Option<String>) -> Self {
        Self {
            client: reqwest::Client::builder()
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            token,
            base_url: BASE_API_URL.to_string(),
        }
    }

    /// Устанавливает токен авторизации
    pub fn set_token(&mut self, token: Option<String>) {
        self.token = token;
    }

    /// Получает текущий токен
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    /// Проверяет, установлен ли токен
    pub fn has_token(&self) -> bool {
        self.token.as_ref().map(|t| !t.trim().is_empty()).unwrap_or(false)
    }

    /// Формирует заголовки для запросов к API
    fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static(USER_AGENT_VALUE));
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));

        if let Some(token) = &self.token {
            let auth_str = format!("OAuth {}", token.trim());
            if let Ok(val) = HeaderValue::from_str(&auth_str) {
                headers.insert(AUTHORIZATION, val);
            }
        }
        headers
    }

    /// Получает треки станции Ротора (по умолчанию «Моя волна» - "user:onyourwave")
    pub async fn get_station_tracks(&self, station_id: &str) -> Result<RotorStationTracks, ApiError> {
        self.get_station_tracks_with_params(station_id, None, None, None).await
    }

    /// Получает треки станции Ротора с фильтрами настроения, языка и характера
    pub async fn get_station_tracks_with_params(
        &self,
        station_id: &str,
        mood: Option<&str>,
        language: Option<&str>,
        character: Option<&str>,
    ) -> Result<RotorStationTracks, ApiError> {
        let mut url = format!("{}/rotor/station/{}/tracks", self.base_url, station_id);
        let mut query_params = Vec::new();
        if let Some(m) = mood {
            query_params.push(format!("mood={}", m));
        }
        if let Some(l) = language {
            query_params.push(format!("language={}", l));
        }
        if let Some(c) = character {
            query_params.push(format!("diversity={}", c));
        }
        if !query_params.is_empty() {
            url.push('?');
            url.push_str(&query_params.join("&"));
        }

        let resp = self
            .client
            .get(&url)
            .headers(self.headers())
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ApiError::InvalidResponse(format!(
                "HTTP {}: {}",
                status, body
            )));
        }

        let api_response: YandexApiResponse<RotorStationTracks> = resp.json().await?;
        Ok(api_response.result)
    }

    /// Получает информацию о доступных потоках трека
    pub async fn get_track_download_info(&self, track_id: &str) -> Result<Vec<DownloadInfo>, ApiError> {
        let url = format!("{}/tracks/{}/download-info", self.base_url, track_id);
        let resp = self
            .client
            .get(&url)
            .headers(self.headers())
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ApiError::InvalidResponse(format!(
                "HTTP {}: {}",
                status, body
            )));
        }

        let api_response: YandexApiResponse<Vec<DownloadInfo>> = resp.json().await?;
        Ok(api_response.result)
    }

    /// Получает прямую ссылку на скачивание/стриминг MP3 файла трека через download-info
    pub async fn get_track_direct_url(&self, track_id: &str) -> Result<String, ApiError> {
        let download_infos = self.get_track_download_info(track_id).await?;
        if download_infos.is_empty() {
            return Err(ApiError::MissingData(format!(
                "Нет доступных ссылок для трека {}",
                track_id
            )));
        }

        let best_info = download_infos
            .iter()
            .filter(|i| i.codec == "mp3")
            .max_by_key(|i| i.bitrate_in_kbps)
            .or_else(|| download_infos.first())
            .ok_or_else(|| ApiError::MissingData("Не найден подходящий аудиопоток".into()))?;

        let xml_resp = self
            .client
            .get(&best_info.download_info_url)
            .headers(self.headers())
            .send()
            .await?
            .text()
            .await?;

        let host = extract_xml_tag(&xml_resp, "host")
            .ok_or_else(|| ApiError::XmlParse("Тег <host> не найден".into()))?;
        let path = extract_xml_tag(&xml_resp, "path")
            .ok_or_else(|| ApiError::XmlParse("Тег <path> не найден".into()))?;
        let ts = extract_xml_tag(&xml_resp, "ts")
            .ok_or_else(|| ApiError::XmlParse("Тег <ts> не найден".into()))?;
        let s = extract_xml_tag(&xml_resp, "s")
            .ok_or_else(|| ApiError::XmlParse("Тег <s> не найден".into()))?;

        let path_without_leading_slash = if path.starts_with('/') {
            &path[1..]
        } else {
            path
        };

        let sign_input = format!("{}{}{}", MD5_SALT, path_without_leading_slash, s);
        let mut hasher = Md5::new();
        hasher.update(sign_input.as_bytes());
        let sign_result = hasher.finalize();
        let sign_hex: String = sign_result.iter().map(|b| format!("{:02x}", b)).collect();

        let direct_url = format!("https://{}/get-mp3/{}/{}{}", host, sign_hex, ts, path);
        Ok(direct_url)
    }

    /// Поиск треков по поисковому запросу
    pub async fn search(&self, query: &str) -> Result<Vec<Track>, ApiError> {
        let url = format!("{}/search?text={}&type=track&page=0", self.base_url, urlencoding_encode(query));
        let resp = self
            .client
            .get(&url)
            .headers(self.headers())
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ApiError::InvalidResponse(format!("HTTP {}: {}", status, body)));
        }

        let api_response: YandexApiResponse<SearchResult> = resp.json().await?;
        let tracks = api_response.result.tracks.map(|t| t.items).unwrap_or_default();
        Ok(tracks)
    }

    /// Получение статуса аккаунта пользователя (включая UID)
    pub async fn get_account_status(&self) -> Result<AccountStatus, ApiError> {
        let url = format!("{}/account/status", self.base_url);
        let resp = self
            .client
            .get(&url)
            .headers(self.headers())
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ApiError::InvalidResponse(format!("HTTP {}: {}", status, body)));
        }

        let api_response: YandexApiResponse<AccountStatus> = resp.json().await?;
        Ok(api_response.result)
    }

    /// Получение любимых треков («Мне нравится»)
    pub async fn get_liked_tracks(&self, uid: &str) -> Result<Vec<Track>, ApiError> {
        let url = format!("{}/users/{}/likes/tracks", self.base_url, uid);
        let resp = self
            .client
            .get(&url)
            .headers(self.headers())
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ApiError::InvalidResponse(format!("HTTP {}: {}", status, body)));
        }

        let liked_resp: YandexApiResponse<LikedTracksResult> = resp.json().await?;
        let track_items = liked_resp.result.library.map(|l| l.tracks).unwrap_or_default();
        if track_items.is_empty() {
            return Ok(Vec::new());
        }

        // Запрашиваем первые 50 полных треков
        let ids: Vec<String> = track_items.into_iter().take(50).map(|t| t.id).collect();
        let ids_param = ids.join(",");

        let tracks_url = format!("{}/tracks", self.base_url);
        let form_body = format!("track-ids={}", ids_param);
        let mut headers = self.headers();
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );

        let tracks_resp = self
            .client
            .post(&tracks_url)
            .headers(headers)
            .body(form_body)
            .send()
            .await?;

        if !tracks_resp.status().is_success() {
            return Ok(Vec::new());
        }

        let full_tracks: YandexApiResponse<Vec<Track>> = tracks_resp.json().await?;
        Ok(full_tracks.result)
    }

    /// Запрос текста песни из открытой глобальной базы LRCLIB (https://lrclib.net)
    pub async fn get_lrclib_lyrics(
        &self,
        artist: &str,
        title: &str,
        duration_secs: Option<u64>,
    ) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
        let clean_artist = artist.trim();
        let clean_title = title.trim();
        if clean_artist.is_empty() || clean_title.is_empty() {
            return Ok(None);
        }

        let encoded_artist = urlencoding_encode(clean_artist);
        let encoded_title = urlencoding_encode(clean_title);

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

        let primary_url = make_url(duration_secs);
        let resp = self
            .client
            .get(&primary_url)
            .header(
                "User-Agent",
                "VolnaMusicPlayer/1.0 (https://github.com/fallkdm/volna)",
            )
            .send()
            .await;

        let resp = match resp {
            Ok(r) if r.status().is_success() => Some(r),
            // Если запрос с точной длительностью вернул 404, повторяем поиск без ограничения длительности
            Ok(r) if r.status() == reqwest::StatusCode::NOT_FOUND && duration_secs.is_some() => {
                let fallback_url = make_url(None);
                if let Ok(r2) = self
                    .client
                    .get(&fallback_url)
                    .header(
                        "User-Agent",
                        "VolnaMusicPlayer/1.0 (https://github.com/fallkdm/volna)",
                    )
                    .send()
                    .await
                {
                    if r2.status().is_success() {
                        Some(r2)
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            _ => None,
        };

        if let Some(response) = resp {
            if let Ok(data) = response.json::<LrclibResponse>().await {
                if let Some(synced) = data.synced_lyrics {
                    let trimmed = synced.trim();
                    if !trimmed.is_empty() {
                        return Ok(Some(trimmed.to_string()));
                    }
                }
                if let Some(plain) = data.plain_lyrics {
                    let trimmed = plain.trim();
                    if !trimmed.is_empty() {
                        return Ok(Some(trimmed.to_string()));
                    }
                }
            }
        }

        Ok(None)
    }

    /// Запрос текста песни для плеера:
    /// - Если Яндекс Музыка вернула синхронизированный LRC с таймкодами — используем его.
    /// - Если Яндекс Музыка вернула статический текст (без таймкодов) или ошибку — запрашиваем LRCLIB.
    /// - Если LRCLIB нашел синхронизированный LRC — отдаем его (полноценное караоке!).
    /// - Если LRCLIB ничего не нашел — отдаем статический текст Яндекса.
    pub async fn get_track_lyrics(
        &self,
        track_id: &str,
        artist: &str,
        title: &str,
        duration_secs: Option<u64>,
    ) -> Result<Option<String>, ApiError> {
        let mut yandex_plain: Option<String> = None;

        // 1. Пробуем запросить официальный синхронизированный LRC из Яндекса
        let timestamp = chrono::Utc::now().timestamp();
        let sign = sign_lyrics_request(track_id, timestamp);
        let lrc_url = format!(
            "{}/tracks/{}/lyrics?format=LRC&timeStamp={}&sign={}",
            self.base_url,
            track_id,
            timestamp,
            urlencoding_encode(&sign)
        );

        let mut headers = self.headers();
        headers.insert(
            HeaderName::from_static("x-yandex-music-client"),
            HeaderValue::from_static("YandexMusicAndroid/24023231"),
        );

        if let Ok(resp) = self.client.get(&lrc_url).headers(headers).send().await {
            if resp.status().is_success() {
                if let Ok(lrc_resp) = resp.json::<YandexApiResponse<TrackLyricsResult>>().await {
                    if let Some(lrc_text) = lrc_resp.result.lyrics.or(lrc_resp.result.full_lyrics) {
                        let trimmed = lrc_text.trim();
                        if !trimmed.is_empty() {
                            if !parse_lrc(trimmed).is_empty() {
                                return Ok(Some(trimmed.to_string()));
                            } else {
                                yandex_plain = Some(trimmed.to_string());
                            }
                        }
                    }
                }
            }
        }

        // Если Яндекс не вернул LRC, пробуем /tracks/{track_id}/supplement на случай статического текста
        if yandex_plain.is_none() {
            let supp_url = format!("{}/tracks/{}/supplement", self.base_url, track_id);
            if let Ok(resp) = self.client.get(&supp_url).headers(self.headers()).send().await {
                if resp.status().is_success() {
                    if let Ok(supp) = resp.json::<YandexApiResponse<TrackSupplement>>().await {
                        if let Some(full) = supp.result.lyrics.and_then(|l| l.full_lyrics) {
                            let trimmed = full.trim();
                            if !trimmed.is_empty() {
                                if !parse_lrc(trimmed).is_empty() {
                                    return Ok(Some(trimmed.to_string()));
                                } else {
                                    yandex_plain = Some(trimmed.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }

        // 2. У Яндекса нет синхронизированных таймкодов (или запрос вернул ошибку).
        // Запрашиваем открытую глобальную базу LRCLIB (https://lrclib.net)
        if let Ok(Some(lrclib_text)) = self.get_lrclib_lyrics(artist, title, duration_secs).await {
            // Если в LRCLIB есть синхронизированные таймкоды — возвращаем их (полноценное караоке!)
            if !parse_lrc(&lrclib_text).is_empty() {
                return Ok(Some(lrclib_text));
            }
            // Если в LRCLIB только статический текст, а у Яндекса текста не было вовсе
            if yandex_plain.is_none() {
                return Ok(Some(lrclib_text));
            }
        }

        // 3. Fallback на статический текст из Яндекс Музыки
        Ok(yandex_plain)
    }

    /// Отправляет событие обратной связи (Feedback) в станцию Ротора
    pub async fn send_feedback(
        &self,
        station_id: &str,
        feedback: RotorFeedback,
    ) -> Result<(), ApiError> {
        let url = format!("{}/rotor/station/{}/feedback", self.base_url, station_id);
        let resp = self
            .client
            .post(&url)
            .headers(self.headers())
            .json(&feedback)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ApiError::InvalidResponse(format!(
                "HTTP {}: {}",
                status, body
            )));
        }

        Ok(())
    }

    /// Уведомление о старте воспроизведения трека (trackStarted)
    pub async fn feedback_track_started(
        &self,
        station_id: &str,
        track_id: &str,
        batch_id: Option<String>,
    ) -> Result<(), ApiError> {
        let feedback = RotorFeedback {
            feedback_type: RotorFeedbackType::TrackStarted,
            track_id: track_id.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            total_played_seconds: None,
            batch_id,
            from: Some("volna-desktop".to_string()),
        };
        self.send_feedback(station_id, feedback).await
    }

    /// Уведомление о завершении трека (trackFinished)
    pub async fn feedback_track_finished(
        &self,
        station_id: &str,
        track_id: &str,
        total_played_seconds: f64,
        batch_id: Option<String>,
    ) -> Result<(), ApiError> {
        let feedback = RotorFeedback {
            feedback_type: RotorFeedbackType::TrackFinished,
            track_id: track_id.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            total_played_seconds: Some(total_played_seconds),
            batch_id,
            from: Some("volna-desktop".to_string()),
        };
        self.send_feedback(station_id, feedback).await
    }

    /// Уведомление о пропуске трека (skip)
    pub async fn feedback_skip(
        &self,
        station_id: &str,
        track_id: &str,
        total_played_seconds: f64,
        batch_id: Option<String>,
    ) -> Result<(), ApiError> {
        let feedback = RotorFeedback {
            feedback_type: RotorFeedbackType::Skip,
            track_id: track_id.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            total_played_seconds: Some(total_played_seconds),
            batch_id,
            from: Some("volna-desktop".to_string()),
        };
        self.send_feedback(station_id, feedback).await
    }

    /// Отметка «Нравится» (like)
    pub async fn feedback_like(
        &self,
        station_id: &str,
        track_id: &str,
        batch_id: Option<String>,
    ) -> Result<(), ApiError> {
        let feedback = RotorFeedback {
            feedback_type: RotorFeedbackType::Like,
            track_id: track_id.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            total_played_seconds: None,
            batch_id,
            from: Some("volna-desktop".to_string()),
        };
        self.send_feedback(station_id, feedback).await
    }

    /// Отметка «Не нравится» (dislike)
    pub async fn feedback_dislike(
        &self,
        station_id: &str,
        track_id: &str,
        batch_id: Option<String>,
    ) -> Result<(), ApiError> {
        let feedback = RotorFeedback {
            feedback_type: RotorFeedbackType::Dislike,
            track_id: track_id.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            total_played_seconds: None,
            batch_id,
            from: Some("volna-desktop".to_string()),
        };
        self.send_feedback(station_id, feedback).await
    }
}

/// Простое URL кодирование строки
fn urlencoding_encode(s: &str) -> String {
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

/// Извлекает содержимое внутри простого XML тега <tag>...</tag>
fn extract_xml_tag<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open_tag = format!("<{}>", tag);
    let close_tag = format!("</{}>", tag);
    let start = xml.find(&open_tag)? + open_tag.len();
    let end = xml[start..].find(&close_tag)? + start;
    Some(xml[start..end].trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_xml_tag() {
        let sample_xml = r#"
        <download-info>
            <host>storage.mds.yandex.net</host>
            <path>/get-mp3/hash/state/track/456</path>
            <ts>1728840000</ts>
            <s>secret123</s>
        </download-info>
        "#;

        assert_eq!(extract_xml_tag(sample_xml, "host"), Some("storage.mds.yandex.net"));
        assert_eq!(extract_xml_tag(sample_xml, "path"), Some("/get-mp3/hash/state/track/456"));
        assert_eq!(extract_xml_tag(sample_xml, "ts"), Some("1728840000"));
        assert_eq!(extract_xml_tag(sample_xml, "s"), Some("secret123"));
        assert_eq!(extract_xml_tag(sample_xml, "nonexistent"), None);
    }

    #[test]
    fn test_urlencode() {
        assert_eq!(urlencoding_encode("hello world"), "hello+world");
        assert_eq!(urlencoding_encode("кино"), "%D0%BA%D0%B8%D0%BD%D0%BE");
    }

    #[test]
    fn test_sign_lyrics_request() {
        let sign = sign_lyrics_request("12345", 1728840000);
        assert!(!sign.is_empty());
        assert_eq!(sign, sign_lyrics_request("12345", 1728840000));
    }

    #[test]
    fn test_lrclib_response_deserialization() {
        let json_camel = r#"{
            "id": 1154804,
            "trackName": "Blessed with a Curse",
            "artistName": "Bring Me the Horizon",
            "plainLyrics": "Ever since this began, I was blessed with a curse\n",
            "syncedLyrics": "[01:01.06] Ever since this began, I was blessed with a curse\n[01:08.02] And for better or for worse\n"
        }"#;

        let parsed: LrclibResponse = serde_json::from_str(json_camel).unwrap();
        assert!(parsed.synced_lyrics.is_some());
        assert!(parsed.plain_lyrics.is_some());
        assert!(!parse_lrc(parsed.synced_lyrics.as_deref().unwrap()).is_empty());
    }
}
