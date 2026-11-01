//! Потокобезопасный статус фонового клипа — читается из IPC, пишется менеджером и UI.
//!
//! `PlayerManager` не может читать `ClipController` напрямую: там `RefCell` и главный
//! поток GTK. Поэтому обмен идёт через этот маленький снимок: UI пишет позицию и
//! признак активного локального клипа, менеджер — источник, формат, загрузку и ошибку.

use std::sync::Mutex;

/// Откуда сейчас клип (или почему его нет).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipSource {
    /// Полноценный клип YouTube.
    Mv,
    /// Статичная обложка (`… - Topic`, ATV или низкий битрейт).
    ArtTrack,
    /// Короткое зацикленное превью Яндекса.
    YandexPreview,
    /// Свой клип альбома из `custom_clips`.
    Custom,
    /// Клипа нет.
    None,
}

impl ClipSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mv => "mv",
            Self::ArtTrack => "art-track",
            Self::YandexPreview => "yandex-preview",
            Self::Custom => "custom",
            Self::None => "none",
        }
    }
}

/// Снимок для `volna cmd clip status`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClipStatus {
    pub enabled: bool,
    pub source: Option<String>,
    pub track_id: Option<String>,
    pub format: Option<String>,
    pub attempt_id: Option<u64>,
    /// Существует локальный pipeline текущего клипа (включая паузу).
    pub local_clip_active: bool,
    pub position_ms: Option<u64>,
    pub download_ms: Option<u64>,
    pub bytes: Option<u64>,
    pub error: Option<String>,
    pub sync_degraded: bool,
}

#[derive(Debug, Default)]
pub struct ClipTelemetry {
    inner: Mutex<ClipStatus>,
}

impl ClipTelemetry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> ClipStatus {
        self.inner.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn update(&self, f: impl FnOnce(&mut ClipStatus)) {
        if let Ok(mut status) = self.inner.lock() {
            f(&mut status);
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.update(|s| s.enabled = enabled);
    }

    /// Новый трек: старые позиция, формат, ошибка и загрузка не должны остаться.
    pub fn begin_track(&self, track_id: &str, attempt_id: u64) {
        self.update(|s| {
            s.track_id = Some(track_id.to_string());
            s.attempt_id = Some(attempt_id);
            s.source = None;
            s.format = None;
            s.position_ms = None;
            s.local_clip_active = false;
            s.download_ms = None;
            s.bytes = None;
            s.error = None;
            s.sync_degraded = false;
        });
    }

    pub fn set_source(&self, source: ClipSource) {
        self.update(|s| s.source = Some(source.as_str().to_string()));
    }

    pub fn set_format(&self, format: Option<String>) {
        self.update(|s| s.format = format);
    }

    pub fn set_download(&self, millis: Option<u64>, bytes: Option<u64>) {
        self.update(|s| {
            s.download_ms = millis;
            s.bytes = bytes;
        });
    }

    pub fn set_error(&self, code: Option<&str>) {
        self.update(|s| s.error = code.map(str::to_string));
    }

    pub fn set_local_active(&self, active: bool) {
        self.update(|s| s.local_clip_active = active);
    }

    pub fn set_position(&self, position_ms: Option<u64>) {
        self.update(|s| s.position_ms = position_ms);
    }

    pub fn set_sync_degraded(&self, degraded: bool) {
        self.update(|s| s.sync_degraded = degraded);
    }

    /// Трек без клипа: источник `none`, старая загрузка не висит.
    pub fn no_clip(&self) {
        self.update(|s| {
            s.source = Some(ClipSource::None.as_str().to_string());
            s.position_ms = None;
            s.local_clip_active = false;
            s.download_ms = None;
            s.bytes = None;
        });
    }

    /// Арт-трек: источник объясняет причину, ошибки нет.
    pub fn art_track(&self) {
        self.update(|s| {
            s.source = Some(ClipSource::ArtTrack.as_str().to_string());
            s.error = None;
            s.position_ms = None;
            s.local_clip_active = false;
        });
    }

    /// Полный сброс (stop/выключение клипов).
    pub fn reset(&self) {
        let enabled = self.snapshot().enabled;
        self.update(|s| {
            *s = ClipStatus {
                enabled,
                ..Default::default()
            };
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_track_clears_previous_download_and_error() {
        let t = ClipTelemetry::new();
        t.set_enabled(true);
        t.begin_track("old", 1);
        t.set_download(Some(1200), Some(15_000_000));
        t.set_error(Some("download-timeout"));

        t.begin_track("new", 2);
        let s = t.snapshot();
        assert!(s.enabled);
        assert_eq!(s.track_id.as_deref(), Some("new"));
        assert_eq!(s.attempt_id, Some(2));
        assert_eq!(s.download_ms, None);
        assert_eq!(s.bytes, None);
        assert_eq!(s.error, None);
        assert!(!s.local_clip_active);
    }

    #[test]
    fn art_track_has_no_error_and_no_position() {
        let t = ClipTelemetry::new();
        t.begin_track("x", 1);
        t.set_error(Some("io"));
        t.art_track();
        let s = t.snapshot();
        assert_eq!(s.source.as_deref(), Some("art-track"));
        assert_eq!(s.error, None);
        assert_eq!(s.position_ms, None);
    }

    #[test]
    fn reset_keeps_enabled_flag() {
        let t = ClipTelemetry::new();
        t.set_enabled(true);
        t.begin_track("x", 3);
        t.set_source(ClipSource::Mv);
        t.reset();
        let s = t.snapshot();
        assert!(s.enabled);
        assert_eq!(s.track_id, None);
        assert_eq!(s.source, None);
    }
}
