pub mod auth;
pub mod error;
pub mod parser;

use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::api::models::{Album, ArtistBrief, Playlist, SearchResult, Track};
use crate::api::provider::{FeedbackEvent, MusicProvider, ProviderId, StationBatch};

pub use auth::*;
pub use error::*;
pub use parser::*;

// ── Сетевые константы InnerTube ─────────────────────────────────────────────
// Google отдаёт разный набор данных (и разные аудиокодеки) в зависимости от
// User-Agent, поэтому строки ниже нельзя менять без перепроверки стримов.

/// Windows Chrome 120 — базовые запросы к `youtubei/v1`.
const UA_CHROME_120: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
/// macOS Safari 26 — `sw.js_data` и VisionOS-подобный `player`.
const UA_SAFARI_26: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_7_3) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15";
/// Windows Chrome 124 — HTML-страницы библиотеки и запросы оценок.
const UA_CHROME_124: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

/// Таймаут пре-флайта основной ноды GGC.
const PROBE_TIMEOUT_PRIMARY_MS: u64 = 1200;
/// Таймаут пре-флайта резервной ноды: ей даём больше времени.
const PROBE_TIMEOUT_FALLBACK_MS: u64 = 2000;

/// Длительности треков по `videoId`: их отдаёт только `/player`, а нужны они и тексту песни,
/// и повторным открытиям трека — держим в памяти, чтобы не дёргать сеть дважды.
static TRACK_DURATIONS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, u64>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

fn track_duration_cached(track_id: &str) -> Option<u64> {
    TRACK_DURATIONS.lock().ok()?.get(track_id).copied()
}

fn store_track_duration(track_id: &str, secs: u64) {
    if let Ok(mut map) = TRACK_DURATIONS.lock() {
        map.insert(track_id.to_string(), secs);
    }
}

/// Маркеры имён заблокированных в РФ кэшей GGC (`sn-...googlevideo.com`).
/// Сравнение по подстроке — намеренно: имя ноды может содержать префикс региона.
const BLOCKED_GGC_NODE_MARKERS: [&str; 7] =
    ["vo-", "238s", "hxb", "5hne", "4g5edn", "4g5e6n", "5g0z"];

/// Клиент YouTube Music на базе Google InnerTube API
#[derive(Clone)]
pub struct YtMusicClient {
    http: reqwest::Client,
    auth: Arc<RwLock<Option<GoogleToken>>>,
    refresh_token: Arc<RwLock<Option<String>>>,
    cookie: Arc<RwLock<Option<String>>>,
    visitor_id: Arc<RwLock<Option<String>>>,
    last_seed: Arc<RwLock<Option<String>>>,
    authuser: Arc<RwLock<Option<String>>>,
    liked_total_count: Arc<RwLock<Option<u32>>>,
}

impl Default for YtMusicClient {
    fn default() -> Self {
        Self::new()
    }
}

