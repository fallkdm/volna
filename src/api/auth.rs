use std::fs::{create_dir_all, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

/// Публичный OAuth client_id приложения Яндекс Музыки; то же значение в открытом проекте
/// MarshalX/yandex-music-token (MIT): web-app/src/api.ts и browser-extension/src/background.js.
pub const CLIENT_ID: &str = "23cabbbdc6cd418abb4b39c32c41195d";

/// Официальный OAuth URL для быстрого получения токена
pub const OAUTH_AUTHORIZE_URL: &str = "https://oauth.yandex.ru/authorize?response_type=token&client_id=23cabbbdc6cd418abb4b39c32c41195d";

fn default_crossfade_duration() -> u32 {
    3
}

fn default_pet_enabled() -> bool {
    true
}

/// Конфигурация приложения Волна
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// OAuth токен Яндекс Музыки
    pub token: Option<String>,
    /// Качество звука: `lossless` (по умолчанию), `best`, `balanced` или `economy`
    #[serde(default)]
    pub quality: Option<String>,
    /// Клиентский токен Genius (справка об исполнителях, если нет в Википедии)
    #[serde(default)]
    pub genius_token: Option<String>,
    /// Клипы на фоне Волны (превью-ролики из Яндекса, полупрозрачно); по умолчанию выключено
    #[serde(default)]
    pub clips: bool,
    /// Питомец на панели плеера: ходит по ней и прыгает в такт басу
    #[serde(default = "default_pet_enabled")]
    pub pet_enabled: bool,
    /// Облик питомца: "wave" (Волнёнок), "cat" (Кот) или "slime" (Слизень)
    #[serde(default)]
    pub pet_style: Option<String>,
    /// Ключ API Last.fm (справка об исполнителях)
    #[serde(default)]
    pub lastfm_key: Option<String>,
    /// Свой OAuth-клиент Google для входа в YouTube Music по коду (тип «TV and Limited Input»).
    /// Пусто — используется встроенный, который Google уже отозвал.
    #[serde(default)]
    pub ytmusic_oauth_client_id: Option<String>,
    #[serde(default)]
    pub ytmusic_oauth_client_secret: Option<String>,
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
    /// Стиль навигации: `wave` (по умолчанию, классический верхний бар) или `spotify` (левый сайдбар)
    #[serde(default)]
    pub nav_style: Option<String>,
    /// Стиль плеера: `floating` (по умолчанию, парящий остров) или `dock` (прижатый док)
    #[serde(default)]
    pub player_layout: Option<String>,
    /// Цветовой акцент: `dynamic` (по умолчанию), `spotify`, `ruby`, `sunset`, `violet`, `ocean`, `monochrome`
    #[serde(default)]
    pub theme_accent: Option<String>,
    /// Свернут ли левый сайдбар (компактный режим)
    #[serde(default)]
    pub sidebar_collapsed: bool,
    /// Активный музыкальный провайдер ("yandex" или "ytmusic")
    #[serde(default)]
    pub active_provider: Option<String>,
    /// Google OAuth refresh token для YouTube Music
    #[serde(default)]
    pub ytmusic_refresh_token: Option<String>,
    /// Browser Cookie для YouTube Music
    #[serde(default)]
    pub ytmusic_cookie: Option<String>,
    /// Индекс Google-аккаунта (`SESSION_INDEX` / `X-Goog-AuthUser`) для YouTube Music
    #[serde(default)]
    pub ytmusic_authuser: Option<String>,
    /// История поиска YouTube Music
    #[serde(default)]
    pub ytmusic_history_items: Vec<serde_json::Value>,
    /// История поиска VK Музыки
    #[serde(default)]
    pub vk_history_items: Vec<serde_json::Value>,
    /// Числовой ID аккаунта VK (для audio.getPlaylists)
    #[serde(default)]
    pub vk_user_id: Option<i64>,
    /// Недавно проигранное (свежие первыми): Волна не должна повторять это после перезапуска
    #[serde(default)]
    pub recent_played: Vec<String>,
    /// Сохраненная громкость пользователя (от 0.0 до 1.0, по умолчанию 0.85)
    #[serde(default)]
    pub volume: Option<f64>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            token: None,
            quality: None,
            genius_token: None,
            clips: false,
            pet_enabled: true,
            pet_style: None,
            lastfm_key: None,
            ytmusic_oauth_client_id: None,
            ytmusic_oauth_client_secret: None,
            last_track: None,
            discord_rpc: false,
            discord_client_id: None,
            station_id: None,
            history_items: Vec::new(),
            ytmusic_history_items: Vec::new(),
            vk_history_items: Vec::new(),
            crossfade: false,
            crossfade_duration: 3,
            nav_style: None,
            player_layout: None,
            theme_accent: None,
            sidebar_collapsed: false,
            active_provider: None,
            ytmusic_refresh_token: None,
            ytmusic_cookie: None,
            ytmusic_authuser: None,
            vk_user_id: None,
            recent_played: Vec::new(),
            volume: None,
        }
    }
}

