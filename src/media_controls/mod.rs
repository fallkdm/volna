//! Системное управление плеером: MPRIS (D-Bus) на Linux, SMTC на Windows

#[cfg(windows)]
mod windows;

/// Запускает системные медиаклавиши. Окно нужно Windows (HWND), поэтому вызывать до `present()`
pub fn start<W: gtk4::prelude::IsA<gtk4::Widget> + gtk4::prelude::NativeExt>(
    window: &W,
    player: crate::player::PlayerManager,
    events: async_channel::Receiver<crate::player::PlayerUiEvent>,
) {
    #[cfg(windows)]
    windows::attach_window(window, player, events);
    #[cfg(not(windows))]
    {
        let _ = window;
        crate::mpris::spawn_mpris_server(player, events);
    }
}