impl YtMusicClient {
    /// Создает клиента в гостевом режиме без авторизации
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
            auth: Arc::new(RwLock::new(None)),
            refresh_token: Arc::new(RwLock::new(None)),
            cookie: Arc::new(RwLock::new(None)),
            visitor_id: Arc::new(RwLock::new(None)),
            last_seed: Arc::new(RwLock::new(None)),
            authuser: Arc::new(RwLock::new(Some("3".to_string()))),
            liked_total_count: Arc::new(RwLock::new(None)),
        }
    }

    /// Создает клиента с сессионной кукой браузера
    pub fn with_cookie(cookie: String) -> Self {
        let detected_authuser = if cookie.contains("authuser=3") {
            Some("3".to_string())
        } else if let Some(pos) = cookie.find("authuser=") {
            let num: String = cookie[pos + 9..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if !num.is_empty() {
                Some(num)
            } else {
                Some("3".to_string())
            }
        } else {
            Some("3".to_string())
        };

        Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
            auth: Arc::new(RwLock::new(None)),
            refresh_token: Arc::new(RwLock::new(None)),
            cookie: Arc::new(RwLock::new(Some(cookie))),
            visitor_id: Arc::new(RwLock::new(None)),
            last_seed: Arc::new(RwLock::new(None)),
            authuser: Arc::new(RwLock::new(detected_authuser)),
            liked_total_count: Arc::new(RwLock::new(None)),
        }
    }

    /// Создает клиента с сохраненным Google OAuth refresh_token
    pub fn with_refresh_token(refresh_token: String) -> Self {
        let client = Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
            auth: Arc::new(RwLock::new(None)),
            refresh_token: Arc::new(RwLock::new(Some(refresh_token.clone()))),
            cookie: Arc::new(RwLock::new(None)),
            visitor_id: Arc::new(RwLock::new(None)),
            last_seed: Arc::new(RwLock::new(None)),
            authuser: Arc::new(RwLock::new(Some("3".to_string()))),
            liked_total_count: Arc::new(RwLock::new(None)),
        };
        let c = client.clone();
        tokio::spawn(async move {
            let _ = c.get_valid_access_token().await;
        });
        client
    }

    /// Устанавливает или обновляет сессионную куку браузера
    pub async fn set_cookie(&self, cookie: String) {
        let mut c = self.cookie.write().await;
        *c = Some(cookie);
    }

    /// Устанавливает токен обновления и инициирует обновление access_token
    pub async fn set_refresh_token(&self, token: String) {
        {
            let mut rt = self.refresh_token.write().await;
            *rt = Some(token.clone());
        }
        if let Ok(token_data) = auth::refresh_access_token(&self.http, &token).await {
            let mut auth_guard = self.auth.write().await;
            *auth_guard = Some(token_data);
        }
    }

    /// Возвращает валидный токен доступа, автоматически обновляя его при необходимости
    pub async fn get_valid_access_token(&self) -> Option<String> {
        // Проверяем существующий активный токен
        {
            let auth_guard = self.auth.read().await;
            if let Some(token) = auth_guard.as_ref()
                && !token.is_expired()
            {
                return Some(token.access_token.clone());
            }
        }

        // Если токена нет или он просрочен — выполняем refresh
        let rt_opt = {
            let guard = self.refresh_token.read().await;
            guard.clone()
        };

        if let Some(rt) = rt_opt {
            match auth::refresh_access_token(&self.http, &rt).await {
                Ok(new_token) => {
                    let access_token = new_token.access_token.clone();
                    let mut auth_guard = self.auth.write().await;
                    *auth_guard = Some(new_token);
                    return Some(access_token);
                }
                Err(e) => {
                    eprintln!("[ytmusic] Ошибка обновления access token: {e}");
                }
            }
        }

        None
    }

    /// Проверяет, аутентифицирован ли клиент
    pub fn is_authenticated(&self) -> bool {
        let has_cookie = self.cookie.try_read().map(|r| r.is_some()).unwrap_or(false);
        let has_token = self
            .refresh_token
            .try_read()
            .map(|r| r.is_some())
            .unwrap_or(false);
        has_cookie || has_token
    }

    /// Возвращает актуальный номер Google профиля X-Goog-AuthUser (по умолчанию "3" для профиля пользователя)
    pub async fn get_authuser(&self) -> String {
        {
            let guard = self.authuser.read().await;
            if let Some(au) = guard.as_ref() {
                return au.clone();
            }
        }
        "3".to_string()
    }

    /// Устанавливает номер Google профиля authuser
    pub async fn set_authuser(&self, authuser: String) {
        let mut guard = self.authuser.write().await;
        *guard = Some(authuser);
    }

    /// Выполнение POST-запроса к InnerTube API с контекстом клиента
    async fn post_innertube(
        &self,
        endpoint: &str,
        client_name: &str,
        client_version: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, YtMusicError> {
        let url = format!("https://music.youtube.com/youtubei/v1/{}", endpoint);

        let mut req_body = serde_json::json!({
            "context": {
                "client": {
                    "clientName": client_name,
                    "clientVersion": client_version,
                    "hl": "ru",
                    "gl": "RU"
                }
            }
        });

        if let (Some(req_obj), Some(body_obj)) = (req_body.as_object_mut(), body.as_object()) {
            for (k, v) in body_obj {
                req_obj.insert(k.clone(), v.clone());
            }
        }

        let mut req = self
            .http
            .post(&url)
            .header("Content-Type", "application/json")
            .header("User-Agent", UA_CHROME_120)
            .header("X-Goog-Api-Format-Version", "2");

        let cookie_opt = {
            let guard = self.cookie.read().await;
            guard.clone()
        };

        if let Some(cookie) = cookie_opt {
            let authuser = self.get_authuser().await;
            req = req
                .header("Cookie", &cookie)
                .header("Origin", "https://music.youtube.com")
                .header("X-Origin", "https://music.youtube.com")
                .header("X-Goog-AuthUser", authuser);

            if let Some(sapisid) = auth::extract_sapisid(&cookie) {
                let auth_header = auth::build_sapisid_hash(&sapisid, "https://music.youtube.com");
                req = req.header("Authorization", auth_header);
            }
        } else if let Some(token) = self.get_valid_access_token().await {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        let resp = req.json(&req_body).send().await?.error_for_status()?;
        let json = resp.json::<serde_json::Value>().await?;
        Ok(json)
    }

    /// Получение или обновление YouTube Visitor ID
    pub async fn get_visitor_id(&self) -> String {
        {
            let guard = self.visitor_id.read().await;
            if let Some(vid) = guard.as_ref() {
                return vid.clone();
            }
        }

        let fetched_id = match self
            .http
            .get("https://www.youtube.com/sw.js_data")
            .header("User-Agent", UA_SAFARI_26)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(body) = resp.text().await {
                    extract_visitor_id(&body)
                } else {
                    None
                }
            }
            _ => None,
        };

        let vid = fetched_id.unwrap_or_else(|| DEFAULT_VISITOR_ID.to_string());
        let mut guard = self.visitor_id.write().await;
        *guard = Some(vid.clone());
        vid
    }

    /// Прямой запрос потока через InnerTube VisionOS API (~200 мс)
    pub async fn get_stream_url_visionos(&self, track_id: &str) -> Result<String, YtMusicError> {
        let json = self.visionos_player_json(track_id).await?;
        let url = parser::parse_stream_url(&json).map_err(YtMusicError::Message)?;

        println!(
            "[ytmusic] Стрим {} получен НАПРЯМУЮ через VisionOS API (itag 251)",
            track_id
        );
        Ok(url)
    }

    /// Длительность трека в секундах (`videoDetails.lengthSeconds` из ответа `/player`).
    /// Результат запоминается в памяти: одна и та же длительность нужна и префетчу, и тексту песни.
    pub async fn track_duration_secs(&self, track_id: &str) -> Option<u64> {
        if let Some(secs) = track_duration_cached(track_id) {
            return Some(secs);
        }
        let json = self.visionos_player_json(track_id).await.ok()?;
        let secs = parser::parse_video_length_secs(&json)?;
        store_track_duration(track_id, secs);
        Some(secs)
    }

    /// Ответ InnerTube VisionOS `/player`: поток и метаданные трека одним запросом
    async fn visionos_player_json(
        &self,
        track_id: &str,
    ) -> Result<serde_json::Value, YtMusicError> {
        let visitor_id = self.get_visitor_id().await;
        let body = serde_json::json!({
            "context": {
                "client": {
                    "clientName": "VISIONOS",
                    "clientVersion": "1.02",
                    "deviceMake": "Apple",
                    "deviceModel": "RealityDevice17,1",
                    "userAgent": UA_SAFARI_26,
                    "osName": "visionOS",
                    "osVersion": "26.5.23O471",
                    "hl": "en",
                    "timeZone": "UTC",
                    "utcOffsetMinutes": 0
                }
            },
            "videoId": track_id,
            "playbackContext": {
                "contentPlaybackContext": {
                    "html5Preference": "HTML5_PREF_WANTS"
                }
            },
            "contentCheckOk": true,
            "racyCheckOk": true
        });

        let resp = self
            .http
            .post("https://www.youtube.com/youtubei/v1/player")
            .header("Content-Type", "application/json")
            .header("X-YouTube-Client-Name", "101")
            .header("X-YouTube-Client-Version", "1.02")
            .header("Origin", "https://www.youtube.com")
            .header("User-Agent", UA_SAFARI_26)
            .header("X-Goog-Visitor-Id", &visitor_id)
            .json(&body)
            .send()
            .await?
            .error_for_status()?;

        let json = resp.json::<serde_json::Value>().await?;
        Ok(json)
    }

    /// Поиск студийных треков ("Songs" / "Песни") с официальным фильтром YouTube Music
    pub async fn search_songs(&self, query: &str) -> Result<Vec<Track>, YtMusicError> {
        let body = serde_json::json!({
            "query": query,
            "params": "EgWKAQIIAWoKEAMQBBAKEAUQCRgB"
        });
        let json = self
            .post_innertube("search", "WEB_REMIX", "1.20240101.01.00", body)
            .await?;
        let res = parser::parse_search_results(&json);
        Ok(res.tracks.map(|t| t.results).unwrap_or_default())
    }

    /// Полнотекстовый поиск по всем сущностям (треки, артисты, альбомы, плейлисты)
    pub async fn search_all(&self, query: &str) -> Result<SearchResult, YtMusicError> {
        let body = serde_json::json!({
            "query": query
        });
        let json = self
            .post_innertube("search", "WEB_REMIX", "1.20240101.01.00", body)
            .await?;
        Ok(parser::parse_search_results(&json))
    }

    /// Текущий запомненный seed трека для станции/волны
    pub async fn last_seed(&self) -> Option<String> {
        let guard = self.last_seed.read().await;
        guard.clone()
    }

    /// Установка seed трека для станции/волны
    pub async fn set_last_seed(&self, seed: String) {
        let mut guard = self.last_seed.write().await;
        *guard = Some(seed);
    }

    /// Запрос информации об артисте (Топ треков, альбомы, фото).
    /// Если передан не UC... ID (например, поисковый запрос или ID из Яндекса),
    /// сначала выполняется поиск артиста в YouTube Music.
    pub async fn get_artist(&self, query_or_id: &str) -> Result<ArtistBrief, YtMusicError> {
        let browse_id = if query_or_id.starts_with("UC") {
            query_or_id.to_string()
        } else {
            let search_res = self.search_all(query_or_id).await?;
            let artist_opt = search_res
                .artists
                .and_then(|sec| sec.results.into_iter().find(|a| a.id.starts_with("UC")));
            match artist_opt {
                Some(a) => a.id,
                None => {
                    return Err(
                        format!("Исполнитель «{}» не найден в YouTube Music", query_or_id).into(),
                    );
                }
            }
        };

        let body = serde_json::json!({
            "browseId": browse_id
        });
        let json = self
            .post_innertube("browse", "WEB_REMIX", "1.20240101.01.00", body)
            .await?;
        parser::parse_artist_browse(&json, &browse_id)
    }

    /// Возвращает общее количество треков из заголовка плейлиста "Мне нравится" (например, 325)
    pub async fn get_liked_total_count(&self) -> Option<u32> {
        *self.liked_total_count.read().await
    }

    /// Загрузка понравившихся треков пользователя (через HTML LM или browse fallback)
    pub async fn get_liked_tracks(&self) -> Result<Vec<Track>, YtMusicError> {
        // 1. Надежный запрос плейлиста LM через HTML с полной сохраненной кукой пользователя
        if let Ok(html) = self
            .fetch_ytmusic_html("https://music.youtube.com/playlist?list=LM")
            .await
        {
            if let Some(cnt) = parser::parse_header_track_count_from_html(&html) {
                println!(
                    "[ytmusic] Официальное общее количество треков в заголовке: {}",
                    cnt
                );
                *self.liked_total_count.write().await = Some(cnt);
            }
            let (mut tracks, mut continuation_opt) =
                parser::parse_tracks_and_continuation_from_html(&html);
            if !tracks.is_empty() {
                println!(
                    "[ytmusic] Загружено {} понравившихся треков через HTML плейлиста LM",
                    tracks.len()
                );

                // Дозагрузка следующих порций через InnerTube browse continuation
                let mut batches = 0;
                while let Some(continuation) = continuation_opt {
                    if tracks.len() >= 500 || batches >= 10 {
                        break;
                    }
                    batches += 1;
                    let body = serde_json::json!({
                        "continuation": continuation
                    });
                    // POST к browse с {"continuation": token}
                    let res = self
                        .post_innertube("browse", "WEB_REMIX", "1.20240101.01.00", body)
                        .await;
                    match res {
                        Ok(json) => {
                            if self.liked_total_count.read().await.is_none()
                                && let Some(cnt) = parser::parse_header_track_count(&json)
                            {
                                *self.liked_total_count.write().await = Some(cnt);
                            }
                            let next_tracks = parser::parse_liked_tracks(&json);
                            let next_cont = parser::find_continuation_token(&json);
                            if next_tracks.is_empty() {
                                break;
                            }
                            let mut seen_ids: std::collections::HashSet<String> =
                                tracks.iter().map(|t| t.id.clone()).collect();
                            for t in next_tracks {
                                if !t.id.is_empty() && seen_ids.insert(t.id.clone()) {
                                    tracks.push(t);
                                }
                            }
                            continuation_opt = next_cont;
                        }
                        Err(e) => {
                            eprintln!(
                                "[ytmusic] Ошибка дозагрузки continuation через browse: {:?}",
                                e
                            );
                            break;
                        }
                    }
                }

                println!(
                    "[ytmusic] Всего загружено {} понравившихся треков",
                    tracks.len()
                );
                return Ok(tracks);
            }
        }

        // 2. Фолбэк через InnerTube browse API
        for browse_id in &["VLLM", "VLSE", "FEmusic_liked_videos"] {
            let body = serde_json::json!({
                "browseId": browse_id
            });
            if let Ok(json) = self
                .post_innertube("browse", "WEB_REMIX", "1.20240101.01.00", body)
                .await
            {
                if let Some(cnt) = parser::parse_header_track_count(&json) {
                    println!(
                        "[ytmusic] Официальное общее количество треков в заголовке browse: {}",
                        cnt
                    );
                    *self.liked_total_count.write().await = Some(cnt);
                }
                let mut tracks = parser::parse_liked_tracks(&json);
                if !tracks.is_empty() {
                    let mut continuation_opt = parser::find_continuation_token(&json);
                    let mut batches = 0;
                    while let Some(continuation) = continuation_opt {
                        if tracks.len() >= 500 || batches >= 10 {
                            break;
                        }
                        batches += 1;
                        let body = serde_json::json!({
                            "continuation": continuation
                        });
                        if let Ok(next_json) = self
                            .post_innertube("browse", "WEB_REMIX", "1.20240101.01.00", body)
                            .await
                        {
                            let next_tracks = parser::parse_liked_tracks(&next_json);
                            let next_cont = parser::find_continuation_token(&next_json);
                            if next_tracks.is_empty() {
                                break;
                            }
                            let mut seen_ids: std::collections::HashSet<String> =
                                tracks.iter().map(|t| t.id.clone()).collect();
                            for t in next_tracks {
                                if !t.id.is_empty() && seen_ids.insert(t.id.clone()) {
                                    tracks.push(t);
                                }
                            }
                            continuation_opt = next_cont;
                        } else {
                            break;
                        }
                    }
                    return Ok(tracks);
                }
            }
        }
        Ok(Vec::new())
    }

    /// Запрос HTML страницы YouTube Music с кукой пользователя
    pub async fn fetch_ytmusic_html(&self, url: &str) -> Result<String, YtMusicError> {
        let cookie_opt = {
            let guard = self.cookie.read().await;
            guard.clone()
        };

        let mut req = self.http.get(url)
            .header(
                "User-Agent",
                UA_CHROME_124,
            )
            .header("Accept-Language", "ru-RU,ru;q=0.9,en-US;q=0.8,en;q=0.7")
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8");

        if let Some(cookie) = cookie_opt {
            req = req.header("Cookie", cookie);
        }

        let resp = req.send().await?.error_for_status()?;
        let text = resp.text().await?;
        Ok(text)
    }

    /// Запрос списка треков альбома или плейлиста
    pub async fn get_album_tracks(&self, browse_id: &str) -> Result<Vec<Track>, YtMusicError> {
        let body = serde_json::json!({
            "browseId": browse_id
        });
        if let Ok(json) = self
            .post_innertube("browse", "WEB_REMIX", "1.20240101.01.00", body)
            .await
        {
            let tracks = parser::parse_album_tracks(&json);
            if !tracks.is_empty() {
                return Ok(tracks);
            }
        }

        // Фолбэк для пользовательских плейлистов (VLPL... / PL...): запрос HTML
        if browse_id.starts_with("VLPL")
            || browse_id.starts_with("PL")
            || browse_id.starts_with("VL")
        {
            let clean_id = browse_id.strip_prefix("VL").unwrap_or(browse_id);
            let url = format!("https://music.youtube.com/playlist?list={clean_id}");
            if let Ok(html) = self.fetch_ytmusic_html(&url).await {
                let tracks = parser::parse_tracks_from_html(&html);
                if !tracks.is_empty() {
                    return Ok(tracks);
                }
            }
        }

        Ok(Vec::new())
    }

    /// Проверяет и при необходимости заменяет заблокированные РКН российские GGC-ноды (Google Global Cache)
    /// на рабочую резервную ноду из параметра `mn` с проверкой доступности
    pub async fn resolve_live_stream_url(&self, raw_url: &str) -> String {
        let fallback_info = build_fallback_stream_url(raw_url);
        let user_agent = UA_SAFARI_26;

        if let Some((fallback_url, primary, fallback)) = &fallback_info
            && is_known_blocked_ggc_node(primary)
        {
            println!(
                "[ytmusic] Обнаружена заблокированная РКН нода GGC ({primary}). Переключаем поток на резервную европейскую ноду Google ({fallback})..."
            );
            if let Some(verified) = check_stream_connectivity(
                &self.http,
                fallback_url,
                user_agent,
                PROBE_TIMEOUT_FALLBACK_MS,
            )
            .await
            {
                println!(
                    "[ytmusic] Резервный поток подтвержден и готов к мгновенному воспроизведению"
                );
                return verified;
            }
            println!(
                "[ytmusic] Резервный поток не ответил на предварительный пинг, используем fallback URL напрямую"
            );
            return fallback_url.clone();
        }

        // Если первичная нода не из черного списка блокировок, проверяем её доступность за 1200 мс
        if let Some(verified) =
            check_stream_connectivity(&self.http, raw_url, user_agent, PROBE_TIMEOUT_PRIMARY_MS)
                .await
        {
            return verified;
        }

        // Если первичная нода зависла или не ответила вовремя, но есть fallback — немедленно переключаемся на него!
        if let Some((fallback_url, primary, fallback)) = fallback_info {
            println!(
                "[ytmusic] Первичная нода ({primary}) не ответила вовремя. Переключаем на резервную ноду ({fallback})..."
            );
            if let Some(verified) = check_stream_connectivity(
                &self.http,
                &fallback_url,
                user_agent,
                PROBE_TIMEOUT_FALLBACK_MS,
            )
            .await
            {
                return verified;
            }
            return fallback_url;
        }

        raw_url.to_string()
    }

    /// Получение прямой ссылки на аудиопоток трека через VisionOS API (~100-200 мс) с автоматическим обходом блокировок GGC
    pub async fn get_stream_url(&self, track_id: &str) -> Result<String, YtMusicError> {
        // Приоритет 1: мгновенный нативный запрос через InnerTube VisionOS API (~100-200 мс)
        let raw_url = match self.get_stream_url_visionos(track_id).await {
            Ok(url) => url,
            Err(e) => {
                eprintln!(
                    "[ytmusic] VisionOS API вернул ошибку ({e}), переключаемся на резервный yt-dlp fallback"
                );
                get_stream_url_ytdlp_fallback(track_id).await?
            }
        };

        let resolved_url = self.resolve_live_stream_url(&raw_url).await;
        Ok(resolved_url)
    }

    /// Отправляет действие оценки (like/like, like/dislike, like/removelike) во все связанные профили Google и домены (YouTube Music и YouTube Video)
    pub async fn send_rating_action(
        &self,
        endpoint: &str,
        track_id: &str,
    ) -> Result<(), YtMusicError> {
        let cookie_opt = {
            let guard = self.cookie.read().await;
            guard.clone()
        };

        let mut success = false;
        let mut last_error: Option<YtMusicError> = None;

        let targets = [
            ("https://music.youtube.com", "WEB_REMIX", "1.20240101.01.00"),
            ("https://www.youtube.com", "WEB", "2.20240101.01.00"),
        ];

        let authusers = ["3", "0"];

        for (origin, client_name, client_version) in &targets {
            for au in &authusers {
                let url = format!("{}/youtubei/v1/{}", origin, endpoint);
                let body = serde_json::json!({
                    "context": {
                        "client": {
                            "clientName": client_name,
                            "clientVersion": client_version,
                            "hl": "ru",
                            "gl": "RU"
                        }
                    },
                    "target": {
                        "videoId": track_id
                    }
                });

                let mut req = self
                    .http
                    .post(&url)
                    .header("Content-Type", "application/json")
                    .header("User-Agent", UA_CHROME_124)
                    .header("X-Goog-Api-Format-Version", "2")
                    .header("Origin", *origin)
                    .header("X-Origin", *origin)
                    .header("Referer", format!("{}/", origin));

                if let Some(cookie) = &cookie_opt {
                    req = req.header("Cookie", cookie).header("X-Goog-AuthUser", *au);

                    if let Some(sapisid) = auth::extract_sapisid(cookie) {
                        let auth_header = auth::build_sapisid_hash(&sapisid, origin);
                        req = req.header("Authorization", auth_header);
                    }
                } else if let Some(token) = self.get_valid_access_token().await {
                    req = req.header("Authorization", format!("Bearer {}", token));
                }

                match req.json(&body).send().await {
                    Ok(resp) if resp.status().is_success() => {
                        success = true;
                    }
                    Ok(resp) => {
                        let status = resp.status();
                        let text = resp.text().await.unwrap_or_default();
                        last_error = Some(YtMusicError::Http { status, body: text });
                    }
                    Err(e) => {
                        last_error = Some(YtMusicError::Network(e));
                    }
                }
            }
        }

        if success {
            println!(
                "[ytmusic] Оценка {endpoint} для {track_id} успешно доставлена в Google YouTube"
            );
            Ok(())
        } else {
            let err_msg = last_error.unwrap_or_else(|| {
                YtMusicError::Message("Не удалось отправить оценку трека".to_string())
            });
            eprintln!("[ytmusic] Ошибка доставки оценки {endpoint} для {track_id}: {err_msg}");
            Err(err_msg)
        }
    }

    /// Отправляет дизлайк для трека (скрыть из рекомендаций)
    pub async fn dislike_track(&self, track_id: &str) -> Result<(), YtMusicError> {
        self.send_rating_action("like/dislike", track_id).await
    }

    /// Добавляет трек в плейлист по его ID
    pub async fn add_track_to_playlist(
        &self,
        playlist_id: &str,
        track_id: &str,
    ) -> Result<(), YtMusicError> {
        let clean_id = playlist_id.strip_prefix("VL").unwrap_or(playlist_id);
        let body = serde_json::json!({
            "playlistId": clean_id,
            "actions": [
                {
                    "action": "ACTION_ADD_VIDEO",
                    "addedVideoId": track_id
                }
            ]
        });
        self.post_innertube(
            "browse/edit_playlist",
            "WEB_REMIX",
            "1.20240101.01.00",
            body,
        )
        .await?;
        Ok(())
    }

    /// Создает приватный плейлист и опционально добавляет в него первый трек
    pub async fn create_playlist(
        &self,
        title: &str,
        initial_track_id: Option<&str>,
    ) -> Result<String, YtMusicError> {
        let mut body = serde_json::json!({
            "title": title,
            "privacyStatus": "PRIVATE"
        });
        if let Some(track_id) = initial_track_id {
            body["videoIds"] = serde_json::json!([track_id]);
        }
        let json = self
            .post_innertube("playlist/create", "WEB_REMIX", "1.20240101.01.00", body)
            .await?;
        let playlist_id = json["playlistId"]
            .as_str()
            .or_else(|| json["browseId"].as_str())
            .ok_or_else(|| {
                let msg = json["error"]["message"]
                    .as_str()
                    .unwrap_or("Не удалось получить ID созданного плейлиста");
                format!("InnerTube playlist/create: {}", msg)
            })?
            .to_string();
        Ok(playlist_id)
    }

    /// Скачивает трек в `~/Music/Волна` и возвращает путь к сохраненному файлу
    pub async fn download_track_file(
        &self,
        track: &Track,
    ) -> Result<std::path::PathBuf, YtMusicError> {
        let stream_url = self.get_stream_url(&track.id).await?;
        let bytes = self
            .http
            .get(&stream_url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;

        let home = std::env::var("HOME")
            .map_err(|e| format!("Не удалось определить домашнюю папку: {e}"))?;
        let dir = std::path::Path::new(&home).join("Music").join("Волна");
        std::fs::create_dir_all(&dir)?;

        let name: String = format!("{} - {}", track.artists_string(), track.title)
            .chars()
            .map(|c| if "/\\:*?\"<>|".contains(c) { '_' } else { c })
            .collect();

        let ext = if stream_url.contains("mime=audio%2Fwebm") || stream_url.contains("opus") {
            "opus"
        } else {
            "m4a"
        };

        let path = dir.join(format!("{name}.{ext}"));
        std::fs::write(&path, &bytes)?;
        Ok(path)
    }

    /// Загружает трек и сохраняет его во внутренний офлайн-кэш приложения
    pub async fn save_track_offline(&self, track: &Track) -> Result<(), YtMusicError> {
        let stream_url = self.get_stream_url(&track.id).await?;
        let bytes = self
            .http
            .get(&stream_url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;

        crate::cache::save_offline(track, &bytes)?;
        Ok(())
    }

    /// Запрос официального текста песни из YouTube Music через InnerTube (вкладка Lyrics)
    pub async fn get_yt_lyrics(&self, track_id: &str) -> Option<String> {
        let body = serde_json::json!({
            "videoId": track_id
        });
        let next_json = self
            .post_innertube("next", "WEB_REMIX", "1.20240101.01.00", body)
            .await
            .ok()?;

        // Поиск browseId вкладки текста в tabs
        let tabs = next_json
            .get("contents")
            .and_then(|c| c.get("singleColumnMusicWatchNextResultsRenderer"))
            .and_then(|w| w.get("tabbedRenderer"))
            .and_then(|t| t.get("watchNextTabbedResultsRenderer"))
            .and_then(|r| r.get("tabs"))
            .and_then(|t| t.as_array())?;

        let mut lyrics_browse_id = None;
        for t in tabs {
            if let Some(tab) = t.get("tabRenderer")
                && let Some(browse_id) = tab
                    .get("endpoint")
                    .and_then(|e| e.get("browseEndpoint"))
                    .and_then(|b| b.get("browseId"))
                    .and_then(|id| id.as_str())
                && browse_id.starts_with("MPLY")
            {
                lyrics_browse_id = Some(browse_id.to_string());
                break;
            }
        }

        let browse_id = lyrics_browse_id?;
        let browse_body = serde_json::json!({
            "browseId": browse_id
        });
        let browse_json = self
            .post_innertube("browse", "WEB_REMIX", "1.20240101.01.00", browse_body)
            .await
            .ok()?;

        // Извлекаем текст песни из musicDescriptionShelfRenderer.description.runs
        let runs = browse_json
            .get("contents")
            .and_then(|c| c.get("sectionListRenderer"))
            .and_then(|s| s.get("contents"))
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|first| first.get("musicDescriptionShelfRenderer"))
            .and_then(|shelf| shelf.get("description"))
            .and_then(|desc| desc.get("runs"))
            .and_then(|r| r.as_array())?;

        let mut text = String::new();
        for r in runs {
            if let Some(t) = r.get("text").and_then(|t| t.as_str()) {
                text.push_str(t);
            }
        }

        let trimmed = text.trim();
        if !trimmed.is_empty() {
            Some(trimmed.to_string())
        } else {
            None
        }
    }

    /// Получение текста песни:
    /// 1. Дисковый кэш (если есть синхронизированный или статический текст)
    /// 2. LRCLIB (глобальная база караоке с синхронизированными таймкодами)
    /// 3. YouTube Music InnerTube (официальный текст)
    /// 4. LRCLIB статический текст fallback
    pub async fn get_track_lyrics(&self, track: &Track) -> Option<String> {
        let cache_path = crate::cache::lyrics_cache_path(&track.id);
        let duration_secs = track.duration_ms.map(|d| d / 1000);
        // Разметка из кэша может быть от другой версии трека: текст верный, а караоке уезжает.
        // Тогда файл не отдаём как синхронизированный, а пробуем найти разметку заново; старый текст
        // держим про запас, чтобы в крайнем случае показать его без врущих таймкодов.
        let mut stale_plain = None;
        if let Ok(cached) = std::fs::read_to_string(&cache_path) {
            let trimmed = cached.trim();
            if crate::api::lrclib::cached_usable(trimmed, duration_secs) {
                return Some(trimmed.to_string());
            }
            if !trimmed.is_empty() {
                stale_plain = Some(crate::api::lrclib::strip_lrc_tags(trimmed));
            }
        }

        let artist = track.artists_string();
        let title = track.title.clone();

        let mut lrclib_plain = None;

        // 1. Пробуем LRCLIB для получения синхронизированного караоке
        if let Some(lrclib_text) =
            crate::api::lrclib::fetch_lrclib_lyrics(&self.http, &artist, &title, duration_secs)
                .await
        {
            let trimmed = lrclib_text.trim();
            if crate::api::lrclib::usable_synced_at(trimmed, duration_secs) {
                let _ = std::fs::write(&cache_path, trimmed);
                return Some(trimmed.to_string());
            } else if !trimmed.is_empty() {
                lrclib_plain = Some(trimmed.to_string());
            }
        }

        // 2. Официальный текст из YouTube Music
        if let Some(yt_lyrics) = self.get_yt_lyrics(&track.id).await {
            let trimmed = yt_lyrics.trim();
            if !trimmed.is_empty() {
                // Отвергнутую разметку не затираем статикой: вдруг длительность в метаданных врёт,
                // и в следующий раз проверка пройдёт — а заново размеченный текст уже не найти.
                if stale_plain.is_none() {
                    let _ = std::fs::write(&cache_path, trimmed);
                }
                return Some(trimmed.to_string());
            }
        }

        // 3. Fallback на статический текст из LRCLIB
        if let Some(plain) = lrclib_plain {
            if stale_plain.is_none() {
                let _ = std::fs::write(&cache_path, &plain);
            }
            return Some(plain);
        }

        // 4. Ничего лучше не нашлось — отдаём текст из отвергнутой разметки, но без таймкодов,
        //    чтобы подсветка не уезжала от музыки
        if let Some(plain) = stale_plain {
            return Some(plain);
        }

        None
    }
}

