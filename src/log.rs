use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Mutex, Once};

use chrono::Local;
use serde::{Deserialize, Serialize};

/// Максимальный размер активного файла лога до ротации (2 MiB).
pub const MAX_LOG_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Количество сохраняемых ротированных файлов (`volna.log.1`..`volna.log.3`).
pub const MAX_ROTATED_FILES: usize = 3;
/// Максимальное число строк в кольцевом буфере памяти текущего процесса.
const RING_BUFFER_CAP: usize = 1000;
/// Префикс маркера старта новой сессии в файле лога.
pub const SESSION_MARKER_PREFIX: &str = "=== START volna pid=";
/// Максимальная длина одной санитизированной строки лога (в символах).
const MAX_SANITIZED_LINE_CHARS: usize = 512;

/// Снимок последних диагностических логов приложения.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogSnapshot {
    pub log_file: String,
    pub file_exists: bool,
    pub file_size_bytes: u64,
    pub total_lines_read: usize,
    pub session_header: Option<String>,
    pub lines: Vec<String>,
}

struct DedupState {
    key: String,
    sample: String,
    repeat_count: usize,
}

struct LoggerState {
    file: Option<File>,
    file_size: u64,
    ring: VecDeque<String>,
    dedup: Option<DedupState>,
}

static INIT_ONCE: Once = Once::new();
static ORIG_STDERR_FD: AtomicI32 = AtomicI32::new(-1);
static ORIG_STDOUT_FD: AtomicI32 = AtomicI32::new(-1);
static LOGGER_STATE: std::sync::LazyLock<Mutex<LoggerState>> =
    std::sync::LazyLock::new(|| Mutex::new(LoggerState::new()));

/// Канонический путь к файлу лога Волны:
/// `$XDG_STATE_HOME/volna/volna.log` (фолбэк `$HOME/.local/state/volna/volna.log`).
pub fn log_file_path() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            home.join(".local").join("state")
        });
    base.join("volna").join("volna.log")
}

fn rotated_path(base: &Path, index: usize) -> PathBuf {
    let name = base
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "volna.log".to_string());
    base.with_file_name(format!("{name}.{index}"))
}

/// Ротирует `path` -> `.1` -> `.2` -> `.3`, если размер `path` >= `max_bytes`.
pub(crate) fn rotate_if_needed_at(path: &Path, max_bytes: u64) -> std::io::Result<bool> {
    let Ok(meta) = std::fs::metadata(path) else {
        return Ok(false);
    };
    if meta.len() < max_bytes {
        return Ok(false);
    }

    let oldest = rotated_path(path, MAX_ROTATED_FILES);
    if oldest.exists() {
        let _ = std::fs::remove_file(&oldest);
    }
    for idx in (1..MAX_ROTATED_FILES).rev() {
        let src = rotated_path(path, idx);
        let dst = rotated_path(path, idx + 1);
        if src.exists() {
            std::fs::rename(&src, &dst)?;
        }
    }
    let first = rotated_path(path, 1);
    std::fs::rename(path, &first)?;
    Ok(true)
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    let mut iter = s.char_indices();
    if let Some((idx, _)) = iter.nth(max_chars) {
        let mut out = s[..idx].to_string();
        out.push_str("...");
        out
    } else {
        s.to_string()
    }
}

fn is_url_boundary(b: u8) -> bool {
    matches!(
        b,
        b' ' | b'\t' | b'\r' | b'\n' | b'"' | b'\'' | b')' | b']' | b'>' | b'<' | 0
    )
}

fn redact_gst_structure_headers(input: &str) -> String {
    let mut out = input.to_string();
    for key in [
        "request-headers=(GstStructure)\"",
        "response-headers=(GstStructure)\"",
    ] {
        loop {
            let Some(start) = out.find(key) else {
                break;
            };
            let content_start = start + key.len();
            let bytes = out.as_bytes();
            let mut i = content_start;
            let mut escaped = false;
            let mut end_quote = None;
            while i < bytes.len() {
                let b = bytes[i];
                if escaped {
                    escaped = false;
                } else if b == b'\\' {
                    escaped = true;
                } else if b == b'"' {
                    end_quote = Some(i);
                    break;
                }
                i += 1;
            }
            if let Some(end) = end_quote {
                if &out[content_start..end] == "<redacted>" {
                    break;
                }
                out.replace_range(content_start..end, "<redacted>");
            } else {
                out.replace_range(content_start.., "<redacted>\"");
                break;
            }
        }
    }
    out
}

fn looks_like_sensitive_query_key(key_lower: &str) -> bool {
    matches!(
        key_lower,
        "sig"
            | "lsig"
            | "sp"
            | "s"
            | "n"
            | "signature"
            | "token"
            | "access_token"
            | "auth"
            | "cookie"
            | "key"
            | "sign"
            | "hash"
            | "hdnea"
            | "session"
            | "ip"
            | "ei"
            | "xpc"
            | "sparams"
            | "expire"
    )
}

