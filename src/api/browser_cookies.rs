//! Импорт сессии YouTube Music из локального браузера.
//!
//! Кука браузера — единственный способ входа без заведения OAuth-клиента.
//! Firefox хранит значения открытым текстом, Chromium-семейство — AES-128-CBC
//! с ключом из keyring (или фиксированным `peanuts`, если keyring недоступен).
//! Приложение забирает свежую сессию при старте, поэтому «кука протухла»
//! лечится перезапуском, а не походом в DevTools.

use std::path::{Path, PathBuf};

/// Куки, без которых сессия YouTube Music не считается авторизованной.
const AUTH_COOKIE_NAMES: &[&str] = &[
    "SID",
    "HSID",
    "SSID",
    "APISID",
    "SAPISID",
    "__Secure-1PSID",
    "__Secure-3PSID",
    "__Secure-1PAPISID",
    "__Secure-3PAPISID",
    "__Secure-1PSIDTS",
    "__Secure-3PSIDTS",
    "LOGIN_INFO",
    "VISITOR_INFO1_LIVE",
];

/// Готовая сессия и подпись источника (для лога и статуса в UI).
pub struct ImportedCookie {
    pub cookie: String,
    pub source: String,
}

/// Пробует Firefox, затем Chromium-семейство.
pub fn import_youtube_cookie() -> Option<ImportedCookie> {
    firefox_cookie().or_else(chromium_cookie)
}

fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()))
}

/// Собирает заголовок Cookie; без SID/PSID это гостевая сессия и она не нужна.
fn cookie_header(entries: &[(String, String)]) -> Option<String> {
    let has_session = entries.iter().any(|(name, value)| {
        !value.is_empty()
            && matches!(
                name.as_str(),
                "SID" | "__Secure-1PSID" | "__Secure-3PSID"
            )
    });
    if !has_session {
        return None;
    }
    let mut seen = std::collections::HashSet::new();
    let mut header = String::new();
    for (name, value) in entries {
        if value.is_empty() || !seen.insert(name.clone()) {
            continue;
        }
        if !header.is_empty() {
            header.push_str("; ");
        }
        header.push_str(name);
        header.push('=');
        header.push_str(value);
    }
    (!header.is_empty()).then_some(header)
}

/// Копия базы: браузер держит файл открытым, читать оригинал нельзя.
fn copy_db(path: &Path) -> Option<PathBuf> {
    let file_name = path.file_name()?.to_string_lossy().to_string();
    let dest = std::env::temp_dir().join(format!(
        "volna-cookies-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
        file_name
    ));
    std::fs::copy(path, &dest).ok()?;
    Some(dest)
}

fn open_readonly(path: &Path) -> Option<rusqlite::Connection> {
    rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()
}

// ───────────────────────────── Firefox (открытый текст) ─────────────────────────────

fn firefox_databases() -> Vec<PathBuf> {
    let home = home_dir();
    let bases = [
        home.join(".mozilla/firefox"),
        home.join(".var/app/org.mozilla.firefox/.mozilla/firefox"),
        home.join("snap/firefox/common/.mozilla/firefox"),
    ];
    let mut databases = Vec::new();
    for base in bases {
        let Ok(entries) = std::fs::read_dir(&base) else {
            continue;
        };
        for entry in entries.flatten() {
            let db = entry.path().join("cookies.sqlite");
            if db.is_file() {
                databases.push(db);
            }
        }
    }
    databases
}

fn firefox_cookie() -> Option<ImportedCookie> {
    for db in firefox_databases() {
        let Some(tmp) = copy_db(&db) else { continue };
        let Some(connection) = open_readonly(&tmp) else {
            let _ = std::fs::remove_file(&tmp);
            continue;
        };
        let mut entries: Vec<(String, String)> = Vec::new();
        if let Ok(mut statement) = connection.prepare(
            "select name, value from moz_cookies \
             where (host like '%youtube.com' or host like '%google.com') and value <> ''",
        ) && let Ok(rows) = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }) {
            entries.extend(
                rows.flatten()
                    .filter(|(name, _)| AUTH_COOKIE_NAMES.contains(&name.as_str())),
            );
        }
        drop(connection);
        let _ = std::fs::remove_file(&tmp);
        if let Some(cookie) = cookie_header(&entries) {
            return Some(ImportedCookie {
                cookie,
                source: format!("Firefox ({})", db.display()),
            });
        }
    }
    None
}