const DEFAULT_VISITOR_ID: &str = "CgtUVWxPX1pQbFpRYyik95bXBjIKCgJSVRIEGgAgUmKrAgqoAkN0c0JDQVVTalFFQkRCNlBETHd2X0xheTVVRWI1alFNRmY4RndJU3NXSi1DQmVmVWlPMUVGRWRDYXBwSVQ0MEdKMmhIMjBmTUJNZVlnek55UVpBaHhBN29aOGZlQVlDRm9ZWGEtQ09aLUVGWnNCajVtOFV2dlJ3Y0ZjT0xoTzNIREItcUlqVWN5VTFYTGJfVlZpbVhjSm1HZzR6VWUzaWxGQ3ZMUUl4NXBkOTduVE9sUktkQ0NRLUdGZDFtQ0lwSGc2bS1xcXdvQlRKRkFSeFk5V3BGN2JHRlR4c2VIdF9jbzc5SmNoT2h1R3AxR2hvU3MwbTJlSzNKbU1Ea3hWS0VJcDA0dnBBRkZZdUY2aWxvUGYzN05zb2lvMC1MT0VmVWpaTFp4bzRP";

/// Извлечение visitor_id из текстового ответа /sw.js_data
fn extract_visitor_id(text: &str) -> Option<String> {
    for prefix in ["Cgs", "Cgt"] {
        let mut search_from = 0;
        while let Some(pos) = text[search_from..].find(prefix) {
            let start = search_from + pos;
            let tail = &text[start..];
            let end = tail.find('"').unwrap_or_else(|| {
                tail.find(|c: char| {
                    !(c.is_ascii_alphanumeric()
                        || c == '_'
                        || c == '-'
                        || c == '%'
                        || c == '='
                        || c == '('
                        || c == ')')
                })
                .unwrap_or(tail.len())
            });
            let candidate = &tail[..end];
            if candidate.len() >= 50 {
                return Some(candidate.to_string());
            }
            search_from = start + prefix.len();
        }
    }
    None
}

