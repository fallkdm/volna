use chrono::Utc;
use md5::{Digest, Md5};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};
use std::fmt;

use super::models::{
    parse_lrc, AccountStatus, Album, AlbumWithTracks, ArtistBrief, DownloadInfo, LikedAlbumItem, LikedTracksResult, Playlist,
    PlaylistWithTracks, SimilarTracks, LrclibResponse, RotorFeedback,
    RotorFeedbackType, RotorStationTracks, SearchResult, Track, TrackSupplement,
    YandexApiResponse,
};

const BASE_API_URL: &str = "https://api.music.yandex.net";
const USER_AGENT_VALUE: &str = "Volna/0.1 (Linux; unofficial client)";
/// Соль подписи ссылок `get-mp3`: публичное значение `SIGN_SALT` из библиотеки MarshalX/yandex-music-api
/// (yandex_music/download_info.py). Сервер на 2026-10-14 подпись не проверяет (принимает и пустую соль).
const MD5_SALT: &str = "XGRlBW9FXlekgbPrRHuSiA";
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

/// Выбирает поток по настройке качества среди MP3 (или, если их нет, среди всех): `best` — максимальный
/// битрейт, `economy` — минимальный, `balanced` — средний по возрастанию.
fn pick_download_info<'a>(infos: &'a [DownloadInfo], quality: &str) -> Option<&'a DownloadInfo> {
    let mut pool: Vec<&DownloadInfo> = infos.iter().filter(|i| i.codec == "mp3").collect();
    if pool.is_empty() {
        pool = infos.iter().collect();
    }
    pool.sort_by_key(|i| i.bitrate_in_kbps);
    match quality {
        "economy" => pool.first().copied(),
        "balanced" => pool.get(pool.len() / 2).copied(),
        _ => pool.last().copied(),
    }
}

/// Результат запроса к LRCLIB
enum LrclibFetch {
    Found(reqwest::Response),
    NotFound,
    Failed,
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

    /// Получает треки станции Ротора (по умолчанию «Волна» - "user:onyourwave")
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

    /// Получает прямую ссылку на MP3-поток трека через download-info и подпись качества вида «MP3 192»
    pub async fn get_track_direct_url(&self, track_id: &str) -> Result<(String, String), ApiError> {
        let download_infos = self.get_track_download_info(track_id).await?;
        if download_infos.is_empty() {
            return Err(ApiError::MissingData(format!(
                "Нет доступных ссылок для трека {}",
                track_id
            )));
        }

        let best_info = pick_download_info(&download_infos, &super::auth::load_quality())
            .ok_or_else(|| ApiError::MissingData("Не найден подходящий аудиопоток".into()))?;
        let label = format!("{} {}", best_info.codec.to_uppercase(), best_info.bitrate_in_kbps);

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
        Ok((direct_url, label))
    }

