use std::fs::{create_dir_all, File};
use std::io::{Read, Write};
use std::path::PathBuf;
use serde::{Deserialize, Serialize};

/// Публичный OAuth client_id приложения Яндекс Музыки; то же значение в открытом проекте
/// MarshalX/yandex-music-token (MIT): web-app/src/api.ts и browser-extension/src/background.js.
pub const CLIENT_ID: &str = "23cabbbdc6cd418abb4b39c32c41195d";

/// Официальный OAuth URL для быстрого получения токена
pub const OAUTH_AUTHORIZE_URL: &str = "https://oauth.yandex.ru/authorize?response_type=token&client_id=23cabbbdc6cd418abb4b39c32c41195d";

fn default_crossfade_duration() -> u32 {
    3
}

/// Конфигурация приложения Волна
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// OAuth токен Яндекс Музыки
    pub token: Option<String>,
    /// Качество звука: `best` (по умолчанию), `balanced` или `economy`
    #[serde(default)]
    pub quality: Option<String>,
    /// Клиентский токен Genius (справка об исполнителях, если нет в Википедии)
    #[serde(default)]
    pub genius_token: Option<String>,
    /// Клипы на фоне Волны (превью-ролики из Яндекса, полупрозрачно); по умолчанию выключено
    #[serde(default)]
    pub clips: bool,
    /// Ключ API Last.fm (справка об исполнителях)
    #[serde(default)]
    pub lastfm_key: Option<String>,
    /// Последний проигранный трек: при запуске показывается вместо заглушки
    #[serde(default)]
    pub last_track: Option<serde_json::Value>,
    /// Статус в Discord (Rich Presence): что играет сейчас; по умолчанию выключено
    #[serde(default)]
    pub discord_rpc: bool,
    /// Application ID приложения из discord.com/developers (имя приложения Discord показывает в статусе)
    #[serde(default)]
    pub discord_client_id: Option<String>,
    /// Выбранная Яндекс Станция (`deviceId`): при запуске приложение подключается к ней само
    #[serde(default)]
    pub station_id: Option<String>,
    /// История поиска: открытые результаты (см. `history.rs`); элементы разбираются по одному
    #[serde(default)]
    pub history_items: Vec<serde_json::Value>,
    /// Плавный переход между треками (fade-out / fade-in)
    #[serde(default)]
    pub crossfade: bool,
    /// Длительность перехода в секундах (по умолчанию 3)
    #[serde(default = "default_crossfade_duration")]
    pub crossfade_duration: u32,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            token: None,
            quality: None,
            genius_token: None,
            clips: false,
            lastfm_key: None,
            last_track: None,
            discord_rpc: false,
            discord_client_id: None,
            station_id: None,
            history_items: Vec::new(),
            crossfade: false,
            crossfade_duration: 3,
        }
    }
}

/// Возвращает путь к файлу конфигурации (~/.config/volna/config.json)
pub fn config_file_path() -> PathBuf {
    let base_dir = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".config")
        });

    base_dir.join("volna").join("config.json")
}

/// Загружает конфигурацию из ~/.config/volna/config.json
pub fn load_config() -> Result<AppConfig, Box<dyn std::error::Error + Send + Sync>> {
    let path = config_file_path();
    if !path.exists() {
        return Ok(AppConfig::default());
    }

    let mut file = File::open(&path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;

    let config: AppConfig = serde_json::from_str(&contents)?;
    Ok(config)
}

/// Сохраняет конфигурацию в ~/.config/volna/config.json
pub fn save_config(config: &AppConfig) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let path = config_file_path();
    if let Some(parent) = path.parent() {
        create_dir_all(parent)?;
    }

    let json = serde_json::to_string_pretty(config)?;
    // в файле токен аккаунта: читать его может только владелец (права ставим и на уже созданный файл)
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(json.as_bytes())?;
    Ok(())
}

/// Получает сохраненный токен или None
pub fn load_token() -> Option<String> {
    load_config().ok().and_then(|cfg| cfg.token).filter(|t| !t.trim().is_empty())
}

/// Ключ Last.fm или None, если не задан
pub fn load_lastfm_key() -> Option<String> {
    load_config().ok().and_then(|c| c.lastfm_key).filter(|t| !t.trim().is_empty())
}

/// Сохраняет ключ Last.fm (пустая строка удаляет)
pub fn save_lastfm_key(key: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut config = load_config().unwrap_or_default();
    let key = key.trim();
    config.lastfm_key = if key.is_empty() { None } else { Some(key.to_string()) };
    save_config(&config)
}

/// Токен Genius или None, если не задан
pub fn load_genius_token() -> Option<String> {
    load_config().ok().and_then(|c| c.genius_token).filter(|t| !t.trim().is_empty())
}

/// Сохраняет токен Genius (пустая строка удаляет)
pub fn save_genius_token(token: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut config = load_config().unwrap_or_default();
    let token = token.trim();
    config.genius_token = if token.is_empty() { None } else { Some(token.to_string()) };
    save_config(&config)
}