/// Извлекает хост и путь с query string из URL вида https://host/path?query
fn parse_url_host_and_path(url: &str) -> Option<(String, String)> {
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let slash_pos = without_scheme.find('/')?;
    let host = without_scheme[..slash_pos].to_string();
    let path = without_scheme[slash_pos..].to_string();
    Some((host, path))
}

/// Извлекает значение параметра query string по имени
fn extract_query_param(url: &str, param: &str) -> Option<String> {
    let key = format!("{param}=");
    let query_start = url.find('?')?;
    let query = &url[query_start + 1..];
    for pair in query.split('&') {
        if let Some(val) = pair.strip_prefix(&key) {
            return Some(val.to_string());
        }
    }
    None
}

/// Формирует fallback URL, заменяя в хосте primary ноду на fallback ноду из параметра mn
fn build_fallback_stream_url(raw_url: &str) -> Option<(String, String, String)> {
    let (host, path) = parse_url_host_and_path(raw_url)?;
    let mn_val = extract_query_param(raw_url, "mn")?;
    let decoded_mn = mn_val.replace("%2C", ",");
    let nodes: Vec<&str> = decoded_mn.split(',').collect();
    if nodes.len() < 2 {
        return None;
    }
    let primary = nodes[0].trim();
    let fallback = nodes[1].trim();
    if primary.is_empty() || fallback.is_empty() || !host.contains(primary) {
        return None;
    }
    let new_host = host.replace(primary, fallback);
    let new_url = format!("https://{new_host}{path}");
    Some((new_url, primary.to_string(), fallback.to_string()))
}