    /// GET с разбором `result`; общий для новых запросов каталога
    async fn get_result<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, ApiError> {
        let resp = self
            .client
            .get(format!("{}{}", self.base_url, path))
            .headers(self.headers())
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ApiError::InvalidResponse(format!("HTTP {}: {}", status, body)));
        }

        let bytes = resp.bytes().await?;
        // Через Value: поиск Яндекса отдаёт ключ `albums` дважды, а прямой разбор в структуру на этом падает.
        // Ошибка serde_json точнее общего «error decoding response body».
        let api_response: YandexApiResponse<T> = serde_json::from_slice::<serde_json::Value>(&bytes)
            .and_then(serde_json::from_value)
            .map_err(|e| ApiError::InvalidResponse(format!("{} ({})", e, path)))?;
        Ok(api_response.result)
    }

    /// Поиск по запросу: треки, альбомы и плейлисты
    pub async fn search(&self, query: &str) -> Result<SearchResult, ApiError> {
        self.get_result(&format!("/search?text={}&type=all&page=0", urlencoding_encode(query))).await
    }

    /// Треки альбома (все тома подряд)
    pub async fn get_album_tracks(&self, album_id: &str) -> Result<Vec<Track>, ApiError> {
        let album: AlbumWithTracks = self.get_result(&format!("/albums/{}/with-tracks", album_id)).await?;
        Ok(album.volumes.into_iter().flatten().collect())
    }

    /// Треки плейлиста
    pub async fn get_playlist_tracks(&self, uid: &str, kind: &str) -> Result<Vec<Track>, ApiError> {
        let playlist: PlaylistWithTracks = self.get_result(&format!("/users/{}/playlists/{}", uid, kind)).await?;
        Ok(playlist.tracks.into_iter().filter_map(|t| t.track).collect())
    }

    /// Плейлисты пользователя
    pub async fn get_user_playlists(&self, uid: &str) -> Result<Vec<Playlist>, ApiError> {
        self.get_result(&format!("/users/{}/playlists/list", uid)).await
    }

    /// Любимые альбомы пользователя
    pub async fn get_liked_albums(&self, uid: &str) -> Result<Vec<Album>, ApiError> {
        let items: Vec<LikedAlbumItem> = self.get_result(&format!("/users/{}/likes/albums?rich=true", uid)).await?;
        Ok(items.into_iter().map(|i| i.album).collect())
    }

    /// Страница артиста: популярные треки и альбомы
    pub async fn get_artist_brief(&self, artist_id: &str) -> Result<ArtistBrief, ApiError> {
        self.get_result(&format!("/artists/{}/brief-info", artist_id)).await
    }

    /// Похожие на трек
    pub async fn get_similar_tracks(&self, track_id: &str) -> Result<Vec<Track>, ApiError> {
        let similar: SimilarTracks = self.get_result(&format!("/tracks/{}/similar", track_id)).await?;
        Ok(similar.similar_tracks)
    }

    /// UID текущего пользователя
    pub async fn get_uid(&self) -> Result<String, ApiError> {
        let uid = self.get_account_status().await?.account.map(|a| a.uid).unwrap_or_default();
        if uid.is_empty() {
            return Err(ApiError::InvalidResponse("не удалось определить UID пользователя".into()));
        }
        Ok(uid)
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
        let ids: Vec<String> = track_items.into_iter().take(300).map(|t| t.id).collect();
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

    /// GET к LRCLIB с повтором: сервис нередко отвечает 429 (лимит на общий IP) или 5xx, а 404 означает «текста нет»
    async fn lrclib_get(&self, url: &str) -> LrclibFetch {
        for attempt in 0..3u64 {
            let sent = self
                .client
                .get(url)
                .header("User-Agent", "VolnaMusicPlayer/1.0 (https://github.com/fallkdm/volna)")
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

        let mut resp = self.lrclib_get(&make_url(duration_secs)).await;
        // Если запрос с точной длительностью вернул 404, повторяем поиск без ограничения длительности
        if matches!(resp, LrclibFetch::NotFound) && duration_secs.is_some() {
            resp = self.lrclib_get(&make_url(None)).await;
        }
        let resp = match resp {
            LrclibFetch::Found(r) => Some(r),
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
    /// - Если /supplement вернул синхронизированный LRC с таймкодами — используем его.
    /// - Если вернул статический текст (без таймкодов) или ошибку — запрашиваем LRCLIB.
    /// - Если LRCLIB нашел синхронизированный LRC — отдаем его (полноценное караоке!).
    /// - Если LRCLIB ничего не нашел — отдаем статический текст Яндекса.
    pub async fn get_track_lyrics(
        &self,
        track_id: &str,
        artist: &str,
        title: &str,
        duration_secs: Option<u64>,
    ) -> Result<Option<String>, ApiError> {
        // Синхронизированный текст из кэша: не ходим в сеть повторно и не упираемся в лимиты LRCLIB
        let cache_path = crate::cache::lyrics_cache_path(track_id);
        if let Ok(cached) = std::fs::read_to_string(&cache_path) {
            if !parse_lrc(&cached).is_empty() {
                return Ok(Some(cached));
            }
        }
        let save_synced = |text: &str| {
            let _ = std::fs::write(&cache_path, text);
        };

        let mut yandex_plain: Option<String> = None;

        // 1. Текст из /tracks/{track_id}/supplement (без подписи)
        if yandex_plain.is_none() {
            let supp_url = format!("{}/tracks/{}/supplement", self.base_url, track_id);
            if let Ok(resp) = self.client.get(&supp_url).headers(self.headers()).send().await {
                if resp.status().is_success() {
                    if let Ok(supp) = resp.json::<YandexApiResponse<TrackSupplement>>().await {
                        if let Some(full) = supp.result.lyrics.and_then(|l| l.full_lyrics) {
                            let trimmed = full.trim();
                            if !trimmed.is_empty() {
                                if !parse_lrc(trimmed).is_empty() {
                                    save_synced(trimmed);
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
                save_synced(&lrclib_text);
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
pub(crate) fn urlencoding_encode(s: &str) -> String {
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
    /// `cargo test explicit_live -- --ignored --nocapture`: Яндекс должен помечать нецензурные треки
    #[tokio::test]
    #[ignore]
    async fn explicit_live() {
        let client = YandexMusicClient::new(crate::api::auth::load_token());
        let res = client.search("Eminem Lose Yourself").await.unwrap();
        let tracks = res.tracks.map(|t| t.results).unwrap_or_default();
        let marked = tracks.iter().filter(|t| t.is_explicit()).count();
        println!("LIVE explicit: {marked} из {}", tracks.len());
        assert!(marked > 0);
    }

    /// Ходит в сеть, токен берёт из конфига: `cargo test lyrics_live -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn lyrics_live() {
        let client = YandexMusicClient::new(crate::api::auth::load_token());
        let supp = client.get_track_lyrics("5705627", "Deftones", "Rosemary", Some(413)).await;
        match supp {
            Ok(Some(text)) => println!("LIVE lyrics: synced_lines={} first={:?}", parse_lrc(&text).len(), text.lines().next()),
            Ok(None) => println!("LIVE lyrics: none"),
            Err(e) => println!("LIVE lyrics error: {e}"),
        }
        let lr = client.get_lrclib_lyrics("Deftones", "Rosemary", Some(413)).await;
        match lr {
            Ok(Some(text)) => println!("LIVE lrclib: synced_lines={} first={:?}", parse_lrc(&text).len(), text.lines().next()),
            Ok(None) => println!("LIVE lrclib: none"),
            Err(e) => println!("LIVE lrclib error: {e}"),
        }
    }

    #[test]
    fn quality_picks_expected_stream() {
        let mk = |codec: &str, kbps| DownloadInfo { codec: codec.into(), bitrate_in_kbps: kbps, download_info_url: String::new(), direct: false };
        let infos = vec![mk("mp3", 192), mk("aac", 256), mk("mp3", 64), mk("mp3", 128)];
        assert_eq!(pick_download_info(&infos, "best").unwrap().bitrate_in_kbps, 192);
        assert_eq!(pick_download_info(&infos, "economy").unwrap().bitrate_in_kbps, 64);
        assert_eq!(pick_download_info(&infos, "balanced").unwrap().bitrate_in_kbps, 128);
        assert_eq!(pick_download_info(&[mk("mp3", 192)], "economy").unwrap().bitrate_in_kbps, 192);
    }

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
