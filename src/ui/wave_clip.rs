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
    /// Обрезать зашитые поля яркостной эвристикой (только превью и свои клипы)
    crop: AtomicBool,
    /// Полноценный клип: выше прозрачность слоя
    full: AtomicBool,
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

    /// Нужен ли кроп полей у этого клипа
    pub fn crop(&self) -> bool {
        self.crop.load(Ordering::Relaxed)
    }

    /// Полноценный клип (другая прозрачность слоя)
    pub fn full(&self) -> bool {
        self.full.load(Ordering::Relaxed)
    }

    fn set_kind(&self, kind: crate::player::ClipKind) {
        self.crop.store(kind != crate::player::ClipKind::YouTubeFull, Ordering::Relaxed);
        self.full.store(kind == crate::player::ClipKind::YouTubeFull, Ordering::Relaxed);
    }
}

/// Позиция звука для стартового seek: (миллисекунды, играет ли звук)
pub type AudioPositionProvider = std::rc::Rc<dyn Fn() -> Option<(u64, bool)>>;

/// Этап запуска: преролл → стартовый seek → играем
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Startup {
    Preroll,
    Seeking,
    Running,
}

/// Запускает и останавливает воспроизведение клипа
pub struct ClipController {
    shared: Arc<ClipShared>,
    pipeline: std::cell::RefCell<Option<gstreamer::Pipeline>>,
    /// Наблюдатель шины: пока он жив, приходят EOS/ASYNC_DONE (при удалении снимается)
    watch: std::cell::RefCell<Option<gstreamer::bus::BusWatchGuard>>,
    provider: std::cell::RefCell<Option<AudioPositionProvider>>,
    /// Какой клип играет: от этого зависит EOS
    kind: std::cell::Cell<crate::player::ClipKind>,
    /// Видео реально в Playing (нужно, чтобы догонять звук после преролла)
    playing: std::cell::Cell<bool>,
}

impl ClipController {
    pub fn new(shared: Arc<ClipShared>) -> Self {
        Self {
            shared,
            pipeline: Default::default(),
            watch: Default::default(),
            provider: Default::default(),
            kind: std::cell::Cell::new(crate::player::ClipKind::YouTubeFull),
            playing: std::cell::Cell::new(false),
        }
    }

    /// Источник позиции звука: вызывается после преролла, чтобы стартовый seek был свежим.
    pub fn set_audio_provider(&self, provider: AudioPositionProvider) {
        *self.provider.borrow_mut() = Some(provider);
    }

    /// Останавливает клип; слой плавно гаснет в шейдере
    pub fn stop(&self) {
        self.watch.borrow_mut().take();
        if let Some(p) = self.pipeline.borrow_mut().take() {
            let _ = p.set_state(gstreamer::State::Null);
        }
        self.shared.active.store(false, Ordering::Relaxed);
    }

