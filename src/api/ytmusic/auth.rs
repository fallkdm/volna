use std::time::{Duration, Instant};

/// Идентификатор клиента официального YouTube on TV приложения
pub const CLIENT_ID: &str = "861556708454-d6t45nm8nvuktk182okjk20r9cq9pk07.apps.googleusercontent.com";

/// Секретный ключ клиента официального YouTube on TV приложения
pub const CLIENT_SECRET: &str = "zxBYhnmFYyWQmFA4_fWahYWg";

/// Ответ Google Device Flow при запросе кода подтверждения устройства
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    #[serde(alias = "verification_uri")]
    pub verification_url: String,
    pub expires_in: u64,
    pub interval: u64,
}

/// Токен доступа Google OAuth
#[derive(Debug, Clone)]
pub struct GoogleToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Instant,
}

impl GoogleToken {
    /// Проверка истечения срока действия токена с 60-секундным буфером безопасности
    pub fn is_expired(&self) -> bool {
        Instant::now() + Duration::from_secs(60) >= self.expires_at
    }
}

/// Запрос кода устройства через Google Device Authorization Flow
pub async fn request_device_code(
    http: &reqwest::Client,
) -> Result<DeviceCodeResponse, Box<dyn std::error::Error + Send + Sync>> {
    let body = format!(
        "client_id={}&scope={}",
        CLIENT_ID,
        "https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fyoutube"
    );

    let resp = http
        .post("https://oauth2.googleapis.com/device/code")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await?
        .error_for_status()?
        .json::<DeviceCodeResponse>()
        .await?;

    Ok(resp)
}

/// Опрос сервера Google OAuth для проверки подтверждения кода пользователем
pub async fn poll_device_token(
    http: &reqwest::Client,
    device_code: &str,
) -> Result<Option<GoogleToken>, Box<dyn std::error::Error + Send + Sync>> {
    let body = format!(
        "client_id={}&client_secret={}&device_code={}&grant_type={}",
        CLIENT_ID,
        CLIENT_SECRET,
        device_code,
        "urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code"
    );

    let resp = http
        .post("https://oauth2.googleapis.com/token")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await?;

    if resp.status().is_success() {
        #[derive(serde::Deserialize)]
        struct TokenSuccessResponse {
            access_token: String,
            expires_in: u64,
            refresh_token: Option<String>,
        }

        let body = resp.json::<TokenSuccessResponse>().await?;
        let expires_at = Instant::now() + Duration::from_secs(body.expires_in);
        Ok(Some(GoogleToken {
            access_token: body.access_token,
            refresh_token: body.refresh_token,
            expires_at,
        }))
    } else {
        #[derive(serde::Deserialize)]
        struct TokenErrorResponse {
            error: String,
            #[serde(default)]
            error_description: Option<String>,
        }

        let err_body = resp.json::<TokenErrorResponse>().await.ok();
        if let Some(err) = err_body {
            if err.error == "authorization_pending" || err.error == "slow_down" {
                return Ok(None);
            }
            return Err(format!(
                "Ошибка авторизации устройства Google: {} ({:?})",
                err.error, err.error_description
            )
            .into());
        }
        Err("Неизвестная ошибка опроса токена Google OAuth".into())
    }
}

/// Обновление токена доступа Google OAuth по refresh_token
pub async fn refresh_access_token(
    http: &reqwest::Client,
    refresh_token: &str,
) -> Result<GoogleToken, Box<dyn std::error::Error + Send + Sync>> {
    let body = format!(
        "client_id={}&client_secret={}&refresh_token={}&grant_type=refresh_token",
        CLIENT_ID,
        CLIENT_SECRET,
        refresh_token
    );

    let resp = http
        .post("https://oauth2.googleapis.com/token")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await?
        .error_for_status()?;

    #[derive(serde::Deserialize)]
    struct RefreshResponse {
        access_token: String,
        expires_in: u64,
        refresh_token: Option<String>,
    }

    let body = resp.json::<RefreshResponse>().await?;
    let expires_at = Instant::now() + Duration::from_secs(body.expires_in);

    Ok(GoogleToken {
        access_token: body.access_token,
        refresh_token: body.refresh_token.or_else(|| Some(refresh_token.to_string())),
        expires_at,
    })
}

