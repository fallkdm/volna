//! Воспроизведение на Яндекс Станции: пока станция привязана, треки играют на ней, а звук приложения молчит.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::manager::{PlayerManager, PlayerUiEvent};
use crate::api::models::Track;
use crate::station::{StationDevice, StationSession, StationState};

/// Подключённая станция и последнее известное её состояние
pub struct CastLink {
    pub session: Arc<StationSession>,
    state: StationState,
    /// Когда пришло `state`: между сообщениями позиция досчитывается по часам
    at: Instant,
    /// Когда на станцию отправили трек (первые секунды состояние ещё от прежнего трека)
    played_at: Instant,
    /// Музыку на станцию запустило это приложение (а не голос или телефон)
    started_here: bool,
}

impl PlayerManager {
    pub fn cast_name(&self) -> Option<String> {
        self.cast.lock().unwrap().as_ref().map(|c| c.session.device.name.clone())
    }

    pub fn cast_active(&self) -> bool {
        self.cast.lock().unwrap().is_some()
    }

    /// Позиция на станции в мс; None, если станция не привязана
    pub fn cast_position_ms(&self) -> Option<u64> {
        let guard = self.cast.lock().unwrap();
        let c = guard.as_ref()?;
        let extra = if c.state.playing { c.at.elapsed().as_secs_f64() } else { 0.0 };
        Some(((c.state.progress + extra).max(0.0) * 1000.0) as u64)
    }

    pub fn cast_duration_secs(&self) -> Option<u64> {
        self.cast.lock().unwrap().as_ref().map(|c| c.state.duration as u64).filter(|d| *d > 0)
    }

    /// Включает трек на станции, если она привязана; true — трек ушёл на станцию
    pub(super) fn cast_play(&self, track: &Track) -> bool {
        let mut guard = self.cast.lock().unwrap();
        let Some(c) = guard.as_mut() else { return false };
        c.session.play_track(&track.id);
        c.played_at = Instant::now();
        c.started_here = true;
        drop(guard);
        self.audio.pause();
        self.emit(PlayerUiEvent::LoadingState(false));
        self.emit(PlayerUiEvent::StreamQuality("Станция".into()));
        self.emit(PlayerUiEvent::PlaybackToggled(true));
        true
    }

    /// Пауза/продолжение на станции; Some(играет ли теперь)
    pub(super) fn cast_toggle(&self) -> Option<bool> {
        let mut guard = self.cast.lock().unwrap();
        let c = guard.as_mut()?;
        c.state.playing = !c.state.playing;
        c.at = Instant::now();
        if c.state.playing { c.session.play() } else { c.session.pause() }
        Some(c.state.playing)
    }

    /// Явная пауза или продолжение на станции; false — станция не привязана
    pub(super) fn cast_set_playing(&self, playing: bool) -> bool {
        let mut guard = self.cast.lock().unwrap();
        let Some(c) = guard.as_mut() else { return false };
        c.state.playing = playing;
        c.at = Instant::now();
        if playing { c.session.play() } else { c.session.pause() }
        true
    }

    pub(super) fn cast_set_volume(&self, volume: f64) -> bool {
        match self.cast.lock().unwrap().as_ref() {
            Some(c) => {
                c.session.set_volume(volume);
                true
            }
            None => false,
        }
    }

    pub(super) fn cast_seek(&self, seconds: u64) -> bool {
        match self.cast.lock().unwrap().as_ref() {
            Some(c) => {
                c.session.command(serde_json::json!({"command": "rewind", "position": seconds}));
                true
            }
            None => false,
        }
    }

