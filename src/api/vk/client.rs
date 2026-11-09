use std::sync::{Arc, RwLock};
use std::time::Duration;

use serde::de::DeserializeOwned;
use zeroize::Zeroizing;

use super::auth::{VK_ANDROID_USER_AGENT, VK_API_VERSION};
use super::models::{
    VkApiResponse, VkAudioDto, VkAudioListDto, VkErrorDto, VkLyricsDto, VkNewsfeedSearchResponse,
    VkPlaylistDto, VkPlaylistListDto, VkStreamMixAudiosDto, VkStreamMixSettingsDto,
};

const VK_API_BASE: &str = "https://api.vk.com";
const MAX_AUDIO_IDS_PER_REQUEST: usize = 100;
const MAX_RETRIES: u32 = 2;

#[derive(Debug, thiserror::Error)]
pub enum VkError {
    #[error("Ошибка сети VK: {0}")]
    Network(#[from] reqwest::Error),
    #[error("VK API вернул HTTP {status}")]
    Http { status: reqwest::StatusCode },
    #[error("Ошибка данных VK: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Требуется повторный вход во ВКонтакте")]
    AuthRevoked,
    #[error("ВКонтакте запросил капчу")]
    Captcha {
        sid: Option<String>,
        image_url: Option<String>,
    },
    #[error("VK API ({code}): {message}")]
    RateLimited { code: i64, message: String },
    #[error("VK API ({code}): {message}")]
    Api { code: i64, message: String },
    #[error("{0}")]
    InvalidInput(String),
    #[error("ВКонтакте не вернул доступный аудиопоток для {0}")]
    MissingStream(String),
}

impl VkError {
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Network(_) | Self::RateLimited { .. } => true,
            Self::Http { status } => {
                status.is_server_error() || *status == reqwest::StatusCode::TOO_MANY_REQUESTS
            }
            _ => false,
        }
    }

    pub fn is_auth(&self) -> bool {
        matches!(self, Self::AuthRevoked)
    }
}

/// Числовая пара идентификаторов, необходимая методам `audio.*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VkAudioId {
    pub owner_id: i64,
    pub audio_id: i64,
}

impl VkAudioId {
    pub fn parse(track_id: &str) -> Result<Self, VkError> {
        let value = track_id.strip_prefix("vk:").ok_or_else(|| {
            VkError::InvalidInput("Идентификатор трека VK должен начинаться с vk:".to_owned())
        })?;
        let (owner_id, audio_id) = value.rsplit_once('_').ok_or_else(|| {
            VkError::InvalidInput("Некорректный идентификатор трека VK".to_owned())
        })?;
        let owner_id = owner_id
            .parse::<i64>()
            .map_err(|_| VkError::InvalidInput("Некорректный owner_id трека VK".to_owned()))?;
        let audio_id = audio_id
            .parse::<i64>()
            .map_err(|_| VkError::InvalidInput("Некорректный audio_id трека VK".to_owned()))?;
        Ok(Self { owner_id, audio_id })
    }

    pub fn as_api_id(self) -> String {
        format!("{}_{}", self.owner_id, self.audio_id)
    }
}

#[derive(Debug, Clone)]
pub struct VkCaptchaChallenge {
    pub sid: Option<String>,
    pub image_url: Option<String>,
}

pub type VkCaptchaHandler =
    Arc<dyn Fn(VkCaptchaChallenge, async_channel::Sender<String>) + Send + Sync + 'static>;

pub struct VkClient {
    http: reqwest::Client,
    token: Zeroizing<String>,
    api_base: String,
    captcha_handler: Arc<RwLock<Option<VkCaptchaHandler>>>,
}

impl VkClient {
    pub fn new(token: impl Into<String>) -> Result<Self, VkError> {
        Self::with_api_base(token, VK_API_BASE)
    }