static CONFIG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn is_cargo_test_binary() -> bool {
    cfg!(test)
        || std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().and_then(|p| p.file_name()).map(|n| n == "deps"))
            .unwrap_or(false)
}

fn config_file_path_with_env(is_test: bool, xdg: Option<&str>, home: Option<&str>) -> PathBuf {
    let base_dir = if let Some(xdg) = xdg.map(str::trim).filter(|v| !v.is_empty()) {
        PathBuf::from(xdg)
    } else if is_test {
        std::env::temp_dir().join(format!("volna-test-config-{}", std::process::id()))
    } else {
        PathBuf::from(home.unwrap_or(".")).join(".config")
    };

    base_dir.join("volna").join("config.json")
}

/// Возвращает путь к файлу конфигурации (~/.config/volna/config.json)
pub fn config_file_path() -> PathBuf {
    let xdg = std::env::var("XDG_CONFIG_HOME").ok();
    let home = std::env::var("HOME").ok();
    config_file_path_with_env(is_cargo_test_binary(), xdg.as_deref(), home.as_deref())
}

/// Атомарно записывает данные в `target` через уникальный временный файл с правами `0o600`.
fn write_atomic_0600(
    target: &Path,
    bytes: &[u8],
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let parent_dir = match target.parent() {
        Some(p) if !p.as_os_str().is_empty() => {
            create_dir_all(p)?;
            p
        }
        _ => Path::new("."),
    };

    let tmp = parent_dir.join(format!(
        "config.json.{}.{}.tmp",
        std::process::id(),
        TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));

    let write_res: Result<(), std::io::Error> = (|| {
        // в файле токен аккаунта: читать его может только владелец (права ставим и на уже созданный файл)
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    })();

    if let Err(err) = write_res {
        let _ = std::fs::remove_file(&tmp);
        return Err(Box::new(err));
    }

    if let Err(err) = std::fs::rename(&tmp, target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(Box::new(err));
    }

    Ok(())
}

/// Сохраняет копию повреждённого файла в `config.json.corrupted` (если она ещё не создана).
fn preserve_corrupted_copy(path: &Path) {
    let corrupted = path.with_extension("json.corrupted");
    if !corrupted.exists() {
        if let Ok(bytes) = std::fs::read(path) {
            let _ = write_atomic_0600(&corrupted, &bytes);
        }
    }
}

fn load_config_at_unlocked(
    path: &Path,
) -> Result<AppConfig, Box<dyn std::error::Error + Send + Sync>> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(AppConfig::default()),
        Err(err) => return Err(Box::new(err)),
    };

    let mut contents = String::new();
    file.read_to_string(&mut contents)?;

    let config: AppConfig = serde_json::from_str(&contents)?;
    Ok(config)
}

