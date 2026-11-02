use std::fs::{create_dir_all, remove_file};
use std::io::Write;
use std::path::{Path, PathBuf};

use zeroize::Zeroizing;

use super::models::{
    VkAuthErrorDto, VkAuthResponse, VkAuthSuccessDto, VkCaptchaRequiredDto, VkTwoFactorRequiredDto,
};

pub const VK_OAUTH_URL: &str = "https://oauth.vk.com/token";
pub const VK_CLIENT_ID: &str = "2685278";
pub const VK_CLIENT_SECRET: &str = "hHbZxrka2uZ6jB1inYsH";
pub const VK_API_VERSION: &str = "5.199";
pub const VK_USER_AGENT: &str =
    "KateMobileAndroid/115 lite-arm64-v8a (Android 14; SDK 34; arm64-v8a; Xiaomi 2201116PG; ru)";

const KEYRING_SERVICE: &str = "io.github.fallkdm.Volna";
const KEYRING_USER: &str = "vk";

/// Учётные данные живут только в памяти на время Direct Auth.
pub struct VkCredentials {
    pub username: String,
    pub password: Zeroizing<String>,
}

impl VkCredentials {
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into().trim().to_owned(),
            password: Zeroizing::new(password.into()),
        }
    }
}

/// Ответ на дополнительную проверку после первого запроса OAuth.
#[derive(Debug, Clone, Default)]
pub struct VkAuthChallengeAnswer {
    pub code: Option<String>,
    pub validation_sid: Option<String>,
    pub captcha_sid: Option<String>,
    pub captcha_key: Option<String>,
}

/// Конечный автомат Direct Auth, включая ветки, требующие действия пользователя.
#[derive(Debug)]
pub enum VkAuthOutcome {
    Success(VkAuthSuccessDto),
    TwoFactorRequired(VkTwoFactorRequiredDto),
    CaptchaRequired(VkCaptchaRequiredDto),
    Error(VkAuthErrorDto),
}

#[derive(Debug, thiserror::Error)]
pub enum VkAuthError {
    #[error("Укажите логин и пароль ВКонтакте")]
    InvalidCredentials,
    #[error("Ошибка сети VK OAuth: {0}")]
    Network(#[from] reqwest::Error),
    #[error("VK OAuth вернул HTTP {0}")]
    Http(reqwest::StatusCode),
    #[error("Некорректный ответ VK OAuth: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("Ошибка хранения токена VK: {0}")]
    Storage(String),
}

/// Выполняет Direct Auth. Пароль и ответ капчи не записываются в конфигурацию или логи.
pub async fn direct_auth(
    http: &reqwest::Client,
    credentials: &VkCredentials,
    challenge: &VkAuthChallengeAnswer,
) -> Result<VkAuthOutcome, VkAuthError> {
    if credentials.username.is_empty() || credentials.password.is_empty() {
        return Err(VkAuthError::InvalidCredentials);
    }

    let form = auth_form(credentials, challenge);
    let response = http
        .post(VK_OAUTH_URL)
        .header("User-Agent", VK_USER_AGENT)
        .header("X-VK-Android-Client", "new")
        .form(&form)
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;

    match serde_json::from_str::<VkAuthResponse>(&body) {
        Ok(VkAuthResponse::Success(success)) => Ok(VkAuthOutcome::Success(success)),
        Ok(VkAuthResponse::TwoFactorRequired(required)) => {
            Ok(VkAuthOutcome::TwoFactorRequired(required))
        }
        Ok(VkAuthResponse::CaptchaRequired(required)) => {
            Ok(VkAuthOutcome::CaptchaRequired(required))
        }
        Ok(VkAuthResponse::Error(error)) => Ok(VkAuthOutcome::Error(error)),
        Err(error) if status.is_success() => Err(VkAuthError::Decode(error)),
        Err(_) => Err(VkAuthError::Http(status)),
    }
}

fn auth_form<'a>(
    credentials: &'a VkCredentials,
    challenge: &'a VkAuthChallengeAnswer,
) -> Vec<(&'a str, &'a str)> {
    let mut form = vec![
        ("grant_type", "password"),
        ("client_id", VK_CLIENT_ID),
        ("client_secret", VK_CLIENT_SECRET),
        ("username", credentials.username.as_str()),
        ("password", credentials.password.as_str()),
        ("scope", "audio,offline"),
        ("2fa_supported", "1"),
        ("v", VK_API_VERSION),
    ];

    if let Some(code) = challenge.code.as_deref().filter(|value| !value.is_empty()) {
        form.push(("code", code));
    }
    if let Some(sid) = challenge
        .validation_sid
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        form.push(("validation_sid", sid));
    }
    if let Some(sid) = challenge
        .captcha_sid
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        form.push(("captcha_sid", sid));
    }
    if let Some(key) = challenge
        .captcha_key
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        form.push(("captcha_key", key));
    }

    form
}