    /// Показывает локальный клип: преролл в Paused, точный seek к позиции звука, затем Playing.
    ///
    /// Блокирующих ожиданий нет: готовность приходит событиями шины, UI не замирает.
    pub fn play_request(&self, request: &crate::player::ClipRequest) {
        self.stop();
        self.kind.set(request.kind);
        self.shared.set_kind(request.kind);
        self.playing.set(false);

        let desc = format!(
            "filesrc location=\"{}\" ! decodebin ! videoconvert ! videoscale add-borders=false ! \
             video/x-raw,format=RGBA,pixel-aspect-ratio=1/1,width=[16,960],height=[16,960] ! \
             appsink name=sink sync=true max-buffers=2 drop=true",
            request.path.replace('\\', "\\\\").replace('"', "\\\"")
        );
        let Ok(element) = gstreamer::parse::launch(&desc) else {
            eprintln!("[clip] не удалось собрать пайплайн клипа");
            return;
        };
        let Ok(pipeline) = element.downcast::<gstreamer::Pipeline>() else { return };
        let Some(sink) = pipeline
            .by_name("sink")
            .and_then(|e| e.downcast::<gstreamer_app::AppSink>().ok())
        else {
            eprintln!("[clip] нет appsink в пайплайне клипа");
            return;
        };

        let shared = self.shared.clone();
        sink.set_callbacks(
            gstreamer_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gstreamer::FlowError::Eos)?;
                    let (Some(buffer), Some(caps)) = (sample.buffer(), sample.caps()) else {
                        return Ok(gstreamer::FlowSuccess::Ok);
                    };
                    let Some(s) = caps.structure(0) else {
                        return Ok(gstreamer::FlowSuccess::Ok);
                    };
                    let (Ok(w), Ok(h)) = (s.get::<i32>("width"), s.get::<i32>("height")) else {
                        return Ok(gstreamer::FlowSuccess::Ok);
                    };
                    let Ok(map) = buffer.map_readable() else {
                        return Ok(gstreamer::FlowSuccess::Ok);
                    };
                    if map.len() == (w * h * 4) as usize
                        && let Ok(mut slot) = shared.frame.lock()
                    {
                        *slot = Some(ClipFrame { width: w, height: h, data: map.to_vec() });
                        shared.fresh.store(true, Ordering::Release);
                    }
                    Ok(gstreamer::FlowSuccess::Ok)
                })
                .build(),
        );

        // машина состояний запуска: AsyncDone приходит и на преролл, и на seek
        let startup = std::rc::Rc::new(std::cell::Cell::new(Startup::Preroll));
        let kind = self.kind.get();
        if let Some(bus) = pipeline.bus() {
            let weak = pipeline.downgrade();
            let shared = self.shared.clone();
            let provider = self.provider.borrow().clone();
            let playing_out = std::rc::Rc::new(std::cell::Cell::new(false));
            let guard = bus.add_watch_local(move |_, msg| {
                use gstreamer::MessageView;
                let Some(pipeline) = weak.upgrade() else {
                    return glib::ControlFlow::Continue;
                };
                match msg.view() {
                    MessageView::AsyncDone(_) => match startup.get() {
                        // диагностика двухстадийного старта
                        Startup::Preroll => {
                            let position = provider.as_ref().and_then(|f| f());
                            let (start_ms, playing) = position.unwrap_or((0, false));
                            playing_out.set(playing);
                            if start_ms > 100 {
                                let _ = pipeline.seek_simple(
                                    gstreamer::SeekFlags::FLUSH | gstreamer::SeekFlags::ACCURATE,
                                    gstreamer::ClockTime::from_mseconds(start_ms),
                                );
                                startup.set(Startup::Seeking);
                            } else {
                                startup.set(Startup::Running);
                                shared.active.store(true, Ordering::Relaxed);
                                if playing {
                                    let _ = pipeline.set_state(gstreamer::State::Playing);
                                    playing_out.set(true);
                                }
                            }
                        }
                        Startup::Seeking => {
                            startup.set(Startup::Running);
                            shared.active.store(true, Ordering::Relaxed);
                            if playing_out.get() {
                                let _ = pipeline.set_state(gstreamer::State::Playing);
                            }
                        }
                        Startup::Running => {}
                    },
                    MessageView::Eos(_) => match kind {
                        // превью и свои клипы зациклены, полный клип держит последний кадр
                        crate::player::ClipKind::YouTubeFull => {}
                        _ => {
                            let _ = pipeline.seek_simple(
                                gstreamer::SeekFlags::FLUSH | gstreamer::SeekFlags::KEY_UNIT,
                                gstreamer::ClockTime::ZERO,
                            );
                        }
                    },
                    MessageView::Error(e) => {
                        eprintln!("[clip] ошибка воспроизведения: {}", e.error());
                        shared.active.store(false, Ordering::Relaxed);
                        return glib::ControlFlow::Break;
                    }
                    _ => {}
                }
                glib::ControlFlow::Continue
            });
            *self.watch.borrow_mut() = guard.ok();
        }

        // Пауза видео до преролла: первый кадр не должен мелькать раньше времени
        if pipeline.set_state(gstreamer::State::Paused).is_ok() {
            *self.pipeline.borrow_mut() = Some(pipeline);
        }
        self.playing.set(false);
    }

    /// Зацикленное беззвучное превью без синхронизации (фон страницы артиста/альбома)
    pub fn play(&self, path: &str) {
        self.stop();
        self.kind.set(crate::player::ClipKind::YandexPreview);
        self.shared.set_kind(crate::player::ClipKind::YandexPreview);
        let desc = format!(
            "filesrc location=\"{}\" ! decodebin ! videoconvert ! videoscale add-borders=false ! \
             video/x-raw,format=RGBA,pixel-aspect-ratio=1/1,width=[16,960],height=[16,960] ! \
             appsink name=sink sync=true max-buffers=2 drop=true",
            path.replace('\\', "\\\\").replace('"', "\\\"")
        );
        let Ok(element) = gstreamer::parse::launch(&desc) else { return };
        let Ok(pipeline) = element.downcast::<gstreamer::Pipeline>() else { return };
        let Some(sink) = pipeline
            .by_name("sink")
            .and_then(|e| e.downcast::<gstreamer_app::AppSink>().ok())
        else {
            return;
        };

        let shared = self.shared.clone();
        sink.set_callbacks(
            gstreamer_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gstreamer::FlowError::Eos)?;
                    let (Some(buffer), Some(caps)) = (sample.buffer(), sample.caps()) else {
                        return Ok(gstreamer::FlowSuccess::Ok);
                    };
                    let Some(s) = caps.structure(0) else {
                        return Ok(gstreamer::FlowSuccess::Ok);
                    };
                    let (Ok(w), Ok(h)) = (s.get::<i32>("width"), s.get::<i32>("height")) else {
                        return Ok(gstreamer::FlowSuccess::Ok);
                    };
                    let Ok(map) = buffer.map_readable() else {
                        return Ok(gstreamer::FlowSuccess::Ok);
                    };
                    if map.len() == (w * h * 4) as usize
                        && let Ok(mut slot) = shared.frame.lock()
                    {
                        *slot = Some(ClipFrame { width: w, height: h, data: map.to_vec() });
                        shared.fresh.store(true, Ordering::Release);
                    }
                    Ok(gstreamer::FlowSuccess::Ok)
                })
                .build(),
        );

        if let Some(bus) = pipeline.bus() {
            let weak = pipeline.downgrade();
            let guard = bus.add_watch_local(move |_, msg| {
                use gstreamer::MessageView;
                match msg.view() {
                    MessageView::Eos(_) => {
                        if let Some(p) = weak.upgrade() {
                            let _ = p.seek_simple(
                                gstreamer::SeekFlags::FLUSH | gstreamer::SeekFlags::KEY_UNIT,
                                gstreamer::ClockTime::ZERO,
                            );
                        }
                    }
                    MessageView::Error(e) => {
                        eprintln!("[clip] ошибка воспроизведения: {}", e.error());
                        return glib::ControlFlow::Break;
                    }
                    _ => {}
                }
                glib::ControlFlow::Continue
            });
            *self.watch.borrow_mut() = guard.ok();
        }

        if pipeline.set_state(gstreamer::State::Playing).is_ok() {
            self.shared.active.store(true, Ordering::Relaxed);
            *self.pipeline.borrow_mut() = Some(pipeline);
        }
    }

    /// Пауза: меняет состояние существующего пайплайна, файл не перезапускается
    pub fn is_playing(&self) -> bool {
        let guard = self.pipeline.borrow();
        guard
            .as_ref()
            .is_some_and(|pipeline| pipeline.current_state() == gstreamer::State::Playing)
    }

    pub fn pause(&self) {
        self.playing.set(false);
        if let Some(pipeline) = self.pipeline.borrow().as_ref() {
            let _ = pipeline.set_state(gstreamer::State::Paused);
        }
    }

    /// Продолжение с текущего кадра
    pub fn resume(&self) {
        self.playing.set(true);
        if let Some(pipeline) = self.pipeline.borrow().as_ref() {
            let _ = pipeline.set_state(gstreamer::State::Playing);
        }
    }

    /// Точная перемотка (караоке, слайдер)
    pub fn seek_ms(&self, ms: u64) {
        if let Some(pipeline) = self.pipeline.borrow().as_ref() {
            let _ = pipeline.seek_simple(
                gstreamer::SeekFlags::FLUSH | gstreamer::SeekFlags::ACCURATE,
                gstreamer::ClockTime::from_mseconds(ms),
            );
        }
    }

    /// Текущая позиция видео
    pub fn position_ms(&self) -> Option<u64> {
        let guard = self.pipeline.borrow();
        let pipeline = guard.as_ref()?;
        pipeline
            .query_position::<gstreamer::ClockTime>()
            .map(|time| time.mseconds())
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