/// Проверяет, принадлежит ли нода к известным блокируемым в РФ российским кэшам GGC
fn is_known_blocked_ggc_node(node: &str) -> bool {
    let n = node.to_lowercase();
    BLOCKED_GGC_NODE_MARKERS
        .iter()
        .any(|marker| n.contains(marker))
}

/// Проверяет доступность стрима и возвращает финальный рабочий URL после редиректов
async fn check_stream_connectivity(
    http: &reqwest::Client,
    url: &str,
    user_agent: &str,
    timeout_ms: u64,
) -> Option<String> {
    let req = http
        .get(url)
        .header("User-Agent", user_agent)
        .header("Range", "bytes=0-1024")
        .timeout(std::time::Duration::from_millis(timeout_ms));

    match req.send().await {
        Ok(resp) if resp.status().is_success() || resp.status().is_redirection() => {
            Some(resp.url().as_str().to_string())
        }
        _ => None,
    }
}

/// Ищет `yt-dlp`: сначала явный путь из `VOLNA_YTDLP`, затем `PATH`.
///
/// Явный путь нужен упаковкам (Flatpak/AppImage), где бинарь лежит рядом с
/// приложением и в `PATH` не попадает.
fn resolve_yt_dlp() -> String {
    if let Some(explicit) = std::env::var_os("VOLNA_YTDLP")
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_file())
    {
        return explicit.to_string_lossy().into_owned();
    }

    let from_path = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join("yt-dlp"))
            .find(|candidate| candidate.is_file())
    });

    match from_path {
        Some(path) => path.to_string_lossy().into_owned(),
        None => "yt-dlp".to_string(),
    }
}

