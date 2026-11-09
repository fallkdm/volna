use std::fs::{create_dir_all, remove_file};
use std::io::Write;
use std::path::{Path, PathBuf};

use zeroize::Zeroizing;

use super::models::{
    VkAuthErrorDto, VkAuthResponse, VkAuthSuccessDto, VkCaptchaRequiredDto, VkTwoFactorRequiredDto,
};

pub const VK_OAUTH_URL: &str = "https://oauth.vk.com/token";
pub const VK_CLIENT_ID: &str = "3140623";
pub const VK_CLIENT_SECRET: &str = "VeWdmVclDCtn6ihuP1nt";
pub const VK_ANDROID_CLIENT_ID: &str = "2274003";
pub const VK_ANDROID_CLIENT_SECRET: &str = "hHbZxrka2uZ6jB1inYsH";
pub const VK_API_VERSION: &str = "5.199";
pub const VK_USER_AGENT: &str = "VK/195 (iPhone; iOS 17.5.1; Scale/3.00)";
pub const VK_ANDROID_USER_AGENT: &str =
    "VKAndroidApp/8.14.1-100136 (Android 14; SDK 34; arm64-v8a; Google Pixel 8; ru; 2400x1080)";

const KEYRING_SERVICE: &str = "io.github.fallkdm.Volna";
const KEYRING_USER: &str = "vk";

/// Последние сырые ответы VK — для кнопки «Копировать ответ VK» в диалоге.
static VK_TRACE: std::sync::LazyLock<std::sync::Mutex<Vec<String>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(Vec::new()));

fn trace(step: &str, body: &str) {
    if let Ok(mut log) = VK_TRACE.lock() {
        log.push(format!("[{step}] {body}"));
        if log.len() > 12 {
            log.remove(0);
        }
    }
}

/// Сырые ответы VK последних шагов (для диагностики пользователем).
pub fn diagnostics() -> String {
    VK_TRACE
        .lock()
        .map(|log| log.join("\n"))
        .unwrap_or_default()
}

/// Стабильный `device_id` процесса: VK использует его в VK ID-запросах.
fn device_id() -> &'static str {
    use std::sync::OnceLock;
    static DEVICE: OnceLock<String> = OnceLock::new();
    DEVICE.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        format!("{:016x}", nanos ^ (std::process::id() as u64))
    })
}

/// Анонимный токен VK (`auth.getAnonymToken`).
///
/// Без него VK отвечает на `ecosystem.*` и `auth.validateAccount`
/// ошибкой «method is unavailable by official client credential».
pub async fn get_anonym_token(http: &reqwest::Client) -> Result<String, VkAuthError> {
    /// Один и тот же токен на все шаги входа: sid привязан к анонимной сессии.
    /// TTL меньше, чем время жизни токена VK.
    static CACHE: std::sync::LazyLock<std::sync::Mutex<Option<(String, std::time::Instant)>>> =
        std::sync::LazyLock::new(|| std::sync::Mutex::new(None));
    const TTL: std::time::Duration = std::time::Duration::from_secs(10 * 60);

    if let Ok(cache) = CACHE.lock()
        && let Some((token, at)) = cache.as_ref()
        && at.elapsed() < TTL
    {
        return Ok(token.clone());
    }

    let params = vec![
        ("api_id", VK_ANDROID_CLIENT_ID),
        ("client_id", VK_ANDROID_CLIENT_ID),
        ("client_secret", VK_ANDROID_CLIENT_SECRET),
        ("https", "1"),
        ("device_id", device_id()),
        ("v", VK_API_VERSION),
    ];
    let sig = calculate_sig("auth.getAnonymToken", &params, VK_ANDROID_CLIENT_SECRET);
    let mut params = params;
    params.push(("sig", &sig));

    let response = http
        .post("https://api.vk.com/method/auth.getAnonymToken")
        .header("User-Agent", VK_ANDROID_USER_AGENT)
        .form(&params)
        .send()
        .await?;
    let body = response.text().await?;
    let json: serde_json::Value = serde_json::from_str(&body)?;
    if let Some(err) = json.get("error") {
        let msg = err
            .get("error_msg")
            .and_then(|v| v.as_str())
            .unwrap_or("VK не выдал анонимный токен");
        return Err(VkAuthError::Api(msg.to_string()));
    }
    let token = json
        .get("response")
        .and_then(|r| r.get("token"))
        .and_then(|t| t.as_str())
        .map(str::to_string)
        .ok_or_else(|| VkAuthError::Api("в ответе нет анонимного токена".to_string()))?;
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some((token.clone(), std::time::Instant::now()));
    }
    Ok(token)
}

