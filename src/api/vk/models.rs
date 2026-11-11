use std::fmt;

use serde::{Deserialize, Deserializer};

use crate::api::models::{Album, Artist, Track, TrackSource};

/// Ответ VK API: либо полезные данные, либо структурированная ошибка.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum VkApiResponse<T> {
    Response { response: T },
    Error { error: VkErrorDto },
}

/// Ошибка вызова `audio.*` или другого метода VK API.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct VkErrorDto {
    pub error_code: i64,
    pub error_msg: String,
    #[serde(default, deserialize_with = "deserialize_optional_id")]
    pub captcha_sid: Option<String>,
    #[serde(default)]
    pub captcha_img: Option<String>,
}

/// Успешный ответ Direct Auth.
///
/// `Debug` намеренно скрывает access token, чтобы секрет не попал в логи или panic-стек.
#[derive(Clone, Deserialize)]
pub struct VkAuthSuccessDto {
    pub access_token: String,
    #[serde(default)]
    pub expires_in: Option<u64>,
    pub user_id: i64,
    #[serde(default)]
    pub email: Option<String>,
}

impl fmt::Debug for VkAuthSuccessDto {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VkAuthSuccessDto")
            .field("access_token", &"[REDACTED]")
            .field("expires_in", &self.expires_in)
            .field("user_id", &self.user_id)
            .field("email", &self.email)
            .finish()
    }
}

/// Ответ Direct Auth: пароль верен, но требуется второй фактор.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct VkTwoFactorRequiredDto {
    pub error: String,
    #[serde(default)]
    pub error_description: Option<String>,
    #[serde(default)]
    pub error_type: Option<String>,
    pub validation_type: String,
    #[serde(default)]
    pub phone_mask: Option<String>,
    #[serde(default)]
    pub email_mask: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_id")]
    pub validation_sid: Option<String>,
    #[serde(default)]
    pub validation_resend: Option<String>,
}

impl VkTwoFactorRequiredDto {
    /// Возвращает адаптированные для UI (заголовок, подзаголовок, плейсхолдер)
    pub fn ui_hints(&self) -> (&'static str, String, &'static str) {
        if self.validation_type.contains("callreset") {
            let sub = match &self.phone_mask {
                Some(mask) => format!("Вам поступит звонок-сброс на номер {mask}. Введите последние 4 цифры входящего номера"),
                None => "Введите последние 4 цифры входящего номера звонящего".to_string(),
            };
            ("Звонок-сброс от VK", sub, "Последние 4 цифры")
        } else if self.validation_type.contains("email") || self.email_mask.is_some() {
            let sub = match &self.email_mask {
                Some(mask) => format!("Код подтверждения отправлен на {mask}"),
                None => "Проверьте входящие письма на вашей почте".to_string(),
            };
            ("Подтверждение по почте", sub, "Код из письма")
        } else if self.validation_type.contains("sms") || self.phone_mask.is_some() {
            let sub = match &self.phone_mask {
                Some(mask) => format!("Код отправлен в SMS на {mask}"),
                None => "Введите код из SMS".to_string(),
            };
            ("SMS-подтверждение", sub, "Код из SMS")
        } else if self.validation_type.contains("app") {
            (
                "Приложение-аутентификатор",
                "Введите код из VK OTP или Authenticator".to_string(),
                "6-значный код",
            )
        } else {
            let sub = self
                .error_description
                .as_deref()
                .unwrap_or("Введите одноразовый проверочный код")
                .to_string();
            ("Проверка безопасности", sub, "Код подтверждения")
        }
    }
}

/// Ответ Direct Auth с требованием капчи.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct VkCaptchaRequiredDto {
    pub error: String,
    #[serde(default)]
    pub error_description: Option<String>,
    #[serde(deserialize_with = "crate::api::models::deserialize_id")]
    pub captcha_sid: String,
    pub captcha_img: String,
}

/// Прочая ошибка Direct Auth.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct VkAuthErrorDto {
    pub error: String,
    #[serde(default)]
    pub error_description: Option<String>,
    #[serde(default)]
    pub error_type: Option<String>,
}

/// Ответ auth.validatePhone при запросе SMS-кода (контракт VK Android / VK X).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct VkValidatePhoneResponseDto {
    pub sid: String,
    #[serde(default)]
    pub validation_type: Option<String>,
    #[serde(default)]
    pub validation_resend: Option<String>,
    #[serde(default)]
    pub code_length: Option<u32>,
    #[serde(default)]
    pub delay: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VkValidatePhoneEnvelope {
    pub response: Option<VkValidatePhoneResponseDto>,
    pub error: Option<VkErrorDto>,
}