async fn get_stream_url_ytdlp_fallback(track_id: &str) -> Result<String, YtMusicError> {
    let yt_dlp_binary = resolve_yt_dlp();
    if yt_dlp_binary == "yt-dlp" {
        // `resolve_yt_dlp` возвращает голое имя только если ничего не нашёл.
        return Err(YtMusicError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "yt-dlp не найден: установите его или задайте путь в VOLNA_YTDLP",
        )));
    }
    let track_url = format!("https://www.youtube.com/watch?v={track_id}");
    let output = tokio::process::Command::new(&yt_dlp_binary)
        .args(["--no-warnings", "-f", "251/bestaudio/140", "-g", &track_url])
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let stdout_str = String::from_utf8_lossy(&out.stdout);
            if let Some(line) = stdout_str.lines().map(|s| s.trim()).find(|s| !s.is_empty()) {
                println!("[ytmusic] Стрим {track_id} успешно получен через yt-dlp (itag 251)");
                return Ok(line.to_string());
            }
            Err(YtMusicError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("yt-dlp не вернул потоковую ссылку для {track_id}"),
            )))
        }
        Ok(out) => {
            let stderr_str = String::from_utf8_lossy(&out.stderr);
            Err(YtMusicError::Io(std::io::Error::other(format!(
                "yt-dlp завершился с ошибкой: {}",
                stderr_str.trim()
            ))))
        }
        Err(e) => Err(YtMusicError::Io(std::io::Error::other(format!(
            "Ошибка запуска yt-dlp ({yt_dlp_binary}): {e}"
        )))),
    }
}

#[async_trait]
impl MusicProvider for YtMusicClient {
    fn provider_id(&self) -> ProviderId {
        ProviderId::YouTubeMusic
    }

