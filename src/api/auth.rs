use std::fs::{create_dir_all, File};
use std::io::{Read, Write};
use std::path::PathBuf;
use serde::{Deserialize, Serialize};

/// Конфигурация приложения Волна
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    /// OAuth токен Яндекс Музыки
    pub token: Option<String>,
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
    let mut file = File::create(&path)?;
    file.write_all(json.as_bytes())?;
    Ok(())
}

/// Получает сохраненный токен или None
pub fn load_token() -> Option<String> {
    load_config().ok().and_then(|cfg| cfg.token).filter(|t| !t.trim().is_empty())
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
}