fn save_config_at_unlocked(
    path: &Path,
    config: &AppConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            create_dir_all(parent)?;
        }
    }

    // Перед заменой валидного конфига сохраняем его резервную копию с правами 0o600
    if let Ok(existing) = std::fs::read_to_string(path) {
        if serde_json::from_str::<AppConfig>(&existing).is_ok() {
            let bak = path.with_extension("json.bak");
            write_atomic_0600(&bak, existing.as_bytes())?;
        }
    }

    let json = serde_json::to_string_pretty(config)?;
    // Атомарная замена: запись «на месте» с truncate даёт пустой/обрезанный JSON
    // любому параллельному чтению (настройки читаются из фоновых задач).
    write_atomic_0600(path, json.as_bytes())
}

fn update_config_at_unlocked<F>(
    path: &Path,
    mutator: F,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    F: FnOnce(&mut AppConfig),
{
    let mut cfg = match load_config_at_unlocked(path) {
        Ok(cfg) => cfg,
        Err(err) => {
            if path.exists() {
                let bak = path.with_extension("json.bak");
                if bak.exists() {
                    if let Ok(recovered) = load_config_at_unlocked(&bak) {
                        recovered
                    } else {
                        preserve_corrupted_copy(path);
                        return Err(err);
                    }
                } else {
                    preserve_corrupted_copy(path);
                    return Err(err);
                }
            } else {
                return Err(err);
            }
        }
    };

    mutator(&mut cfg);
    save_config_at_unlocked(path, &cfg)
}

fn load_config_unlocked() -> Result<AppConfig, Box<dyn std::error::Error + Send + Sync>> {
    load_config_at_unlocked(&config_file_path())
}

fn save_config_unlocked(
    config: &AppConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    save_config_at_unlocked(&config_file_path(), config)
}

#[cfg(test)]
fn load_config_at(path: &Path) -> Result<AppConfig, Box<dyn std::error::Error + Send + Sync>> {
    let _guard = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    load_config_at_unlocked(path)
}

#[cfg(test)]
fn save_config_at(
    path: &Path,
    config: &AppConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _guard = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    save_config_at_unlocked(path, config)
}

#[cfg(test)]
fn update_config_at<F>(
    path: &Path,
    mutator: F,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    F: FnOnce(&mut AppConfig),
{
    let _guard = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    update_config_at_unlocked(path, mutator)
}

/// Загружает конфигурацию из ~/.config/volna/config.json
pub fn load_config() -> Result<AppConfig, Box<dyn std::error::Error + Send + Sync>> {
    let _guard = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    load_config_unlocked()
}

/// Сохраняет конфигурацию в ~/.config/volna/config.json
pub fn save_config(config: &AppConfig) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _guard = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    save_config_unlocked(config)
}

/// Атомарно читает, изменяет и сохраняет конфигурацию под `CONFIG_LOCK`,
/// восстанавливаясь из `.json.bak` при повреждении основного файла и не затирая его при полном сбое.
pub fn update_config<F>(mutator: F) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    F: FnOnce(&mut AppConfig),
{
    let _guard = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    update_config_at_unlocked(&config_file_path(), mutator)
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
    let key = key.trim();
    update_config(|config| {
        config.lastfm_key = if key.is_empty() { None } else { Some(key.to_string()) };
    })
}

/// Токен Genius или None, если не задан
pub fn load_genius_token() -> Option<String> {
    load_config().ok().and_then(|c| c.genius_token).filter(|t| !t.trim().is_empty())
}

/// Сохраняет токен Genius (пустая строка удаляет)
pub fn save_genius_token(token: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let token = token.trim();
    update_config(|config| {
        config.genius_token = if token.is_empty() { None } else { Some(token.to_string()) };
    })
}

/// Выбранное качество звука (`lossless`, `best`, `balanced`, `economy`); при отсутствии или ошибке — `lossless`
pub fn load_quality() -> String {
    match load_config().ok().and_then(|c| c.quality).as_deref() {
        Some(q @ ("lossless" | "best" | "balanced" | "economy")) => q.to_string(),
        _ => "lossless".to_string(),
    }
}

/// Сохраняет выбранное качество звука
pub fn save_quality(quality: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    update_config(|config| {
        config.quality = Some(quality.to_string());
    })
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
    update_config(|config| {
        config.discord_rpc = on;
    })
}

pub fn save_discord_client_id(id: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let id = Some(id.trim().to_string()).filter(|i| !i.is_empty());
    update_config(|config| {
        config.discord_client_id = id;
    })
}

pub fn save_clips(on: bool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    update_config(|config| {
        config.clips = on;
    })
}

/// Настройки плавного перехода между треками: (включен, длительность в секундах)
pub fn load_crossfade() -> (bool, u32) {
    let c = load_config().unwrap_or_default();
    let duration = if c.crossfade_duration == 0 { 3 } else { c.crossfade_duration };
    (c.crossfade, duration)
}

/// Сохраняет настройки плавного перехода между треками
pub fn save_crossfade(enabled: bool, duration: u32) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    update_config(|config| {
        config.crossfade = enabled;
        config.crossfade_duration = duration;
    })
}

