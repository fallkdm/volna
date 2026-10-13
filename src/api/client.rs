use chrono::Utc;
use md5::{Digest, Md5};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};
use std::fmt;

use super::models::{
    DownloadInfo, RotorFeedback, RotorFeedbackType, RotorStationTracks, YandexApiResponse,
};

const BASE_API_URL: &str = "https://api.music.yandex.net";
const USER_AGENT_VALUE: &str = "YandexMusic/2024.1 (Linux; Volna Desktop)";
const MD5_SALT: &str = "XGRlBW9FXlekgb0rTaf0yN";

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
        let url = format!("{}/rotor/station/{}/tracks", self.base_url, station_id);
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

        // Выбираем аудио лучшего качества (предпочтительно mp3 с наибольшим битрейтом)
        let best_info = download_infos
            .iter()
            .filter(|i| i.codec == "mp3")
            .max_by_key(|i| i.bitrate_in_kbps)
            .or_else(|| download_infos.first())
            .ok_or_else(|| ApiError::MissingData("Не найден подходящий аудиопоток".into()))?;

        // Запрашиваем метаданные потока (XML)
        let xml_resp = self
            .client
            .get(&best_info.download_info_url)
            .headers(self.headers())
            .send()
            .await?
            .text()
            .await?;

        // Извлекаем параметры из XML
        let host = extract_xml_tag(&xml_resp, "host")
            .ok_or_else(|| ApiError::XmlParse("Тег <host> не найден".into()))?;
        let path = extract_xml_tag(&xml_resp, "path")
            .ok_or_else(|| ApiError::XmlParse("Тег <path> не найден".into()))?;
        let ts = extract_xml_tag(&xml_resp, "ts")
            .ok_or_else(|| ApiError::XmlParse("Тег <ts> не найден".into()))?;
        let s = extract_xml_tag(&xml_resp, "s")
            .ok_or_else(|| ApiError::XmlParse("Тег <s> не найден".into()))?;

        // Вычисляем MD5 хэш: md5(SALT + path[1..] + s)
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

        // Прямая ссылка для стриминга
        let direct_url = format!("https://{}/get-mp3/{}/{}{}", host, sign_hex, ts, path);
        Ok(direct_url)
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
}

