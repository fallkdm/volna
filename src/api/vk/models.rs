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
    #[serde(default, deserialize_with = "deserialize_optional_id")]
    pub validation_sid: Option<String>,
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
    pub fn stream_url(&self) -> Option<&str> {
        self.url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
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
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct VkStreamMixAudiosDto {
    #[serde(default, alias = "audios")]
    pub items: Vec<VkAudioDto>,
    #[serde(default)]
    pub context: Option<String>,
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