    fn display_name(&self) -> &'static str {
        "YouTube Music"
    }

    fn is_authenticated(&self) -> bool {
        self.is_authenticated()
    }

    async fn get_stream_url(
        &self,
        track_id: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        self.get_stream_url(track_id).await.map_err(Into::into)
    }

    async fn search(
        &self,
        query: &str,
    ) -> Result<SearchResult, Box<dyn std::error::Error + Send + Sync>> {
        self.search_all(query).await.map_err(Into::into)
    }

    async fn get_liked_tracks(
        &self,
    ) -> Result<Vec<Track>, Box<dyn std::error::Error + Send + Sync>> {
        self.get_liked_tracks().await.map_err(Into::into)
    }

    async fn get_liked_total_count(&self) -> Option<u32> {
        self.get_liked_total_count().await
    }

    async fn set_track_like(
        &self,
        track_id: &str,
        liked: bool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let endpoint = if liked {
            "like/like"
        } else {
            "like/removelike"
        };
        self.send_rating_action(endpoint, track_id)
            .await
            .map_err(Into::into)
    }

    async fn set_track_dislike(
        &self,
        track_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.dislike_track(track_id).await.map_err(Into::into)
    }

    async fn get_liked_albums(
        &self,
    ) -> Result<Vec<Album>, Box<dyn std::error::Error + Send + Sync>> {
        let body = serde_json::json!({
            "browseId": "FEmusic_liked_albums"
        });
        let json = self
            .post_innertube("browse", "WEB_REMIX", "1.20240101.01.00", body)
            .await?;
        Ok(parser::parse_albums(&json))
    }

    async fn get_playlists(
        &self,
    ) -> Result<Vec<Playlist>, Box<dyn std::error::Error + Send + Sync>> {
        // 1. Надежный запрос библиотеки плейлистов через HTML
        if let Ok(html) = self
            .fetch_ytmusic_html("https://music.youtube.com/library/playlists")
            .await
        {
            let playlists = parser::parse_playlists_from_html(&html);
            if !playlists.is_empty() {
                println!(
                    "[ytmusic] Загружено {} плейлистов через HTML library/playlists",
                    playlists.len()
                );
                return Ok(playlists);
            }
        }

        // 2. Фолбэк на feed/library и playlist?list=LM (где /guide содержит все плейлисты пользователя)
        for url in &[
            "https://music.youtube.com/feed/library",
            "https://music.youtube.com/playlist?list=LM",
        ] {
            if let Ok(html) = self.fetch_ytmusic_html(url).await {
                let playlists = parser::parse_playlists_from_html(&html);
                if !playlists.is_empty() {
                    println!(
                        "[ytmusic] Загружено {} плейлистов через HTML {}",
                        playlists.len(),
                        url
                    );
                    return Ok(playlists);
                }
            }
        }

        // 3. Фолбэк на InnerTube browse API
        let body = serde_json::json!({
            "browseId": "FEmusic_liked_playlists"
        });
        if let Ok(json) = self
            .post_innertube("browse", "WEB_REMIX", "1.20240101.01.00", body)
            .await
        {
            let playlists = parser::parse_playlists(&json);
            if !playlists.is_empty() {
                return Ok(playlists);
            }
        }

        Ok(Vec::new())
    }

    async fn get_station_tracks(
        &self,
        station_id: &str,
        batch_id: Option<&str>,
    ) -> Result<StationBatch, Box<dyn std::error::Error + Send + Sync>> {
        if let Some(continuation) = batch_id {
            let body = serde_json::json!({
                "continuation": continuation
            });
            let json = self
                .post_innertube("next", "WEB_REMIX", "1.20240101.01.00", body)
                .await?;
            let (tracks, cont) = parser::parse_radio_batch(&json);
            return Ok(StationBatch {
                batch_id: cont.unwrap_or_default(),
                tracks,
                chips: parser::parse_station_chips(&json),
            });
        }

        // 0. Вариант станции из чипа YouTube Music: `variant:<playlistId>` (`RDAT…`)
        //
        // Подпись чипа не фильтрует текущий микс, а открывает другой его сид-вариант,
        // поэтому `playlistId` из чипа подставляется в запрос вместо `RDAMVM<seed>`.
        if let Some(playlist_id) = station_id.strip_prefix("variant:") {
            let playlist_id = playlist_id.trim();
            if playlist_id.is_empty() {
                return Err("пустой playlistId варианта станции".into());
            }
            let seed_video_id = self
                .last_seed
                .read()
                .await
                .clone()
                .unwrap_or_else(|| "bgKxYnTEN5Y".to_string());
            let json = self
                .post_innertube(
                    "next",
                    "WEB_REMIX",
                    "1.20240101.01.00",
                    serde_json::json!({
                        "videoId": seed_video_id,
                        "playlistId": playlist_id,
                    }),
                )
                .await?;
            let (tracks, continuation) = parser::parse_radio_batch(&json);
            let chips = parser::parse_station_chips(&json);
            if tracks.is_empty() {
                // Вариант не отдал треков — возвращаем базовый автомикс того же сида
                let base_json = self
                    .post_innertube(
                        "next",
                        "WEB_REMIX",
                        "1.20240101.01.00",
                        serde_json::json!({
                            "videoId": seed_video_id,
                            "playlistId": format!("RDAMVM{}", seed_video_id),
                        }),
                    )
                    .await?;
                let (base_tracks, base_cont) = parser::parse_radio_batch(&base_json);
                return Ok(StationBatch {
                    batch_id: base_cont.unwrap_or_default(),
                    tracks: base_tracks,
                    chips: parser::parse_station_chips(&base_json),
                });
            }
            return Ok(StationBatch {
                batch_id: continuation.unwrap_or_default(),
                tracks,
                chips,
            });
        }

        // 1. Определение seed_video_id
        let seed_video_id: String = if let Some(stripped) = station_id.strip_prefix("track:") {
            stripped.to_string()
        } else if let Some(stripped) = station_id.strip_prefix("seed:") {
            stripped.to_string()
        } else if let Some(idx) = station_id.find("track:") {
            station_id[idx + 6..].to_string()
        } else if let Some(idx) = station_id.find("seed:") {
            station_id[idx + 5..].to_string()
        } else if station_id.len() == 11 && !station_id.starts_with("UC") {
            station_id.to_string()
        } else if let Some(stripped) = station_id.strip_prefix("artist:") {
            if let Ok(brief) = self.get_artist(stripped).await {
                if let Some(t) = brief.popular_tracks.into_iter().find(|t| !t.id.is_empty()) {
                    t.id
                } else {
                    "bgKxYnTEN5Y".to_string()
                }
            } else {
                "bgKxYnTEN5Y".to_string()
            }
        } else {
            // station_id == "user:onyourwave" или "radio" или contains("onyourwave")
            let cached = self.last_seed.read().await.clone();
            if let Some(seed) = cached {
                seed
            } else {
                let liked_first = match self.get_liked_tracks().await {
                    Ok(tracks) => tracks.into_iter().find(|t| !t.id.is_empty()).map(|t| t.id),
                    Err(_) => None,
                };
                liked_first.unwrap_or_else(|| "bgKxYnTEN5Y".to_string())
            }
        };

        // Сохраняем использованный seed_video_id в last_seed
        {
            let mut guard = self.last_seed.write().await;
            *guard = Some(seed_video_id.clone());
        }

        let body = serde_json::json!({
            "videoId": seed_video_id,
            "playlistId": format!("RDAMVM{}", seed_video_id)
        });

        let json = self
            .post_innertube("next", "WEB_REMIX", "1.20240101.01.00", body)
            .await?;
        let (mut tracks, mut continuation) = parser::parse_radio_batch(&json);
        let mut chips = parser::parse_station_chips(&json);

        // Фолбэк: если треки не вернулись, делаем аварийный запрос с проверенным seed
        if tracks.is_empty() && seed_video_id != "bgKxYnTEN5Y" {
            let fb_seed = "bgKxYnTEN5Y";
            let fb_body = serde_json::json!({
                "videoId": fb_seed,
                "playlistId": format!("RDAMVM{}", fb_seed)
            });
            if let Ok(fb_json) = self
                .post_innertube("next", "WEB_REMIX", "1.20240101.01.00", fb_body)
                .await
            {
                let (fb_tracks, fb_cont) = parser::parse_radio_batch(&fb_json);
                if !fb_tracks.is_empty() {
                    let mut guard = self.last_seed.write().await;
                    *guard = Some(fb_seed.to_string());
                    tracks = fb_tracks;
                    continuation = fb_cont;
                    chips = parser::parse_station_chips(&fb_json);
                }
            }
        }

        Ok(StationBatch {
            batch_id: continuation.unwrap_or_default(),
            tracks,
            chips,
        })
    }

    async fn send_feedback(
        &self,
        _event: FeedbackEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn get_track_lyrics(&self, track: &Track) -> Option<String> {
        self.get_track_lyrics(track).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_yt_dlp() {
        let binary = resolve_yt_dlp();
        assert!(!binary.is_empty());
        // Должен либо находить существующий путь, либо "yt-dlp"
        if binary.contains('/') {
            assert!(std::path::Path::new(&binary).exists());
        } else {
            assert_eq!(binary, "yt-dlp");
        }
    }

    #[test]
    fn test_extract_visitor_id() {
        let sample_cgs = r#")]}'\n[["yt.sw.adr",null,[[["ru","RU",null,"1.1.1.1",null,null,null,null,null,null,null,"Robot","Bot","CgsSampleVisitorIdLongEnoughToPassTheFiftyCharactersMinimumValidationCheck1234567890%3D"]]]"#;
        let extracted_cgs = extract_visitor_id(sample_cgs);
        assert_eq!(
            extracted_cgs.as_deref(),
            Some(
                "CgsSampleVisitorIdLongEnoughToPassTheFiftyCharactersMinimumValidationCheck1234567890%3D"
            )
        );

        let sample_cgt = r#")]}'\n[["yt.sw.adr",null,[[["ru","RU",null,"1.1.1.1",null,null,null,null,null,null,null,"Robot","Bot","CgtSampleVisitorIdLongEnoughToPassTheFiftyCharactersMinimumValidationCheck1234567890"]]]"#;
        let extracted_cgt = extract_visitor_id(sample_cgt);
        assert_eq!(
            extracted_cgt.as_deref(),
            Some(
                "CgtSampleVisitorIdLongEnoughToPassTheFiftyCharactersMinimumValidationCheck1234567890"
            )
        );

        let too_short = r#"["CgsShort123"]"#;
        assert_eq!(extract_visitor_id(too_short), None);

        let empty = "";
        assert_eq!(extract_visitor_id(empty), None);
    }

    #[tokio::test]
    async fn test_get_visitor_id_cached() {
        let client = YtMusicClient::new();
        {
            let mut guard = client.visitor_id.write().await;
            *guard = Some("CgsCustomCachedVisitorId1234567890".to_string());
        }
        let vid = client.get_visitor_id().await;
        assert_eq!(vid, "CgsCustomCachedVisitorId1234567890");
    }

    #[tokio::test]
    #[ignore]
    async fn test_visionos_stream_live() {
        let client = YtMusicClient::new();
        // dQw4w9WgXcQ — Never Gonna Give You Up
        let stream_url = client
            .get_stream_url("dQw4w9WgXcQ")
            .await
            .expect("Failed to get stream url");
        assert!(stream_url.starts_with("https://"));
        assert!(stream_url.contains("videoplayback"));
    }

    #[tokio::test]
    #[ignore]
    async fn test_problematic_track_stream_live() {
        let client = YtMusicClient::new();
        // lK5Oy7c57Ss — Три дня дождя — Перезаряжай
        let stream_url = client
            .get_stream_url("lK5Oy7c57Ss")
            .await
            .expect("Failed to get stream url");
        println!("[test] Final stream URL: {stream_url}");
        assert!(stream_url.starts_with("https://"));
        assert!(stream_url.contains("videoplayback"));
        // Хост не должен указывать на заблокированную ноду
        let (host, _) = parse_url_host_and_path(&stream_url).expect("Failed to parse host");
        println!("[test] Verified host: {host}");
        assert!(
            !host.contains("sn-hxb54vo-238s"),
            "Host should not contain blocked node!"
        );
        assert!(host.contains("sn-u15hn5-cc") || host.contains("googlevideo.com"));
    }

    #[tokio::test]
    #[ignore]
    async fn test_user_library_live() {
        let cookie = crate::api::auth::load_ytmusic_cookie().expect("Cookie not found in config");
        let client = YtMusicClient::with_cookie(cookie);

        let liked = client
            .get_liked_tracks()
            .await
            .expect("Failed to get liked tracks");
        println!("[test] Loaded {} liked tracks", liked.len());
        assert!(!liked.is_empty(), "Liked tracks should not be empty!");
        let first = &liked[0];
        println!(
            "[test] First liked track: {} - {} (cover: {:?})",
            first.title,
            first.artists_string(),
            first.cover_uri
        );

        let playlists = client
            .get_playlists()
            .await
            .expect("Failed to get playlists");
        println!("[test] Loaded {} playlists", playlists.len());
        for p in &playlists {
            println!("[test] Playlist: {} ({})", p.title, p.uid);
        }
        assert!(!playlists.is_empty(), "Playlists should not be empty!");

        // Проверяем получение треков одного из плейлистов
        let pl_tracks = client
            .get_album_tracks("VLPLcHCtI7UKhDqwiMLD3A_nTUx7DJxKQt2c")
            .await
            .expect("Failed to get playlist tracks");
        println!(
            "[test] Loaded {} tracks for playlist TikTok Songs",
            pl_tracks.len()
        );
        assert!(
            !pl_tracks.is_empty(),
            "Playlist TikTok Songs should not be empty!"
        );
        println!(
            "[test] First track in TikTok Songs: {} - {}",
            pl_tracks[0].title,
            pl_tracks[0].artists_string()
        );
    }

    #[test]
    fn test_clean_playlist_id_and_ext() {
        let pl_id = "VLPLcHCtI7UKhDqwiMLD3A";
        let clean = pl_id.strip_prefix("VL").unwrap_or(pl_id);
        assert_eq!(clean, "PLcHCtI7UKhDqwiMLD3A");

        let stream_webm =
            "https://rr---sn.googlevideo.com/videoplayback?mime=audio%2Fwebm&itag=251";
        let ext_webm = if stream_webm.contains("mime=audio%2Fwebm") || stream_webm.contains("opus")
        {
            "opus"
        } else {
            "m4a"
        };
        assert_eq!(ext_webm, "opus");

        let stream_m4a = "https://rr---sn.googlevideo.com/videoplayback?mime=audio%2Fmp4&itag=140";
        let ext_m4a = if stream_m4a.contains("mime=audio%2Fwebm") || stream_m4a.contains("opus") {
            "opus"
        } else {
            "m4a"
        };
        assert_eq!(ext_m4a, "m4a");
    }

    #[test]
    fn test_fallback_stream_url_bypass() {
        let raw_url = "https://rr3---sn-hxb54vo-238s.googlevideo.com/videoplayback?expire=1793497812&ei=abc&mn=sn-hxb54vo-238s,sn-u15hn5-cc&itag=251";
        let fallback_res = build_fallback_stream_url(raw_url);
        assert!(fallback_res.is_some());
        let (fallback_url, primary, fallback) = fallback_res.unwrap();
        assert_eq!(primary, "sn-hxb54vo-238s");
        assert_eq!(fallback, "sn-u15hn5-cc");
        assert_eq!(
            fallback_url,
            "https://rr3---sn-u15hn5-cc.googlevideo.com/videoplayback?expire=1793497812&ei=abc&mn=sn-hxb54vo-238s,sn-u15hn5-cc&itag=251"
        );
        assert!(is_known_blocked_ggc_node(&primary));
        assert!(!is_known_blocked_ggc_node(&fallback));
    }

    #[test]
    fn test_extract_query_param_url_encoded() {
        let url = "https://rr.googlevideo.com/videoplayback?id=123&mn=sn-hxb54vo-238s%2Csn-oj5hn5-5g&other=xyz";
        let mn = extract_query_param(url, "mn");
        assert_eq!(mn, Some("sn-hxb54vo-238s%2Csn-oj5hn5-5g".to_string()));
    }
}
