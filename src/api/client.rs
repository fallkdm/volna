use chrono::Utc;
use md5::{Digest, Md5};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};
use std::fmt;
use std::sync::Arc;

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
/// Устройство аккаунта из `quasar.yandex.net/glagol/device_list`
pub struct GlagolDevice {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub cert_pem: String,
}

/// Сведения о плейлисте для страницы своего плейлиста
pub struct PlaylistMeta {
    pub private: bool,
    pub description: String,
    pub owner: String,
    pub modified: String,
}

#[derive(Clone)]
pub struct YandexMusicClient {
    client: reqwest::Client,
    token: Option<String>,
    base_url: String,
    cached_uid: Arc<std::sync::Mutex<Option<String>>>,
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
            cached_uid: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Устанавливает токен авторизации
    pub fn set_token(&mut self, token: Option<String>) {
        self.token = token;
        if let Ok(mut c) = self.cached_uid.lock() {
            *c = None;
        }
    }

    /// Устанавливает закэшированный UID пользователя
    pub fn set_cached_uid(&self, uid: String) {
        if let Ok(mut c) = self.cached_uid.lock() {
            *c = Some(uid);
        }
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
        // фильтры Ротор принимает отдельным запросом настроек, а не параметрами /tracks
        if station_id == "user:onyourwave" {
            let body = serde_json::json!({
                "moodEnergy": mood.unwrap_or("all"),
                "diversity": character.unwrap_or("default"),
                "language": language.unwrap_or("any"),
            });
            let settings_url = format!("{}/rotor/station/{}/settings2", self.base_url, station_id);
            let r = self.client.post(&settings_url).headers(self.headers()).json(&body).send().await?;
            if !r.status().is_success() {
                eprintln!("rotor settings2: HTTP {}", r.status());
            }
        }
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

    /// Получает прямую ссылку на MP3-поток трека через download-info и подпись качества вида «MP3 192»
    pub async fn get_track_direct_url(&self, track_id: &str) -> Result<(String, String), ApiError> {
        // Хранилище Яндекса отвечает рывками: то 100 мс, то 2–6 с. Если первая попытка не уложилась в 900 мс,
        // параллельно стартует вторая (новая одноразовая ссылка), берётся та, что завершилась успешно раньше.
        let first = Box::pin(self.get_track_direct_url_once(track_id));
        let second = Box::pin(async {
            tokio::time::sleep(std::time::Duration::from_millis(900)).await;
            self.get_track_direct_url_once(track_id).await
        });
        match futures_util::future::select_ok([first as std::pin::Pin<Box<dyn std::future::Future<Output = _> + Send + '_>>, second]).await {
            Ok((url, _)) => Ok(url),
            // редкий сбой ответа лечится повтором с новой ссылкой
            Err(ApiError::XmlParse(_)) => {
                tokio::time::sleep(std::time::Duration::from_millis(700)).await;
                self.get_track_direct_url_once(track_id).await
            }
            Err(e) => Err(e),
        }
    }

    /// Ссылка на трек и «прогрев»: первый кусок файла запрашивается заранее, чтобы холодное хранилище Яндекса
    /// (у части треков первый байт приходит через 3–4 с) уже было готово к моменту переключения
    pub async fn prefetch_track(&self, track_id: &str) -> Option<(String, String)> {
        let (url, label) = self.get_track_direct_url(track_id).await.ok()?;
        let warm = self.client.get(&url).header(reqwest::header::RANGE, "bytes=0-65535").send().await;
        if let Ok(resp) = warm {
            let _ = resp.bytes().await;
        }
        Some((url, label))
    }

    async fn get_track_direct_url_once(&self, track_id: &str) -> Result<(String, String), ApiError> {
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

        let host = extract_xml_tag(&xml_resp, "host").ok_or_else(|| {
            // начало ответа помогает понять причину (лимит, капча, просроченная ссылка); без токенов, ответ публичный
            let head: String = xml_resp.chars().filter(|c| !c.is_control()).take(80).collect();
            ApiError::XmlParse(format!("в ответе Яндекса нет адреса потока («{}»)", head))
        })?;
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

    /// POST формой (`application/x-www-form-urlencoded`) с разбором `result` как JSON
    async fn post_form(&self, path: &str, form: &[(&str, String)]) -> Result<serde_json::Value, ApiError> {
        let body = form
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding_encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        let mut headers = self.headers();
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
        let resp = self
            .client
            .post(format!("{}{}", self.base_url, path))
            .headers(headers)
            .body(body)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ApiError::InvalidResponse(format!("HTTP {}: {}", status, body)));
        }
        let v: serde_json::Value = resp.json().await.unwrap_or_default();
        Ok(v.get("result").cloned().unwrap_or_default())
    }

    /// Идентификаторы треков из «Мне нравится»
    pub async fn get_liked_track_ids(&self, uid: &str) -> Result<Vec<String>, ApiError> {
        let v: serde_json::Value = self.get_result(&format!("/users/{}/likes/tracks", uid)).await?;
        Ok(v["library"]["tracks"]
            .as_array()
            .map(|a| a.iter().filter_map(|t| t["id"].as_str().map(String::from).or_else(|| t["id"].as_u64().map(|n| n.to_string()))).collect())
            .unwrap_or_default())
    }

    /// Поставить или снять «Мне нравится»
    pub async fn set_track_like(&self, uid: &str, track_id: &str, like: bool) -> Result<(), ApiError> {
        let action = if like { "add-multiple" } else { "remove" };
        self.post_form(&format!("/users/{}/likes/tracks/{}", uid, action), &[("track-ids", track_id.to_string())]).await?;
        Ok(())
    }

    /// «Не нравится» на трек
    pub async fn dislike_track(&self, uid: &str, track_id: &str) -> Result<(), ApiError> {
        self.post_form(&format!("/users/{}/dislikes/tracks/add-multiple", uid), &[("track-ids", track_id.to_string())]).await?;
        Ok(())
    }

    /// Создаёт приватный плейлист и возвращает его `kind`
    pub async fn create_playlist(&self, uid: &str, title: &str) -> Result<String, ApiError> {
        let r = self
            .post_form(
                &format!("/users/{}/playlists/create", uid),
                &[("title", title.to_string()), ("visibility", "private".to_string())],
            )
            .await?;
        match &r["kind"] {
            serde_json::Value::String(s) => Ok(s.clone()),
            serde_json::Value::Number(n) => Ok(n.to_string()),
            _ => Err(ApiError::InvalidResponse("в ответе нет номера нового плейлиста".into())),
        }
    }

    /// Приватность, описание, владелец и дата изменения плейлиста
    pub async fn get_playlist_meta(&self, uid: &str, kind: &str) -> Result<PlaylistMeta, ApiError> {
        let v: serde_json::Value = self.get_result(&format!("/users/{}/playlists/{}", uid, kind)).await?;
        let text = |k: &str| v[k].as_str().unwrap_or("").to_string();
        Ok(PlaylistMeta {
            private: v["visibility"].as_str() == Some("private"),
            description: text("description"),
            owner: v["owner"]["name"].as_str().or(v["owner"]["login"].as_str()).unwrap_or("").to_string(),
            modified: text("modified"),
        })
    }

    /// Клип трека; если у этой версии клипа нет, берётся клип той же песни того же исполнителя
    /// (у «Extended Mix» клипа может не быть, а у «Kazantip 2005 Mix» есть)
    pub async fn get_clip_url_for_track(&self, track: &Track) -> Option<String> {
        if let Some(url) = self.get_track_clip_url(&track.id).await {
            return Some(url);
        }
        let same_title = |t: &Track| t.title.trim().to_lowercase() == track.title.trim().to_lowercase();
        // сначала версии той же песни на том же альбоме: у «Extended Mix» клипа нет, а у «Kazantip 2005 Mix» рядом есть
        let mut candidates: Vec<Track> = Vec::new();
        if let Some(album) = track.albums.first() {
            candidates = self.get_album_tracks(&album.id).await.unwrap_or_default();
        }
        // затем поиск: тот же заголовок и общий исполнитель
        let artist_ids: Vec<&str> = track.artists.iter().map(|a| a.id.as_str()).collect();
        let query = format!("{} {}", track.artists_string(), track.title);
        if let Some(found) = self.search(&query).await.ok().and_then(|r| r.tracks) {
            candidates.extend(found.results.into_iter().filter(|t| t.artists.iter().any(|a| artist_ids.contains(&a.id.as_str()))));
        }
        for t in candidates.into_iter().filter(|t| t.id != track.id && same_title(t)).take(5) {
            if let Some(url) = self.get_track_clip_url(&t.id).await {
                return Some(url);
            }
        }
        None
    }

    /// Устройства аккаунта (колонки, приложения) с сертификатами для локального соединения
    pub async fn glagol_devices(&self) -> Result<Vec<GlagolDevice>, ApiError> {
        let mut headers = self.headers();
        if let Some(t) = &self.token {
            if let Ok(v) = HeaderValue::from_str(&format!("oauth {}", t.trim())) {
                headers.insert(AUTHORIZATION, v);
            }
        }
        let resp = self.client.get("https://quasar.yandex.net/glagol/device_list").headers(headers).send().await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(ApiError::InvalidResponse(format!("HTTP {}", status)));
        }
        let v: serde_json::Value = resp.json().await?;
        Ok(v["devices"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|d| GlagolDevice {
                        id: d["id"].as_str().unwrap_or("").to_string(),
                        name: d["name"].as_str().unwrap_or("").to_string(),
                        platform: d["platform"].as_str().unwrap_or("").to_string(),
                        cert_pem: d["glagol"]["security"]["server_certificate"].as_str().unwrap_or("").to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Токен разговора со станцией
    pub async fn glagol_token(&self, device_id: &str, platform: &str) -> Result<String, ApiError> {
        let mut headers = self.headers();
        if let Some(t) = &self.token {
            if let Ok(v) = HeaderValue::from_str(&format!("oauth {}", t.trim())) {
                headers.insert(AUTHORIZATION, v);
            }
        }
        let resp = self
            .client
            .get(format!("https://quasar.yandex.net/glagol/token?device_id={}&platform={}", urlencoding_encode(device_id), urlencoding_encode(platform)))
            .headers(headers)
            .send()
            .await?;
        let status = resp.status();
        let v: serde_json::Value = resp.json().await.unwrap_or_default();
        v["token"].as_str().map(String::from).ok_or_else(|| ApiError::InvalidResponse(format!("HTTP {}: токен станции не выдан", status)))
    }

    /// Раздел «О треке»: пары (заголовок, значение) — исполнитель, авторы, лейбл, источник
    pub async fn get_track_credits(&self, track_id: &str) -> Result<Vec<(String, String)>, ApiError> {
        let v: serde_json::Value = self.get_result(&format!("/tracks/{}/credits", track_id)).await?;
        Ok(v["credits"]
            .as_array()
            .map(|a| a.iter().filter_map(|c| Some((c["title"].as_str()?.to_string(), c["value"].as_str()?.to_string()))).collect())
            .unwrap_or_default())
    }

    /// Загружает обложку плейлиста (multipart, поле `image`); возвращает адрес новой обложки
    pub async fn upload_playlist_cover(&self, uid: &str, kind: &str, file_name: &str, mime: &str, data: Vec<u8>) -> Result<Option<String>, ApiError> {
        let boundary = "----volna7e1f0c2a";
        let mut body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"image\"; filename=\"{file_name}\"\r\nContent-Type: {mime}\r\n\r\n"
        )
        .into_bytes();
        body.extend_from_slice(&data);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let mut headers = self.headers();
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_str(&format!("multipart/form-data; boundary={boundary}")).map_err(|e| ApiError::InvalidResponse(e.to_string()))?,
        );
        let resp = self
            .client
            .post(format!("{}/users/{}/playlists/{}/cover/upload", self.base_url, uid, kind))
            .headers(headers)
            .body(body)
            .send()
            .await?;
        let status = resp.status();
        let v: serde_json::Value = resp.json().await.unwrap_or_default();
        if !status.is_success() || v.get("error").is_some() {
            return Err(ApiError::InvalidResponse(format!("HTTP {}: {}", status, v["error"]["name"].as_str().unwrap_or(""))));
        }
        Ok(v["result"]["ogImage"].as_str().or(v["result"]["cover"]["uri"].as_str()).map(String::from))
    }

    /// Переименовывает плейлист
    pub async fn rename_playlist(&self, uid: &str, kind: &str, title: &str) -> Result<(), ApiError> {
        self.post_form(&format!("/users/{}/playlists/{}/name", uid, kind), &[("value", title.to_string())]).await?;
        Ok(())
    }

    /// Скачивает mp3 трека целиком
    pub async fn download_track_bytes(&self, track_id: &str) -> Result<Vec<u8>, ApiError> {
        let (url, _) = self.get_track_direct_url(track_id).await?;
        Ok(self.client.get(&url).send().await?.error_for_status()?.bytes().await?.to_vec())
    }

    /// Сохраняет трек для прослушивания без интернета (внутри приложения)
    pub async fn save_track_offline(&self, track: &Track) -> Result<(), ApiError> {
        let bytes = self.download_track_bytes(&track.id).await?;
        crate::cache::save_offline(track, &bytes).map_err(|e| ApiError::InvalidResponse(e.to_string()))
    }

    /// Скачивает трек в `~/Music/Волна` и возвращает путь к файлу
    pub async fn download_track(&self, track: &Track) -> Result<std::path::PathBuf, ApiError> {
        let bytes = self.download_track_bytes(&track.id).await?;
        let home = std::env::var("HOME").map_err(|e| ApiError::InvalidResponse(e.to_string()))?;
        let dir = std::path::Path::new(&home).join("Music").join("Волна");
        std::fs::create_dir_all(&dir).map_err(|e| ApiError::InvalidResponse(e.to_string()))?;
        let name: String = format!("{} - {}", track.artists_string(), track.title)
            .chars()
            .map(|c| if "/\\:*?\"<>|".contains(c) { '_' } else { c })
            .collect();
        let path = dir.join(format!("{}.mp3", name));
        std::fs::write(&path, &bytes).map_err(|e| ApiError::InvalidResponse(e.to_string()))?;
        Ok(path)
    }

    /// Ключи `uid:kind` понравившихся плейлистов
    pub async fn get_liked_playlist_keys(&self, uid: &str) -> Result<Vec<String>, ApiError> {
        let v: serde_json::Value = self.get_result(&format!("/users/{}/likes/playlists", uid)).await?;
        let id = |x: &serde_json::Value| x.as_str().map(String::from).or_else(|| x.as_u64().map(|n| n.to_string())).unwrap_or_default();
        Ok(v.as_array()
            .map(|a| a.iter().map(|i| format!("{}:{}", id(&i["playlist"]["uid"]), id(&i["playlist"]["kind"]))).collect())
            .unwrap_or_default())
    }

    /// Поставить или снять «Нравится» плейлисту (`key` вида `uid:kind`)
    pub async fn set_playlist_like(&self, uid: &str, key: &str, like: bool) -> Result<(), ApiError> {
        let action = if like { "add-multiple" } else { "remove" };
        self.post_form(&format!("/users/{}/likes/playlists/{}", uid, action), &[("playlist-ids", key.to_string())]).await?;
        Ok(())
    }

    pub async fn set_playlist_description(&self, uid: &str, kind: &str, text: &str) -> Result<(), ApiError> {
        self.post_form(&format!("/users/{}/playlists/{}/description", uid, kind), &[("value", text.to_string())]).await?;
        Ok(())
    }

    pub async fn set_playlist_private(&self, uid: &str, kind: &str, private: bool) -> Result<(), ApiError> {
        let value = if private { "private" } else { "public" };
        self.post_form(&format!("/users/{}/playlists/{}/visibility", uid, kind), &[("value", value.to_string())]).await?;
        Ok(())
    }

    pub async fn delete_playlist(&self, uid: &str, kind: &str) -> Result<(), ApiError> {
        self.post_form(&format!("/users/{}/playlists/{}/delete", uid, kind), &[]).await?;
        Ok(())
    }

    /// Добавляет трек в начало плейлиста
    pub async fn add_track_to_playlist(&self, uid: &str, kind: &str, track: &Track) -> Result<(), ApiError> {
        self.add_tracks_to_playlist(uid, kind, std::slice::from_ref(track)).await
    }

    /// Добавляет треки в начало плейлиста одним изменением
    pub async fn add_tracks_to_playlist(&self, uid: &str, kind: &str, tracks: &[Track]) -> Result<(), ApiError> {
        let pl: serde_json::Value = self.get_result(&format!("/users/{}/playlists/{}", uid, kind)).await?;
        let revision = pl["revision"].as_u64().unwrap_or(1);
        let items: Vec<_> = tracks
            .iter()
            .map(|t| serde_json::json!({"id": t.id, "albumId": t.albums.first().map(|a| a.id.clone()).unwrap_or_default()}))
            .collect();
        let diff = serde_json::json!([{"op": "insert", "at": 0, "tracks": items}]);
        self.post_form(
            &format!("/users/{}/playlists/{}/change", uid, kind),
            &[("diff", diff.to_string()), ("revision", revision.to_string())],
        )
        .await?;
        Ok(())
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
        if let Ok(c) = self.cached_uid.lock() {
            if let Some(uid) = c.as_ref() {
                return Ok(uid.clone());
            }
        }
        let uid = self.get_account_status().await?.account.map(|a| a.uid).unwrap_or_default();
        if uid.is_empty() {
            return Err(ApiError::InvalidResponse("не удалось определить UID пользователя".into()));
        }
        if let Ok(mut c) = self.cached_uid.lock() {
            *c = Some(uid.clone());
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

        let mut plain_fallback: Option<String> = None;
        let mut resp = self.lrclib_get(&make_url(duration_secs)).await;
        // Если запрос с точной длительностью вернул 404, повторяем поиск без ограничения длительности
        if matches!(resp, LrclibFetch::NotFound) && duration_secs.is_some() {
            resp = self.lrclib_get(&make_url(None)).await;
        }
        let resp = match resp {
            LrclibFetch::Found(r) => Some(r),
            // LRCLIB не отвечает (429/5xx/сеть): поиск упрётся туда же и съест таймаут загрузки текста,
            // а обычный текст Яндекса тогда пропадёт
            LrclibFetch::Failed => return Ok(None),
            LrclibFetch::NotFound => None,
        };

        if let Some(response) = resp {
            if let Ok(data) = response.json::<LrclibResponse>().await {
                if let Some(synced) = data.synced_lyrics {
                    let trimmed = synced.trim();
                    if usable_synced(trimmed) {
                        return Ok(Some(trimmed.to_string()));
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

        // Точное совпадение дало только обычный текст: ищем запись с таймингами (в базе она бывает
        // отдельно, иногда с перепутанными названием и исполнителем)
        if let Some(synced) = self.lrclib_search_synced(clean_artist, clean_title, duration_secs).await {
            return Ok(Some(synced));
        }
        Ok(plain_fallback)
    }

    /// Поиск в LRCLIB записи с синхронизированным текстом той же длительности (±3 с)
    async fn lrclib_search_synced(&self, artist: &str, title: &str, duration_secs: Option<u64>) -> Option<String> {
        let url = format!("https://lrclib.net/api/search?q={}", urlencoding_encode(&format!("{title} {artist}")));
        let LrclibFetch::Found(resp) = self.lrclib_get(&url).await else { return None };
        let items: Vec<serde_json::Value> = resp.json().await.ok()?;
        let (title_n, artist_n) = (normalize_name(title), normalize_name(artist));
        // исполнители у нас через запятую, в LRCLIB бывает один из них: хватает совпадения по первому
        let first_artist = normalize_name(artist.split(',').next().unwrap_or(artist));
        // вхождение подстрокой только для имён от 4 символов: иначе «m» совпала бы с кем угодно;
        // имя из одних знаков («!!!») нормализуется в пустое и сравнивается как есть
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
            (t == title_n && same_artist(&a, &raw)) || (!artist_n.is_empty() && t == artist_n && a == title_n)
        };
        items
            .iter()
            .filter(|v| names_match(v))
            .filter(|v| match (duration_secs, v["duration"].as_f64()) {
                (Some(want), Some(got)) => (got - want as f64).abs() <= 3.0,
                _ => true,
            })
            .find_map(|v| v["syncedLyrics"].as_str().map(str::trim).filter(|t| usable_synced(t)).map(String::from))
    }

    /// Ссылка на превью-клип трека из `/supplement` (поле `clips[0].previewUrl`); у многих треков клипа нет
    pub async fn get_track_clip_url(&self, track_id: &str) -> Option<String> {
        let url = format!("{}/tracks/{}/supplement", self.base_url, track_id);
        let resp = self.client.get(&url).headers(self.headers()).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let body: serde_json::Value = resp.json().await.ok()?;
        let link = body["result"]["clips"][0]["previewUrl"].as_str()?;
        // только https и только хост Яндекса: ссылка приходит из ответа сервера
        (link.starts_with("https://") && link.contains(".yandex.net/")).then(|| link.to_string())
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
            if usable_synced(&cached) {
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
                                if usable_synced(trimmed) {
                                    save_synced(trimmed);
                                    return Ok(Some(trimmed.to_string()));
                                } else {
                                    // огрызок с таймкодами показываем как обычный текст, иначе экран снова
                                    // станет «караоке» из одной строки
                                    yandex_plain = Some(strip_lrc_tags(trimmed));
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
            if usable_synced(&lrclib_text) {
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

/// Синхронизированный текст годится, только если в нём хотя бы несколько строк: в базах встречаются
/// огрызки из одной строки (например, у Kavinsky — Nightcall), и тогда весь экран показывал одну фразу
fn usable_synced(text: &str) -> bool {
    parse_lrc(text).len() >= 4
}

/// Убирает таймкоды `[mm:ss.xx]` в начале строк, оставляя сам текст
fn strip_lrc_tags(text: &str) -> String {
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

/// Название для сравнения: без регистра, без скобок («(feat. …)», «[Remix]»), хвоста « - Remastered» и пунктуации
fn normalize_name(s: &str) -> String {
    let mut depth = 0u32;
    let no_brackets: String = s
        .chars()
        .filter(|c| match c {
            '(' | '[' => { depth += 1; false }
            ')' | ']' => { depth = depth.saturating_sub(1); false }
            _ => depth == 0,
        })
        .collect();
    let head = no_brackets.split(" - ").next().unwrap_or("");
    head.to_lowercase().chars().filter(|c| c.is_alphanumeric() || c.is_whitespace()).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
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
    /// Доля треков с таймингами среди выдачи по нескольким запросам: `cargo test lyrics_coverage_live -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn lyrics_coverage_live() {
        let client = YandexMusicClient::new(crate::api::auth::load_token());
        let (mut total, mut synced) = (0, 0);
        for q in ["Три дня дождя", "Radiohead", "Katy Perry", "Скриптонит", "Imagine Dragons", "Монеточка"] {
            let tracks = client.search(q).await.unwrap().tracks.map(|t| t.results).unwrap_or_default();
            for t in tracks.iter().take(8) {
                let dur = t.duration_ms.map(|d| d / 1000);
                let got = client.get_lrclib_lyrics(&t.artists_string(), &t.title, dur).await.ok().flatten();
                let ok = got.as_deref().is_some_and(|x| !parse_lrc(x).is_empty());
                total += 1;
                synced += ok as u32;
                if !ok { println!("БЕЗ ТАЙМИНГОВ: {} — {} ({})", t.artists_string(), t.title, if got.is_some() { "есть обычный текст" } else { "текста нет" }); }
            }
        }
        println!("LIVE coverage: {synced} из {total}");
    }

    /// Запись с таймингами лежит в LRCLIB с перепутанными названием и исполнителем
    #[tokio::test]
    #[ignore]
    async fn lyrics_swapped_live() {
        let client = YandexMusicClient::new(crate::api::auth::load_token());
        let text = client.get_lrclib_lyrics("Три дня дождя", "Сотни гортензий", Some(169)).await.unwrap().unwrap();
        println!("LIVE swapped: synced_lines={}", parse_lrc(&text).len());
        assert!(!parse_lrc(&text).is_empty());
    }

    /// `cargo test colors_live -- --ignored --nocapture`: какие цвета Яндекс присылает для трека
    #[tokio::test]
    #[ignore]
    async fn colors_live() {
        let client = YandexMusicClient::new(crate::api::auth::load_token());
        for q in ["Сплин Линия жизни", "Серега Пират Я поднимаю свою голову вверх"] {
            let t = client.search(q).await.unwrap().tracks.map(|t| t.results).unwrap_or_default();
            if let Some(t) = t.first() {
                println!("LIVE {} — {}: {:?}", t.artists_string(), t.title, t.derived_colors);
            }
        }
    }

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
    fn strip_lrc_tags_keeps_text() {
        assert_eq!(strip_lrc_tags("[00:12.42] There is a nightcall\nplain"), "There is a nightcall\nplain");
        assert!(parse_lrc(&strip_lrc_tags("[00:12.42] There is a nightcall")).is_empty());
    }

    #[test]
    fn one_line_lrc_is_not_usable() {
        assert!(!usable_synced("[00:12.42] There is a nightcall"));
        assert!(usable_synced("[00:01.00] a\n[00:02.00] b\n[00:03.00] c\n[00:04.00] d"));
    }

    #[test]
    fn normalize_name_strips_noise() {
        assert_eq!(normalize_name("Creep (feat. X) - Remastered"), "creep");
        assert_eq!(normalize_name("  Don't  Stop! [Live]"), "dont stop");
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

#[cfg(test)]
mod stream_speed_tests {
    use super::*;

    /// Замер скорости потока у живого Яндекса: `cargo test stream_speed_live -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn stream_speed_live() {
        let client = YandexMusicClient::new(crate::api::auth::load_token());
        for id in ["3264", "818304", "475562"] {
            let t = std::time::Instant::now();
            let (url, label) = client.get_track_direct_url(id).await.expect("ссылка");
            let link_ms = t.elapsed().as_millis();
            let t = std::time::Instant::now();
            let resp = reqwest::get(&url).await.expect("поток");
            let ttfb = t.elapsed().as_millis();
            let total_len = resp.content_length().unwrap_or(0);
            let mut stream = resp.bytes_stream();
            let (mut got, mut marks) = (0usize, Vec::new());
            use futures_util::StreamExt;
            while let Some(chunk) = stream.next().await {
                got += chunk.expect("кусок").len();
                for mb in [1usize, 2, 4] {
                    if got >= mb * 1_000_000 && !marks.iter().any(|(m, _)| *m == mb) {
                        marks.push((mb, t.elapsed().as_millis()));
                    }
                }
            }
            eprintln!(
                "трек {id} ({label}): ссылка {link_ms} мс, первый байт {ttfb} мс, файл {} МБ целиком за {} мс, отметки МБ→мс {:?}",
                total_len / 1_000_000,
                t.elapsed().as_millis(),
                marks
            );
        }
    }
}