/// Все наблюдаемые ветки ответа `oauth.vk.com/token`.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum VkAuthResponse {
    Success(VkAuthSuccessDto),
    TwoFactorRequired(VkTwoFactorRequiredDto),
    CaptchaRequired(VkCaptchaRequiredDto),
    Error(VkAuthErrorDto),
}

/// Обложки разных размеров, вложенные в альбом или плейлист VK.
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
pub struct VkThumbDto {
    #[serde(default)]
    pub photo_68: Option<String>,
    #[serde(default)]
    pub photo_300: Option<String>,
    #[serde(default)]
    pub photo_600: Option<String>,
}

impl VkThumbDto {
    /// Выбирает наибольшую доступную обложку, сохраняя исходный HTTPS URL.
    pub fn best_url(&self) -> Option<&str> {
        self.photo_600
            .as_deref()
            .or(self.photo_300.as_deref())
            .or(self.photo_68.as_deref())
    }
}

/// Альбом внутри аудиозаписи VK.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct VkAlbumDto {
    pub id: i64,
    #[serde(default)]
    pub owner_id: Option<i64>,
    pub title: String,
    #[serde(default)]
    pub thumb: Option<VkThumbDto>,
}

impl VkAlbumDto {
    pub fn into_album(self) -> Album {
        let cover = self
            .thumb
            .as_ref()
            .and_then(VkThumbDto::best_url)
            .map(str::to_owned);
        let id = match self.owner_id {
            Some(owner_id) => format!("vk:{owner_id}_{}", self.id),
            None => format!("vk:album:{}", self.id),
        };

        Album {
            id,
            title: Some(self.title),
            og_image: cover,
            ..Default::default()
        }
    }
}

/// Исполнитель из `main_artists`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct VkArtistDto {
    #[serde(default, deserialize_with = "deserialize_optional_id")]
    pub id: Option<String>,
    pub name: String,
}

impl VkArtistDto {
    fn into_artist(self) -> Artist {
        Artist {
            id: self.id.unwrap_or_default(),
            name: self.name,
            cover: None,
        }
    }
}

/// Прямой поток аудиозаписи (чистый MP3 или AAC поток).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct VkAudioStreamDto {
    #[serde(rename = "type")]
    pub stream_type: String,
    pub url: String,
    #[serde(default)]
    pub fallback_url: Option<String>,
}

/// Аудиозапись VK в ответах медиатеки, поиска и VK Mix.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct VkAudioDto {
    pub id: i64,
    pub owner_id: i64,
    pub artist: String,
    pub title: String,
    #[serde(default)]
    pub subtitle: Option<String>,
    pub duration: u64,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub audio_streams: Option<Vec<VkAudioStreamDto>>,
    #[serde(default)]
    pub lyrics_id: Option<i64>,
    #[serde(default)]
    pub album: Option<VkAlbumDto>,
    #[serde(default)]
    pub main_artists: Option<Vec<VkArtistDto>>,
    #[serde(default)]
    pub is_explicit: Option<bool>,
}

impl VkAudioDto {
    /// Стабильный идентификатор трека, не пересекающийся с другими провайдерами.
    pub fn track_id(&self) -> String {
        format!("vk:{}_{}", self.owner_id, self.id)
    }

    /// Пустые URL считаются отсутствующими, а не пригодными для воспроизведения.
    /// Приоритет отдается прямым стабильным потокам:
    /// 1) `type == "flac"` / `"lossless"` из `audio_streams` (Lossless FLAC поток без потерь);
    /// 2) `type == "mp3"` из `audio_streams` (чистый MP3 320 кбит/с, без mpegts демультиплексора, идеальная перемотка и кэширование на диск);
    /// 3) `type == "aac"` из `audio_streams`;
    /// 4) фолбэк на `self.url` (m3u8 или обычный url).
    pub fn stream_url(&self) -> Option<&str> {
        if let Some(streams) = &self.audio_streams {
            if let Some(stream) = streams
                .iter()
                .find(|s| (s.stream_type == "flac" || s.stream_type == "lossless") && !s.url.trim().is_empty())
            {
                return Some(stream.url.trim());
            }
            if let Some(stream) = streams
                .iter()
                .find(|s| s.stream_type == "mp3" && !s.url.trim().is_empty())
            {
                return Some(stream.url.trim());
            }
            if let Some(stream) = streams
                .iter()
                .find(|s| s.stream_type == "aac" && !s.url.trim().is_empty())
            {
                return Some(stream.url.trim());
            }
        }
        self.url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
    }

    pub fn is_playable(&self) -> bool {
        let title_stub = self.title.contains("Аудио доступно на vk.com")
            || self.title.contains("в официальных приложениях ВКонтакте");
        let has_url = self.stream_url().is_some();
        !title_stub && has_url
    }