    fn with_api_base(
        token: impl Into<String>,
        api_base: impl Into<String>,
    ) -> Result<Self, VkError> {
        let token = Zeroizing::new(token.into());
        if token.trim().is_empty() {
            return Err(VkError::InvalidInput("Пустой токен VK".to_owned()));
        }

        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "X-VK-Android-Client",
            reqwest::header::HeaderValue::from_static("new"),
        );
        let http = reqwest::Client::builder()
            .user_agent(VK_ANDROID_USER_AGENT)
            .default_headers(headers)
            .timeout(Duration::from_secs(20))
            .build()?;

        Ok(Self {
            http,
            token,
            api_base: api_base.into(),
            captcha_handler: Arc::new(RwLock::new(None)),
        })
    }

    pub fn set_captcha_handler(&self, handler: VkCaptchaHandler) {
        *self
            .captcha_handler
            .write()
            .unwrap_or_else(|error| error.into_inner()) = Some(handler);
    }

    pub fn is_authenticated(&self) -> bool {
        !self.token.trim().is_empty()
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        params: &[(&str, &str)],
    ) -> Result<T, VkError> {
        let url = format!("{}/method/{method}", self.api_base);
        let mut attempt = 0;
        let mut captcha_answer: Option<(Option<String>, String)> = None;
        let mut captcha_handled = false;

        loop {
            let result = async {
                let mut form = params.to_vec();
                form.push(("access_token", self.token.as_str()));
                form.push(("v", VK_API_VERSION));
                if let Some((sid, key)) = captcha_answer.as_ref() {
                    if let Some(sid) = sid.as_deref() {
                        form.push(("captcha_sid", sid));
                    }
                    form.push(("captcha_key", key));
                }

                let response = self.http.post(&url).form(&form).send().await?;
                let status = response.status();
                let body = response.text().await?;
                decode_api_response(status, &body)
            }
            .await;

            match result {
                Err(error) if error.is_retryable() && attempt < MAX_RETRIES => {
                    attempt += 1;
                    let delay = Duration::from_millis(250 * 2_u64.pow(attempt - 1));
                    tokio::time::sleep(delay).await;
                }
                Err(VkError::Captcha { sid, image_url }) if !captcha_handled => {
                    let handler = self
                        .captcha_handler
                        .read()
                        .unwrap_or_else(|error| error.into_inner())
                        .clone();
                    let Some(handler) = handler else {
                        return Err(VkError::Captcha { sid, image_url });
                    };
                    let (respond, response) = async_channel::bounded(1);
                    handler(
                        VkCaptchaChallenge {
                            sid: sid.clone(),
                            image_url: image_url.clone(),
                        },
                        respond,
                    );
                    let answer =
                        tokio::time::timeout(Duration::from_secs(300), response.recv()).await;
                    match answer {
                        Ok(Ok(key)) if !key.trim().is_empty() => {
                            captcha_answer = Some((sid, key));
                            captcha_handled = true;
                            attempt = 0;
                        }
                        _ => return Err(VkError::Captcha { sid, image_url }),
                    }
                }
                result => return result,
            }
        }
    }

    pub async fn get_liked_audio(
        &self,
        owner_id: Option<i64>,
        offset: u32,
        count: u32,
    ) -> Result<VkAudioListDto, VkError> {
        let offset = offset.to_string();
        let count = count.to_string();
        let owner_id_str = owner_id.map(|id| id.to_string());
        let mut params = vec![("offset", offset.as_str()), ("count", count.as_str())];
        if let Some(ref id) = owner_id_str {
            params.push(("owner_id", id.as_str()));
        }
        self.call("audio.get", &params).await
    }

    pub async fn get_album_audios(&self, owner_id: i64, album_id: i64) -> Result<Vec<VkAudioDto>, VkError> {
        let owner_str = owner_id.to_string();
        let album_str = album_id.to_string();
        let mut audios = Vec::new();
        let mut offset = 0u32;
        loop {
            let offset_str = offset.to_string();
            let res: VkAudioListDto = self
                .call(
                    "audio.get",
                    &[
                        ("owner_id", &owner_str),
                        ("album_id", &album_str),
                        ("offset", &offset_str),
                        ("count", "100"),
                    ],
                )
                .await?;
            if res.items.is_empty() {
                break;
            }
            let count = res.items.len() as u32;
            audios.extend(res.items);
            if audios.len() as u64 >= res.count || count < 100 {
                break;
            }
            offset += count;
        }
        Ok(audios)
    }

    pub async fn search_audio(
        &self,
        query: &str,
        offset: u32,
        count: u32,
    ) -> Result<VkAudioListDto, VkError> {
        if query.trim().is_empty() {
            return Err(VkError::InvalidInput("Пустой поисковый запрос".to_owned()));
        }
        let offset = offset.to_string();
        let count = count.to_string();
        self.call(
            "audio.search",
            &[("q", query.trim()), ("offset", &offset), ("count", &count)],
        )
        .await
    }

    pub async fn search_playlists(
        &self,
        query: &str,
        count: u32,
    ) -> Result<VkPlaylistListDto, VkError> {
        let count_str = count.to_string();
        self.call(
            "audio.searchPlaylists",
            &[("q", query), ("count", &count_str)],
        )
        .await
    }

    pub async fn get_playlists(
        &self,
        owner_id: i64,
        offset: u32,
        count: u32,
    ) -> Result<VkPlaylistListDto, VkError> {
        let owner_id = owner_id.to_string();
        let offset = offset.to_string();
        let count = count.to_string();
        self.call(
            "audio.getPlaylists",
            &[
                ("owner_id", &owner_id),
                ("offset", &offset),
                ("count", &count),
            ],
        )
        .await
    }

    pub async fn get_playlist_by_id(
        &self,
        owner_id: i64,
        playlist_id: i64,
    ) -> Result<VkPlaylistDto, VkError> {
        let owner_id = owner_id.to_string();
        let playlist_id = playlist_id.to_string();
        self.call(
            "audio.getPlaylistById",
            &[("owner_id", &owner_id), ("playlist_id", &playlist_id)],
        )
        .await
    }

    pub async fn get_playlist_audios(
        &self,
        owner_id: i64,
        playlist_id: i64,
        access_key: Option<&str>,
    ) -> Result<Vec<VkAudioDto>, VkError> {
        let owner_str = owner_id.to_string();
        let playlist_str = playlist_id.to_string();
        let mut audios = Vec::new();
        let mut offset = 0u32;
        loop {
            let offset_str = offset.to_string();
            let mut params = vec![
                ("owner_id", owner_str.as_str()),
                ("playlist_id", playlist_str.as_str()),
                ("album_id", playlist_str.as_str()),
                ("offset", offset_str.as_str()),
                ("count", "100"),
            ];
            if let Some(key) = access_key {
                params.push(("access_key", key));
            }
            let res: VkAudioListDto = self.call("audio.get", &params).await?;
            if res.items.is_empty() {
                break;
            }
            let count = res.items.len() as u32;
            audios.extend(res.items);
            if audios.len() as u64 >= res.count || count < 100 {
                break;
            }
            offset += count;
        }
        Ok(audios)
    }

    /// Обновляет до 100 аудиозаписей за один вызов `audio.getById`.
    pub async fn get_audio_by_id(&self, track_ids: &[String]) -> Result<Vec<VkAudioDto>, VkError> {
        if track_ids.is_empty() {
            return Ok(Vec::new());
        }
        if track_ids.len() > MAX_AUDIO_IDS_PER_REQUEST {
            return Err(VkError::InvalidInput(format!(
                "audio.getById принимает не более {MAX_AUDIO_IDS_PER_REQUEST} ID"
            )));
        }

        let audios = track_ids
            .iter()
            .map(|track_id| VkAudioId::parse(track_id).map(VkAudioId::as_api_id))
            .collect::<Result<Vec<_>, _>>()?
            .join(",");
        self.call("audio.getById", &[("audios", &audios)]).await
    }

    pub async fn get_stream_url(&self, track_id: &str) -> Result<String, VkError> {
        let tracks = self.get_audio_by_id(&[track_id.to_owned()]).await?;
        let expected = VkAudioId::parse(track_id)?;
        tracks
            .into_iter()
            .find(|track| track.id == expected.audio_id && track.owner_id == expected.owner_id)
            .and_then(|track| track.stream_url().map(str::to_owned))
            .filter(|url| !url.trim().is_empty())
            .ok_or_else(|| VkError::MissingStream(track_id.to_owned()))
    }

    pub async fn get_stream_mix_settings(&self) -> Result<VkStreamMixSettingsDto, VkError> {
        self.call("audio.getStreamMixSettings", &[]).await
    }

    pub async fn get_stream_mix_audios(
        &self,
        mix_id: &str,
        settings: &[String],
        context: Option<&str>,
        audio_id: Option<&str>,
        count: u32,
    ) -> Result<VkStreamMixAudiosDto, VkError> {
        let settings = serde_json::to_string(settings)?;
        let count = count.to_string();
        let mut params = vec![
            ("mix_id", mix_id),
            ("stream_mix_settings", settings.as_str()),
            ("count", count.as_str()),
        ];
        if let Some(context) = context.filter(|context| !context.is_empty()) {
            params.push(("context", context));
        }
        if let Some(audio_id) = audio_id.filter(|a| !a.is_empty()) {
            params.push(("audio_id", audio_id));
        }
        self.call("audio.getStreamMixAudios", &params).await
    }

    pub async fn search_newsfeed_audios(
        &self,
        query: &str,
        count: u32,
    ) -> Result<Vec<VkAudioDto>, VkError> {
        let count_str = count.to_string();
        let resp: VkNewsfeedSearchResponse = self
            .call(
                "newsfeed.search",
                &[("q", query.trim()), ("count", &count_str)],
            )
            .await?;
        let mut audios = Vec::new();
        for item in resp.items {
            for att in item.attachments {
                if att.attachment_type == "audio" {
                    if let Some(audio) = att.audio {
                        audios.push(audio);
                    }
                }
            }
        }
        Ok(audios)
    }

    pub async fn get_lyrics(&self, track_id: &str, lyrics_id: i64) -> Result<VkLyricsDto, VkError> {
        let id = VkAudioId::parse(track_id)?;
        let audio_id = id.as_api_id();
        let lyrics_id = lyrics_id.to_string();
        self.call(
            "audio.getLyrics",
            &[("audio_id", &audio_id), ("lyrics_id", &lyrics_id)],
        )
        .await
    }

    pub async fn set_track_like(&self, track_id: &str, liked: bool) -> Result<(), VkError> {
        let id = VkAudioId::parse(track_id)?;
        let audio_id = id.audio_id.to_string();
        let owner_id = id.owner_id.to_string();
        let method = if liked { "audio.add" } else { "audio.delete" };
        self.call::<serde_json::Value>(method, &[("audio_id", &audio_id), ("owner_id", &owner_id)])
            .await?;
        Ok(())
    }

    pub async fn set_track_dislike(&self, track_id: &str, disliked: bool) -> Result<(), VkError> {
        let id = VkAudioId::parse(track_id)?;
        let audio_id = id.audio_id.to_string();
        let owner_id = id.owner_id.to_string();
        let method = if disliked {
            "audio.addDislike"
        } else {
            "audio.removeDislike"
        };
        self.call::<serde_json::Value>(method, &[("audio_id", &audio_id), ("owner_id", &owner_id)])
            .await?;
        Ok(())
    }
}