/// Сохраняет токен в Secret Service, а при его недоступности — в изолированный файл `0600`.
pub fn save_vk_token(token: &str) -> Result<(), VkAuthError> {
    let token = token.trim();
    if token.is_empty() {
        return Err(VkAuthError::Storage("Пустой токен".to_owned()));
    }

    match keyring_entry().and_then(|entry| entry.set_password(token)) {
        Ok(()) => remove_fallback_token(&fallback_token_path()),
        Err(_) => write_fallback_token(&fallback_token_path(), token),
    }
}

/// Читает токен сначала из Secret Service, затем из изолированного fallback-файла.
pub fn load_vk_token() -> Option<String> {
    if let Ok(token) = keyring_entry().and_then(|entry| entry.get_password()) {
        if !token.trim().is_empty() {
            return Some(token);
        }
    }

    std::fs::read_to_string(fallback_token_path())
        .ok()
        .map(|token| token.trim().to_owned())
        .filter(|token| !token.is_empty())
}

/// Удаляет токен из обоих хранилищ. Второе удаление выполняется даже после ошибки первого.
pub fn clear_vk_token() -> Result<(), VkAuthError> {
    let mut first_error = None;

    if let Ok(entry) = keyring_entry() {
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(error) => {
                first_error = Some(VkAuthError::Storage(error.to_string()));
            }
        }
    }

    if let Err(error) = remove_fallback_token(&fallback_token_path()) {
        if first_error.is_none() {
            first_error = Some(error);
        }
    }

    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn keyring_entry() -> Result<keyring::Entry, keyring::Error> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
}

fn fallback_token_path() -> PathBuf {
    crate::api::auth::config_file_path()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("vk_token")
}

fn write_fallback_token(path: &Path, token: &str) -> Result<(), VkAuthError> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    if let Some(parent) = path.parent() {
        create_dir_all(parent).map_err(|error| VkAuthError::Storage(error.to_string()))?;
    }

    let temporary = path.with_extension("tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|error| VkAuthError::Storage(error.to_string()))?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|error| VkAuthError::Storage(error.to_string()))?;
    file.write_all(token.as_bytes())
        .map_err(|error| VkAuthError::Storage(error.to_string()))?;
    file.sync_all()
        .map_err(|error| VkAuthError::Storage(error.to_string()))?;
    std::fs::rename(&temporary, path).map_err(|error| VkAuthError::Storage(error.to_string()))?;
    Ok(())
}

fn remove_fallback_token(path: &Path) -> Result<(), VkAuthError> {
    match remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(VkAuthError::Storage(error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_form_contains_continuation_parameters() {
        let credentials = VkCredentials::new("+79990000000", "not-a-real-password");
        let challenge = VkAuthChallengeAnswer {
            code: Some("123456".to_owned()),
            validation_sid: Some("validation-test".to_owned()),
            captcha_sid: Some("captcha-test".to_owned()),
            captcha_key: Some("captcha-answer".to_owned()),
        };
        let form = auth_form(&credentials, &challenge);

        assert!(form.contains(&("grant_type", "password")));
        assert!(form.contains(&("scope", "audio,offline")));
        assert!(form.contains(&("2fa_supported", "1")));
        assert!(form.contains(&("code", "123456")));
        assert!(form.contains(&("validation_sid", "validation-test")));
        assert!(form.contains(&("captcha_sid", "captcha-test")));
        assert!(form.contains(&("captcha_key", "captcha-answer")));
    }

    #[test]
    fn fallback_token_is_private_and_replaced_atomically() {
        use std::os::unix::fs::PermissionsExt;

        let directory =
            std::env::temp_dir().join(format!("volna-vk-token-test-{}", uuid::Uuid::new_v4()));
        let path = directory.join("vk_token");
        write_fallback_token(&path, "first-test-token").expect("fallback token must be written");
        write_fallback_token(&path, "second-test-token").expect("fallback token must be replaced");

        let contents = std::fs::read_to_string(&path).expect("fallback token must be readable");
        assert_eq!(contents, "second-test-token");
        let mode = std::fs::metadata(&path)
            .expect("token metadata must exist")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);

        remove_fallback_token(&path).expect("fallback token must be removed");
        assert!(!path.exists());
        std::fs::remove_dir(&directory).expect("test directory must be removed");
    }
}
