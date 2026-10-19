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

/// Границы картинки без чёрных полос по краям (клипы бывают вертикальными, вписанными в кадр 16:9):
/// (x0, y0, x1, y1) в пикселях. None, если картинки не видно (затемнение) или вырезка слишком мала.
pub fn content_bounds(f: &ClipFrame) -> Option<(i32, i32, i32, i32)> {
    let (w, h) = (f.width as usize, f.height as usize);
    if w < 16 || h < 16 || f.data.len() < w * h * 4 {
        return None;
    }
    let lit = |x: usize, y: usize| {
        let i = (y * w + x) * 4;
        f.data[i].max(f.data[i + 1]).max(f.data[i + 2]) > 24
    };
    // столбец или строка с содержимым: хотя бы 3% отсчётов не чёрные
    let col = |x: usize| (0..h).step_by(4).filter(|&y| lit(x, y)).count() * 100 > h / 4 * 3;
    let row = |y: usize| (0..w).step_by(4).filter(|&x| lit(x, y)).count() * 100 > w / 4 * 3;
    let x0 = (0..w).find(|&x| col(x))?;
    let x1 = (0..w).rev().find(|&x| col(x))? + 1;
    let y0 = (0..h).find(|&y| row(y))?;
    let y1 = (0..h).rev().find(|&y| row(y))? + 1;
    // небольшой отступ внутрь: край полосы размыт сжатием
    let (x0, x1, y0, y1) = ((x0 + 2).min(x1), x1.saturating_sub(2), (y0 + 2).min(y1), y1.saturating_sub(2));
    if (x1 - x0) * 5 < w || (y1 - y0) * 5 < h {
        return None;
    }
    Some((x0 as i32, y0 as i32, x1 as i32, y1 as i32))
}

/// Заставка вместо клипа: Яндекс иногда отдаёт обложку (с размытыми полями) как видео без движения.
/// Клип без движения за весь ролик клипом не считаем.
pub fn is_static(path: &str) -> bool {
    let src = format!("filesrc location=\"{}\" ! decodebin", path.replace('\\', "\\\\").replace('"', "\\\""));
    source_is_static(&src)
}

/// Декодирует ролик целиком в 64×36 серых кадров и сравнивает каждый с первым
fn source_is_static(source: &str) -> bool {
    use gstreamer::prelude::*;
    const SIZE: usize = 64 * 36;
    let desc = format!("{source} ! videoconvert ! videoscale ! video/x-raw,format=GRAY8,width=64,height=36 ! appsink name=sink sync=false");
    let Ok(element) = gstreamer::parse::launch(&desc) else { return false };
    let Ok(pipeline) = element.downcast::<gstreamer::Pipeline>() else { return false };
    let Some(sink) = pipeline.by_name("sink").and_then(|e| e.downcast::<gstreamer_app::AppSink>().ok()) else { return false };
    if pipeline.set_state(gstreamer::State::Playing).is_err() {
        return false;
    }
    let (mut first, mut frames, mut max_diff) = (Vec::<u8>::new(), 0, 0.0_f64);
    while let Ok(sample) = sink.pull_sample() {
        let Some(map) = sample.buffer().and_then(|b| b.map_readable().ok()) else { continue };
        if map.len() < SIZE {
            continue;
        }
        if first.is_empty() {
            first = map[..SIZE].to_vec();
        }
        let diff = first.iter().zip(&map[..SIZE]).map(|(a, b)| a.abs_diff(*b) as f64).sum::<f64>() / SIZE as f64;
        max_diff = max_diff.max(diff);
        frames += 1;
    }
    let _ = pipeline.set_state(gstreamer::State::Null);
    // у настоящих клипов отличие от первого кадра больше 20, у заставок меньше 0.3
    frames >= 4 && max_diff < 3.0
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

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: i32, h: i32, lit: impl Fn(i32, i32) -> bool) -> ClipFrame {
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let v = if lit(x, y) { 200 } else { 0 };
                data.extend_from_slice(&[v, v, v, 255]);
            }
        }
        ClipFrame { width: w, height: h, data }
    }

    #[test]
    fn pillarbox_is_cropped() {
        let f = frame(640, 360, |x, _| (200..440).contains(&x));
        let (x0, y0, x1, y1) = content_bounds(&f).unwrap();
        assert!((200..210).contains(&x0) && (430..440).contains(&x1));
        assert!(y0 < 10 && y1 > 350);
    }

    #[test]
    fn still_picture_is_static_and_motion_is_not() {
        gstreamer::init().unwrap();
        assert!(source_is_static("videotestsrc num-buffers=30 pattern=black ! video/x-raw,width=160,height=90"));
        assert!(!source_is_static("videotestsrc num-buffers=30 pattern=ball ! video/x-raw,width=160,height=90"));
    }

    #[test]
    fn black_frame_has_no_bounds() {
        assert!(content_bounds(&frame(640, 360, |_, _| false)).is_none());
    }
}
