//! Системные медиаклавиши и оверлей Windows (SMTC) через souvlaki

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use glib::Cast;
use gtk4::prelude::*;
use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig};

use crate::api::models::Track;
use crate::player::{PlayerManager, PlayerUiEvent};

thread_local! {
    // controls должны жить до конца работы приложения
    static ACTIVE_CONTROLS: RefCell<Option<Rc<RefCell<MediaControls>>>> = const { RefCell::new(None) };
}

fn set_track(controls: &mut MediaControls, track: &Track) {
    let artist = track.artists_string();
    let cover = track.cover_url(400);
    let meta = MediaMetadata {
        title: Some(&track.title),
        artist: Some(&artist),
        album: track.albums.first().and_then(|a| a.title.as_deref()),
        cover_url: cover.as_deref(),
        duration: track.duration_ms.map(Duration::from_millis),
    };
    if let Err(e) = controls.set_metadata(meta) {
        eprintln!("[media_controls] не удалось обновить метаданные: {e}");
    }
}

fn set_playing(controls: &mut MediaControls, player: &PlayerManager, playing: bool) {
    let progress = Some(MediaPosition(Duration::from_secs(player.audio.position_seconds().unwrap_or(0))));
    let state = if playing { MediaPlayback::Playing { progress } } else { MediaPlayback::Paused { progress } };
    if let Err(e) = controls.set_playback(state) {
        eprintln!("[media_controls] не удалось обновить состояние воспроизведения: {e}");
    }
}

fn setup<W: IsA<gtk4::Widget> + gtk4::prelude::NativeExt>(win: &W, player: PlayerManager, rx: async_channel::Receiver<PlayerUiEvent>) {
    let hwnd = win
        .surface()
        .and_then(|s| s.downcast::<gdk4_win32::Win32Surface>().ok())
        .map(|s| s.handle().0 as *mut std::ffi::c_void);
    let Some(hwnd) = hwnd else {
        eprintln!("[media_controls] нет HWND окна: системные медиаклавиши отключены");
        return;
    };
    let mut controls = match MediaControls::new(PlatformConfig { dbus_name: "volna", display_name: "Волна", hwnd: Some(hwnd) }) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[media_controls] souvlaki не запустился: {e}");
            return;
        }
    };

    let p = player.clone();
    let attached = controls.attach(move |event| match event {
        MediaControlEvent::Toggle => p.toggle_play(),
        MediaControlEvent::Play | MediaControlEvent::Pause => {
            let want_play = matches!(event, MediaControlEvent::Play);
            if want_play != p.audio.is_playing() {
                p.toggle_play();
            }
        }
        MediaControlEvent::Next => p.next_track(),
        MediaControlEvent::Previous => p.prev_track(),
        MediaControlEvent::SetPosition(pos) => p.seek(pos.0.as_secs()),
        _ => {}
    });
    if let Err(e) = attached {
        eprintln!("[media_controls] не удалось подключить обработчик: {e}");
        return;
    }

    // Трек мог начаться раньше, чем окно получило HWND
    if let Some(track) = player.state.lock().ok().and_then(|s| s.current_track.clone()) {
        set_track(&mut controls, &track);
        set_playing(&mut controls, &player, player.audio.is_playing());
    }

    let controls = Rc::new(RefCell::new(controls));
    ACTIVE_CONTROLS.with(|c| *c.borrow_mut() = Some(controls.clone()));

    glib::spawn_future_local(async move {
        while let Ok(event) = rx.recv().await {
            let mut c = controls.borrow_mut();
            match event {
                PlayerUiEvent::TrackStarted { track, .. } => {
                    set_track(&mut c, &track);
                    set_playing(&mut c, &player, true);
                }
                PlayerUiEvent::PlaybackToggled(playing) => set_playing(&mut c, &player, playing),
                _ => {}
            }
        }
    });
}

/// Подключает SMTC, когда у окна появляется HWND (вызывать до `present()`)
pub fn attach_window<W: IsA<gtk4::Widget> + gtk4::prelude::NativeExt>(win: &W, player: PlayerManager, rx: async_channel::Receiver<PlayerUiEvent>) {
    if win.is_realized() {
        setup(win, player, rx);
    } else {
        // realize приходит один раз, поэтому FnOnce-данные кладём в Option
        let args = RefCell::new(Some((player, rx)));
        win.connect_realize(move |w| {
            if let Some((player, rx)) = args.take() {
                setup(w, player, rx);
            }
        });
    }
}