    /// Привязывает станцию: дальше всё играет на ней
    pub fn cast_connect(&self, device: StationDevice) {
        let me = self.clone();
        tokio::spawn(async move {
            let client = me.client.lock().unwrap().clone();
            let name = device.name.clone();
            let device_id = device.device_id.clone();
            let (session, mut rx) = match StationSession::connect(&client, device).await {
                Ok(v) => v,
                Err(e) => {
                    me.emit(PlayerUiEvent::Error(format!("Не удалось подключиться к станции «{}»: {}", name, e)));
                    return;
                }
            };
            let session = Arc::new(session);
            me.audio.pause();
            *me.cast.lock().unwrap() = Some(CastLink { session: session.clone(), state: StationState::default(), at: Instant::now(), played_at: Instant::now(), started_here: false });
            if let Ok(mut cfg) = crate::api::auth::load_config() {
                cfg.station_id = Some(device_id);
                let _ = crate::api::auth::save_config(&cfg);
            }
            me.emit(PlayerUiEvent::CastChanged(Some(name.clone())));
            me.emit(PlayerUiEvent::StatusMessage(format!("Станция «{}» подключена: музыка играет на ней", name)));

            let mut was_playing = false;
            while let Some(st) = rx.recv().await {
                let ended = {
                    let mut guard = me.cast.lock().unwrap();
                    // отключили вручную или привязали другую станцию
                    let Some(c) = guard.as_mut().filter(|c| Arc::ptr_eq(&c.session, &session)) else { return };
                    let prev = std::mem::replace(&mut c.state, st.clone());
                    c.at = Instant::now();
                    // трек дошёл до конца и станция начала заново или перешла к своему: ведём очередь сами
                    prev.duration > 30.0
                        && prev.duration - prev.progress < 3.0
                        && st.progress + 1.0 < prev.progress
                        && c.played_at.elapsed() > Duration::from_secs(5)
                };
                if st.playing != was_playing {
                    was_playing = st.playing;
                    me.emit(PlayerUiEvent::PlaybackToggled(st.playing));
                }
                me.emit(PlayerUiEvent::Progress { current_sec: st.progress as u64, total_sec: st.duration as u64 });
                if ended {
                    me.on_track_finished();
                }
            }
            // соединение оборвалось
            let mut guard = me.cast.lock().unwrap();
            if guard.as_ref().is_some_and(|c| Arc::ptr_eq(&c.session, &session)) {
                *guard = None;
                drop(guard);
                me.emit(PlayerUiEvent::CastChanged(None));
                me.emit(PlayerUiEvent::StatusMessage(format!("Станция «{}» отключилась", name)));
            }
        });
    }

    /// При выходе из приложения останавливает станцию, если музыку на ней включило приложение
    pub fn cast_stop_on_exit(&self) {
        let guard = self.cast.lock().unwrap();
        let Some(c) = guard.as_ref().filter(|c| c.started_here && c.state.playing) else { return };
        c.session.pause();
        drop(guard);
        // команда уходит из фоновой задачи: даём ей время до завершения процесса
        std::thread::sleep(Duration::from_millis(300));
    }

    /// Отвязывает станцию и забывает выбор. Музыка, которую включило приложение, останавливается на станции
    /// и продолжается в приложении с того же места.
    pub fn cast_disconnect(&self) {
        let Some(link) = self.cast.lock().unwrap().take() else { return };
        if let Ok(mut cfg) = crate::api::auth::load_config() {
            cfg.station_id = None;
            let _ = crate::api::auth::save_config(&cfg);
        }
        self.emit(PlayerUiEvent::CastChanged(None));
        self.emit(PlayerUiEvent::StatusMessage("Станция отключена".into()));
        if !link.started_here {
            return; // на станции играет не то, что включило приложение: не трогаем
        }
        link.session.pause();
        if !link.state.playing {
            return;
        }
        let pos_ms = ((link.state.progress + link.at.elapsed().as_secs_f64()).max(0.0) * 1000.0) as u64;
        let Some(track) = self.state.lock().unwrap().current_track.clone() else { return };
        self.play_track_stream(&track);
        // поток ещё открывается: перематываем на прежнее место, когда он начнёт играть
        let audio = self.audio.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            audio.seek_ms(pos_ms);
        });
    }

    /// При запуске подключается к ранее выбранной станции, если она в сети
    pub fn cast_autoconnect(&self) {
        let Some(id) = crate::api::auth::load_config().ok().and_then(|c| c.station_id) else { return };
        let me = self.clone();
        tokio::spawn(async move {
            let client = me.client.lock().unwrap().clone();
            if !client.has_token() {
                return;
            }
            if let Ok(found) = crate::station::find_stations(&client, Duration::from_secs(4)).await {
                if let Some(dev) = found.into_iter().find(|d| d.device_id == id) {
                    me.cast_connect(dev);
                }
            }
        });
    }
}
