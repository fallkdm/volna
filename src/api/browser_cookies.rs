//! Импорт сессии YouTube Music из локального браузера.
//!
//! Кука браузера — единственный способ входа без заведения OAuth-клиента.
//! Firefox хранит значения открытым текстом, Chromium-семейство — AES-128-CBC
//! с ключом из keyring (`secret-tool` или `kwallet-query`, иначе фиксированный `peanuts`).
//! Приложение забирает свежую сессию при старте и при необходимости переимпортирует
//! её во время работы, поэтому ротация `*PSIDTS` в браузере подхватывается автоматически.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Куки, без которых сессия YouTube Music не считается авторизованной или полноценной.
const AUTH_COOKIE_NAMES: &[&str] = &[
    "SID",
    "HSID",
    "SSID",
    "APISID",
    "SAPISID",
    "SIDCC",
    "__Secure-1PSID",
    "__Secure-3PSID",
    "__Secure-1PAPISID",
    "__Secure-3PAPISID",
    "__Secure-1PSIDTS",
    "__Secure-3PSIDTS",
    "__Secure-1PSIDCC",
    "__Secure-3PSIDCC",
    "LOGIN_INFO",
    "VISITOR_INFO1_LIVE",
];

const DEFAULT_SECRET_TOOL_APPS: &[&str] = &[
    "chrome",
    "Chrome Safe Storage",
    "chromium",
    "Chromium Safe Storage",
    "Brave Safe Storage",
    "Microsoft Edge Safe Storage",
    "Vivaldi Safe Storage",
];

const DEFAULT_KWALLET_FOLDERS: &[Option<&str>] =
    &[Some("Chrome Keys"), Some("Chromium Keys"), None];

const DEFAULT_KWALLET_ENTRIES: &[&str] = &[
    "Chrome Safe Storage",
    "Chromium Safe Storage",
    "Brave Safe Storage",
    "Microsoft Edge Safe Storage",
    "Vivaldi Safe Storage",
];

/// Готовая сессия и подпись источника (для лога и статуса в UI).
pub struct ImportedCookie {
    pub cookie: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RawCookieEntry {
    host: String,
    name: String,
    value: String,
}

/// Пробует Firefox, затем Chromium-семейство.
pub fn import_youtube_cookie() -> Option<ImportedCookie> {
    firefox_cookie().or_else(chromium_cookie)
}

fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()))
}

/// Разрешены строго домены YouTube / YouTube Music (без `.google.com` и сторонних поддоменов).
fn is_ytmusic_cookie_host(host: &str) -> bool {
    let normalized = host.trim().to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "music.youtube.com" | ".music.youtube.com" | ".youtube.com" | "youtube.com"
    )
}

/// Порядок приоритета доменов при совпадении имён кук:
/// сначала `music.youtube.com`, затем `.youtube.com`, затем `youtube.com`.
fn ytmusic_host_priority(host: &str) -> u8 {
    match host.trim().to_ascii_lowercase().as_str() {
        "music.youtube.com" | ".music.youtube.com" => 0,
        ".youtube.com" => 1,
        "youtube.com" => 2,
        _ => 3,
    }
}

/// Фильтрует сырые куки строго по YouTube-доменам, сортирует по приоритету хоста и собирает заголовок.
fn build_ytmusic_cookie_header(mut entries: Vec<RawCookieEntry>) -> Option<String> {
    entries.retain(|entry| {
        !entry.value.is_empty()
            && is_ytmusic_cookie_host(&entry.host)
            && AUTH_COOKIE_NAMES.contains(&entry.name.as_str())
    });
    entries.sort_by_key(|entry| ytmusic_host_priority(&entry.host));
    let pairs: Vec<(String, String)> = entries
        .into_iter()
        .map(|entry| (entry.name, entry.value))
        .collect();
    cookie_header(&pairs)
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
    let mut seen = HashSet::new();
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
        let mut entries: Vec<RawCookieEntry> = Vec::new();
        if let Ok(mut statement) = connection.prepare(
            "select host, name, value from moz_cookies \
             where host in ('.youtube.com', 'youtube.com', 'music.youtube.com', '.music.youtube.com') \
               and value <> ''",
        ) && let Ok(rows) = statement.query_map([], |row| {
            Ok(RawCookieEntry {
                host: row.get::<_, String>(0)?,
                name: row.get::<_, String>(1)?,
                value: row.get::<_, String>(2)?,
            })
        }) {
            entries.extend(rows.flatten());
        }
        drop(connection);
        let _ = std::fs::remove_file(&tmp);
        if let Some(cookie) = build_ytmusic_cookie_header(entries) {
            return Some(ImportedCookie {
                cookie,
                source: format!("Firefox ({})", db.display()),
            });
        }
    }
    None
}