fn shorten_url(raw_url: &str) -> String {
    let (scheme, rest) = if let Some(r) = raw_url.strip_prefix("https://") {
        ("https://", r)
    } else if let Some(r) = raw_url.strip_prefix("http://") {
        ("http://", r)
    } else {
        return raw_url.to_string();
    };

    let (before_frag, _) = rest.split_once('#').unwrap_or((rest, ""));
    let (authority_and_path, query) = match before_frag.split_once('?') {
        Some((ap, q)) => (ap, Some(q)),
        None => (before_frag, None),
    };
    let (host, path) = match authority_and_path.find('/') {
        Some(idx) => (&authority_and_path[..idx], &authority_and_path[idx..]),
        None => (authority_and_path, ""),
    };

    let host_lower = host.to_lowercase();
    let is_googlevideo = host_lower.contains("googlevideo.com");
    let is_yandex_strm = host_lower.contains("yandex.net")
        && (host_lower.contains("strm")
            || host_lower.starts_with("s3.")
            || path.contains("/get-mp3/")
            || path.contains("/vh-")
            || path.len() > 64);

    let has_sensitive_query = query.is_some_and(|q| {
        q.len() > 80
            || q.split('&').any(|pair| {
                let k = pair.split('=').next().unwrap_or("").to_lowercase();
                looks_like_sensitive_query_key(&k)
            })
    });

    if !is_googlevideo && !is_yandex_strm && !has_sensitive_query && raw_url.len() <= 140 {
        return raw_url.to_string();
    }

    let path_out = if is_yandex_strm {
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut kept_segs = Vec::new();
        for seg in segments.iter().take(2) {
            // Длинные сегменты подписи/хеша (> 20 символов) не выводим
            if seg.len() <= 20 && kept_segs.len() < 2 {
                kept_segs.push(*seg);
            } else {
                break;
            }
        }
        if kept_segs.len() < segments.len() {
            if kept_segs.is_empty() {
                "/...".to_string()
            } else {
                format!("/{}/...", kept_segs.join("/"))
            }
        } else {
            path.to_string()
        }
    } else if is_googlevideo {
        if path.len() <= 40 {
            path.to_string()
        } else {
            truncate_chars(path, 32)
        }
    } else if path.len() > 48 {
        truncate_chars(path, 40)
    } else {
        path.to_string()
    };

    let query_out = match query {
        Some(q) if !q.is_empty() => {
            let mut kept_params = Vec::new();
            let mut dropped_any = false;
            for pair in q.split('&') {
                if pair.is_empty() {
                    continue;
                }
                let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                if matches!(k, "id" | "itag" | "mn" | "dur") {
                    let max_len = if k == "id" { 24 } else { 64 };
                    let clean_v = if v.len() > max_len {
                        truncate_chars(v, max_len)
                    } else {
                        v.to_string()
                    };
                    kept_params.push(format!("{k}={clean_v}"));
                } else {
                    dropped_any = true;
                }
            }
            if kept_params.is_empty() {
                if dropped_any {
                    "?...".to_string()
                } else {
                    String::new()
                }
            } else if dropped_any {
                format!("?{}&...", kept_params.join("&"))
            } else {
                format!("?{}", kept_params.join("&"))
            }
        }
        _ => String::new(),
    };

    format!("{scheme}{host}{path_out}{query_out}")
}

fn sanitize_urls_in_text(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut pos = 0;
    while pos < input.len() {
        let rest = &input[pos..];
        let next_https = rest.find("https://");
        let next_http = rest.find("http://");
        let rel = match (next_https, next_http) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) | (None, Some(a)) => Some(a),
            (None, None) => None,
        };
        let Some(offset) = rel else {
            result.push_str(rest);
            break;
        };

        let url_start = pos + offset;
        result.push_str(&input[pos..url_start]);

        let bytes = input.as_bytes();
        let mut end = url_start;
        while end < bytes.len() && !is_url_boundary(bytes[end]) {
            end += 1;
        }
        // Если URL прямо перед запятой и пробелом (или точкой в конце предложения), отделим завершающую пунктуацию
        while end > url_start && matches!(bytes[end - 1], b',' | b';' | b'.') {
            if end == bytes.len() || bytes[end].is_ascii_whitespace() {
                end -= 1;
            } else {
                break;
            }
        }

        let raw_url = &input[url_start..end];
        result.push_str(&shorten_url(raw_url));
        pos = end;
    }
    result
}

fn redact_secret_headers_and_kv(input: &str) -> String {
    let mut out = input.to_string();

    // 1. Заголовки вида `Cookie: ...`, `Set-Cookie: ...`, `Authorization: ...`
    for header in ["Cookie:", "Set-Cookie:", "Authorization:"] {
        let mut search_from = 0;
        loop {
            let lower = out.to_ascii_lowercase();
            let header_lower = header.to_ascii_lowercase();
            let Some(rel) = lower[search_from..].find(&header_lower) else {
                break;
            };
            let idx = search_from + rel;
            // Проверяем границу слева, чтобы не зацепить чужое слово
            if idx > 0 {
                let prev = out.as_bytes()[idx - 1];
                if prev.is_ascii_alphanumeric() || prev == b'_' || prev == b'-' {
                    search_from = idx + header.len();
                    continue;
                }
            }
            let val_start = idx + header.len();
            let rest = &out[val_start..];
            let trimmed_offset = rest.len() - rest.trim_start_matches(' ').len();
            let content_start = val_start + trimmed_offset;
            if content_start >= out.len() {
                break;
            }
            let end = out[content_start..]
                .find(['\n', '\r'])
                .map(|o| content_start + o)
                .unwrap_or(out.len());
            let val = &out[content_start..end];
            if val.len() > 12 && !val.starts_with("<redacted") {
                out.replace_range(content_start..end, "<redacted>");
                search_from = content_start + "<redacted>".len();
            } else {
                search_from = end;
            }
        }
    }

    // 2. Ключ=значение для секретов (`token=...`, `cookie=...`, `sig=...`, `lsig=...`, `access_token=...`)
    for key in [
        "access_token",
        "auth_token",
        "token",
        "cookie",
        "lsig",
        "signature",
        "sig",
    ] {
        let mut search_from = 0;
        loop {
            let lower = out.to_ascii_lowercase();
            let Some(rel) = lower[search_from..].find(key) else {
                break;
            };
            let idx = search_from + rel;
            let prev_ok = idx == 0 || {
                let prev = out.as_bytes()[idx - 1];
                !prev.is_ascii_alphanumeric() && prev != b'_'
            };
            let after_idx = idx + key.len();
            if !prev_ok || after_idx >= out.len() {
                search_from = after_idx;
                continue;
            }
            let sep = out.as_bytes()[after_idx];
            if sep != b'=' {
                search_from = after_idx;
                continue;
            }
            let val_start = after_idx + 1;
            let bytes = out.as_bytes();
            let (quoted, start, end) = if val_start < bytes.len() && bytes[val_start] == b'"' {
                let s = val_start + 1;
                let mut e = s;
                while e < bytes.len() && bytes[e] != b'"' {
                    e += 1;
                }
                (true, s, e)
            } else {
                let mut e = val_start;
                while e < bytes.len()
                    && !matches!(
                        bytes[e],
                        b' ' | b'\t' | b'\r' | b'\n' | b'&' | b';' | b',' | b')' | b']' | b'"' | b'\''
                    )
                {
                    e += 1;
                }
                (false, val_start, e)
            };

            let val = &out[start..end];
            if val.len() > 8 && !val.starts_with("<redacted") {
                out.replace_range(start..end, "<redacted>");
                search_from = start + "<redacted>".len() + usize::from(quoted);
            } else {
                search_from = end + usize::from(quoted);
            }
        }
    }

    out
}

