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