/// Вычисляет контрольную подпись `sig` для методов API ВКонтакте.
pub fn calculate_sig(method: &str, params: &[(&str, &str)], secret: &str) -> String {
    use md5::{Digest, Md5};
    let mut sorted = params.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let mut s = format!("/method/{method}?");
    for (k, v) in sorted {
        s.push_str(k);
        s.push('=');
        s.push_str(v);
    }
    s.push_str(secret);
    let mut hasher = Md5::new();
    hasher.update(s.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
}


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
    pub login_sid: Option<String>,
    pub validation_sid: Option<String>,
    pub captcha_sid: Option<String>,
    pub captcha_key: Option<String>,
    pub resend: bool,
}

/// Результат шага валидации аккаунта (auth.validateAccount).
#[derive(Debug, Clone)]
pub enum VkValidateOutcome {
    /// Требуется код (почта или SMS)
    CodeRequired {
        sid: String,
        method: String,
        mask: String,
        /// Значение `tg_flow` из сессии: без него ecosystem.sendOtpEmail не отправит код
        tg_flow: Option<String>,
    },
    /// Сразу ввод пароля
    PasswordOnly {
        sid: String,
    },
    /// Требуется ввод капчи
    CaptchaRequired {
        captcha_sid: String,
        captcha_img: String,
    },
    /// Ошибка авторизации
    Error(String),
}

/// Результат проверки одноразового кода (ecosystem.checkOtp).
#[derive(Debug, Clone)]
pub enum VkCheckOtpOutcome {
    Success {
        sid: String,
        next_step: String,
        /// Токен обмена для auth.refreshTokens (VK ID отдаёт его после проверки кода)
        exchange_token: Option<String>,
        /// Хэш сессии из ответа checkOtp: нужен для обмена на токен
        auth_hash: Option<String>,
    },
    Error(String),
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
    /// Ответ VK API с ошибкой: текст показывается пользователю как есть
    #[error("VK: {0}")]
    Api(String),
}

/// Валидирует аккаунт VK ID: проверяет логин, инициирует отправку кода на почту/SMS или запрашивает капчу.
pub async fn validate_account(
    http: &reqwest::Client,
    login: &str,
    captcha: Option<(&str, &str)>,
) -> Result<VkValidateOutcome, VkAuthError> {
    let login = login.trim();
    if login.is_empty() {
        return Err(VkAuthError::InvalidCredentials);
    }

    // Первый запрос — как в VK X (auth_without_password), второй, если VK
    // настаивает на пароле, — во флоу tg_flow, где доступен код на почту.
    let mut response = validate_request(
        http,
        login,
        captcha,
        "auth_without_password",
        "email,sms,push,codegen,callreset,password",
    )
    .await?;


    // Второй запрос не шлём: VK отвечает капчей и это только усугубляет.
    parse_validate_response(http, &response).await
}

/// Один запрос `auth.validateAccount` с заданным flow_type.
async fn validate_request(
    http: &reqwest::Client,
    login: &str,
    captcha: Option<(&str, &str)>,
    flow_type: &str,
    supported_ways: &str,
) -> Result<serde_json::Value, VkAuthError> {
    let anonym = get_anonym_token(http).await?;
    let mut params = vec![
        ("api_id", VK_ANDROID_CLIENT_ID),
        ("access_token", anonym.as_str()),
        ("login", login),
        ("force_password", "0"),
        ("passkey_supported", "0"),
        ("supported_ways", supported_ways),
        ("flow_type", flow_type),
        ("sak_version", "1.112"),
        ("https", "1"),
        ("lang", "ru"),
        ("v", VK_API_VERSION),
    ];
    let c_sid;
    let c_key;
    if let Some((sid, key)) = captcha {
        c_sid = sid.to_string();
        c_key = key.to_string();
        params.push(("captcha_sid", &c_sid));
        params.push(("captcha_key", &c_key));
    }

    eprintln!("[vk] Отправка auth.validateAccount для login={login} ({flow_type})");
    let response = http
        .post("https://api.vk.com/method/auth.validateAccount")
        .header("User-Agent", VK_ANDROID_USER_AGENT)
        .form(&params)
        .send()
        .await?;
    let body = response.text().await?;
    eprintln!("[vk] Ответ auth.validateAccount: {body}");
    trace("auth.validateAccount", &body);
    Ok(serde_json::from_str(&body)?)
}