/// Извлекает SAPISID из строки cookie (ищет __Secure-3PAPISID= или SAPISID=)
pub fn extract_sapisid(cookie: &str) -> Option<String> {
    let clean = cookie.trim();
    let clean = clean
        .strip_prefix("Cookie:")
        .or_else(|| clean.strip_prefix("cookie:"))
        .unwrap_or(clean)
        .trim();
    let mut fallback_sapisid = None;

    for part in clean.split(';') {
        let part = part.trim();
        if let Some(val) = part.strip_prefix("__Secure-3PAPISID=") {
            let val = val.trim().trim_matches('"');
            if !val.is_empty() {
                return Some(val.to_string());
            }
        } else if let Some(val) = part.strip_prefix("SAPISID=") {
            let val = val.trim().trim_matches('"');
            if !val.is_empty() {
                fallback_sapisid = Some(val.to_string());
            }
        }
    }

    fallback_sapisid
}

/// Очищает строку Cookie от лишних префиксов и пробелов, сохраняя все параметры (включая LOGIN_INFO для авторизации профиля).
pub fn sanitize_cookie(cookie: &str) -> String {
    let clean = cookie.trim();
    let clean = clean
        .strip_prefix("Cookie:")
        .or_else(|| clean.strip_prefix("cookie:"))
        .unwrap_or(clean)
        .trim();

    clean
        .split(';')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

/// Генерирует заголовок SAPISIDHASH для авторизации InnerTube API по cookie
pub fn build_sapisid_hash(sapisid: &str, origin: &str) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    build_sapisid_hash_at(sapisid, origin, now)
}

/// Вспомогательная функция генерации SAPISIDHASH с заданным timestamp
pub fn build_sapisid_hash_at(sapisid: &str, origin: &str, timestamp: u64) -> String {
    let to_hash = format!("{timestamp} {sapisid} {origin}");
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(to_hash.as_bytes());
    let digest: String = hasher.finalize().iter().map(|b| format!("{:02x}", b)).collect();
    format!("SAPISIDHASH {timestamp}_{digest}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_sapisid() {
        let cookie_secure = "SID=abc; __Secure-3PAPISID=secret3papisid123; HSID=def";
        assert_eq!(
            extract_sapisid(cookie_secure),
            Some("secret3papisid123".to_string())
        );

        let cookie_plain = "Cookie: SID=abc; SAPISID=secretsapisid456; SSID=ghi";
        assert_eq!(
            extract_sapisid(cookie_plain),
            Some("secretsapisid456".to_string())
        );

        // При наличии обоих предпочтение отдается __Secure-3PAPISID
        let cookie_both = "SAPISID=plain; __Secure-3PAPISID=secure_preferred";
        assert_eq!(
            extract_sapisid(cookie_both),
            Some("secure_preferred".to_string())
        );

        let cookie_none = "foo=bar; baz=qux";
        assert_eq!(extract_sapisid(cookie_none), None);
    }

    #[test]
    fn test_build_sapisid_hash() {
        let sapisid = "test_sapisid";
        let origin = "https://music.youtube.com";
        let hash_str = build_sapisid_hash(sapisid, origin);
        assert!(hash_str.starts_with("SAPISIDHASH "));

        // Проверяем детерминированный хеш
        let ts = 1700000000u64;
        let deterministic = build_sapisid_hash_at(sapisid, origin, ts);
        use sha1::{Digest, Sha1};
        let mut hasher = Sha1::new();
        hasher.update(b"1700000000 test_sapisid https://music.youtube.com");
        let expected_digest: String = hasher.finalize().iter().map(|b| format!("{:02x}", b)).collect();
        assert_eq!(deterministic, format!("SAPISIDHASH 1700000000_{expected_digest}"));
    }
}