/// Выбранное качество звука (`best`, `balanced`, `economy`); при отсутствии или ошибке — `best`
pub fn load_quality() -> String {
    match load_config().ok().and_then(|c| c.quality).as_deref() {
        Some(q @ ("balanced" | "economy")) => q.to_string(),
        _ => "best".to_string(),
    }
}

/// Сохраняет выбранное качество звука
pub fn save_quality(quality: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut config = load_config().unwrap_or_default();
    config.quality = Some(quality.to_string());
    save_config(&config)
}

/// Включены ли клипы на фоне
pub fn load_clips() -> bool {
    load_config().map(|c| c.clips).unwrap_or(false)
}

/// Настройки Discord: (включено, Application ID)
pub fn load_discord() -> (bool, Option<String>) {
    let c = load_config().unwrap_or_default();
    (c.discord_rpc, c.discord_client_id.filter(|i| !i.trim().is_empty()))
}

pub fn save_discord_rpc(on: bool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut config = load_config().unwrap_or_default();
    config.discord_rpc = on;
    save_config(&config)
}

pub fn save_discord_client_id(id: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut config = load_config().unwrap_or_default();
    config.discord_client_id = Some(id.trim().to_string()).filter(|i| !i.is_empty());
    save_config(&config)
}

pub fn save_clips(on: bool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut config = load_config().unwrap_or_default();
    config.clips = on;
    save_config(&config)
}

/// Настройки плавного перехода между треками: (включен, длительность в секундах)
pub fn load_crossfade() -> (bool, u32) {
    let c = load_config().unwrap_or_default();
    let duration = if c.crossfade_duration == 0 { 3 } else { c.crossfade_duration };
    (c.crossfade, duration)
}

/// Сохраняет настройки плавного перехода между треками
pub fn save_crossfade(enabled: bool, duration: u32) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut config = load_config().unwrap_or_default();
    config.crossfade = enabled;
    config.crossfade_duration = duration;
    save_config(&config)
}

/// Сохраняет токен в файл конфигурации
pub fn save_token(token: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let clean = extract_token_from_input(token);
    let mut config = load_config().unwrap_or_default();
    config.token = Some(clean);
    save_config(&config)
}

/// Удаляет сохраненный токен (выход из аккаунта)
pub fn clear_token() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut config = load_config().unwrap_or_default();
    config.token = None;
    save_config(&config)
}

/// Извлекает чистый токен из произвольного ввода пользователя
/// (например из URL https://music.yandex.ru/#access_token=y0_AgAAAA...&token_type=bearer)
pub fn extract_token_from_input(raw: &str) -> String {
    let trimmed = raw.trim();

    // Если скопирован URL или фрагмент с access_token=
    if let Some(pos) = trimmed.find("access_token=") {
        let after = &trimmed[pos + "access_token=".len()..];
        let token_part = after.split('&').next().unwrap_or(after);
        return token_part.trim().to_string();
    }

    // Если скопирован с префиксом OAuth
    if let Some(rest) = trimmed.strip_prefix("OAuth ") {
        return rest.trim().to_string();
    }

    trimmed.to_string()
}

/// Ответ эндпоинта Device Code
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    pub interval: u64,
    pub expires_in: u64,
}

/// Запрашивает device_code для авторизации по коду на ya.ru/device
pub async fn request_device_code() -> Result<DeviceCodeResponse, Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let resp = client
        .post("https://oauth.yandex.ru/device/code")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!("client_id={}", CLIENT_ID))
        .send()
        .await?
        .json::<DeviceCodeResponse>()
        .await?;

    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_token() {
        assert_eq!(
            extract_token_from_input("y0_AgAAAAABcDeF"),
            "y0_AgAAAAABcDeF"
        );
        assert_eq!(
            extract_token_from_input("https://music.yandex.ru/#access_token=y0_AgAAAA12345&token_type=bearer"),
            "y0_AgAAAA12345"
        );
        assert_eq!(
            extract_token_from_input("access_token=y0_SecretToken&other=1"),
            "y0_SecretToken"
        );
        assert_eq!(
            extract_token_from_input("OAuth y0_MyToken"),
            "y0_MyToken"
        );
    }

    #[test]
    fn test_crossfade_config() {
        let default_cfg = AppConfig::default();
        assert!(!default_cfg.crossfade);
        assert_eq!(default_cfg.crossfade_duration, 3);

        let json = "{}";
        let parsed: AppConfig = serde_json::from_str(json).unwrap();
        assert!(!parsed.crossfade);
        assert_eq!(parsed.crossfade_duration, 3);

        let json_custom = r#"{"crossfade": true, "crossfade_duration": 5}"#;
        let parsed_custom: AppConfig = serde_json::from_str(json_custom).unwrap();
        assert!(parsed_custom.crossfade);
        assert_eq!(parsed_custom.crossfade_duration, 5);
    }
}