/// Разбор ответа validateAccount: капча, пароль или код.
async fn parse_validate_response(
    http: &reqwest::Client,
    json: &serde_json::Value,
) -> Result<VkValidateOutcome, VkAuthError> {
    if let Some(err) = json.get("error") {
        let error_code = err.get("error_code").and_then(|v| v.as_i64()).unwrap_or(0);
        if error_code == 14 {
            let captcha_sid = err
                .get("captcha_sid")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let captcha_img = err
                .get("captcha_img")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            return Ok(VkValidateOutcome::CaptchaRequired {
                captcha_sid,
                captcha_img,
            });
        }
        let msg = err
            .get("error_msg")
            .and_then(|v| v.as_str())
            .unwrap_or("Ошибка валидации аккаунта ВКонтакте");
        return Ok(VkValidateOutcome::Error(msg.to_string()));
    }

    let Some(resp) = json.get("response") else {
        return Ok(VkValidateOutcome::Error(
            "Не удалось разобрать ответ VK API".to_string(),
        ));
    };
    let sid = resp
        .get("sid")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let flow_name = resp.get("flow_name").and_then(|v| v.as_str()).unwrap_or("");
    if flow_name == "need_registration" {
        return Ok(VkValidateOutcome::Error(
            "Аккаунт с таким логином не найден в ВКонтакте".to_string(),
        ));
    }

    let next_step = resp.get("next_step");
    let mut method = next_step
        .and_then(|n| n.get("verification_method"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let mask = next_step
        .and_then(|n| n.get("external_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let tg_flow = resp
        .get("tg_flow")
        .and_then(|v| v.as_str())
        .or_else(|| {
            next_step
                .and_then(|n| n.get("tg_flow"))
                .and_then(|v| v.as_str())
        })
        .map(str::to_string);

    // Список способов у VK надёжнее, чем next_step: часто его нет вовсе.
    if method.as_deref() != Some("email") && !sid.is_empty() {
        if let Ok(json) = get_verification_methods(http, &sid).await {
            let response = json.get("response").unwrap_or(&json);
            let methods = response
                .get("methods")
                .or_else(|| response.get("ways"))
                .and_then(|m| m.as_array())
                .cloned()
                .unwrap_or_default();
            let names: Vec<String> = methods
                .iter()
                .filter_map(|item| {
                    item.get("name")
                        .or_else(|| item.get("method"))
                        .or_else(|| item.get("type"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                })
                .collect();
            if names.iter().any(|name| name == "email") {
                method = Some("email".to_string());
            } else if method.is_none() {
                if let Some(first) = names.first() {
                    method = Some(first.clone());
                }
            }
        }
    }

    match method.as_deref() {
        Some("email") | Some("sms") | Some("push") | Some("codegen") | Some("callreset") => {
            Ok(VkValidateOutcome::CodeRequired {
                sid,
                method: method.unwrap_or_else(|| "email".to_string()),
                mask,
                tg_flow,
            })
        }
        _ => Ok(VkValidateOutcome::PasswordOnly { sid }),
    }
}

/// Какие способы подтверждения VK реально разрешает для этой сессии.
pub async fn get_verification_methods(
    http: &reqwest::Client,
    sid: &str,
) -> Result<serde_json::Value, VkAuthError> {
    let anonym = get_anonym_token(http).await?;
    let params = vec![
        ("api_id", VK_ANDROID_CLIENT_ID),
        ("access_token", anonym.as_str()),
        ("sid", sid),
        ("https", "1"),
        ("v", VK_API_VERSION),
    ];
    let response = http
        .post("https://api.vk.com/method/ecosystem.getVerificationMethods")
        .header("User-Agent", VK_ANDROID_USER_AGENT)
        .form(&params)
        .send()
        .await?;
    let body = response.text().await?;
    eprintln!("[vk] Ответ ecosystem.getVerificationMethods: {body}");
    trace("ecosystem.getVerificationMethods", &body);
    Ok(serde_json::from_str(&body)?)
}

/// Проверяет одноразовый код (ecosystem.checkOtp), полученный на почту или по SMS.
pub async fn check_otp(
    http: &reqwest::Client,
    sid: &str,
    code: &str,
    method: &str,
) -> Result<VkCheckOtpOutcome, VkAuthError> {
    let code = code.trim();
    let anonym = get_anonym_token(http).await?;
    let params = vec![
        ("api_id", VK_ANDROID_CLIENT_ID),
        ("access_token", anonym.as_str()),
        ("sid", sid),
        ("code", code),
        ("verification_method", method),
        ("flow_type", "tg_flow"),
        ("sak_version", "1.142"),
        ("https", "1"),
        ("device_id", device_id()),
        ("v", VK_API_VERSION),
    ];

    eprintln!("[vk] Отправка ecosystem.checkOtp для sid={sid}, method={method}");
    let response = http
        .post("https://api.vk.com/method/ecosystem.checkOtp")
        .header("User-Agent", VK_ANDROID_USER_AGENT)
        .form(&params)
        .send()
        .await?;

    let body = response.text().await?;
    eprintln!("[vk] Ответ ecosystem.checkOtp: {body}");
    trace("ecosystem.checkOtp", &body);

    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
        if let Some(err) = json.get("error") {
            let msg = err
                .get("error_msg")
                .and_then(|v| v.as_str())
                .unwrap_or("Неверный код подтверждения");
            return Ok(VkCheckOtpOutcome::Error(msg.to_string()));
        }
        if let Some(resp) = json.get("response") {
            let next_sid = resp
                .get("sid")
                .and_then(|v| v.as_str())
                .unwrap_or(sid)
                .to_string();
            let next_step = resp
                .get("next_step")
                .and_then(|n| n.get("verification_method"))
                .and_then(|v| v.as_str())
                .unwrap_or("password")
                .to_string();
            let exchange_token = resp
                .get("exchange_token")
                .and_then(|v| v.as_str())
                .or_else(|| {
                    resp.get("session")
                        .and_then(|s| s.get("exchange_token"))
                        .and_then(|v| v.as_str())
                })
                .or_else(|| {
                    resp.get("tokens")
                        .and_then(|t| t.get("exchange_token"))
                        .and_then(|v| v.as_str())
                })
                .map(str::to_string);
            let auth_hash = resp
                .get("auth_hash")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            return Ok(VkCheckOtpOutcome::Success {
                sid: next_sid,
                next_step,
                exchange_token,
                auth_hash,
            });
        }
    }

    Ok(VkCheckOtpOutcome::Error(
        "Не удалось проверить код подтверждения".to_string(),
    ))
}

/// Отправляет код подтверждения на привязанную почту (`ecosystem.sendOtpEmail`).
///
/// В отличие от Direct Auth это не звонок-сброс: VK присылает 6-значный код письмом.
pub async fn send_otp_email(
    http: &reqwest::Client,
    sid: &str,
    tg_flow: Option<&str>,
) -> Result<(), VkAuthError> {
    let anonym = get_anonym_token(http).await?;
    let mut params = vec![
        ("api_id", VK_ANDROID_CLIENT_ID),
        ("access_token", anonym.as_str()),
        ("sid", sid),
        ("flow_type", "tg_flow"),
        ("sak_version", "1.142"),
        ("https", "1"),
        ("device_id", device_id()),
        ("v", VK_API_VERSION),
    ];
    if let Some(flow) = tg_flow.filter(|value| !value.is_empty()) {
        params.push(("tg_flow", flow));
    }
    let params = params;

    eprintln!("[vk] Отправка ecosystem.sendOtpEmail для sid={sid}");
    let response = http
        .post("https://api.vk.com/method/ecosystem.sendOtpEmail")
        .header("User-Agent", VK_USER_AGENT)
        .form(&params)
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    eprintln!("[vk] Ответ ecosystem.sendOtpEmail: {body}");
    trace("ecosystem.sendOtpEmail", &body);

    match serde_json::from_str::<serde_json::Value>(&body) {
        Ok(json) => {
            if let Some(err) = json.get("error") {
                let code = err.get("error_code").and_then(|v| v.as_i64()).unwrap_or(0);
                let subcode = err.get("error_subcode").and_then(|v| v.as_i64()).unwrap_or(0);
                let msg = err
                    .get("error_msg")
                    .and_then(|v| v.as_str())
                    .unwrap_or("не удалось отправить код на почту");
                let text = err
                    .get("error_text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                return Err(VkAuthError::Api(format!(
                    "{msg} (код {code}, подкод {subcode}){}{text}",
                    if text.is_empty() { "" } else { ": " }
                )));
            }
            Ok(())
        }
        Err(error) if status.is_success() => Err(VkAuthError::Decode(error)),
        Err(_) => Err(VkAuthError::Http(status)),
    }
}

/// Обменивает сессию VK ID на `exchange_token` (`auth.getExchangeToken`).
///
/// В VK X этот шаг идёт между проверкой кода и `auth.refreshTokens`, когда
/// в ответе checkOtp ещё нет готового access_token.
pub async fn get_exchange_token(
    http: &reqwest::Client,
    sid: &str,
    auth_hash: Option<&str>,
) -> Result<Option<String>, VkAuthError> {
    let anonym = get_anonym_token(http).await?;
    let mut params = vec![
        ("api_id", VK_ANDROID_CLIENT_ID),
        ("access_token", anonym.as_str()),
        ("sid", sid),
        ("https", "1"),
        ("device_id", device_id()),
        ("v", VK_API_VERSION),
    ];
    if let Some(hash) = auth_hash.filter(|value| !value.is_empty()) {
        params.push(("auth_hash", hash));
    }

    let response = http
        .post("https://api.vk.com/method/auth.getExchangeToken")
        .header("User-Agent", VK_ANDROID_USER_AGENT)
        .form(&params)
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    eprintln!("[vk] Ответ auth.getExchangeToken: {body}");
    trace("auth.getExchangeToken", &body);

    let json: serde_json::Value = match serde_json::from_str(&body) {
        Ok(json) => json,
        Err(error) if status.is_success() => return Err(VkAuthError::Decode(error)),
        Err(_) => return Err(VkAuthError::Http(status)),
    };
    if let Some(err) = json.get("error") {
        let msg = err
            .get("error_msg")
            .and_then(|v| v.as_str())
            .unwrap_or("VK не выдал exchange_token");
        eprintln!("[vk] getExchangeToken: {msg}");
        return Ok(None);
    }
    let response = json.get("response").unwrap_or(&json);
    Ok(response
        .get("exchange_token")
        .and_then(|v| v.as_str())
        .or_else(|| {
            response
                .get("tokens")
                .and_then(|t| t.get(0))
                .and_then(|t| t.get("exchange_token"))
                .and_then(|v| v.as_str())
        })
        .map(str::to_string))
}

/// Меняет `exchange_token` на полноценный токен официального клиента VK Android.
///
/// Именно у клиента `2274003` метод `audio.get` разблокирован, в отличие от
/// сторонних standalone-клиентов.
pub async fn refresh_tokens(
    http: &reqwest::Client,
    exchange_token: &str,
) -> Result<(String, i64), VkAuthError> {
    let params = vec![
        ("client_id", VK_ANDROID_CLIENT_ID),
        ("client_secret", VK_ANDROID_CLIENT_SECRET),
        ("exchange_tokens", exchange_token),
        ("scope", "all"),
        ("initiator", "expired_token"),
        ("v", VK_API_VERSION),
    ];
    let sig = calculate_sig("auth.refreshTokens", &params, VK_ANDROID_CLIENT_SECRET);
    let mut params = params;
    params.push(("sig", &sig));

    eprintln!("[vk] Отправка auth.refreshTokens");
    let response = http
        .post("https://api.vk.com/method/auth.refreshTokens")
        .header("User-Agent", VK_ANDROID_USER_AGENT)
        .form(&params)
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    eprintln!("[vk] Ответ auth.refreshTokens: {body}");

    let json: serde_json::Value = match serde_json::from_str(&body) {
        Ok(json) => json,
        Err(error) if status.is_success() => return Err(VkAuthError::Decode(error)),
        Err(_) => return Err(VkAuthError::Http(status)),
    };
    if let Some(err) = json.get("error") {
        let msg = err
            .get("error_msg")
            .and_then(|v| v.as_str())
            .unwrap_or("VK не выдал токен");
        return Err(VkAuthError::Api(msg.to_string()));
    }
    let response = json.get("response").unwrap_or(&json);

    // Ответ бывает плоским ({access_token}) или со списком токенов
    let token_obj = response
        .get("access_token")
        .map(|_| response)
        .or_else(|| response.get("tokens").and_then(|t| t.get(0)))
        .unwrap_or(response);
    let access_token = token_obj
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| VkAuthError::Api("в ответе нет access_token".to_string()))?
        .to_string();
    let user_id = token_obj
        .get("user_id")
        .and_then(|v| v.as_i64())
        .or_else(|| response.get("user_id").and_then(|v| v.as_i64()))
        .unwrap_or(0);

    let user_id = if user_id != 0 {
        user_id
    } else {
        fetch_user_id(http, &access_token).await.unwrap_or(0)
    };
    Ok((access_token, user_id))
}

/// ID пользователя по токену (если VK не вернул его вместе с токеном).
async fn fetch_user_id(http: &reqwest::Client, token: &str) -> Option<i64> {
    let url = format!(
        "https://api.vk.com/method/users.get?v={VK_API_VERSION}&access_token={token}"
    );
    let response = http
        .get(&url)
        .header("User-Agent", VK_ANDROID_USER_AGENT)
        .send()
        .await
        .ok()?;
    let json: serde_json::Value = response.json().await.ok()?;
    json.get("response")?
        .get(0)?
        .get("id")?
        .as_i64()
}

/// Выполняет Direct Auth для получения access_token.
pub async fn direct_auth(
    http: &reqwest::Client,
    credentials: &VkCredentials,
    challenge: &VkAuthChallengeAnswer,
) -> Result<VkAuthOutcome, VkAuthError> {
    let phone_confirmed = challenge
        .login_sid
        .as_deref()
        .is_some_and(|value| !value.is_empty());
    if credentials.username.is_empty()
        || (credentials.password.is_empty() && !phone_confirmed)
    {
        return Err(VkAuthError::InvalidCredentials);
    }

    let anonymous_token = get_anonym_token(http).await.ok();
    let form = auth_form(credentials, challenge, anonymous_token.as_deref());
    eprintln!("[vk] Отправка oauth.vk.com/token для пользователя {}", credentials.username);
    let response = http
        .post(VK_OAUTH_URL)
        .header("User-Agent", VK_ANDROID_USER_AGENT)
        .form(&form)
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    eprintln!("[vk] Ответ oauth.vk.com/token: {body}");
    trace("oauth.vk.com/token", &body);

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
    anonymous_token: Option<&'a str>,
) -> Vec<(&'a str, &'a str)> {
    let phone_confirmed = challenge
        .login_sid
        .as_deref()
        .is_some_and(|value| !value.is_empty());
    // VK X: после подтверждения телефона кодом grant_type становится
    // `phone_confirmation_sid` (а не `password`), иначе VK снова просит звонок.
    let grant_type = if phone_confirmed && credentials.password.is_empty() {
        "without_password"
    } else if phone_confirmed {
        "phone_confirmation_sid"
    } else {
        "password"
    };

    let mut form = vec![
        ("grant_type", grant_type),
        ("client_id", VK_ANDROID_CLIENT_ID),
        ("client_secret", VK_ANDROID_CLIENT_SECRET),
        ("username", credentials.username.as_str()),
        ("password", credentials.password.as_str()),
        // Как в VK X: один и тот же клиент, полный набор прав и trust-флаги.
        ("scope", "all"),
        ("2fa_supported", "1"),
        ("libverify_support", "1"),
        ("device_trusted_hash_support", "1"),
        ("flow_type", "tg_flow"),
        ("sak_version", "1.142"),
        ("supported_ways", "push,email"),
        ("v", VK_API_VERSION),
    ];

    if let Some(code) = challenge.code.as_deref().filter(|value| !value.is_empty()) {
        form.push(("code", code));
    }
    // `validation_sid` — только для продолжения звонка/2FA.
    if let Some(validation) = challenge
        .validation_sid
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        form.push(("validation_sid", validation));
    }
    // Подтверждённая кодом сессия передаётся в `sid`; grant_type уже
    // равен `phone_confirmation_sid` (см. выше).
    if let Some(login_sid) = challenge
        .login_sid
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        form.push(("sid", login_sid));
    }
    if let Some(token) = anonymous_token.filter(|value| !value.is_empty()) {
        form.push(("anonymous_token", token));
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
    if challenge.resend {
        // VK игнорирует один resend=1; force_sms просит код по SMS,
        // а не звонок-сброс (validation_resend: 2fa_email/2fa_sms).
        form.push(("resend", "1"));
        form.push(("force_sms", "1"));
    }

    form
}

/// Парсит access_token и user_id из введённой ссылки (blank.html#access_token=...) или строки токена.
/// Если user_id не найден в ссылке, запрашивает его через users.get.
pub async fn parse_and_validate_token(
    http: &reqwest::Client,
    input: &str,
) -> Result<(String, i64), String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Введите токен или скопированную ссылку".to_string());
    }

    let token = if let Some(pos) = input.find("access_token=") {
        let rest = &input[pos + "access_token=".len()..];
        let token_part = rest.split('&').next().unwrap_or(rest);
        token_part.trim().to_string()
    } else {
        input.to_string()
    };

    if token.is_empty() {
        return Err("Не удалось найти access_token в строке".to_string());
    }

    let user_id_from_url = if let Some(pos) = input.find("user_id=") {
        let rest = &input[pos + "user_id=".len()..];
        let id_part = rest.split('&').next().unwrap_or(rest);
        id_part.parse::<i64>().ok()
    } else {
        None
    };

    let user_id = if let Some(id) = user_id_from_url {
        id
    } else {
        #[derive(serde::Deserialize)]
        struct UsersGetResponse {
            response: Option<Vec<VkUserDto>>,
        }
        #[derive(serde::Deserialize)]
        struct VkUserDto {
            id: i64,
        }

        let url = format!("https://api.vk.com/method/users.get?v=5.199&access_token={token}");
        let resp = http
            .get(&url)
            .header("User-Agent", VK_USER_AGENT)
            .send()
            .await
            .map_err(|e| format!("Ошибка сети: {e}"))?;

        let body = resp.text().await.map_err(|e| format!("Ошибка ответа: {e}"))?;
        let parsed: UsersGetResponse = serde_json::from_str(&body)
            .map_err(|_| "Неверный токен или ошибка VK API".to_string())?;

        parsed
            .response
            .and_then(|users| users.into_iter().next())
            .map(|u| u.id)
            .ok_or_else(|| "Не удалось получить ID пользователя по токену".to_string())?
    };

    Ok((token, user_id))
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
            login_sid: Some("login-test".to_owned()),
            captcha_sid: Some("captcha-test".to_owned()),
            captcha_key: Some("captcha-answer".to_owned()),
            ..Default::default()
        };
        let form = auth_form(&credentials, &challenge, Some("anon-test"));

        assert!(form.contains(&("grant_type", "phone_confirmation_sid")));
        assert!(form.contains(&("client_id", VK_ANDROID_CLIENT_ID)));
        assert!(form.contains(&("client_secret", VK_ANDROID_CLIENT_SECRET)));
        assert!(form.contains(&("scope", "all")));
        assert!(form.contains(&("2fa_supported", "1")));
        assert!(form.contains(&("code", "123456")));
        assert!(form.contains(&("sid", "login-test")));
        assert!(!form.iter().any(|(key, _)| *key == "phone_confirmation_sid"));
        assert!(form.contains(&("validation_sid", "validation-test")));
        assert!(form.contains(&("anonymous_token", "anon-test")));
        assert!(form.contains(&("flow_type", "tg_flow")));
        assert!(form.contains(&("supported_ways", "push,email")));
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