// ───────────────────────── Chromium (AES-128-CBC + keyring) ─────────────────────────

fn chromium_databases() -> Vec<PathBuf> {
    let home = home_dir();
    let bases = [
        home.join(".config/google-chrome"),
        home.join(".config/chromium"),
        home.join(".config/BraveSoftware/Brave-Browser"),
        home.join(".config/microsoft-edge"),
        home.join(".config/vivaldi"),
        home.join(".var/app/com.google.Chrome/config/google-chrome"),
        home.join(".var/app/org.chromium.Chromium/config/chromium"),
    ];
    let mut databases = Vec::new();
    for base in bases {
        let Ok(entries) = std::fs::read_dir(&base) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name != "Default" && !name.starts_with("Profile") {
                continue;
            }
            let db = entry.path().join("Cookies");
            if db.is_file() {
                databases.push(db);
            }
        }
    }
    databases
}

/// Ключ Chrome: из Secret Service, иначе фиксированный «peanuts».
fn chromium_password() -> String {
    for application in ["chrome", "Chrome Safe Storage", "chromium"] {
        let output = std::process::Command::new("secret-tool")
            .args(["lookup", "application", application])
            .output();
        if let Ok(output) = output {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !text.is_empty() {
                return text;
            }
        }
    }
    "peanuts".to_string()
}

/// PBKDF2-HMAC-SHA1 с одной итерацией — так Chrome выводит ключ из пароля.
fn pbkdf2_sha1_key(password: &[u8], salt: &[u8], key_len: usize) -> Vec<u8> {
    use sha1::{Digest, Sha1};
    fn hmac_sha1(key: &[u8], message: &[u8]) -> [u8; 20] {
        let mut padded = [0u8; 64];
        if key.len() > 64 {
            let digest = Sha1::digest(key);
            padded[..20].copy_from_slice(&digest);
        } else {
            padded[..key.len()].copy_from_slice(key);
        }
        let mut inner = Vec::with_capacity(64 + message.len());
        let mut outer = Vec::with_capacity(64 + 20);
        for byte in padded {
            inner.push(byte ^ 0x36);
            outer.push(byte ^ 0x5c);
        }
        inner.extend_from_slice(message);
        let inner_hash = Sha1::digest(&inner);
        outer.extend_from_slice(&inner_hash);
        let out = Sha1::digest(&outer);
        let mut result = [0u8; 20];
        result.copy_from_slice(&out);
        result
    }
    let mut message = salt.to_vec();
    message.extend_from_slice(&1u32.to_be_bytes());
    hmac_sha1(password, &message)[..key_len].to_vec()
}

/// Расшифровка значения куки Chromium. Значение может быть с 32-байтовым
/// префиксом SHA256(host_key) — выбираем тот вариант, который похож на текст.
fn decrypt_chromium(blob: &[u8], password: &str) -> Option<String> {
    if blob.len() < 16 {
        return None;
    }
    let data = if blob.starts_with(b"v10") || blob.starts_with(b"v11") {
        &blob[3..]
    } else {
        blob
    };
    if data.is_empty() || data.len() % 16 != 0 {
        return None;
    }
    let key = pbkdf2_sha1_key(password.as_bytes(), b"saltysalt", 16);
    let mut buffer = data.to_vec();
    let cipher = cbc::Decryptor::<aes::Aes128>::new_from_slices(&key, &[b' '; 16]).ok()?;
    use aes::cipher::block_padding::NoPadding;
    use aes::cipher::{BlockDecryptMut, KeyIvInit};
    let plain = cipher.decrypt_padded_mut::<NoPadding>(&mut buffer).ok()?;

    let printable = |bytes: &[u8]| {
        !bytes.is_empty()
            && bytes
                .iter()
                .all(|byte| *byte == b'\t' || (0x20..=0x7e).contains(byte))
    };
    // Chrome шифрует с PKCS7-дополнением, а в новых версиях приклеивает
    // 32-байтовый SHA256(host_key) перед значением — пробуем оба варианта.
    let strip_padding = |bytes: &[u8]| -> Vec<u8> {
        let Some(&last) = bytes.last() else {
            return Vec::new();
        };
        let pad = last as usize;
        if (1..=16).contains(&pad)
            && bytes.len() >= pad
            && bytes[bytes.len() - pad..].iter().all(|byte| *byte == last)
        {
            bytes[..bytes.len() - pad].to_vec()
        } else {
            bytes.to_vec()
        }
    };
    for candidate in [plain, plain.get(32..).unwrap_or(&[])] {
        let value = strip_padding(candidate);
        if printable(&value) {
            return Some(String::from_utf8_lossy(&value).to_string());
        }
    }
    None
}

