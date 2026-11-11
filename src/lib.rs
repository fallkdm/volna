pub mod api;
pub mod cache;
pub mod discord;
pub mod ipc;
pub mod mpris;
pub mod player;
pub mod station;
pub mod ui;

pub use player::cast;

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
}