// ───────────────────────── Chromium (AES-128-CBC + keyring) ─────────────────────────

fn is_safe_profile_dir_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
}

/// Извлекает приоритетные имена профилей из Chrome `Local State`
/// (`profile.last_used`, затем `profile.last_active_profiles`).
fn parse_local_state_profiles(json_str: &str) -> Vec<String> {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) else {
        return Vec::new();
    };
    let Some(profile_obj) = val.get("profile") else {
        return Vec::new();
    };

    let mut ordered = Vec::new();
    let mut seen = HashSet::new();

    if let Some(last_used) = profile_obj.get("last_used").and_then(|v| v.as_str()) {
        let name = last_used.trim();
        if is_safe_profile_dir_name(name) && seen.insert(name.to_string()) {
            ordered.push(name.to_string());
        }
    }

    if let Some(active) = profile_obj
        .get("last_active_profiles")
        .and_then(|v| v.as_array())
    {
        for item in active {
            if let Some(name) = item.as_str().map(str::trim)
                && is_safe_profile_dir_name(name)
                && seen.insert(name.to_string())
            {
                ordered.push(name.to_string());
            }
        }
    }

    ordered
}

/// Возвращает базы `Cookies` внутри одной директории браузера,
/// ставя активный профиль из `Local State` первым, а остальные — фолбэком.
fn chromium_databases_for_base(base: &Path) -> Vec<PathBuf> {
    let local_state_profiles = std::fs::read_to_string(base.join("Local State"))
        .ok()
        .map(|s| parse_local_state_profiles(&s))
        .unwrap_or_default();

    let mut discovered_profiles = Vec::new();
    if let Ok(entries) = std::fs::read_dir(base) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name != "Default" && !name.starts_with("Profile") {
                continue;
            }
            if entry.path().join("Cookies").is_file() {
                discovered_profiles.push(name);
            }
        }
    }
    discovered_profiles.sort_by(|a, b| match (a.as_str(), b.as_str()) {
        ("Default", "Default") => std::cmp::Ordering::Equal,
        ("Default", _) => std::cmp::Ordering::Less,
        (_, "Default") => std::cmp::Ordering::Greater,
        _ => a.cmp(b),
    });

    let mut ordered_profiles = Vec::new();
    let mut seen = HashSet::new();
    for name in local_state_profiles
        .into_iter()
        .chain(discovered_profiles.into_iter())
    {
        if seen.insert(name.clone()) {
            ordered_profiles.push(name);
        }
    }

    ordered_profiles
        .into_iter()
        .map(|name| base.join(name).join("Cookies"))
        .filter(|db| db.is_file())
        .collect()
}

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
        databases.extend(chromium_databases_for_base(&base));
    }
    databases
}

/// Очищает вывод `kwallet-query` / `secret-tool` только от хвостовых CR/LF.
fn trim_kwallet_output(raw: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(raw);
    let trimmed = text.trim_end_matches(['\r', '\n']);
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Возвращает список кошельков KWallet (`kdewallet` первым, затем найденные `*.kwl`).
fn available_kwallets() -> Vec<String> {
    let mut wallets = vec!["kdewallet".to_string()];
    let mut seen = HashSet::from(["kdewallet".to_string()]);

    let kwallet_dir = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".local/share"))
        .join("kwalletd");

    if let Ok(entries) = std::fs::read_dir(&kwallet_dir) {
        let mut extra: Vec<String> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("kwl") {
                    path.file_stem()
                        .and_then(|s| s.to_str())
                        .map(str::to_string)
                } else {
                    None
                }
            })
            .collect();
        extra.sort();
        for name in extra {
            if !name.is_empty() && seen.insert(name.clone()) {
                wallets.push(name);
            }
        }
    }

    wallets
}

