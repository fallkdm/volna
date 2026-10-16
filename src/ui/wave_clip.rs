//! Превью-клип на фоне Волны: короткий зацикленный ролик декодируется GStreamer в RGBA-кадры,
//! а в видеокарту кадры загружает `wave_gl` (текстура, смешивается шейдером).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use gstreamer::prelude::*;

/// Готовый кадр (RGBA, строки без выравнивания)
pub struct ClipFrame {
    pub width: i32,
    pub height: i32,
    pub data: Vec<u8>,
}

/// Общее состояние между потоком декодера и отрисовкой
#[derive(Default)]
pub struct ClipShared {
    frame: Mutex<Option<ClipFrame>>,
    fresh: AtomicBool,
    active: AtomicBool,
}

impl ClipShared {
    /// Идёт ли клип (нужно ли перерисовывать каждый кадр и показывать слой)
    pub fn active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    /// Новый кадр, если появился с прошлого вызова
    pub fn take_fresh(&self) -> Option<ClipFrame> {
        if self.fresh.swap(false, Ordering::AcqRel) {
            self.frame.lock().ok()?.take()
        } else {
            None
        }
    }
}

/// Запускает и останавливает воспроизведение клипа
pub struct ClipController {
    shared: Arc<ClipShared>,
    pipeline: std::cell::RefCell<Option<gstreamer::Pipeline>>,
    /// Наблюдатель шины: пока он жив, на конец ролика приходит перемотка в начало (при удалении наблюдатель снимается)
    watch: std::cell::RefCell<Option<gstreamer::bus::BusWatchGuard>>,
}

impl ClipController {
    pub fn new(shared: Arc<ClipShared>) -> Self {
        Self { shared, pipeline: Default::default(), watch: Default::default() }
    }

    /// Останавливает клип; слой плавно гаснет в шейдере
    pub fn stop(&self) {
        self.watch.borrow_mut().take();
        if let Some(p) = self.pipeline.borrow_mut().take() {
            let _ = p.set_state(gstreamer::State::Null);
        }
        self.shared.active.store(false, Ordering::Relaxed);
    }

    /// Запускает зацикленное воспроизведение файла без звука
    pub fn play(&self, path: &str) {
        self.stop();
        let desc = format!(
            "filesrc location=\"{}\" ! decodebin ! videoconvert ! videoscale ! \
             video/x-raw,format=RGBA,width=640 ! appsink name=sink sync=true max-buffers=2 drop=true",
            path.replace('\\', "\\\\").replace('"', "\\\"")
        );
        let Ok(element) = gstreamer::parse::launch(&desc) else { return };
        let Ok(pipeline) = element.downcast::<gstreamer::Pipeline>() else { return };
        let Some(sink) = pipeline.by_name("sink").and_then(|e| e.downcast::<gstreamer_app::AppSink>().ok()) else { return };

        let shared = self.shared.clone();
        sink.set_callbacks(
            gstreamer_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gstreamer::FlowError::Eos)?;
                    let (Some(buffer), Some(caps)) = (sample.buffer(), sample.caps()) else { return Ok(gstreamer::FlowSuccess::Ok) };
                    let Some(s) = caps.structure(0) else { return Ok(gstreamer::FlowSuccess::Ok) };
                    let (Ok(w), Ok(h)) = (s.get::<i32>("width"), s.get::<i32>("height")) else { return Ok(gstreamer::FlowSuccess::Ok) };
                    let Ok(map) = buffer.map_readable() else { return Ok(gstreamer::FlowSuccess::Ok) };
                    // RGBA без выравнивания строк: ширина кратна 4 после videoscale, но проверим размер
                    if map.len() == (w * h * 4) as usize {
                        if let Ok(mut slot) = shared.frame.lock() {
                            *slot = Some(ClipFrame { width: w, height: h, data: map.to_vec() });
                            shared.fresh.store(true, Ordering::Release);
                        }
                    }
                    Ok(gstreamer::FlowSuccess::Ok)
                })
                .build(),
        );

        // конец ролика: перемотка в начало (петля)
        if let Some(bus) = pipeline.bus() {
            let weak = pipeline.downgrade();
            let guard = bus.add_watch_local(move |_, msg| {
                use gstreamer::MessageView;
                match msg.view() {
                    MessageView::Eos(_) => {
                        if let Some(p) = weak.upgrade() {
                            let _ = p.seek_simple(gstreamer::SeekFlags::FLUSH | gstreamer::SeekFlags::KEY_UNIT, gstreamer::ClockTime::ZERO);
                        }
                        glib::ControlFlow::Continue
                    }
                    MessageView::Error(e) => {
                        eprintln!("[clip] ошибка воспроизведения: {}", e.error());
                        glib::ControlFlow::Break
                    }
                    _ => glib::ControlFlow::Continue,
                }
            });
            *self.watch.borrow_mut() = guard.ok();
        }

        if pipeline.set_state(gstreamer::State::Playing).is_ok() {
            self.shared.active.store(true, Ordering::Relaxed);
            *self.pipeline.borrow_mut() = Some(pipeline);
        }
    }
}