fn decode_api_response<T: DeserializeOwned>(
    status: reqwest::StatusCode,
    body: &str,
) -> Result<T, VkError> {
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(VkError::AuthRevoked);
    }

    match serde_json::from_str::<VkApiResponse<T>>(body) {
        Ok(VkApiResponse::Response { response }) if status.is_success() => Ok(response),
        Ok(VkApiResponse::Error { error }) => Err(map_api_error(error)),
        Ok(VkApiResponse::Response { .. }) => Err(VkError::Http { status }),
        Err(error) if status.is_success() => Err(VkError::Json(error)),
        Err(_) => Err(VkError::Http { status }),
    }
}

fn map_api_error(error: VkErrorDto) -> VkError {
    match error.error_code {
        5 => VkError::AuthRevoked,
        14 => VkError::Captcha {
            sid: error.captcha_sid,
            image_url: error.captcha_img,
        },
        code @ (6 | 9) => VkError::RateLimited {
            code,
            message: error.error_msg,
        },
        code => VkError::Api {
            code,
            message: error.error_msg,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn read_request(stream: &mut tokio::net::TcpStream) -> String {
        let mut request = Vec::new();
        let mut expected_len = None;

        loop {
            let mut chunk = [0_u8; 2048];
            let read = stream.read(&mut chunk).await.expect("request must arrive");
            if read == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..read]);

            if expected_len.is_none() {
                if let Some(headers_end) = request
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .map(|position| position + 4)
                {
                    let headers = String::from_utf8_lossy(&request[..headers_end]);
                    let content_len = headers
                        .lines()
                        .filter_map(|line| line.split_once(':'))
                        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    expected_len = Some(headers_end + content_len);
                }
            }

            if expected_len.is_some_and(|length| request.len() >= length) {
                break;
            }
        }

        String::from_utf8(request).expect("test request must be UTF-8")
    }

    async fn respond(mut stream: tokio::net::TcpStream, body: &str) -> String {
        let request = read_request(&mut stream).await;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .await
            .expect("test response must be sent");
        request
    }

    #[test]
    fn track_id_round_trip_supports_negative_owner() {
        let id = VkAudioId::parse("vk:-2000001_456240123").expect("VK track ID must parse");
        assert_eq!(id.owner_id, -2_000_001);
        assert_eq!(id.audio_id, 456_240_123);
        assert_eq!(id.as_api_id(), "-2000001_456240123");
        assert!(VkAudioId::parse("456240123").is_err());
        assert!(VkAudioId::parse("vk:not-a-number_456240123").is_err());
    }

    #[test]
    fn api_errors_are_classified_without_exposing_response_bodies() {
        let auth = map_api_error(VkErrorDto {
            error_code: 5,
            error_msg: "revoked".to_owned(),
            captcha_sid: None,
            captcha_img: None,
        });
        assert!(auth.is_auth());
        assert!(!auth.is_retryable());

        let rate_limit = map_api_error(VkErrorDto {
            error_code: 6,
            error_msg: "too fast".to_owned(),
            captcha_sid: None,
            captcha_img: None,
        });
        assert!(rate_limit.is_retryable());

        let captcha = map_api_error(VkErrorDto {
            error_code: 14,
            error_msg: "captcha".to_owned(),
            captcha_sid: Some("sid".to_owned()),
            captcha_img: Some("https://api.example.invalid/captcha.jpg".to_owned()),
        });
        match captcha {
            VkError::Captcha { sid, image_url } => {
                assert_eq!(sid.as_deref(), Some("sid"));
                assert_eq!(
                    image_url.as_deref(),
                    Some("https://api.example.invalid/captcha.jpg")
                );
            }
            _ => panic!("expected captcha"),
        }
    }

    #[test]
    fn successful_and_error_envelopes_are_decoded() {
        let list: VkAudioListDto = decode_api_response(
            reqwest::StatusCode::OK,
            r#"{"response":{"count":0,"items":[]}}"#,
        )
        .expect("successful response must decode");
        assert_eq!(list.count, 0);

        let error = decode_api_response::<VkAudioListDto>(
            reqwest::StatusCode::BAD_REQUEST,
            "not json and no sensitive body in the error",
        )
        .expect_err("invalid HTTP response must fail");
        assert!(matches!(error, VkError::Http { .. }));
        assert!(!error.to_string().contains("not json"));
    }

    #[tokio::test]
    async fn captcha_handler_resumes_the_original_api_call() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener must bind");
        let address = listener.local_addr().expect("test address must exist");
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for attempt in 0..2 {
                let (stream, _) = listener
                    .accept()
                    .await
                    .expect("test connection must arrive");
                let body = if attempt == 0 {
                    r#"{"error":{"error_code":14,"error_msg":"captcha","captcha_sid":"test-sid","captcha_img":"https://api.example.invalid/captcha.jpg"}}"#
                } else {
                    r#"{"response":{"count":0,"items":[]}}"#
                };
                requests.push(respond(stream, body).await);
            }
            requests
        });

        let client = VkClient::with_api_base("test-access-token", format!("http://{address}"))
            .expect("test client must be created");
        client.set_captcha_handler(Arc::new(|challenge, respond| {
            assert_eq!(challenge.sid.as_deref(), Some("test-sid"));
            respond
                .try_send("test-answer".to_owned())
                .expect("captcha answer must be sent");
        }));

        client
            .get_liked_audio(None, 0, 20)
            .await
            .expect("original call must succeed after captcha");
        let requests = server.await.expect("test server must finish");
        assert_eq!(requests.len(), 2);
        let first_body = requests[0]
            .split_once("\r\n\r\n")
            .expect("first HTTP parts")
            .1;
        let second_body = requests[1]
            .split_once("\r\n\r\n")
            .expect("second HTTP parts")
            .1;
        assert!(!first_body.contains("captcha_key"));
        assert!(second_body.contains("captcha_sid=test-sid"));
        assert!(second_body.contains("captcha_key=test-answer"));
        assert!(second_body.contains("offset=0"));
        assert!(second_body.contains("count=20"));
    }

    #[tokio::test]
    async fn retries_rate_limits_and_sends_credentials_only_in_the_form() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener must bind");
        let address = listener.local_addr().expect("test address must exist");
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for attempt in 0..3 {
                let (stream, _) = listener
                    .accept()
                    .await
                    .expect("test connection must arrive");
                let body = if attempt < 2 {
                    r#"{"error":{"error_code":6,"error_msg":"too fast"}}"#
                } else {
                    r#"{"response":{"count":1,"items":[]}}"#
                };
                requests.push(respond(stream, body).await);
            }
            requests
        });

        let client = VkClient::with_api_base("test-access-token", format!("http://{address}"))
            .expect("test client must be created");
        let response = client
            .get_liked_audio(None, 20, 50)
            .await
            .expect("third response must succeed after two retries");
        assert_eq!(response.count, 1);

        let requests = server.await.expect("test server must finish");
        assert_eq!(requests.len(), 3);
        for request in requests {
            let (headers, body) = request.split_once("\r\n\r\n").expect("HTTP parts");
            let lowercase_headers = headers.to_ascii_lowercase();
            assert!(headers.starts_with("POST /method/audio.get HTTP/1.1"));
            assert!(lowercase_headers.contains(&format!("user-agent: {}", VK_ANDROID_USER_AGENT.to_ascii_lowercase())));
            assert!(lowercase_headers.contains("x-vk-android-client: new"));
            assert!(!headers.contains("test-access-token"));
            assert!(body.contains("access_token=test-access-token"));
            assert!(body.contains("v=5.199"));
            assert!(body.contains("offset=20"));
            assert!(body.contains("count=50"));
        }
    }

    #[tokio::test]
    async fn get_liked_audio_includes_owner_id_when_provided() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener must bind");
        let address = listener.local_addr().expect("test address must exist");
        let server = tokio::spawn(async move {
            let (stream, _) = listener
                .accept()
                .await
                .expect("test connection must arrive");
            respond(stream, r#"{"response":{"count":0,"items":[]}}"#).await
        });

        let client = VkClient::with_api_base("test-access-token", format!("http://{address}"))
            .expect("test client must be created");
        let response = client
            .get_liked_audio(Some(12345678), 0, 30)
            .await
            .expect("call must succeed");
        assert_eq!(response.count, 0);

        let request = server.await.expect("test server must finish");
        let (_headers, body) = request.split_once("\r\n\r\n").expect("HTTP parts");
        assert!(body.contains("owner_id=12345678"));
        assert!(body.contains("offset=0"));
        assert!(body.contains("count=30"));
    }
}