/// Санитизирует строку лога: сворачивает в одну строку, вырезает огромные signed URL
/// (`googlevideo.com/videoplayback?...`, `strm.*.yandex.net/...`), GStreamer `details` headers
/// и секреты (`Cookie`, `token`, `sig`, `lsig`), сохраняя читаемый хост и `id`/`itag`/`mn`/`dur`.
pub fn sanitize_log_line(line: &str) -> String {
    let single_line = line.replace("\r\n", " ").replace(['\r', '\n'], " ");
    let trimmed = single_line.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let no_gst_headers = redact_gst_structure_headers(trimmed);
    let short_urls = sanitize_urls_in_text(&no_gst_headers);
    let redacted = redact_secret_headers_and_kv(&short_urls);
    truncate_chars(&redacted, MAX_SANITIZED_LINE_CHARS)
}

fn has_timestamp_prefix(line: &str) -> bool {
    if line.starts_with(SESSION_MARKER_PREFIX) {
        return true;
    }
    // Формат `[YYYY-MM-DD HH:MM:SS.mmm]`
    let b = line.as_bytes();
    b.len() >= 25
        && b[0] == b'['
        && b[1..5].iter().all(|c| c.is_ascii_digit())
        && b[5] == b'-'
        && b[8] == b'-'
        && b[11] == b' '
        && b[14] == b':'
        && b[17] == b':'
        && b[24] == b']'
}

fn format_with_timestamp(line: &str) -> String {
    if has_timestamp_prefix(line) {
        line.to_string()
    } else {
        let now = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let pid = std::process::id();
        format!("[{now}] [pid={pid}] {line}")
    }
}

fn write_all_fd(fd: i32, bytes: &[u8]) {
    if fd < 0 || bytes.is_empty() {
        return;
    }
    let mut offset = 0;
    while offset < bytes.len() {
        let written = unsafe {
            libc::write(
                fd,
                bytes[offset..].as_ptr() as *const libc::c_void,
                bytes.len() - offset,
            )
        };
        if written > 0 {
            offset += written as usize;
        } else if written == -1 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            // Не падаем на EPIPE / EBADF / EIO при закрытом tty или pipe
            break;
        } else {
            break;
        }
    }
}

fn write_original_stderr_line(line: &str) {
    let fd = ORIG_STDERR_FD.load(Ordering::SeqCst);
    let fallback_fd = if fd >= 0 {
        fd
    } else {
        ORIG_STDOUT_FD.load(Ordering::SeqCst)
    };
    if fallback_fd >= 0 {
        let mut msg = String::with_capacity(line.len() + 1);
        msg.push_str(line);
        msg.push('\n');
        write_all_fd(fallback_fd, msg.as_bytes());
    }
}

fn open_log_file_append(path: &Path) -> std::io::Result<(File, u64)> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    Ok((file, size))
}

fn push_ring_line(ring: &mut VecDeque<String>, line: String) {
    if ring.len() >= RING_BUFFER_CAP {
        ring.pop_front();
    }
    ring.push_back(line);
}

fn strip_log_prefix(line: &str) -> &str {
    let mut rest = line;
    if !rest.starts_with(SESSION_MARKER_PREFIX) && has_timestamp_prefix(rest) {
        rest = rest[25..].trim_start_matches(' ');
    }
    if let Some(after_pid) = rest.strip_prefix("[pid=") {
        if let Some(close_idx) = after_pid.find(']') {
            if !after_pid[..close_idx].is_empty()
                && after_pid[..close_idx].bytes().all(|b| b.is_ascii_digit())
            {
                rest = after_pid[close_idx + 1..].trim_start_matches(' ');
            }
        }
    }
    rest
}

fn is_glib_hhmmss_mmm(b: &[u8]) -> bool {
    b.len() >= 12
        && b[0].is_ascii_digit()
        && b[1].is_ascii_digit()
        && b[2] == b':'
        && b[3].is_ascii_digit()
        && b[4].is_ascii_digit()
        && b[5] == b':'
        && b[6].is_ascii_digit()
        && b[7].is_ascii_digit()
        && b[8] == b'.'
        && b[9].is_ascii_digit()
        && b[10].is_ascii_digit()
        && b[11].is_ascii_digit()
}