    /// Преобразует аудиозапись VK в общую модель трека «Волны».
    pub fn into_track(self) -> Track {
        let id = self.track_id();
        let available = Some(self.stream_url().is_some());
        let version = self
            .subtitle
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        let album = self.album.map(VkAlbumDto::into_album);
        let cover = album.as_ref().and_then(|album| album.og_image.clone());
        let mut artists: Vec<Artist> = self
            .main_artists
            .unwrap_or_default()
            .into_iter()
            .filter(|artist| !artist.name.trim().is_empty())
            .map(VkArtistDto::into_artist)
            .collect();

        if artists.is_empty() && !self.artist.trim().is_empty() {
            artists.push(Artist {
                id: String::new(),
                name: self.artist,
                cover: None,
            });
        }

        Track {
            id,
            title: self.title,
            version,
            artists,
            albums: album.into_iter().collect(),
            duration_ms: Some(self.duration.saturating_mul(1000)),
            cover_uri: None,
            og_image: cover,
            available,
            content_warning: self
                .is_explicit
                .unwrap_or(false)
                .then(|| "explicit".to_owned()),
            derived_colors: None,
            source: TrackSource::Vk,
        }
    }
}

/// Список аудиозаписей с общим количеством результатов.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct VkAudioListDto {
    pub count: u64,
    #[serde(default)]
    pub items: Vec<VkAudioDto>,
}

/// Плейлист VK. Альбомы моделируются отдельно через `VkAlbumDto`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct VkPlaylistDto {
    pub id: i64,
    pub owner_id: i64,
    pub title: String,
    #[serde(default)]
    pub count: Option<u64>,
    #[serde(default)]
    pub photo: Option<VkThumbDto>,
    #[serde(default)]
    pub audios: Vec<VkAudioDto>,
    #[serde(default)]
    pub access_key: Option<String>,
    #[serde(default)]
    pub create_time: Option<u64>,
    #[serde(default)]
    pub update_time: Option<u64>,
}

/// Список плейлистов с общим количеством результатов.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct VkPlaylistListDto {
    pub count: u64,
    #[serde(default)]
    pub items: Vec<VkPlaylistDto>,
}

/// Дерево настроек VK Mix.
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
pub struct VkStreamMixSettingsDto {
    #[serde(default)]
    pub categories: Vec<VkStreamMixSettingsCategoryDto>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct VkStreamMixSettingsCategoryDto {
    pub category_id: String,
    #[serde(default, alias = "name")]
    pub title: Option<String>,
    #[serde(default)]
    pub options: Vec<VkStreamMixSettingsOptionDto>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct VkStreamMixSettingsOptionDto {
    pub option_id: String,
    #[serde(default, alias = "name")]
    pub title: Option<String>,
}

/// Порция VK Mix и курсор следующей порции.
/// В зависимости от версии API и окружения VK может возвращать как объект `{"items": [...], "context": "..."}`,
/// так и прямой массив `[track1, track2, ...]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VkStreamMixAudiosDto {
    pub items: Vec<VkAudioDto>,
    pub context: Option<String>,
}

impl<'de> Deserialize<'de> for VkStreamMixAudiosDto {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct ObjectFormat {
            #[serde(default, alias = "audios")]
            items: Vec<VkAudioDto>,
            #[serde(default)]
            context: Option<String>,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Format {
            Object(ObjectFormat),
            Array(Vec<VkAudioDto>),
        }

        match Format::deserialize(deserializer)? {
            Format::Object(obj) => Ok(Self {
                items: obj.items,
                context: obj.context,
            }),
            Format::Array(items) => Ok(Self {
                items,
                context: None,
            }),
        }
    }
}

/// Текст песни VK с синхронизированными строками.
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
pub struct VkLyricsDto {
    #[serde(default)]
    pub timestamps: Vec<VkLyricTimestampDto>,
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct VkLyricTimestampDto {
    pub begin: u64,
    #[serde(default)]
    pub end: Option<u64>,
    pub line: String,
}

/// Числовые идентификаторы VK иногда приходят строками. Поддерживаем оба варианта.
fn deserialize_optional_id<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(value)) => Ok(Some(value)),
        Some(serde_json::Value::Number(value)) => Ok(Some(value.to_string())),
        Some(other) => Err(serde::de::Error::custom(format!(
            "expected string or number id, got {other}"
        ))),
    }
}