/// Выбранный стиль навигации: `wave` (по умолчанию, верхний бар) или `spotify` (левый сайдбар)
pub fn load_nav_style() -> String {
    match load_config().ok().and_then(|c| c.nav_style).as_deref() {
        Some("spotify") => "spotify".to_string(),
        _ => "wave".to_string(),
    }
}

/// Сохраняет стиль навигации
pub fn save_nav_style(style: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    update_config(|config| {
        config.nav_style = Some(style.to_string());
    })
}

/// Выбранный стиль плеера: `floating` (по умолчанию, парящий остров) или `dock` (прижатый док)
pub fn load_player_layout() -> String {
    match load_config().ok().and_then(|c| c.player_layout).as_deref() {
        Some("dock") => "dock".to_string(),
        _ => "floating".to_string(),
    }
}

/// Сохраняет стиль плеера
pub fn save_player_layout(layout: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    update_config(|config| {
        config.player_layout = Some(layout.to_string());
    })
}

/// Выбранный акцент темы: `dynamic` (по умолчанию), `spotify`, `ruby`, `sunset`, `violet`, `ocean`, `monochrome`
pub fn load_theme_accent() -> String {
    match load_config().ok().and_then(|c| c.theme_accent).as_deref() {
        Some(a @ ("spotify" | "ruby" | "sunset" | "violet" | "ocean" | "monochrome")) => a.to_string(),
        _ => "dynamic".to_string(),
    }
}

/// Сохраняет акцент темы
pub fn save_theme_accent(accent: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    update_config(|config| {
        config.theme_accent = Some(accent.to_string());
    })
}

/// Питомец на плеере включён?
pub fn load_pet_enabled() -> bool {
    load_config().map(|c| c.pet_enabled).unwrap_or(true)
}

pub fn save_pet_enabled(enabled: bool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    update_config(|config| {
        config.pet_enabled = enabled;
    })
}

/// Облик питомца: "wave", "cat" или "slime"
pub fn load_pet_style() -> String {
    match load_config().ok().and_then(|c| c.pet_style).as_deref() {
        Some(style @ ("cat" | "slime")) => style.to_string(),
        _ => "wave".to_string(),
    }
}

pub fn save_pet_style(style: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    update_config(|config| {
        config.pet_style = Some(style.to_string());
    })
}

/// Состояние свернутости сайдбара
pub fn load_sidebar_collapsed() -> bool {
    load_config().map(|c| c.sidebar_collapsed).unwrap_or(false)
}

/// Сохраняет состояние свернутости сайдбара
pub fn save_sidebar_collapsed(collapsed: bool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    update_config(|config| {
        config.sidebar_collapsed = collapsed;
    })
}

/// Загружает сохраненную громкость пользователя (от 0.0 до 1.0, по умолчанию 0.85)
pub fn load_volume() -> f64 {
    load_config().ok().and_then(|c| c.volume).unwrap_or(0.85).clamp(0.0, 1.0)
}