fn normalize_dedup_body(body: &str) -> String {
    // 1. Нормализуем внутреннее время GLib (`**: HH:MM:SS.mmm:`)
    let mut temp = body.to_string();
    if let Some(marker) = temp.find("**: ") {
        let ts_start = marker + 4;
        if is_glib_hhmmss_mmm(&temp.as_bytes()[ts_start..]) {
            temp.replace_range(ts_start..ts_start + 12, "<time>");
        }
    }

    // 2. Нормализуем hex-адреса (`0x55a1b2c3d4e0` -> `0x*`)
    let bytes = temp.as_bytes();
    let mut out = String::with_capacity(temp.len());
    let mut i = 0;
    while i < bytes.len() {
        if i + 2 < bytes.len() && bytes[i] == b'0' && matches!(bytes[i + 1], b'x' | b'X') {
            let prev_ok = i == 0 || (!bytes[i - 1].is_ascii_alphanumeric() && bytes[i - 1] != b'_');
            let mut j = i + 2;
            while j < bytes.len() && bytes[j].is_ascii_hexdigit() {
                j += 1;
            }
            let hex_len = j - (i + 2);
            let next_ok = j == bytes.len() || (!bytes[j].is_ascii_alphanumeric() && bytes[j] != b'_');
            if prev_ok && hex_len >= 4 && next_ok {
                out.push_str("0x*");
                i = j;
                continue;
            }
        }
        let ch = temp[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }

    // 3. Нормализуем меняющиеся размеры в Gtk-WARNING (`min 26` -> `min <n>`)
    if body.contains("Gtk-WARNING") || body.contains("adjusted size") {
        normalize_gtk_min_sizes(&out)
    } else {
        out
    }
}

fn normalize_gtk_min_sizes(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if i + 4 < bytes.len() && &bytes[i..i + 4] == b"min " {
            let prev_ok =
                i == 0 || (!bytes[i - 1].is_ascii_alphanumeric() && bytes[i - 1] != b'_');
            let mut j = i + 4;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            let digits_len = j - (i + 4);
            let next_ok =
                j == bytes.len() || (!bytes[j].is_ascii_alphanumeric() && bytes[j] != b'_');
            if prev_ok && digits_len > 0 && next_ok {
                out.push_str("min <n>");
                i = j;
                continue;
            }
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Возвращает ключ дедупликации и исходное тело строки; для `[PANIC]` и `=== START` возвращает `None`.
fn dedup_key(formatted: &str) -> Option<(String, String)> {
    if formatted.starts_with(SESSION_MARKER_PREFIX) {
        return None;
    }
    let body = strip_log_prefix(formatted).trim();
    if body.is_empty() || body.contains("[PANIC]") {
        return None;
    }
    Some((normalize_dedup_body(body), body.to_string()))
}

impl LoggerState {
    fn new() -> Self {
        Self {
            file: None,
            file_size: 0,
            ring: VecDeque::with_capacity(RING_BUFFER_CAP),
            dedup: None,
        }
    }

    fn write_line_to_outputs(&mut self, log_path: &Path, line: &str) {
        if self.file.is_none() {
            if let Ok((f, sz)) = open_log_file_append(log_path) {
                self.file = Some(f);
                self.file_size = sz;
            }
        }

        if let Some(file) = self.file.as_mut() {
            let mut line_bytes = line.as_bytes().to_vec();
            line_bytes.push(b'\n');
            if file.write_all(&line_bytes).is_ok() {
                let _ = file.flush();
                self.file_size = self.file_size.saturating_add(line_bytes.len() as u64);
            }
        }

        if self.file_size >= MAX_LOG_FILE_BYTES {
            let real_len = self
                .file
                .as_ref()
                .and_then(|f| f.metadata().ok())
                .map(|m| m.len())
                .unwrap_or(self.file_size);
            if real_len >= MAX_LOG_FILE_BYTES {
                self.file = None;
                if rotate_if_needed_at(log_path, MAX_LOG_FILE_BYTES).unwrap_or(false) {
                    if let Ok((new_file, new_sz)) = open_log_file_append(log_path) {
                        self.file = Some(new_file);
                        self.file_size = new_sz;
                    }
                } else if let Ok((reopened, sz)) = open_log_file_append(log_path) {
                    self.file = Some(reopened);
                    self.file_size = sz;
                }
            }
        }

        push_ring_line(&mut self.ring, line.to_string());
    }

    fn flush_dedup(&mut self, log_path: &Path) -> Option<String> {
        let prev = self.dedup.take()?;
        if prev.repeat_count == 0 {
            return None;
        }
        let summary_body = format!(
            "[log] {} (повторено {} раз)",
            truncate_chars(&prev.sample, 200),
            prev.repeat_count
        );
        let summary = format_with_timestamp(&summary_body);
        self.write_line_to_outputs(log_path, &summary);
        Some(summary)
    }

    fn record_line(&mut self, log_path: &Path, formatted: &str) -> (Option<String>, Option<String>) {
        let next_key = dedup_key(formatted);
        if let (Some((key, _)), Some(prev)) = (&next_key, self.dedup.as_mut()) {
            if prev.key == *key {
                prev.repeat_count = prev.repeat_count.saturating_add(1);
                return (None, None);
            }
        }

        let flushed = self.flush_dedup(log_path);
        self.write_line_to_outputs(log_path, formatted);
        self.dedup = next_key.map(|(key, sample)| DedupState {
            key,
            sample,
            repeat_count: 0,
        });
        (flushed, Some(formatted.to_string()))
    }
}

fn record_formatted_line(log_path: &Path, formatted: &str) {
    let mut guard = LOGGER_STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (flushed, written) = guard.record_line(log_path, formatted);
    drop(guard);

    if let Some(summary) = flushed {
        write_original_stderr_line(&summary);
    }
    if let Some(line) = written {
        write_original_stderr_line(&line);
    }
}

fn create_cloexec_pipe() -> std::io::Result<(i32, i32)> {
    let mut fds = [0i32; 2];
    #[cfg(target_os = "linux")]
    {
        let rc = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
        if rc == 0 {
            return Ok((fds[0], fds[1]));
        }
    }
    let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    unsafe {
        libc::fcntl(fds[0], libc::F_SETFD, libc::FD_CLOEXEC);
        libc::fcntl(fds[1], libc::F_SETFD, libc::FD_CLOEXEC);
    }
    Ok((fds[0], fds[1]))
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn configure_c_stdio_buffering() {
    unsafe extern "C" {
        static mut stdout: *mut libc::FILE;
        static mut stderr: *mut libc::FILE;
    }
    unsafe {
        if !stdout.is_null() {
            libc::setvbuf(stdout, std::ptr::null_mut(), libc::_IOLBF, 0);
        }
        if !stderr.is_null() {
            libc::setvbuf(stderr, std::ptr::null_mut(), libc::_IONBF, 0);
        }
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn configure_c_stdio_buffering() {}

/// Инициализирует централизованное логирование для GUI-режима Волны:
/// - ротирует лог-файл при >= 2 MiB;
/// - пишет маркер старта сессии;
/// - ставит panic hook (payload + location в файл и оригинальный stderr);
/// - перехватывает stdout/stderr через pipe + dup2 в фоновый поток с санитизацией URL и секретов.
///
/// Не вызывать в `volna mcp` и `volna cmd`!
pub fn init_gui_logging() {
    INIT_ONCE.call_once(|| {
        let log_path = log_file_path();
        if let Some(parent) = log_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = rotate_if_needed_at(&log_path, MAX_LOG_FILE_BYTES);

        if let Ok((file, size)) = open_log_file_append(&log_path) {
            if let Ok(mut guard) = LOGGER_STATE.lock() {
                guard.file = Some(file);
                guard.file_size = size;
            }
        }

        // Сохраняем оригинальные stdout/stderr fd до dup2
        let orig_out = unsafe { libc::fcntl(libc::STDOUT_FILENO, libc::F_DUPFD_CLOEXEC, 3) };
        let orig_err = unsafe { libc::fcntl(libc::STDERR_FILENO, libc::F_DUPFD_CLOEXEC, 3) };
        ORIG_STDOUT_FD.store(orig_out, Ordering::SeqCst);
        ORIG_STDERR_FD.store(orig_err, Ordering::SeqCst);

        // Устанавливаем panic hook до/вместе с перехватом потоков
        let panic_path = log_path.clone();
        std::panic::set_hook(Box::new(move |info| {
            let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
                *s
            } else if let Some(s) = info.payload().downcast_ref::<String>() {
                s.as_str()
            } else {
                "<non-string panic payload>"
            };
            let location = info
                .location()
                .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()))
                .unwrap_or_else(|| "<unknown>".to_string());
            let thread_name = std::thread::current()
                .name()
                .unwrap_or("unnamed")
                .to_string();
            let raw_msg = format!(
                "[PANIC] thread={thread_name} location={location} payload={payload}"
            );
            let sanitized = sanitize_log_line(&raw_msg);
            let formatted = format_with_timestamp(&sanitized);
            record_formatted_line(&panic_path, &formatted);
        }));

        // Маркер старта сессии
        let start_marker = format!(
            "{SESSION_MARKER_PREFIX}{} version={} time={} ===",
            std::process::id(),
            env!("CARGO_PKG_VERSION"),
            Local::now().format("%Y-%m-%d %H:%M:%S%.3f")
        );
        record_formatted_line(&log_path, &start_marker);

        // Перехватываем fd 1 (stdout) и fd 2 (stderr)
        match create_cloexec_pipe() {
            Ok((read_fd, write_fd)) => {
                unsafe {
                    libc::dup2(write_fd, libc::STDOUT_FILENO);
                    libc::dup2(write_fd, libc::STDERR_FILENO);
                    if write_fd != libc::STDOUT_FILENO && write_fd != libc::STDERR_FILENO {
                        libc::close(write_fd);
                    }
                }
                configure_c_stdio_buffering();

                let reader_path = log_path.clone();
                let _ = std::thread::Builder::new()
                    .name("volna-logger".into())
                    .spawn(move || {
                        use std::os::unix::io::FromRawFd;
                        let file = unsafe { File::from_raw_fd(read_fd) };
                        let mut reader = BufReader::new(file);
                        let mut buf = Vec::with_capacity(1024);
                        loop {
                            buf.clear();
                            match reader.read_until(b'\n', &mut buf) {
                                Ok(0) => break,
                                Ok(_) => {
                                    while matches!(buf.last(), Some(b'\n' | b'\r')) {
                                        buf.pop();
                                    }
                                    if buf.is_empty() {
                                        continue;
                                    }
                                    let raw_line = String::from_utf8_lossy(&buf);
                                    let sanitized = sanitize_log_line(&raw_line);
                                    if sanitized.is_empty() {
                                        continue;
                                    }
                                    let formatted = format_with_timestamp(&sanitized);
                                    record_formatted_line(&reader_path, &formatted);
                                }
                                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                                Err(_) => break,
                            }
                        }
                    });
            }
            Err(e) => {
                let err_line = format_with_timestamp(&format!(
                    "[log] Не удалось перехватить stdout/stderr: {e}"
                ));
                record_formatted_line(&log_path, &err_line);
            }
        }
    });
}

fn read_lines_lossy(path: &Path) -> Vec<String> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    let mut reader = BufReader::new(file);
    let mut lines = Vec::new();
    let mut buf = Vec::with_capacity(512);
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) => break,
            Ok(_) => {
                while matches!(buf.last(), Some(b'\n' | b'\r')) {
                    buf.pop();
                }
                lines.push(String::from_utf8_lossy(&buf).into_owned());
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    lines
}

fn is_error_or_warning_line(line: &str) -> bool {
    let lower = line.to_lowercase();
    const PATTERNS: [&str; 22] = [
        "ошибка",
        "ошибк",
        "предупреждение",
        "паника",
        "[panic]",
        "panic",
        "не удалось",
        "завис",
        "прерван",
        "error",
        "err:",
        "warn",
        "failed",
        "failure",
        "fatal",
        "critical",
        "timeout",
        "timed out",
        "could not",
        "flow-return",
        "status=4",
        "status=5",
    ];
    PATTERNS.iter().any(|p| lower.contains(p))
}

/// Читает недавние логи из указанного файла (и ротированных `.1`..`.3`) напрямую с диска.
pub(crate) fn read_recent_logs_from_path(
    path: &Path,
    lines: usize,
    errors_only: bool,
    grep: Option<&str>,
    current_session_only: bool,
) -> LogSnapshot {
    let meta = std::fs::metadata(path).ok();
    let file_exists = meta.as_ref().is_some_and(|m| m.is_file());
    let file_size_bytes = meta.map(|m| m.len()).unwrap_or(0);
    let log_file = path.display().to_string();

    let mut source_lines: Vec<String> = Vec::new();

    if current_session_only {
        let active_lines = read_lines_lossy(path);
        if let Some(pos) = active_lines
            .iter()
            .rposition(|l| l.starts_with(SESSION_MARKER_PREFIX))
        {
            source_lines.extend(active_lines.into_iter().skip(pos));
        } else {
            // Если текущая сессия успела уйти за ротацию, пробуем найти маркер в .1
            let prev_path = rotated_path(path, 1);
            let prev_lines = read_lines_lossy(&prev_path);
            if let Some(pos) = prev_lines
                .iter()
                .rposition(|l| l.starts_with(SESSION_MARKER_PREFIX))
            {
                source_lines.extend(prev_lines.into_iter().skip(pos));
            }
            source_lines.extend(active_lines);
        }
    } else {
        // Собираем в хронологическом порядке: .3 -> .2 -> .1 -> active
        for idx in (1..=MAX_ROTATED_FILES).rev() {
            let rot = rotated_path(path, idx);
            if rot.exists() {
                source_lines.extend(read_lines_lossy(&rot));
            }
        }
        source_lines.extend(read_lines_lossy(path));
    }

    let total_lines_read = source_lines.len();
    let session_header = source_lines
        .iter()
        .rev()
        .find(|l| l.starts_with(SESSION_MARKER_PREFIX))
        .cloned()
        .or_else(|| {
            // Если current_session_only == false и маркер не попал, поищем в текущем файле
            read_lines_lossy(path)
                .into_iter()
                .rev()
                .find(|l| l.starts_with(SESSION_MARKER_PREFIX))
        });

    let grep_terms: Vec<String> = grep
        .map(|g| {
            g.split('|')
                .map(|part| part.trim().to_lowercase())
                .filter(|part| !part.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let mut filtered: Vec<String> = source_lines
        .into_iter()
        .filter(|line| {
            if errors_only && !is_error_or_warning_line(line) {
                return false;
            }
            if !grep_terms.is_empty() {
                let lower = line.to_lowercase();
                if !grep_terms.iter().any(|term| lower.contains(term)) {
                    return false;
                }
            }
            true
        })
        .collect();

    if lines == 0 {
        filtered.clear();
    } else if filtered.len() > lines {
        let drop_count = filtered.len() - lines;
        filtered.drain(0..drop_count);
    }

    LogSnapshot {
        log_file,
        file_exists,
        file_size_bytes,
        total_lines_read,
        session_header,
        lines: filtered,
    }
}

/// Читает недавние логи приложения Волна с диска (безопасно вызывать из `volna mcp` и `volna cmd logs`).
pub fn read_recent_logs(
    lines: usize,
    errors_only: bool,
    grep: Option<&str>,
    current_session_only: bool,
) -> LogSnapshot {
    let path = log_file_path();
    read_recent_logs_from_path(&path, lines, errors_only, grep, current_session_only)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn make_temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "volna_log_test_{name}_{}_{}",
            std::process::id(),
            nanos
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_sanitize_log_line_googlevideo_and_gst_details() {
        let raw = r#"[gst slot 0] ошибка: Could not open resource for reading. (Some("gst-resource-error-quark: details, flow-return=(int)-5, request-headers=(GstStructure)\"User-Agent=Safari, Cookie=SECRET123\", uri=(string)\"https://rr6---sn-gvnuxaxjvh-axq6.googlevideo.com/videoplayback?expire=1793497812&ei=secretEi&ip=1.2.3.4&id=o-ABCDEF1234&itag=251&mn=sn-gvnuxaxjvh-axq6%2Csn-5hne6nsy&dur=214.5&sig=LONG_SECRET_SIG_999&lsig=LONG_SECRET_LSIG_888\""))"#;
        let clean = sanitize_log_line(raw);
        assert!(clean.contains("rr6---sn-gvnuxaxjvh-axq6.googlevideo.com/videoplayback"));
        assert!(clean.contains("id=o-ABCDEF1234"));
        assert!(clean.contains("itag=251"));
        assert!(clean.contains("mn=sn-gvnuxaxjvh-axq6%2Csn-5hne6nsy"));
        assert!(clean.contains("dur=214.5"));
        assert!(clean.contains("flow-return=(int)-5"));
        assert!(!clean.contains("LONG_SECRET_SIG_999"));
        assert!(!clean.contains("LONG_SECRET_LSIG_888"));
        assert!(!clean.contains("SECRET123"));
        assert!(!clean.contains("secretEi"));
        assert!(!clean.contains('\n'));

        let long_id_url = "https://rr1---sn-test.googlevideo.com/videoplayback?id=o-ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890&itag=251&sig=SECRET";
        let clean_long = sanitize_log_line(long_id_url);
        assert!(clean_long.contains("id=o-ABCDEFGHIJKLMNOPQRSTUV..."));
        assert!(!clean_long.contains("1234567890"));
        assert!(!clean_long.contains("SECRET"));
    }

    #[test]
    fn test_sanitize_log_line_yandex_strm_and_secrets() {
        let raw_strm = "[audio] play_url https://strm-msk-01.strm.yandex.net/get-mp3/0123456789abcdef0123456789abcdef01234567/1793497812/track123.mp3?sign=verysecret&token=tok123456789";
        let clean_strm = sanitize_log_line(raw_strm);
        assert!(clean_strm.contains("https://strm-msk-01.strm.yandex.net/get-mp3/..."));
        assert!(!clean_strm.contains("0123456789abcdef"));
        assert!(!clean_strm.contains("tok123456789"));

        let raw_hdr = "Cookie: SAPISID=very_secret_cookie_value_12345; SID=another_secret\nAuthorization: Bearer secret_jwt_token_value";
        let clean_hdr = sanitize_log_line(raw_hdr);
        assert!(!clean_hdr.contains("very_secret_cookie_value_12345"));
        assert!(!clean_hdr.contains("secret_jwt_token_value"));
        assert!(clean_hdr.contains("Cookie: <redacted>"));

        let raw_kv = "token=my_super_secret_token sig=very_long_sig lsig=very_long_lsig normal_signal=ok";
        let clean_kv = sanitize_log_line(raw_kv);
        assert!(!clean_kv.contains("my_super_secret_token"));
        assert!(!clean_kv.contains("very_long_sig"));
        assert!(!clean_kv.contains("very_long_lsig"));
        assert!(clean_kv.contains("normal_signal=ok"));
    }

    #[test]
    fn test_log_rotation_three_generations() {
        let dir = make_temp_dir("rotate");
        let log_path = dir.join("volna.log");

        for cycle in 1..=4 {
            std::fs::write(&log_path, format!("generation-{cycle}-payload-1234567890")).unwrap();
            let rotated = rotate_if_needed_at(&log_path, 10).unwrap();
            assert!(rotated);
            assert!(!log_path.exists());
        }

        assert_eq!(
            std::fs::read_to_string(rotated_path(&log_path, 1)).unwrap(),
            "generation-4-payload-1234567890"
        );
        assert_eq!(
            std::fs::read_to_string(rotated_path(&log_path, 2)).unwrap(),
            "generation-3-payload-1234567890"
        );
        assert_eq!(
            std::fs::read_to_string(rotated_path(&log_path, 3)).unwrap(),
            "generation-2-payload-1234567890"
        );
        assert!(!rotated_path(&log_path, 4).exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_read_recent_logs_filtering_and_sessions() {
        let dir = make_temp_dir("read");
        let log_path = dir.join("volna.log");
        let rot1 = rotated_path(&log_path, 1);

        std::fs::write(
            &rot1,
            "=== START volna pid=10 version=0.1.0 time=2026-01-01 00:00:00.000 ===\n\
             [2026-01-01 00:00:01.000] [pid=10] [ytmusic] old error from rotated\n",
        )
        .unwrap();

        std::fs::write(
            &log_path,
            "=== START volna pid=11 version=0.1.0 time=2026-01-01 01:00:00.000 ===\n\
             [2026-01-01 01:00:01.000] [pid=11] [play] normal playback\n\
             === START volna pid=12 version=0.1.0 time=2026-01-01 02:00:00.000 ===\n\
             [2026-01-01 02:00:01.000] [pid=12] [play] started track A\n\
             [2026-01-01 02:00:02.000] [pid=12] [gst slot 0] ошибка: connection timeout\n\
             [2026-01-01 02:00:03.000] [pid=12] [ytmusic] stream resolved mode=fallback\n",
        )
        .unwrap();

        // current_session_only = true -> только с pid=12
        let cur = read_recent_logs_from_path(&log_path, 50, false, None, true);
        assert!(cur.file_exists);
        assert!(cur.file_size_bytes > 0);
        assert_eq!(cur.total_lines_read, 4);
        assert_eq!(
            cur.session_header.as_deref(),
            Some("=== START volna pid=12 version=0.1.0 time=2026-01-01 02:00:00.000 ===")
        );
        assert_eq!(cur.lines.len(), 4);

        // errors_only = true -> только ошибка в текущей сессии
        let cur_err = read_recent_logs_from_path(&log_path, 50, true, None, true);
        assert_eq!(cur_err.lines.len(), 1);
        assert!(cur_err.lines[0].contains("connection timeout"));

        // current_session_only = false + grep + rotated
        let all_yt = read_recent_logs_from_path(&log_path, 50, false, Some("ytmusic"), false);
        assert_eq!(all_yt.lines.len(), 2);
        assert!(all_yt.lines[0].contains("old error from rotated"));
        assert!(all_yt.lines[1].contains("stream resolved"));

        // Ограничение числа строк
        let tail = read_recent_logs_from_path(&log_path, 2, false, None, true);
        assert_eq!(tail.lines.len(), 2);
        assert!(tail.lines[0].contains("connection timeout"));
        assert!(tail.lines[1].contains("mode=fallback"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_read_recent_logs_pipe_grep_alternatives() {
        let dir = make_temp_dir("pipe_grep");
        let log_path = dir.join("volna.log");

        std::fs::write(
            &log_path,
            "=== START volna pid=42 version=0.1.0 time=2026-01-01 00:00:00.000 ===\n\
             [2026-01-01 00:00:01.000] [pid=42] [ui] tab -> search\n\
             [2026-01-01 00:00:02.000] [pid=42] [discord] статус: Track — Artist\n\
             [2026-01-01 00:00:03.000] [pid=42] [play] #1 Track — Artist\n\
             [2026-01-01 00:00:04.000] [pid=42] [UI] settings open\n",
        )
        .unwrap();

        let matched = read_recent_logs_from_path(&log_path, 50, false, Some("[ui]|[play]"), true);
        assert_eq!(matched.lines.len(), 3);
        assert!(matched.lines[0].contains("[ui] tab -> search"));
        assert!(matched.lines[1].contains("[play] #1"));
        assert!(matched.lines[2].contains("[UI] settings open"));

        // Пробелы вокруг `|` и пустые альтернативы корректно игнорируются
        let spaced = read_recent_logs_from_path(&log_path, 50, false, Some(" [discord] | | [play] "), true);
        assert_eq!(spaced.lines.len(), 2);
        assert!(spaced.lines[0].contains("[discord]"));
        assert!(spaced.lines[1].contains("[play]"));

        // Обычный одиночный grep по-прежнему работает
        let single = read_recent_logs_from_path(&log_path, 50, false, Some("discord"), true);
        assert_eq!(single.lines.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_record_line_dedup_and_never_collapses_panic_or_start() {
        let dir = make_temp_dir("dedup");
        let log_path = dir.join("volna.log");
        let mut state = LoggerState::new();

        // Маркеры старта никогда не схлопываются
        let s1 = "=== START volna pid=1 version=0.1.0 time=2026-01-01 00:00:00.000 ===";
        let s2 = "=== START volna pid=1 version=0.1.0 time=2026-01-01 00:00:00.000 ===";
        assert!(state.record_line(&log_path, s1).1.is_some());
        assert!(state.record_line(&log_path, s2).1.is_some());

        // Серия однотипных Gtk-WARNING с разными временами GLib и hex-указателями схлопывается
        let w1 = "[2026-01-01 00:00:01.000] [pid=1] (volna:1): Gtk-WARNING **: 00:00:01.000: GtkBox 0x55a1b2c3d4e0 adjusted size";
        let w2 = "[2026-01-01 00:00:01.016] [pid=1] (volna:1): Gtk-WARNING **: 00:00:01.016: GtkBox 0x55a1b2c3d990 adjusted size";
        let w3 = "[2026-01-01 00:00:01.032] [pid=1] (volna:1): Gtk-WARNING **: 00:00:01.032: GtkBox 0x55a1b2c3e100 adjusted size";
        assert_eq!(state.record_line(&log_path, w1), (None, Some(w1.to_string())));
        assert_eq!(state.record_line(&log_path, w2), (None, None));
        assert_eq!(state.record_line(&log_path, w3), (None, None));

        // При приходе другой строки счётчик сбрасывается перед ней
        let next = "[2026-01-01 00:00:02.000] [pid=1] [ui] tab -> library";
        let (flushed, written) = state.record_line(&log_path, next);
        let summary = flushed.expect("summary line must be flushed before next unique line");
        assert!(summary.contains("[log] (volna:1): Gtk-WARNING"));
        assert!(summary.contains("(повторено 2 раз)"));
        assert_eq!(written.as_deref(), Some(next));

        // Строки с [PANIC] никогда не схлопываются
        let p1 = "[2026-01-01 00:00:03.000] [pid=1] [PANIC] thread=main location=a.rs:1:1 payload=boom";
        let p2 = "[2026-01-01 00:00:03.001] [pid=1] [PANIC] thread=main location=a.rs:1:1 payload=boom";
        assert!(state.record_line(&log_path, p1).1.is_some());
        assert!(state.record_line(&log_path, p2).1.is_some());

        // Серия однотипных Gtk-WARNING с меняющимися размерами `min <digits>` тоже схлопывается
        let m1 = "[2026-01-01 00:00:04.000] [pid=1] (volna:1): Gtk-WARNING **: 00:00:04.000: GtkBox 0x55a1b2c3d4e0 adjusted size horizontal min 26 must not decrease below min 69";
        let m2 = "[2026-01-01 00:00:04.016] [pid=1] (volna:1): Gtk-WARNING **: 00:00:04.016: GtkBox 0x55a1b2c3d990 adjusted size horizontal min 34 must not decrease below min 69";
        assert_eq!(state.record_line(&log_path, m1), (None, Some(m1.to_string())));
        assert_eq!(state.record_line(&log_path, m2), (None, None));

        let after_min = "[2026-01-01 00:00:05.000] [pid=1] [ui] tab -> wave";
        let (flushed_min, written_min) = state.record_line(&log_path, after_min);
        let min_summary = flushed_min.expect("summary line must be flushed after min-size dedup");
        assert!(min_summary.contains("[log] (volna:1): Gtk-WARNING"));
        assert!(min_summary.contains("(повторено 1 раз)"));
        assert_eq!(written_min.as_deref(), Some(after_min));

        // Проверяем как ring-буфер, так и файл на диске
        assert_eq!(state.ring.len(), 10);
        assert!(state.ring[3].contains("(повторено 2 раз)"));
        assert_eq!(state.ring[4], next);
        assert!(state.ring[8].contains("(повторено 1 раз)"));
        assert_eq!(state.ring[9], after_min);

        let disk = read_lines_lossy(&log_path);
        assert_eq!(disk.len(), 10);
        assert!(disk[3].contains("(повторено 2 раз)"));
        assert_eq!(disk[4], next);
        assert!(disk[8].contains("(повторено 1 раз)"));
        assert_eq!(disk[9], after_min);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