struct BrowserKeyHints {
    secret_tool_apps: &'static [&'static str],
    kwallet_folders: &'static [Option<&'static str>],
    kwallet_entries: &'static [&'static str],
}

fn key_hints_for_base(base: Option<&Path>) -> BrowserKeyHints {
    let path_str = base
        .map(|p| p.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    if path_str.contains("google-chrome") || path_str.contains("com.google.chrome") {
        BrowserKeyHints {
            secret_tool_apps: &["chrome", "Chrome Safe Storage"],
            kwallet_folders: &[Some("Chrome Keys"), None, Some("Chromium Keys")],
            kwallet_entries: &["Chrome Safe Storage", "Chromium Safe Storage"],
        }
    } else if path_str.contains("chromium") {
        BrowserKeyHints {
            secret_tool_apps: &["chromium", "Chromium Safe Storage", "chrome"],
            kwallet_folders: &[Some("Chromium Keys"), None, Some("Chrome Keys")],
            kwallet_entries: &["Chromium Safe Storage", "Chrome Safe Storage"],
        }
    } else if path_str.contains("brave") {
        BrowserKeyHints {
            secret_tool_apps: &["brave", "Brave Safe Storage", "chrome"],
            kwallet_folders: &[Some("Brave Keys"), Some("Chrome Keys"), None],
            kwallet_entries: &["Brave Safe Storage", "Chrome Safe Storage"],
        }
    } else if path_str.contains("microsoft-edge") {
        BrowserKeyHints {
            secret_tool_apps: &["microsoft-edge", "Microsoft Edge Safe Storage", "chrome"],
            kwallet_folders: &[Some("Chrome Keys"), None],
            kwallet_entries: &["Microsoft Edge Safe Storage", "Chrome Safe Storage"],
        }
    } else if path_str.contains("vivaldi") {
        BrowserKeyHints {
            secret_tool_apps: &["vivaldi", "Vivaldi Safe Storage", "chrome"],
            kwallet_folders: &[Some("Chrome Keys"), None],
            kwallet_entries: &["Vivaldi Safe Storage", "Chrome Safe Storage"],
        }
    } else {
        BrowserKeyHints {
            secret_tool_apps: DEFAULT_SECRET_TOOL_APPS,
            kwallet_folders: DEFAULT_KWALLET_FOLDERS,
            kwallet_entries: DEFAULT_KWALLET_ENTRIES,
        }
    }
}

fn lookup_secret_tool_password(apps: &[&str]) -> Option<(String, String)> {
    for app in apps {
        let output = std::process::Command::new("secret-tool")
            .args(["lookup", "application", app])
            .output();
        match output {
            Ok(out) if out.status.success() => {
                if let Some(secret) = trim_kwallet_output(&out.stdout) {
                    let source = format!("secret-tool (application={app})");
                    println!("[cookies] Ключ расшифровки Chromium получен через {source}");
                    return Some((secret, source));
                }
            }
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return None;
            }
            Err(_) => {}
        }
    }
    None
}

fn lookup_kwallet_password(
    folders: &[Option<&str>],
    entries: &[&str],
) -> Option<(String, String)> {
    let wallets = available_kwallets();
    for wallet in &wallets {
        for folder in folders {
            for entry in entries {
                let mut cmd = std::process::Command::new("kwallet-query");
                cmd.arg("-r").arg(entry);
                if let Some(folder_name) = folder {
                    cmd.arg("-f").arg(folder_name);
                }
                cmd.arg(wallet);

                match cmd.output() {
                    Ok(out) if out.status.success() => {
                        if let Some(secret) = trim_kwallet_output(&out.stdout) {
                            let folder_label = folder.unwrap_or("<default>");
                            let source = format!(
                                "kwallet-query (wallet={wallet}, folder={folder_label}, entry={entry})"
                            );
                            println!("[cookies] Ключ расшифровки Chromium получен через {source}");
                            return Some((secret, source));
                        }
                    }
                    Ok(_) => {}
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                        return None;
                    }
                    Err(_) => {}
                }
            }
        }
    }
    None
}