/// Сохраняет громкость пользователя в файл конфигурации
pub fn save_volume(vol: f64) {
    let clamped = vol.clamp(0.0, 1.0);
    let _ = update_config(|cfg| {
        cfg.volume = Some(clamped);
    });
}

/// Сохраняет токен в файл конфигурации
pub fn save_token(token: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let clean = extract_token_from_input(token);
    update_config(|config| {
        config.token = Some(clean);
    })
}

/// Удаляет сохраненный токен (выход из аккаунта)
pub fn clear_token() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    update_config(|config| {
        config.token = None;
    })
}

/// Загружает сохраненного активного провайдера (по умолчанию ProviderId::Yandex)
pub fn load_active_provider() -> crate::api::provider::ProviderId {
    match load_config().ok().and_then(|c| c.active_provider).as_deref() {
        Some("ytmusic") => crate::api::provider::ProviderId::YouTubeMusic,
        Some("vk") => crate::api::provider::ProviderId::Vk,
        _ => crate::api::provider::ProviderId::Yandex,
    }
}

/// Сохраняет активного провайдера в конфигурацию
pub fn save_active_provider(provider: crate::api::provider::ProviderId) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let val = match provider {
        crate::api::provider::ProviderId::YouTubeMusic => "ytmusic",
        crate::api::provider::ProviderId::Vk => "vk",
        crate::api::provider::ProviderId::Yandex => "yandex",
    };
    update_config(|config| {
        config.active_provider = Some(val.to_string());
    })
}

/// Числовой ID нужен `audio.getPlaylists`; это не секрет и не заменяет токен.
pub fn load_vk_user_id() -> Option<i64> {
    load_config().ok().and_then(|config| config.vk_user_id)
}

pub fn save_vk_user_id(user_id: i64) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    update_config(|config| {
        config.vk_user_id = Some(user_id);
    })
}

pub fn clear_vk_user_id() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    update_config(|config| {
        config.vk_user_id = None;
    })
}

/// Загружает refresh-токен YouTube Music или None
pub fn load_ytmusic_refresh_token() -> Option<String> {
    load_config().ok().and_then(|c| c.ytmusic_refresh_token).filter(|t| !t.trim().is_empty())
}

/// Сохраняет refresh-токен YouTube Music
pub fn save_ytmusic_refresh_token(token: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let clean = token.trim();
    update_config(|config| {
        config.ytmusic_refresh_token = if clean.is_empty() { None } else { Some(clean.to_string()) };
    })
}

/// Удаляет refresh-токен YouTube Music
pub fn clear_ytmusic_refresh_token() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    update_config(|config| {
        config.ytmusic_refresh_token = None;
    })
}

/// Загружает сохраненную куку YouTube Music или None
pub fn load_ytmusic_cookie() -> Option<String> {
    load_config().ok().and_then(|c| c.ytmusic_cookie).filter(|t| !t.trim().is_empty())
}

/// Сохраняет полную куку YouTube Music (включая LOGIN_INFO для привязки к аккаунту и каналу пользователя)
/// Свой OAuth-клиент Google из настроек (id, secret); пустые строки, если не задан.
pub fn load_ytmusic_oauth_client() -> (Option<String>, Option<String>) {
    match load_config() {
        Ok(config) => (
            config
                .ytmusic_oauth_client_id
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty()),
            config
                .ytmusic_oauth_client_secret
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty()),
        ),
        Err(_) => (None, None),
    }
}

/// Сохраняет свой OAuth-клиент Google (пустая строка — сбросить)
pub fn save_ytmusic_oauth_client(id: Option<&str>, secret: Option<&str>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let clean_id = id
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);
    let clean_secret = secret
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);
    update_config(|config| {
        config.ytmusic_oauth_client_id = clean_id;
        config.ytmusic_oauth_client_secret = clean_secret;
    })
}

pub fn save_ytmusic_cookie(cookie: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let sanitized = crate::api::ytmusic::sanitize_cookie(cookie);
    update_config(|config| {
        config.ytmusic_cookie = if sanitized.is_empty() { None } else { Some(sanitized) };
    })
}

