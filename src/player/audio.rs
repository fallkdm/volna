use gstreamer_play::Play;
use std::sync::{Arc, Mutex};

/// Аудио-бэкенд на базе GStreamer Play
pub struct AudioBackend {
    play: Play,
    is_playing: Arc<Mutex<bool>>,
}

impl AudioBackend {
    /// Инициализирует аудио бэкенд
    pub fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        gstreamer::init()?;
        let play = Play::new(None::<gstreamer_play::PlayVideoRenderer>);
        let is_playing = Arc::new(Mutex::new(false));

        Ok(Self { play, is_playing })
    }

    /// Загружает и начинает воспроизведение трека по прямой ссылке
    pub fn play_url(&self, url: &str) {
        self.play.set_uri(Some(url));
        self.play.play();
        if let Ok(mut lock) = self.is_playing.lock() {
            *lock = true;
        }
    }

    /// Возобновляет воспроизведение
    pub fn play(&self) {
        self.play.play();
        if let Ok(mut lock) = self.is_playing.lock() {
            *lock = true;
        }
    }

    /// Приостанавливает воспроизведение
    pub fn pause(&self) {
        self.play.pause();
        if let Ok(mut lock) = self.is_playing.lock() {
            *lock = false;
        }
    }

    /// Переключает состояние воспроизведения
    pub fn toggle_play(&self) -> bool {
        let current = self.is_playing();
        if current {
            self.pause();
            false
        } else {
            self.play();
            true
        }
    }

    /// Проверяет, играет ли сейчас аудио
    pub fn is_playing(&self) -> bool {
        self.is_playing.lock().map(|l| *l).unwrap_or(false)
    }

    /// Перемотка к позиции в секундах
    pub fn seek_seconds(&self, seconds: u64) {
        let time = gstreamer::ClockTime::from_seconds(seconds);
        self.play.seek(time);
    }

    /// Текущая позиция воспроизведения в секундах
    pub fn position_seconds(&self) -> Option<u64> {
        self.play.position().map(|t| t.seconds())
    }

    /// Полная длительность трека в секундах
    pub fn duration_seconds(&self) -> Option<u64> {
        self.play.duration().map(|t| t.seconds())
    }

    /// Установка громкости (от 0.0 до 1.0)
    pub fn set_volume(&self, volume: f64) {
        self.play.set_volume(volume.clamp(0.0, 1.0));
    }

    /// Получение текущей громкости
    pub fn volume(&self) -> f64 {
        self.play.volume()
    }
}