/// Ответ auth.validateAccount
#[derive(Debug, Clone, Deserialize, Default)]
pub struct VkValidateAccountResponse {
    #[serde(default)]
    pub is_phone: Option<bool>,
    #[serde(default)]
    pub is_email: Option<bool>,
    #[serde(default)]
    pub sid: Option<String>,
    #[serde(default)]
    pub pass_sid: Option<String>,
    #[serde(default)]
    pub login: Option<String>,
    #[serde(default)]
    pub next_step: Option<VkValidateNextStepDto>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct VkValidateNextStepDto {
    #[serde(default)]
    pub verification_method: Option<String>,
    #[serde(default)]
    pub has_another_verification_methods: Option<bool>,
    #[serde(default)]
    pub external_id: Option<String>,
}

/// Ответ ecosystem.checkOtp
#[derive(Debug, Clone, Deserialize, Default)]
pub struct VkCheckOtpResponse {
    #[serde(default)]
    pub sid: Option<String>,
    #[serde(default)]
    pub profile_exist: Option<bool>,
    #[serde(default)]
    pub can_skip_password: Option<bool>,
    #[serde(default)]
    pub next_step: Option<VkValidateNextStepDto>,
}

/// Ответ newsfeed.search с прикрепленными аудиовложениями
#[derive(Debug, Clone, Deserialize, Default)]
pub struct VkNewsfeedSearchResponse {
    #[serde(default)]
    pub items: Vec<VkNewsfeedItemDto>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct VkNewsfeedItemDto {
    #[serde(default)]
    pub attachments: Vec<VkAttachmentDto>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct VkAttachmentDto {
    #[serde(rename = "type")]
    pub attachment_type: String,
    #[serde(default)]
    pub audio: Option<VkAudioDto>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_url_prefers_flac_then_mp3_then_aac_then_fallback_url() {
        let json_flac = r#"{
            "id": 1,
            "owner_id": 2,
            "artist": "Test",
            "title": "Track",
            "duration": 120,
            "url": "https://cs.vkuseraudio.net/p1/track.m3u8",
            "audio_streams": [
                {
                    "type": "aac",
                    "url": "https://cs.vkuseraudio.net/p1/track.aac"
                },
                {
                    "type": "mp3",
                    "url": "https://cs.vkuseraudio.net/p1/track.mp3"
                },
                {
                    "type": "flac",
                    "url": "https://cs.vkuseraudio.net/p1/track.flac"
                }
            ]
        }"#;
        let audio_flac: VkAudioDto = serde_json::from_str(json_flac).expect("valid audio json");
        assert_eq!(audio_flac.stream_url(), Some("https://cs.vkuseraudio.net/p1/track.flac"));

        let json_lossless = r#"{
            "id": 1,
            "owner_id": 2,
            "artist": "Test",
            "title": "Track",
            "duration": 120,
            "url": "https://cs.vkuseraudio.net/p1/track.m3u8",
            "audio_streams": [
                {
                    "type": "lossless",
                    "url": "https://cs.vkuseraudio.net/p1/track.lossless"
                },
                {
                    "type": "mp3",
                    "url": "https://cs.vkuseraudio.net/p1/track.mp3"
                }
            ]
        }"#;
        let audio_lossless: VkAudioDto = serde_json::from_str(json_lossless).expect("valid audio json");
        assert_eq!(audio_lossless.stream_url(), Some("https://cs.vkuseraudio.net/p1/track.lossless"));

        let json = r#"{
            "id": 1,
            "owner_id": 2,
            "artist": "Test",
            "title": "Track",
            "duration": 120,
            "url": "https://cs.vkuseraudio.net/p1/track.m3u8",
            "audio_streams": [
                {
                    "type": "aac",
                    "url": "https://cs.vkuseraudio.net/p1/track.aac"
                },
                {
                    "type": "mp3",
                    "url": "https://cs.vkuseraudio.net/p1/track.mp3"
                }
            ]
        }"#;

        let audio: VkAudioDto = serde_json::from_str(json).expect("valid audio json");
        assert_eq!(audio.stream_url(), Some("https://cs.vkuseraudio.net/p1/track.mp3"));

        let json_aac_only = r#"{
            "id": 1,
            "owner_id": 2,
            "artist": "Test",
            "title": "Track",
            "duration": 120,
            "url": "https://cs.vkuseraudio.net/p1/track.m3u8",
            "audio_streams": [
                {
                    "type": "aac",
                    "url": "https://cs.vkuseraudio.net/p1/track.aac"
                }
            ]
        }"#;
        let audio_aac: VkAudioDto = serde_json::from_str(json_aac_only).expect("valid audio json");
        assert_eq!(audio_aac.stream_url(), Some("https://cs.vkuseraudio.net/p1/track.aac"));

        let json_fallback = r#"{
            "id": 1,
            "owner_id": 2,
            "artist": "Test",
            "title": "Track",
            "duration": 120,
            "url": "https://cs.vkuseraudio.net/p1/track.m3u8"
        }"#;
        let audio_fallback: VkAudioDto = serde_json::from_str(json_fallback).expect("valid audio json");
        assert_eq!(audio_fallback.stream_url(), Some("https://cs.vkuseraudio.net/p1/track.m3u8"));
    }
}

