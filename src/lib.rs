pub mod api;
pub mod cache;
pub mod discord;
pub mod ipc;
pub mod log;
pub mod mpris;
pub mod player;
pub mod station;
pub mod ui;

pub use player::cast;

/// Возвращает общий переиспользуемый HTTP-клиент с таймаутом по умолчанию 15 секунд.
/// Индивидуальный таймаут и заголовки при необходимости задаются на конкретном `RequestBuilder`.
pub fn shared_http_client() -> &'static reqwest::Client {
    static HTTP_CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap_or_default()
    })
}

/// Возвращает переиспользуемый HTTP-клиент без пула idle-соединений (`pool_max_idle_per_host(0)`).
/// Нужен для CDN вроде `googlevideo.com`, где полуживые keep-alive сокеты приводят к обрывам.
pub fn no_keepalive_http_client() -> &'static reqwest::Client {
    static HTTP_CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .pool_max_idle_per_host(0)
            .build()
            .unwrap_or_default()
    })
}

/// Возвращает освободившуюся heap-память системе (glibc не делает это сам).
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub fn trim_allocator() {
    unsafe extern "C" {
        fn malloc_trim(pad: usize) -> std::os::raw::c_int;
    }
    unsafe {
        let _ = malloc_trim(0);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub fn trim_allocator() {}


#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[repr(C)]
struct MallInfo2 {
    arena: usize,
    ordblks: usize,
    smblks: usize,
    hblks: usize,
    hblkhd: usize,
    usmblks: usize,
    fsmblks: usize,
    uordblks: usize,
    fordblks: usize,
    keepcost: usize,
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
unsafe extern "C" {
    fn mallinfo2() -> MallInfo2;
}

/// Возвращает текущий срез статистики памяти glibc (mallinfo2).
pub fn memory_info() -> Option<serde_json::Value> {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        let info = unsafe { mallinfo2() };
        let to_mb = |bytes: usize| -> f64 {
            ((bytes as f64) / (1024.0 * 1024.0) * 100.0).round() / 100.0
        };
        Some(serde_json::json!({
            "arena_mb": to_mb(info.arena),
            "uordblks_mb": to_mb(info.uordblks),
            "fordblks_mb": to_mb(info.fordblks),
            "hblkhd_mb": to_mb(info.hblkhd),
        }))
    }
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    {
        None
    }
}

/// Возвращает текущий VmRSS процесса в MiB из `/proc/self/status` без heap-аллокаций.
pub fn current_rss_mb() -> f64 {
    #[cfg(target_os = "linux")]
    {
        use std::io::Read;
        let Ok(mut file) = std::fs::File::open("/proc/self/status") else {
            return 0.0;
        };
        let mut buf = [0u8; 4096];
        let Ok(n) = file.read(&mut buf) else {
            return 0.0;
        };
        for line in buf[..n].split(|&b| b == b'\n') {
            if let Some(rest) = line.strip_prefix(b"VmRSS:") {
                if let Ok(s) = std::str::from_utf8(rest) {
                    if let Some(kb) = s.split_whitespace().next().and_then(|v| v.parse::<u64>().ok()) {
                        return kb as f64 / 1024.0;
                    }
                }
                break;
            }
        }
        0.0
    }
    #[cfg(not(target_os = "linux"))]
    {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_info_structure() {
        let info = memory_info();
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        {
            let val = info.expect("memory_info should be Some on Linux GNU");
            assert!(val.get("arena_mb").is_some());
            assert!(val.get("uordblks_mb").is_some());
            assert!(val.get("fordblks_mb").is_some());
            assert!(val.get("hblkhd_mb").is_some());
        }
    }

    #[test]
    fn test_current_rss_mb_positive() {
        #[cfg(target_os = "linux")]
        assert!(current_rss_mb() > 0.0);
    }
}