/// Удаляет куку YouTube Music и привязанный authuser
pub fn clear_ytmusic_cookie() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    update_config(|config| {
        config.ytmusic_cookie = None;
        config.ytmusic_authuser = None;
    })
}

/// Загружает сохранённый индекс профиля Google (`SESSION_INDEX`) для YouTube Music
pub fn load_ytmusic_authuser() -> Option<String> {
    load_config()
        .ok()
        .and_then(|c| c.ytmusic_authuser)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Сохраняет индекс профиля Google (`SESSION_INDEX`) для YouTube Music (пустая строка очищает)
pub fn save_ytmusic_authuser(authuser: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let clean = authuser.trim();
    update_config(|config| {
        config.ytmusic_authuser = if clean.is_empty() {
            None
        } else {
            Some(clean.to_string())
        };
    })
}

/// Очищает сохранённый индекс профиля Google (`SESSION_INDEX`) для YouTube Music
pub fn clear_ytmusic_authuser() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    update_config(|config| {
        config.ytmusic_authuser = None;
    })
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

    #[test]
    fn test_provider_and_ytmusic_config() {
        let json = r#"{"active_provider": "ytmusic", "ytmusic_refresh_token": "1//test_token"}"#;
        let cfg: AppConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.active_provider.as_deref(), Some("ytmusic"));
        assert_eq!(cfg.ytmusic_refresh_token.as_deref(), Some("1//test_token"));

        let default_cfg = AppConfig::default();
        assert_eq!(default_cfg.active_provider, None);
        assert_eq!(default_cfg.ytmusic_refresh_token, None);
    }

    #[test]
    fn test_ytmusic_cookie_config() {
        let json = r#"{"ytmusic_cookie": "SAPISID=12345; __Secure-3PAPISID=67890"}"#;
        let cfg: AppConfig = serde_json::from_str(json).unwrap();
        assert_eq!(
            cfg.ytmusic_cookie.as_deref(),
            Some("SAPISID=12345; __Secure-3PAPISID=67890")
        );

        let default_cfg = AppConfig::default();
        assert_eq!(default_cfg.ytmusic_cookie, None);
        assert_eq!(default_cfg.ytmusic_authuser, None);
    }

    #[test]
    fn test_ytmusic_authuser_persistence() {
        let json = r#"{"ytmusic_authuser": "3"}"#;
        let cfg: AppConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.ytmusic_authuser.as_deref(), Some("3"));

        let missing_json = r#"{"ytmusic_cookie": "SID=1"}"#;
        let parsed_missing: AppConfig = serde_json::from_str(missing_json).unwrap();
        assert_eq!(parsed_missing.ytmusic_authuser, None);

        // Проверяем save / load / clear через изолированный тестовый конфиг
        save_ytmusic_authuser(" 3 ").unwrap();
        assert_eq!(load_ytmusic_authuser().as_deref(), Some("3"));

        save_ytmusic_authuser("0").unwrap();
        assert_eq!(load_ytmusic_authuser().as_deref(), Some("0"));

        save_ytmusic_authuser("   ").unwrap();
        assert_eq!(load_ytmusic_authuser(), None);

        save_ytmusic_authuser("2").unwrap();
        assert_eq!(load_ytmusic_authuser().as_deref(), Some("2"));
        clear_ytmusic_authuser().unwrap();
        assert_eq!(load_ytmusic_authuser(), None);
    }

    #[test]
    fn test_volume_config() {
        let default_cfg = AppConfig::default();
        assert_eq!(default_cfg.volume, None);

        let json = "{}";
        let parsed: AppConfig = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.volume, None);

        let json_custom = r#"{"volume": 0.42}"#;
        let parsed_custom: AppConfig = serde_json::from_str(json_custom).unwrap();
        assert_eq!(parsed_custom.volume, Some(0.42));
    }

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(prefix: &str) -> Self {
            static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "volna-{}-{}-{}",
                prefix,
                std::process::id(),
                n
            ));
            let _ = std::fs::remove_dir_all(&path);
            create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn test_config_path_is_isolated_in_tests() {
        assert!(is_cargo_test_binary());

        let expected_isolated = std::env::temp_dir()
            .join(format!("volna-test-config-{}", std::process::id()))
            .join("volna")
            .join("config.json");

        assert_eq!(
            config_file_path_with_env(true, None, Some("/home/tester")),
            expected_isolated
        );
        assert_eq!(
            config_file_path_with_env(true, Some(""), Some("/home/tester")),
            expected_isolated
        );
        assert_eq!(
            config_file_path_with_env(true, Some("   "), Some("/home/tester")),
            expected_isolated
        );
        assert_eq!(
            config_file_path_with_env(true, Some("/tmp/custom-xdg"), Some("/home/tester")),
            PathBuf::from("/tmp/custom-xdg").join("volna").join("config.json")
        );

        assert_eq!(
            config_file_path_with_env(false, None, Some("/home/tester")),
            PathBuf::from("/home/tester")
                .join(".config")
                .join("volna")
                .join("config.json")
        );
        assert_eq!(
            config_file_path_with_env(false, Some("   "), Some("/home/tester")),
            PathBuf::from("/home/tester")
                .join(".config")
                .join("volna")
                .join("config.json")
        );
        assert_eq!(
            config_file_path_with_env(false, Some("/tmp/custom-xdg"), Some("/home/tester")),
            PathBuf::from("/tmp/custom-xdg").join("volna").join("config.json")
        );
        assert_eq!(
            config_file_path_with_env(false, None, None),
            PathBuf::from(".").join(".config").join("volna").join("config.json")
        );

        if let Ok(home) = std::env::var("HOME") {
            let real_dir = PathBuf::from(&home).join(".config").join("volna");
            assert!(!config_file_path_with_env(true, None, Some(&home)).starts_with(&real_dir));
            if std::env::var("XDG_CONFIG_HOME")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .is_none()
            {
                let active_path = config_file_path();
                assert!(!active_path.starts_with(&real_dir));
                assert_eq!(active_path, expected_isolated);
            }
        }
    }

    #[test]
    fn test_concurrent_update_config_preserves_fields_and_backup() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TestDir::new("concurrent-cfg");
        let path = dir.path().join("volna").join("config.json");

        let mut initial = AppConfig::default();
        initial.token = Some("secret-yandex-token".to_string());
        initial.quality = Some("lossless".to_string());
        initial.volume = Some(0.5);
        save_config_at(&path, &initial).unwrap();

        let mut handles = Vec::new();
        for tid in 0..8usize {
            let path = path.clone();
            handles.push(std::thread::spawn(move || {
                for iter in 0..10usize {
                    update_config_at(&path, |cfg| {
                        cfg.recent_played.push(format!("t{}-i{}", tid, iter));
                        match tid {
                            0 => cfg.clips = true,
                            1 => cfg.crossfade = true,
                            2 => cfg.pet_enabled = false,
                            3 => cfg.sidebar_collapsed = true,
                            4 => cfg.discord_rpc = true,
                            5 => cfg.nav_style = Some("spotify".to_string()),
                            6 => cfg.player_layout = Some("dock".to_string()),
                            7 => cfg.volume = Some(0.75),
                            _ => unreachable!(),
                        }
                    })
                    .unwrap();
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        let final_cfg = load_config_at(&path).unwrap();
        assert_eq!(final_cfg.token.as_deref(), Some("secret-yandex-token"));
        assert_eq!(final_cfg.quality.as_deref(), Some("lossless"));
        assert!(final_cfg.clips);
        assert!(final_cfg.crossfade);
        assert!(!final_cfg.pet_enabled);
        assert!(final_cfg.sidebar_collapsed);
        assert!(final_cfg.discord_rpc);
        assert_eq!(final_cfg.nav_style.as_deref(), Some("spotify"));
        assert_eq!(final_cfg.player_layout.as_deref(), Some("dock"));
        assert_eq!(final_cfg.volume, Some(0.75));
        assert_eq!(final_cfg.recent_played.len(), 80);
        for tid in 0..8usize {
            for iter in 0..10usize {
                assert!(final_cfg.recent_played.contains(&format!("t{}-i{}", tid, iter)));
            }
        }

        let bak = path.with_extension("json.bak");
        assert!(bak.exists());
        let bak_cfg = load_config_at(&bak).unwrap();
        assert_eq!(bak_cfg.token.as_deref(), Some("secret-yandex-token"));

        let path_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let bak_mode = std::fs::metadata(&bak).unwrap().permissions().mode() & 0o777;
        assert_eq!(path_mode, 0o600);
        assert_eq!(bak_mode, 0o600);

        let leftover_tmp = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().ends_with(".tmp"));
        assert!(!leftover_tmp);
    }

    #[test]
    fn test_corrupted_config_recovers_from_backup_and_never_wipes() {
        use std::os::unix::fs::PermissionsExt;

        // 1. Повреждён основной config.json, но .bak валиден -> восстанавливаем токен из .bak
        let dir1 = TestDir::new("recover-bak");
        let path1 = dir1.path().join("config.json");

        let mut cfg1 = AppConfig::default();
        cfg1.token = Some("keep-my-token".to_string());
        cfg1.volume = Some(0.42);
        save_config_at(&path1, &cfg1).unwrap();
        update_config_at(&path1, |c| c.crossfade = true).unwrap();
        assert!(path1.with_extension("json.bak").exists());

        std::fs::write(&path1, "{ broken json").unwrap();
        assert!(load_config_at(&path1).is_err());

        update_config_at(&path1, |c| c.clips = true).unwrap();
        let recovered = load_config_at(&path1).unwrap();
        assert_eq!(recovered.token.as_deref(), Some("keep-my-token"));
        assert!(recovered.clips);
        assert_eq!(recovered.volume, Some(0.42));
        assert!(!recovered.crossfade);

        // 2. Ни основного, ни .bak нет (основной битый, .bak отсутствует) -> .corrupted и Err, не затирая файл
        let dir2 = TestDir::new("corrupt-no-bak");
        let path2 = dir2.path().join("config.json");
        let broken_raw = "{not-valid-json";
        std::fs::write(&path2, broken_raw).unwrap();

        let res = update_config_at(&path2, |c| {
            c.token = Some("must-not-overwrite".to_string());
        });
        assert!(res.is_err());
        assert_eq!(std::fs::read_to_string(&path2).unwrap(), broken_raw);

        let corrupted = path2.with_extension("json.corrupted");
        assert!(corrupted.exists());
        assert_eq!(std::fs::read_to_string(&corrupted).unwrap(), broken_raw);
        let corrupted_mode = std::fs::metadata(&corrupted).unwrap().permissions().mode() & 0o777;
        assert_eq!(corrupted_mode, 0o600);

        // Повторный вызов не перезаписывает уже сохранённый .corrupted
        let another_broken = "{another-broken";
        std::fs::write(&path2, another_broken).unwrap();
        assert!(update_config_at(&path2, |c| c.clips = true).is_err());
        assert_eq!(std::fs::read_to_string(&path2).unwrap(), another_broken);
        assert_eq!(std::fs::read_to_string(&corrupted).unwrap(), broken_raw);

        // 3. Основной битый и .bak тоже битый -> сохраняется .corrupted и возвращается Err
        let dir3 = TestDir::new("corrupt-bad-bak");
        let path3 = dir3.path().join("config.json");
        let broken_main = "{broken-main";
        std::fs::write(&path3, broken_main).unwrap();
        std::fs::write(path3.with_extension("json.bak"), "{broken-bak").unwrap();

        let res3 = update_config_at(&path3, |c| c.clips = true);
        assert!(res3.is_err());
        assert_eq!(std::fs::read_to_string(&path3).unwrap(), broken_main);
        assert_eq!(
            std::fs::read_to_string(path3.with_extension("json.corrupted")).unwrap(),
            broken_main
        );
    }
}
