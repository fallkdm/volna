use std::sync::Arc;
use mpris_server::zbus::fdo;
use mpris_server::{
    Metadata, PlaybackRate, PlaybackStatus, PlayerInterface, Property, RootInterface,
    Server, Time, TrackId, Volume,
};

use crate::player::{PlayerManager, PlayerUiEvent};

pub struct MprisHandler {
    player: PlayerManager,
}

impl MprisHandler {
    pub fn new(player: PlayerManager) -> Self {
        Self { player }
    }
}

impl RootInterface for MprisHandler {
    async fn identity(&self) -> fdo::Result<String> {
        Ok("Волна (Volna)".to_string())
    }

    async fn desktop_entry(&self) -> fdo::Result<String> {
        Ok("io.github.fallkdm.Volna".to_string())
    }

    async fn can_quit(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn quit(&self) -> fdo::Result<()> {
        std::process::exit(0);
    }

    async fn can_raise(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn raise(&self) -> fdo::Result<()> {
        Ok(())
    }

    async fn fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn set_fullscreen(&self, _fullscreen: bool) -> mpris_server::zbus::Result<()> {
        Ok(())
    }

    async fn can_set_fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn has_track_list(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn supported_uri_schemes(&self) -> fdo::Result<Vec<String>> {
        Ok(vec!["https".to_string(), "http".to_string(), "file".to_string()])
    }

    async fn supported_mime_types(&self) -> fdo::Result<Vec<String>> {
        Ok(vec!["audio/mpeg".to_string(), "audio/flac".to_string()])
    }
}

impl PlayerInterface for MprisHandler {
    async fn next(&self) -> fdo::Result<()> {
        self.player.next_track();
        Ok(())
    }

    async fn previous(&self) -> fdo::Result<()> {
        self.player.prev_track();
        Ok(())
    }

    async fn pause(&self) -> fdo::Result<()> {
        self.player.audio.pause();
        self.player.emit(PlayerUiEvent::PlaybackToggled(false));
        Ok(())
    }

    async fn play_pause(&self) -> fdo::Result<()> {
        self.player.toggle_play();
        Ok(())
    }

    async fn stop(&self) -> fdo::Result<()> {
        self.player.audio.pause();
        self.player.seek(0);
        self.player.emit(PlayerUiEvent::PlaybackToggled(false));
        Ok(())
    }

    async fn play(&self) -> fdo::Result<()> {
        self.player.audio.play();
        self.player.emit(PlayerUiEvent::PlaybackToggled(true));
        Ok(())
    }

    async fn seek(&self, offset: Time) -> fdo::Result<()> {
        let current = self.player.audio.position_seconds().unwrap_or(0) as i64;
        let new_pos = (current + offset.as_secs()).max(0) as u64;
        self.player.seek(new_pos);
        Ok(())
    }

    async fn set_position(&self, _track_id: TrackId, position: Time) -> fdo::Result<()> {
        self.player.seek(position.as_secs().max(0) as u64);
        Ok(())
    }

    async fn open_uri(&self, _uri: String) -> fdo::Result<()> {
        Ok(())
    }

    async fn playback_status(&self) -> fdo::Result<PlaybackStatus> {
        if self.player.audio.is_playing() {
            Ok(PlaybackStatus::Playing)
        } else {
            Ok(PlaybackStatus::Paused)
        }
    }

    async fn loop_status(&self) -> fdo::Result<mpris_server::LoopStatus> {
        Ok(mpris_server::LoopStatus::None)
    }

    async fn set_loop_status(&self, _loop_status: mpris_server::LoopStatus) -> mpris_server::zbus::Result<()> {
        Ok(())
    }

    async fn rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn set_rate(&self, _rate: PlaybackRate) -> mpris_server::zbus::Result<()> {
        Ok(())
    }

    async fn minimum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn maximum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn shuffle(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn set_shuffle(&self, _shuffle: bool) -> mpris_server::zbus::Result<()> {
        Ok(())
    }

    async fn metadata(&self) -> fdo::Result<Metadata> {
        let mut meta = Metadata::new();
        if let Ok(state) = self.player.state.lock() {
            if let Some(track) = &state.current_track {
                meta.set_title(Some(&track.title));
                meta.set_artist(Some(vec![track.artists_string()]));
                if let Some(album) = track.albums.first() {
                    meta.set_album(album.title.as_deref());
                }
                if let Some(dur_ms) = track.duration_ms {
                    meta.set_length(Some(Time::from_millis(dur_ms as i64)));
                }
                if let Ok(track_id) = TrackId::try_from(format!("/org/mpris/MediaPlayer2/Track/{}", track.id)) {
                    meta.set_trackid(Some(track_id));
                }
                if let Some(cover_url) = track.cover_url(400) {
                    meta.set_art_url(Some(cover_url));
                }
            }
        }
        Ok(meta)
    }

    async fn volume(&self) -> fdo::Result<Volume> {
        Ok(self.player.audio.volume())
    }

    async fn set_volume(&self, volume: Volume) -> mpris_server::zbus::Result<()> {
        self.player.set_volume(volume);
        Ok(())
    }

    async fn position(&self) -> fdo::Result<Time> {
        let secs = self.player.audio.position_seconds().unwrap_or(0);
        Ok(Time::from_secs(secs as i64))
    }

    async fn can_go_next(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn can_go_previous(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn can_play(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn can_pause(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn can_seek(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn can_control(&self) -> fdo::Result<bool> {
        Ok(true)
    }
}

/// Запускает MPRIS2 сервер в фоновом Tokio таске и слушает обновления состояния плеера
pub fn spawn_mpris_server(
    player: PlayerManager,
    event_rx: async_channel::Receiver<PlayerUiEvent>,
) {
    tokio::spawn(async move {
        let handler = MprisHandler::new(player.clone());
        let server = match Server::new("volna", handler).await {
            Ok(s) => Arc::new(s),
            Err(e) => {
                eprintln!("Не удалось запустить MPRIS сервер: {}", e);
                return;
            }
        };

        // Слушаем события плеера и обновляем свойства MPRIS D-Bus
        while let Ok(event) = event_rx.recv().await {
            match event {
                PlayerUiEvent::TrackStarted { track, .. } => {
                    let mut meta = Metadata::new();
                    meta.set_title(Some(&track.title));
                    meta.set_artist(Some(vec![track.artists_string()]));
                    if let Some(album) = track.albums.first() {
                        meta.set_album(album.title.as_deref());
                    }
                    if let Some(dur_ms) = track.duration_ms {
                        meta.set_length(Some(Time::from_millis(dur_ms as i64)));
                    }
                    if let Ok(track_id) = TrackId::try_from(format!("/org/mpris/MediaPlayer2/Track/{}", track.id)) {
                        meta.set_trackid(Some(track_id));
                    }
                    if let Some(cover_url) = track.cover_url(400) {
                        meta.set_art_url(Some(cover_url));
                    }

                    let _ = server
                        .properties_changed([
                            Property::PlaybackStatus(PlaybackStatus::Playing),
                            Property::Metadata(meta),
                        ])
                        .await;
                }
                PlayerUiEvent::PlaybackToggled(is_playing) => {
                    let status = if is_playing {
                        PlaybackStatus::Playing
                    } else {
                        PlaybackStatus::Paused
                    };
                    let _ = server.properties_changed([Property::PlaybackStatus(status)]).await;
                }
                _ => {}
            }
        }
    });
}