fn chromium_password_with_source(base: Option<&Path>) -> (String, String) {
    let hints = key_hints_for_base(base);

    if let Some(found) = lookup_secret_tool_password(hints.secret_tool_apps) {
        return found;
    }

    if let Some(found) = lookup_kwallet_password(hints.kwallet_folders, hints.kwallet_entries) {
        return found;
    }

    eprintln!(
        "[cookies] Ключ расшифровки Chromium не найден ни в secret-tool, ни в KWallet — используем fallback 'peanuts'"
    );
    ("peanuts".to_string(), "fallback:peanuts".to_string())
}

/// Ключ Chrome: сначала из `secret-tool`, затем из `kwallet-query`, иначе фиксированный `peanuts`.
#[allow(dead_code)]
fn chromium_password() -> String {
    chromium_password_with_source(None).0
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
    let mut password_cache: HashMap<PathBuf, (String, String)> = HashMap::new();

    for db in chromium_databases() {
        let browser_base = db.parent().and_then(|p| p.parent()).map(Path::to_path_buf);
        let (password, key_source) = if let Some(ref base) = browser_base {
            password_cache
                .entry(base.clone())
                .or_insert_with(|| chromium_password_with_source(Some(base)))
                .clone()
        } else {
            chromium_password_with_source(None)
        };

        let Some(tmp) = copy_db(&db) else { continue };
        let Some(connection) = open_readonly(&tmp) else {
            let _ = std::fs::remove_file(&tmp);
            continue;
        };

        let mut entries: Vec<RawCookieEntry> = Vec::new();
        let mut decrypt_attempts = 0usize;
        let mut decrypt_failures = 0usize;

        if let Ok(mut statement) = connection.prepare(
            "select host_key, name, value, encrypted_value from cookies \
             where host_key in ('.youtube.com', 'youtube.com', 'music.youtube.com', '.music.youtube.com')",
        ) && let Ok(rows) = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        }) {
            for row in rows.flatten() {
                let (host, name, plain, encrypted) = row;
                if !is_ytmusic_cookie_host(&host) || !AUTH_COOKIE_NAMES.contains(&name.as_str()) {
                    continue;
                }
                let value = if !plain.is_empty() {
                    plain
                } else {
                    decrypt_attempts += 1;
                    if let Some(decrypted) = decrypt_chromium(&encrypted, &password) {
                        decrypted
                    } else {
                        decrypt_failures += 1;
                        continue;
                    }
                };
                entries.push(RawCookieEntry { host, name, value });
            }
        }
        drop(connection);
        let _ = std::fs::remove_file(&tmp);

        if decrypt_attempts > 0 && decrypt_failures == decrypt_attempts {
            eprintln!(
                "[cookies] Не удалось расшифровать ни одну из {decrypt_attempts} YouTube cookie в {} (источник ключа: {key_source})",
                db.display()
            );
        }

        if let Some(cookie) = build_ytmusic_cookie_header(entries) {
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

    struct TempDirGuard(PathBuf);

    impl TempDirGuard {
        fn new(prefix: &str) -> Self {
            static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "volna-cookies-test-{}-{}-{}",
                prefix,
                std::process::id(),
                n
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

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
    fn google_com_cookies_are_excluded_and_youtube_priority_is_respected() {
        let raw = vec![
            RawCookieEntry {
                host: ".google.com".to_string(),
                name: "SID".to_string(),
                value: "google-shadow-sid".to_string(),
            },
            RawCookieEntry {
                host: ".google.com".to_string(),
                name: "__Secure-3PSIDTS".to_string(),
                value: "google-shadow-ts".to_string(),
            },
            RawCookieEntry {
                host: "accounts.google.com".to_string(),
                name: "SAPISID".to_string(),
                value: "google-sapisid".to_string(),
            },
            RawCookieEntry {
                host: "notyoutube.com".to_string(),
                name: "HSID".to_string(),
                value: "evil".to_string(),
            },
            RawCookieEntry {
                host: "youtube.com".to_string(),
                name: "SID".to_string(),
                value: "bare-yt-sid".to_string(),
            },
            RawCookieEntry {
                host: ".youtube.com".to_string(),
                name: "SID".to_string(),
                value: "dot-yt-sid".to_string(),
            },
            RawCookieEntry {
                host: ".youtube.com".to_string(),
                name: "SAPISID".to_string(),
                value: "yt-sapisid".to_string(),
            },
            RawCookieEntry {
                host: "music.youtube.com".to_string(),
                name: "__Secure-3PSIDTS".to_string(),
                value: "music-yt-ts".to_string(),
            },
            RawCookieEntry {
                host: ".youtube.com".to_string(),
                name: "__Secure-3PSIDTS".to_string(),
                value: "dot-yt-ts".to_string(),
            },
        ];

        let header = build_ytmusic_cookie_header(raw).expect("valid youtube cookie header");
        assert!(!header.contains("google-shadow"));
        assert!(!header.contains("google-sapisid"));
        assert!(!header.contains("evil"));
        assert_eq!(
            header,
            "__Secure-3PSIDTS=music-yt-ts; SID=dot-yt-sid; SAPISID=yt-sapisid"
        );
    }

    #[test]
    fn only_google_com_cookies_yields_none() {
        let raw = vec![
            RawCookieEntry {
                host: ".google.com".to_string(),
                name: "SID".to_string(),
                value: "google-only".to_string(),
            },
            RawCookieEntry {
                host: ".google.com".to_string(),
                name: "SAPISID".to_string(),
                value: "google-sapi".to_string(),
            },
        ];
        assert!(build_ytmusic_cookie_header(raw).is_none());
    }

    #[test]
    fn local_state_orders_active_profile_first_and_keeps_fallbacks() {
        let dir = TempDirGuard::new("local-state");
        let base = dir.path();

        for profile in ["Default", "Profile 1", "Profile 2"] {
            let pdir = base.join(profile);
            std::fs::create_dir_all(&pdir).unwrap();
            std::fs::write(pdir.join("Cookies"), b"sqlite-stub").unwrap();
        }

        let local_state = serde_json::json!({
            "profile": {
                "last_used": "Profile 1",
                "last_active_profiles": ["Profile 2", "Profile 1", "../escape", "MissingProfile"]
            }
        });
        std::fs::write(base.join("Local State"), local_state.to_string()).unwrap();

        let dbs = chromium_databases_for_base(base);
        let names: Vec<String> = dbs
            .iter()
            .map(|p| {
                p.parent()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();

        assert_eq!(names, vec!["Profile 1", "Profile 2", "Default"]);
    }

    #[test]
    fn local_state_missing_falls_back_to_deterministic_profile_order() {
        let dir = TempDirGuard::new("no-local-state");
        let base = dir.path();

        for profile in ["Profile 2", "Profile 1", "Default", "System Profile"] {
            let pdir = base.join(profile);
            std::fs::create_dir_all(&pdir).unwrap();
            std::fs::write(pdir.join("Cookies"), b"sqlite-stub").unwrap();
        }

        let dbs = chromium_databases_for_base(base);
        let names: Vec<String> = dbs
            .iter()
            .map(|p| {
                p.parent()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();

        assert_eq!(names, vec!["Default", "Profile 1", "Profile 2"]);
    }

    #[test]
    fn trim_kwallet_output_strips_trailing_crlf_only() {
        assert_eq!(
            trim_kwallet_output(b"secretKey==\n").as_deref(),
            Some("secretKey==")
        );
        assert_eq!(
            trim_kwallet_output(b"secretKey==\r\n").as_deref(),
            Some("secretKey==")
        );
        assert_eq!(trim_kwallet_output(b"\r\n"), None);
        assert_eq!(trim_kwallet_output(b""), None);
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
