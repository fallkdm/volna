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
    load_config().ok().and_then(|cfg| cfg.token)
}

/// Сохраняет токен в файл конфигурации
pub fn save_token(token: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut config = load_config().unwrap_or_default();
    config.token = Some(token.trim().to_string());
    save_config(&config)
}