fn chromium_cookie() -> Option<ImportedCookie> {
    let password = chromium_password();
    for db in chromium_databases() {
        let Some(tmp) = copy_db(&db) else { continue };
        let Some(connection) = open_readonly(&tmp) else {
            let _ = std::fs::remove_file(&tmp);
            continue;
        };
        let mut entries: Vec<(String, String)> = Vec::new();
        if let Ok(mut statement) = connection.prepare(
            "select name, value, encrypted_value from cookies \
             where host_key like '%youtube.com' or host_key like '%google.com'",
        ) && let Ok(rows) = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        }) {
            for row in rows.flatten() {
                let (name, plain, encrypted) = row;
                if !AUTH_COOKIE_NAMES.contains(&name.as_str()) {
                    continue;
                }
                let value = if !plain.is_empty() {
                    plain
                } else if let Some(decrypted) = decrypt_chromium(&encrypted, &password) {
                    decrypted
                } else {
                    continue;
                };
                entries.push((name, value));
            }
        }
        drop(connection);
        let _ = std::fs::remove_file(&tmp);
        if let Some(cookie) = cookie_header(&entries) {
            return Some(ImportedCookie {
                cookie,
                source: format!("Chromium ({})", db.display()),
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_cookie_is_rejected() {
        let entries = vec![("VISITOR_INFO1_LIVE".to_string(), "abc".to_string())];
        assert!(cookie_header(&entries).is_none());
    }

    #[test]
    fn session_cookie_is_joined_once() {
        let entries = vec![
            ("SID".to_string(), "a".to_string()),
            ("SID".to_string(), "b".to_string()),
            ("HSID".to_string(), "c".to_string()),
        ];
        assert_eq!(cookie_header(&entries).as_deref(), Some("SID=a; HSID=c"));
    }

    #[test]
    fn decrypts_chrome_v10_cookie_with_padding() {
        use aes::cipher::{BlockEncryptMut, KeyIvInit};
        let key = pbkdf2_sha1_key(b"peanuts", b"saltysalt", 16);
        let iv = [b' '; 16];
        let mut plain = b"SID=abc".to_vec();
        let pad = 16 - plain.len() % 16;
        plain.extend(std::iter::repeat(pad as u8).take(pad));
        let cipher = cbc::Encryptor::<aes::Aes128>::new_from_slices(&key, &iv).unwrap();
        let mut buffer = plain.clone();
        let encrypted = cipher
            .encrypt_padded_mut::<aes::cipher::block_padding::NoPadding>(&mut buffer, plain.len())
            .unwrap()
            .to_vec();
        let mut blob = b"v10".to_vec();
        blob.extend_from_slice(&encrypted);
        assert_eq!(decrypt_chromium(&blob, "peanuts").as_deref(), Some("SID=abc"));
    }

    #[test]
    fn pbkdf2_matches_known_chrome_key() {
        // Chrome на Linux: PBKDF2-HMAC-SHA1("peanuts", "saltysalt", 1 итерация, 16 байт)
        let key = pbkdf2_sha1_key(b"peanuts", b"saltysalt", 16);
        let expected = [
            0xfd, 0x62, 0x1f, 0xe5, 0xa2, 0xb4, 0x02, 0x53, 0x9d, 0xfa, 0x14, 0x7c, 0xa9, 0x27,
            0x27, 0x78,
        ];
        assert_eq!(key, expected);
    }
}
