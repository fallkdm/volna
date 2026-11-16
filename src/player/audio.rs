use gstreamer::prelude::*;
use gstreamer_play::{Play, PlaySignalAdapter};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const PLAY_STATE_STOPPED: u8 = 0;
const PLAY_STATE_BUFFERING: u8 = 1;
const PLAY_STATE_PAUSED: u8 = 2;
const PLAY_STATE_PLAYING: u8 = 3;

/// Результат ожидания фактического старта воспроизведения слота.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartOutcome {
    Started,
    Failed(String),
    Cancelled,
}

type ErrorCallback = Arc<Mutex<Option<Arc<dyn Fn(String) + Send + Sync>>>>;
type BufferingCallback = Arc<Mutex<Option<Arc<dyn Fn(u8) + Send + Sync>>>>;

/// Проверяет, должен ли слот транслировать события буферизации в UI:
/// допускается активный воспроизводимый слот или не устаревший стартующий слот.
#[inline]
fn should_emit_buffering(
    slot_index: usize,
    active_slot_idx: usize,
    is_slot_playing: bool,
    slot_gen: u64,
    start_wait_gen: u64,
) -> bool {
    let is_active = active_slot_idx % 2 == slot_index && is_slot_playing;
    let is_starting = slot_gen != 0 && start_wait_gen == slot_gen;
    is_active || is_starting
}

// spectrum: nfft = 2*bands-2 = 1024 → ~43 Гц на полосу при 44.1 кГц, окно ~23 мс
const SPECTRUM_BANDS: u32 = 513;
const SPECTRUM_THRESHOLD_DB: f32 = -80.0;
const SPECTRUM_INTERVAL_NS: u64 = 33_000_000;
// Кик и суббас
const BASS_LO_HZ: f32 = 40.0;
const BASS_HI_HZ: f32 = 150.0;
// Ниже GATE_DB — тишина; RANGE_DB — окно под пиком трека, которое растягивается на 0..1
const GATE_DB: f32 = -60.0;
const RANGE_DB: f32 = 12.0;
const PEAK_FALL_DB: f32 = 0.1; // за сообщение (~3 дБ/с)
const ATTACK: f32 = 0.8;
const RELEASE: f32 = 0.2;

/// Детектор баса: dB низких полос spectrum → 0..1 с быстрой атакой и медленным спадом
struct BassMeter {
    peak_db: f32,
    level: f32,
}

impl BassMeter {
    fn new() -> Self {
        Self { peak_db: GATE_DB + RANGE_DB, level: 0.0 }
    }

    /// `mags_db` — магнитуды полос с нулевой (dB), `band_hz` — ширина полосы
    fn push(&mut self, mags_db: &[f32], band_hz: f32) -> f32 {
        // Средняя мощность полос 40–150 Гц, обратно в dB
        let (sum, n) = mags_db
            .iter()
            .enumerate()
            .filter(|(i, _)| (BASS_LO_HZ..=BASS_HI_HZ).contains(&((*i as f32 + 0.5) * band_hz)))
            .fold((0.0_f32, 0), |(s, n), (_, db)| (s + 10f32.powf(db / 10.0), n + 1));
        let db = if n > 0 { 10.0 * (sum / n as f32).log10() } else { SPECTRUM_THRESHOLD_DB };
        // Автоусиление: пик медленно сползает, так что тихие треки тоже качают; пол пика = гейт
        self.peak_db = db.max(self.peak_db - PEAK_FALL_DB).max(GATE_DB + RANGE_DB);
        // Экспандер x²: удар кика ≈ 1, ровный бас между ударами — почти 0
        let x = ((db - self.peak_db + RANGE_DB) / RANGE_DB).clamp(0.0, 1.0);
        let target = x * x;
        let k = if target > self.level { ATTACK } else { RELEASE };
        self.level += (target - self.level) * k;
        self.level
    }
}

/// Уровни баса в порядке потока: (running-time конца интервала, 0..1)
type BassQueue = Arc<Mutex<VecDeque<(u64, f32)>>>;

/// Вставляет spectrum в audio-filter playbin (до регулятора громкости — уровень от неё не зависит)
/// и складывает уровни баса в `queue` с отметкой времени потока
fn attach_bass_analyzer(
    pipeline: &gstreamer::Element,
    queue: BassQueue,
) -> Result<(), Box<dyn std::error::Error>> {
    let filter = gstreamer::parse::bin_from_description(
        &format!(
            "audioconvert ! spectrum bands={SPECTRUM_BANDS} threshold={SPECTRUM_THRESHOLD_DB} \
             interval={SPECTRUM_INTERVAL_NS} post-messages=true message-magnitude=true ! audioconvert"
        ),
        true,
    )?;
    let bus = pipeline.bus().ok_or("у конвейера нет шины")?;
    pipeline.set_property("audio-filter", &filter);

    let meter = Mutex::new(BassMeter::new());
    // sync-message приходит в потоке стриминга и не трогает bus-watch самого gstreamer-play (EOS и пр.)
    bus.enable_sync_message_emission();
    bus.connect_sync_message(Some("element"), move |_, msg| {
        let Some(s) = msg.structure().filter(|s| s.has_name("spectrum")) else { return };
        let Some(spectrum) = msg.src().and_then(|o| o.downcast_ref::<gstreamer::Element>()) else { return };
        let (Ok(mags), Ok(rt), Ok(dur)) = (
            s.get::<gstreamer::List>("magnitude"),
            s.get::<u64>("running-time"),
            s.get::<u64>("duration"),
        ) else {
            return;
        };
        let rate = spectrum
            .static_pad("sink")
            .and_then(|p| p.current_caps())
            .and_then(|c| c.structure(0).and_then(|cs| cs.get::<i32>("rate").ok()))
            .unwrap_or(44_100);
        let band_hz = rate as f32 / 2.0 / mags.len().max(1) as f32;
        let db: Vec<f32> = mags
            .iter()
            .take((BASS_HI_HZ / band_hz) as usize + 1)
            .filter_map(|v| v.get().ok())
            .collect();
        let value = meter.lock().map(|mut m| m.push(&db, band_hz)).unwrap_or(0.0);

        let end = rt.saturating_add(dur);
        if let Ok(mut q) = queue.lock() {
            // Время потока пошло назад — seek или новый трек: старые значения не нужны
            if q.back().is_some_and(|&(t, _)| t > end) {
                q.clear();
            }
            // Экран волны скрыт и никто не читает — держим только последние ~8 с
            if q.len() >= 256 {
                q.pop_front();
            }
            q.push_back((end, value));
        }
    });
    Ok(())
}

/// Сбрасывает громкость системного sink (pulsesink / pipewiresink) в 100% (unmuted),
/// чтобы PipeWire/WirePlumber stream-restore никогда не запоминал 0% для `volna`.
fn reset_system_sink_volume(element: &gstreamer::Element) {
    use gtk4::glib::prelude::ObjectExt;
    if let Some(prop) = element.find_property("volume") {
        if prop.value_type() == f64::static_type() {
            element.set_property("volume", 1.0f64);
        }
    }
    if let Some(prop) = element.find_property("mute") {
        if prop.value_type() == bool::static_type() {
            element.set_property("mute", false);
        }
    }
}

/// Устанавливает программную громкость (0.0..=1.0) на GStreamer-элементе `volume`.
fn set_element_volume(volume_el: &gstreamer::Element, vol: f64) {
    use gtk4::glib::prelude::ObjectExt;
    volume_el.set_property("volume", vol.clamp(0.0, 1.0));
}

/// Читает текущую программную громкость (0.0..=1.0) с GStreamer-элемента `volume`.
fn element_volume(volume_el: &gstreamer::Element) -> f64 {
    use gtk4::glib::prelude::ObjectExt;
    volume_el.property::<f64>("volume").clamp(0.0, 1.0)
}

/// Собирает изолированный `audio-sink` bin (`volume ! autoaudiosink`) для слота.
/// Вся регулировка громкости, преролл (0.0) и кроссфейд происходят внутри `volume`,
/// а реальный системный sink всегда остаётся на 1.0 (100%).
fn build_slot_audio_sink(
    index: usize,
    initial_volume: f64,
) -> Result<(gstreamer::Bin, gstreamer::Element), Box<dyn std::error::Error + Send + Sync>> {
    let volume = gstreamer::ElementFactory::make("volume")
        .name(format!("slot-{}-volume", index))
        .build()?;
    set_element_volume(&volume, initial_volume);

    let auto_sink = gstreamer::ElementFactory::make("autoaudiosink")
        .name(format!("slot-{}-autosink", index))
        .build()?;

    if let Some(bin) = auto_sink.downcast_ref::<gstreamer::Bin>() {
        bin.connect_element_added(|_, child| {
            reset_system_sink_volume(child);
            if let Some(inner_bin) = child.downcast_ref::<gstreamer::Bin>() {
                inner_bin.connect_element_added(|_, grandchild| {
                    reset_system_sink_volume(grandchild);
                });
            }
        });
    }

    let sink_bin = gstreamer::Bin::with_name(&format!("slot-{}-sink-bin", index));
    sink_bin.add_many([&volume, &auto_sink])?;
    volume.link(&auto_sink)?;

    let volume_sink_pad = volume
        .static_pad("sink")
        .ok_or("у элемента volume отсутствует sink pad")?;
    let ghost_pad = gstreamer::GhostPad::with_target(&volume_sink_pad)?;
    sink_bin.add_pad(&ghost_pad)?;

    Ok((sink_bin, volume))
}

/// Контекст фиксации ошибки слота из любого источника (`PlaySignalAdapter` или `pipeline.bus()`).
/// Гарантирует обновление состояния слота без GLib `MainContext` и однократный вызов глобального `on_error`.
#[derive(Clone)]
struct SlotErrorCtx {
    index: usize,
    last_error: Arc<Mutex<Option<String>>>,
    error_gen: Arc<AtomicU64>,
    is_prerolled: Arc<AtomicBool>,
    is_playing: Arc<Mutex<bool>>,
    slot_gen: Arc<AtomicU64>,
    start_wait_gen: Arc<AtomicU64>,
    reported_error_gen: Arc<AtomicU64>,
    active_slot: Arc<AtomicUsize>,
    on_error: ErrorCallback,
}

impl SlotErrorCtx {
    fn record(&self, source: &str, err_text: String, details_str: Option<String>) {
        let full_err = match details_str {
            Some(d) if !d.is_empty() => format!("{err_text} ({d})"),
            _ => err_text,
        };
        let is_first_for_run = if let Ok(mut guard) = self.last_error.lock() {
            if guard.is_none() {
                *guard = Some(full_err.clone());
                true
            } else {
                false
            }
        } else {
            true
        };
        if is_first_for_run {
            eprintln!("[gst slot {}] ошибка ({}): {}", self.index, source, full_err);
        }

        self.error_gen.fetch_add(1, Ordering::SeqCst);
        self.is_prerolled.store(false, Ordering::SeqCst);
        if let Ok(mut lp) = self.is_playing.lock() {
            *lp = false;
        }

        let cur_gen = self.slot_gen.load(Ordering::SeqCst);
        let handled_by_waiter =
            cur_gen != 0 && self.start_wait_gen.load(Ordering::SeqCst) == cur_gen;
        if handled_by_waiter {
            // Ошибку уже забирает wait_for_slot_ready; помечаем cur_gen обработанным,
            // чтобы запоздалый сигнал из GLib MainContext не вызвал on_error повторно после сброса start_wait_gen.
            self.reported_error_gen.store(cur_gen, Ordering::SeqCst);
            return;
        }
        if self.active_slot.load(Ordering::SeqCst) % 2 == self.index {
            let prev_reported = self.reported_error_gen.swap(cur_gen, Ordering::SeqCst);
            if prev_reported != cur_gen {
                let cb_opt = self.on_error.lock().ok().and_then(|g| g.clone());
                if let Some(cb) = cb_opt {
                    cb(full_err);
                }
            }
        }
    }
}

/// Слот плеера GStreamer для двухканального воспроизведения и бесшовного кроссфейда
struct PlayerSlot {
    play: Play,
    volume: gstreamer::Element,
    _adapter: PlaySignalAdapter,
    is_playing: Arc<Mutex<bool>>,
    bass: BassQueue,
    pub current_uri: Arc<Mutex<Option<String>>>,
    pub is_prerolled: Arc<AtomicBool>,
    pub slot_gen: Arc<AtomicU64>,
    pub last_error: Arc<Mutex<Option<String>>>,
    pub error_gen: Arc<AtomicU64>,
    pub state_seq: Arc<AtomicU64>,
    pub upward_seq: Arc<AtomicU64>,
    pub eos_seq: Arc<AtomicU64>,
    pub play_state: Arc<AtomicU8>,
    /// Поколение `slot_gen`, для которого сейчас выполняется ожидание старта с `on_outcome`.
    pub start_wait_gen: Arc<AtomicU64>,
    /// Последнее `slot_gen`, для которого уже был вызван глобальный `on_error` (защита от дубля).
    pub reported_error_gen: Arc<AtomicU64>,
}

impl PlayerSlot {
    fn set_slot_volume(&self, vol: f64) {
        set_element_volume(&self.volume, vol);
    }

    fn slot_volume(&self) -> f64 {
        element_volume(&self.volume)
    }

    fn bump_gen(&self) -> u64 {
        self.slot_gen.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn clear_error(&self) {
        if let Ok(mut guard) = self.last_error.lock() {
            *guard = None;
        }
        self.reported_error_gen.store(u64::MAX, Ordering::SeqCst);
    }

    fn wait_handle(&self, initial_saw_upward: bool) -> SlotWaitHandle {
        SlotWaitHandle {
            play: self.play.clone(),
            error_gen: self.error_gen.clone(),
            initial_error_gen: self.error_gen.load(Ordering::SeqCst),
            last_error: self.last_error.clone(),
            state_seq: self.state_seq.clone(),
            initial_state_seq: self.state_seq.load(Ordering::SeqCst),
            upward_seq: self.upward_seq.clone(),
            initial_upward_seq: self.upward_seq.load(Ordering::SeqCst),
            eos_seq: self.eos_seq.clone(),
            initial_eos_seq: self.eos_seq.load(Ordering::SeqCst),
            play_state: self.play_state.clone(),
            initial_saw_upward,
        }
    }

    fn new(
        index: usize,
        active_slot: Arc<AtomicUsize>,
        on_error: ErrorCallback,
        on_buffering: BufferingCallback,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let play = Play::new(None::<gstreamer_play::PlayVideoRenderer>);
        let adapter = PlaySignalAdapter::new(&play);

        let is_playing = Arc::new(Mutex::new(false));
        let current_uri = Arc::new(Mutex::new(None));
        let is_prerolled = Arc::new(AtomicBool::new(false));
        let slot_gen = Arc::new(AtomicU64::new(0));
        let last_error = Arc::new(Mutex::new(None));
        let error_gen = Arc::new(AtomicU64::new(0));
        let state_seq = Arc::new(AtomicU64::new(0));
        let upward_seq = Arc::new(AtomicU64::new(0));
        let eos_seq = Arc::new(AtomicU64::new(0));
        let play_state = Arc::new(AtomicU8::new(PLAY_STATE_STOPPED));
        let start_wait_gen = Arc::new(AtomicU64::new(0));
        let reported_error_gen = Arc::new(AtomicU64::new(u64::MAX));

        let err_ctx = SlotErrorCtx {
            index,
            last_error: last_error.clone(),
            error_gen: error_gen.clone(),
            is_prerolled: is_prerolled.clone(),
            is_playing: is_playing.clone(),
            slot_gen: slot_gen.clone(),
            start_wait_gen: start_wait_gen.clone(),
            reported_error_gen: reported_error_gen.clone(),
            active_slot: active_slot.clone(),
            on_error,
        };
        let adapter_err_ctx = err_ctx.clone();
        adapter.connect_error(move |_, err, details| {
            let err_text = err.to_string();
            let details_str = details.map(|d| d.to_string());
            adapter_err_ctx.record("adapter", err_text, details_str);
        });
        adapter.connect_warning(move |_, err, _| {
            eprintln!("[gst slot {}] предупреждение: {}", index, err);
        });
        let st_seq = state_seq.clone();
        let st_code = play_state.clone();
        adapter.connect_state_changed(move |_, state| {
            let code = match state {
                gstreamer_play::PlayState::Stopped => PLAY_STATE_STOPPED,
                gstreamer_play::PlayState::Buffering => PLAY_STATE_BUFFERING,
                gstreamer_play::PlayState::Paused => PLAY_STATE_PAUSED,
                gstreamer_play::PlayState::Playing => PLAY_STATE_PLAYING,
                _ => PLAY_STATE_STOPPED,
            };
            st_code.store(code, Ordering::SeqCst);
            st_seq.fetch_add(1, Ordering::SeqCst);
            eprintln!("[gst slot {}] состояние: {:?}", index, state);
        });
        let buf_active = active_slot;
        let buf_playing = is_playing.clone();
        let buf_slot_gen = slot_gen.clone();
        let buf_wait_gen = start_wait_gen.clone();
        let buf_cb_arc = on_buffering;
        adapter.connect_buffering(move |_, percent| {
            let pct = percent.clamp(0, 100) as u8;
            if pct % 25 == 0 {
                eprintln!("[gst slot {}] буфер {}%", index, pct);
            }
            let is_slot_playing = buf_playing.lock().map(|l| *l).unwrap_or(false);
            if should_emit_buffering(
                index,
                buf_active.load(Ordering::SeqCst),
                is_slot_playing,
                buf_slot_gen.load(Ordering::SeqCst),
                buf_wait_gen.load(Ordering::SeqCst),
            ) {
                let cb_opt = buf_cb_arc.lock().ok().and_then(|g| g.clone());
                if let Some(cb) = cb_opt {
                    cb(pct);
                }
            }
        });

        let pipeline = play.pipeline();
        use gtk4::glib::prelude::ObjectExt;
        if pipeline.find_property("flags").is_some() {
            // download, а не buffering: queue2 тянет поток целиком во временный файл настолько
            // быстро, насколько позволяет сеть, и не переигрывает BUFFERING->PLAYING каждые
            // buffer-duration секунд (это давало провалы на 4-7 секунде трека). Так же было до
            // коммита с предзагрузкой, и так же работает преролл: в PAUSED буфер заполняется
            // заранее, поэтому старт следующего трека остаётся мгновенным.
            // soft-volume не указываем: громкость контролируется нашим элементом `volume` в audio-sink.
            pipeline.set_property_from_str("flags", "audio+download");
        }
        if pipeline.find_property("buffer-duration").is_some() {
            let _ = pipeline.set_property("buffer-duration", 2_000_000_000i64); // 2 секунды буфера
        }
        if pipeline.find_property("buffer-size").is_some() {
            let _ = pipeline.set_property("buffer-size", 512 * 1024i32); // 512 КБ
        }
        if pipeline.find_property("ring-buffer-max-size").is_some() {
            // 0 = отключено: в режиме audio+download playbin скачивает поток целиком во временный
            // файл без 8-мегабайтного потолка в ОЗУ (иначе после 8 МБ чтение из souphttpsrc
            // блокируется и HTTP-сокет закрывается сервером по таймауту).
            let _ = pipeline.set_property("ring-buffer-max-size", 0u64);
        }

        let (sink_bin, volume) = build_slot_audio_sink(index, 1.0)?;
        pipeline.set_property("audio-sink", &sink_bin);

        pipeline.connect("element-setup", false, |values| {
            if let Some(element) = values.get(1).and_then(|v| v.get::<gstreamer::Element>().ok()) {
                let name = element.name();
                let factory_name = element.factory().map(|f| f.name().to_string()).unwrap_or_default();
                if name.starts_with("hlsdemux") || factory_name.contains("hlsdemux") {
                    if element.has_property("low-watermark-fragments") {
                        let _ = element.set_property("low-watermark-fragments", 2.0f64);
                    }
                    if element.has_property("high-watermark-fragments") {
                        let _ = element.set_property("high-watermark-fragments", 5.0f64);
                    }
                    if element.has_property("connection-speed") {
                        let _ = element.set_property("connection-speed", 10000u32);
                    }
                }
            }
            None
        });

        let source_uri = current_uri.clone();
        pipeline.connect("source-setup", false, move |values| {
            if let Some(source) = values.get(1).and_then(|v| v.get::<gstreamer::Element>().ok()) {
                let fallback_uri = source_uri.lock().ok().and_then(|g| g.clone());
                configure_http_source(&source, fallback_uri.as_deref());
            }
            None
        });

        // Прямая подписка на sync-сигналы шины конвейера: фиксирует ошибки и смену
        // состояния независимо от того, крутится ли GLib MainContext (актуально для CLI/tokio::test).
        if let Some(bus) = pipeline.bus() {
            bus.enable_sync_message_emission();
            let err_ctx_bus = err_ctx;
            bus.connect_sync_message(Some("error"), move |_, msg| {
                if let gstreamer::MessageView::Error(err) = msg.view() {
                    let err_text = err.error().to_string();
                    let details_str = err.debug().map(|d| d.to_string());
                    err_ctx_bus.record("bus", err_text, details_str);
                }
            });

            let bus_pipeline_weak = ObjectExt::downgrade(&pipeline);
            let bus_st_seq = state_seq.clone();
            let bus_st_code = play_state.clone();
            let bus_upward_seq = upward_seq.clone();
            bus.connect_sync_message(Some("state-changed"), move |_, msg| {
                let Some(pipe) = bus_pipeline_weak.upgrade() else { return };
                if msg
                    .src()
                    .is_some_and(|src| src == pipe.upcast_ref::<gstreamer::Object>())
                {
                    if let gstreamer::MessageView::StateChanged(sc) = msg.view() {
                        let moved_upward = (matches!(
                            sc.old(),
                            gstreamer::State::Null | gstreamer::State::Ready
                        ) && matches!(
                            sc.current(),
                            gstreamer::State::Paused | gstreamer::State::Playing
                        )) || (sc.old() == gstreamer::State::Paused
                            && sc.current() == gstreamer::State::Playing);
                        if moved_upward {
                            bus_upward_seq.fetch_add(1, Ordering::SeqCst);
                        }

                        let next_code = match sc.current() {
                            gstreamer::State::Playing => Some(PLAY_STATE_PLAYING),
                            gstreamer::State::Paused => Some(PLAY_STATE_PAUSED),
                            gstreamer::State::Null | gstreamer::State::Ready
                                if !matches!(
                                    sc.old(),
                                    gstreamer::State::Null | gstreamer::State::Ready
                                ) =>
                            {
                                Some(PLAY_STATE_STOPPED)
                            }
                            _ => None,
                        };
                        if let Some(code) = next_code {
                            bus_st_code.store(code, Ordering::SeqCst);
                            bus_st_seq.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                }
            });

            let bus_eos_seq = eos_seq.clone();
            bus.connect_sync_message(Some("eos"), move |_, _| {
                bus_eos_seq.fetch_add(1, Ordering::SeqCst);
            });
        }

        let bass = BassQueue::default();
        if let Err(e) = attach_bass_analyzer(&play.pipeline(), bass.clone()) {
            eprintln!("Анализатор баса недоступен для слота {}: {}", index, e);
        }

        Ok(Self {
            play,
            volume,
            _adapter: adapter,
            is_playing,
            bass,
            current_uri,
            is_prerolled,
            slot_gen,
            last_error,
            error_gen,
            state_seq,
            upward_seq,
            eos_seq,
            play_state,
            start_wait_gen,
            reported_error_gen,
        })
    }
}

/// Отложенный преролл, который нужно выполнить в освободившийся слот.
struct PendingPreroll<'a> {
    pending: &'a Arc<Mutex<Option<String>>>,
    /// URI слота, который сейчас звучит: его повторно прогревать не нужно.
    playing_uri: &'a Arc<Mutex<Option<String>>>,
    slot_play: &'a Play,
    slot_volume: &'a gstreamer::Element,
    slot_uri: &'a Arc<Mutex<Option<String>>>,
    slot_prerolled: &'a Arc<AtomicBool>,
    slot_gen: &'a Arc<AtomicU64>,
    slot_last_error: &'a Arc<Mutex<Option<String>>>,
    slot_idx: usize,
}

impl PendingPreroll<'_> {
    fn flush(&self) {
        let Some(url) = self.pending.lock().ok().and_then(|mut p| p.take()) else { return };
        if self.playing_uri.lock().map(|c| c.as_deref() == Some(url.as_str())).unwrap_or(false) {
            return;
        }
        preroll_slot(
            self.slot_play,
            self.slot_volume,
            self.slot_uri,
            self.slot_prerolled,
            self.slot_gen,
            self.slot_last_error,
            self.slot_idx,
            &url,
        );
    }
}

pub fn is_hls_url(url: &str) -> bool {
    url.contains(".m3u8")
        || url.contains("/s/v1/")
        || (url.contains("vkuseraudio.net") && !url.contains(".mp3") && !url.contains("type=2"))
}

fn pipeline_buffering_config(url: &str) -> (&'static str, i64, i32) {
    if url.starts_with("file://") || is_hls_url(url) {
        // Для локальных файлов и HLS плейлистов флаги без buffering и без download,
        // чтобы playbin3 не создавал паразитный queue2 с сетевым ожиданием
        ("audio", -1i64, -1i32)
    } else if url.contains("googlevideo.com") {
        // Для YouTube Music (Opus ~160k, весь трек 3–4 МБ): 512 КБ / 2 с хватает для
        // мгновенного старта (~26 секунд звука) без долгого зависания в Buffering
        ("audio+download", 2_000_000_000i64, 512 * 1024i32)
    } else {
        // Для остальных прогрессивных HTTP-потоков (Яндекс MP3/FLAC и др.):
        // с ring-buffer-max-size = 0 и audio+download queue2 сразу начинает играть после первых
        // 512 КБ и беспрерывно докачивает весь файл до конца во временное хранилище без пауз чтения сокета
        ("audio+download", 2_000_000_000i64, 512 * 1024i32)
    }
}

fn configure_http_source(source: &gstreamer::Element, fallback_uri: Option<&str>) {
    if source.has_property("user-agent") {
        let _ = source.set_property(
            "user-agent",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_7_3) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15",
        );
    }
    if source.has_property("iradio-mode") {
        let _ = source.set_property("iradio-mode", false);
    }
    if source.has_property("blocksize") {
        let _ = source.set_property("blocksize", 131072u32); // 128 КБ
    }
    if source.has_property("keep-alive") {
        let _ = source.set_property("keep-alive", true);
    }
    if source.has_property("timeout") {
        let _ = source.set_property("timeout", 15u32);
    }
    if source.has_property("retries") {
        let _ = source.set_property("retries", 5i32);
    }

    let location_uri = if source.has_property("location") {
        source.property::<Option<String>>("location")
    } else {
        None
    };
    let is_googlevideo = location_uri
        .as_deref()
        .is_some_and(|u| u.contains("googlevideo.com"))
        || fallback_uri.is_some_and(|u| u.contains("googlevideo.com"));
    if is_googlevideo && source.has_property("extra-headers") {
        let headers = gstreamer::Structure::builder("extra-headers")
            .field("Referer", "https://www.youtube.com/")
            .field("Origin", "https://www.youtube.com")
            .build();
        let _ = source.set_property("extra-headers", &headers);
    }
}

fn configure_pipeline_for_url(pipeline: &gstreamer::Element, url: &str) {
    let (flags, buffer_duration, buffer_size) = pipeline_buffering_config(url);
    if pipeline.find_property("flags").is_some() {
        pipeline.set_property_from_str("flags", flags);
    }
    if pipeline.find_property("buffer-duration").is_some() {
        let _ = pipeline.set_property("buffer-duration", buffer_duration);
    }
    if pipeline.find_property("buffer-size").is_some() {
        let _ = pipeline.set_property("buffer-size", buffer_size);
    }
}

/// Переводит слот в состояние «прогретого» преролла: поток открыт, начало декодировано,
/// программная громкость 0.0 (внутри GStreamer-элемента `volume`, не затрагивая системный микшер).
/// Повторный вызов для того же URL ничего не делает, чтобы не рвать уже установленное соединение
/// и не сбрасывать буфер.
fn preroll_slot(
    play: &Play,
    volume: &gstreamer::Element,
    current_uri: &Arc<Mutex<Option<String>>>,
    is_prerolled: &Arc<AtomicBool>,
    slot_gen: &Arc<AtomicU64>,
    last_error: &Arc<Mutex<Option<String>>>,
    idx: usize,
    url: &str,
) {
    if let Ok(cur) = current_uri.lock() {
        if cur.as_deref() == Some(url) && is_prerolled.load(Ordering::SeqCst) {
            return;
        }
    }
    slot_gen.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut err) = last_error.lock() {
        *err = None;
    }
    configure_pipeline_for_url(&play.pipeline(), url);
    play.stop();
    set_element_volume(volume, 0.0);
    if let Ok(mut cur) = current_uri.lock() {
        *cur = Some(url.to_string());
    }
    play.set_uri(Some(url));
    play.pause(); // Начинает сетевую буферизацию до 100% без воспроизведения звука
    is_prerolled.store(true, Ordering::SeqCst);
    eprintln!("[audio] Аппаратная предбуферизация следующего трека в слот {}: {}", idx, url);
}

enum WaitSlotResult {
    Playing,
    Failed(String),
    Timeout(gstreamer::State),
    Cancelled,
}

struct SlotWaitHandle {
    play: Play,
    error_gen: Arc<AtomicU64>,
    initial_error_gen: u64,
    last_error: Arc<Mutex<Option<String>>>,
    state_seq: Arc<AtomicU64>,
    initial_state_seq: u64,
    upward_seq: Arc<AtomicU64>,
    initial_upward_seq: u64,
    eos_seq: Arc<AtomicU64>,
    initial_eos_seq: u64,
    play_state: Arc<AtomicU8>,
    initial_saw_upward: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum StartPollSignal {
    Pending,
    Ready,
    Failed(String),
}

struct StartProgressTracker {
    saw_upward_progress: bool,
    stopped_seen_at: Option<tokio::time::Instant>,
}

impl StartProgressTracker {
    fn new(initial_saw_upward: bool) -> Self {
        Self {
            saw_upward_progress: initial_saw_upward,
            stopped_seen_at: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn step(
        &mut self,
        now: tokio::time::Instant,
        pipeline_state: gstreamer::State,
        error_changed: bool,
        last_error: Option<String>,
        state_changed: bool,
        cur_play_state: u8,
        upward_seen: bool,
        eos_seen: bool,
    ) -> StartPollSignal {
        if error_changed || last_error.is_some() {
            return StartPollSignal::Failed(
                last_error.unwrap_or_else(|| "Ошибка воспроизведения GStreamer".to_string()),
            );
        }

        if pipeline_state == gstreamer::State::Playing {
            return StartPollSignal::Ready;
        }

        if eos_seen {
            return StartPollSignal::Failed(
                "поток завершился (EOS) до начала воспроизведения".to_string(),
            );
        }

        if upward_seen {
            self.saw_upward_progress = true;
        }

        // Ранний PLAY_STATE_STOPPED от асинхронного stop()/set_uri() предыдущего трека игнорируем:
        // срывом текущего запуска считается только откат в Null/Ready после движения вверх
        // (или падение слота, который стартовал уже из прероллированного состояния Paused).
        let stopped_after_start = self.saw_upward_progress
            && state_changed
            && cur_play_state == PLAY_STATE_STOPPED
            && matches!(pipeline_state, gstreamer::State::Null | gstreamer::State::Ready);

        if stopped_after_start {
            let since = *self.stopped_seen_at.get_or_insert(now);
            if now.duration_since(since).as_millis() >= 40 {
                return StartPollSignal::Failed(
                    "поток остановился при старте (State::Stopped)".to_string(),
                );
            }
        } else {
            self.stopped_seen_at = None;
        }

        StartPollSignal::Pending
    }
}

async fn wait_for_slot_ready(
    handle: &SlotWaitHandle,
    timeout_ms: u128,
    is_cancelled: impl Fn() -> bool,
) -> WaitSlotResult {
    let wait_start = tokio::time::Instant::now();
    let mut tracker = StartProgressTracker::new(handle.initial_saw_upward);

    loop {
        if is_cancelled() {
            return WaitSlotResult::Cancelled;
        }

        let pipeline_state = handle.play.pipeline().current_state();
        let error_changed = handle.error_gen.load(Ordering::SeqCst) != handle.initial_error_gen;
        let last_error = handle.last_error.lock().ok().and_then(|g| g.clone());
        let state_changed = handle.state_seq.load(Ordering::SeqCst) != handle.initial_state_seq;
        let cur_play_state = handle.play_state.load(Ordering::SeqCst);
        let upward_seen = handle.upward_seq.load(Ordering::SeqCst) != handle.initial_upward_seq;
        let eos_seen = handle.eos_seq.load(Ordering::SeqCst) != handle.initial_eos_seq;

        match tracker.step(
            tokio::time::Instant::now(),
            pipeline_state,
            error_changed,
            last_error,
            state_changed,
            cur_play_state,
            upward_seen,
            eos_seen,
        ) {
            StartPollSignal::Ready => return WaitSlotResult::Playing,
            StartPollSignal::Failed(reason) => return WaitSlotResult::Failed(reason),
            StartPollSignal::Pending => {}
        }

        if wait_start.elapsed().as_millis() > timeout_ms {
            return WaitSlotResult::Timeout(pipeline_state);
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
    }
}

/// Определяет, нужно ли вызывать `pause()` для слота, чтобы не будить неактивный остановленный слот
/// из состояния `Null`/`Ready` обратно в `Buffering -> Paused`.
fn should_pause_slot(
    slot_idx: usize,
    active_idx: usize,
    was_playing: bool,
    pipeline_state: gstreamer::State,
) -> bool {
    slot_idx == active_idx || was_playing || pipeline_state == gstreamer::State::Playing
}

/// Аудио-бэкенд на базе двух слотов GStreamer Play для одновременного воспроизведения и настоящей сводки (кроссфейда)
pub struct AudioBackend {
    slots: [PlayerSlot; 2],
    active_slot: Arc<AtomicUsize>,
    user_volume: Arc<Mutex<f64>>,
    fade_gen: Arc<AtomicU64>,
    /// Поколение идущей прямо сейчас сводки; 0 — сводки нет. Пока сводка идёт, исходящий
    /// слот ещё звучит, и его нельзя останавливать прероллом (иначе затухание обрывается).
    fading_gen: Arc<AtomicU64>,
    /// Преролл, отложенный до конца сводки: URL, который нужно подготовить в освободившийся слот.
    pending_preroll: Arc<Mutex<Option<String>>>,
    on_error: ErrorCallback,
    on_buffering: BufferingCallback,
}

impl AudioBackend {
    /// Инициализирует аудио бэкенд с двумя независимыми слотами плеера
    pub fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        gstreamer::init()?;
        let active_slot = Arc::new(AtomicUsize::new(0));
        let on_error: ErrorCallback = Arc::new(Mutex::new(None));
        let on_buffering: BufferingCallback = Arc::new(Mutex::new(None));
        let slot0 = PlayerSlot::new(0, active_slot.clone(), on_error.clone(), on_buffering.clone())?;
        let slot1 = PlayerSlot::new(1, active_slot.clone(), on_error.clone(), on_buffering.clone())?;

        let initial_vol = crate::api::auth::load_volume();
        slot0.set_slot_volume(initial_vol);
        slot1.set_slot_volume(0.0);
        let user_volume = Arc::new(Mutex::new(initial_vol));
        let fade_gen = Arc::new(AtomicU64::new(0));
        let fading_gen = Arc::new(AtomicU64::new(0));

        Ok(Self {
            slots: [slot0, slot1],
            active_slot,
            user_volume,
            fade_gen,
            fading_gen,
            pending_preroll: Arc::new(Mutex::new(None)),
            on_error,
            on_buffering,
        })
    }

    /// Активный в данный момент слот плеера
    #[inline]
    fn active(&self) -> &PlayerSlot {
        let idx = self.active_slot.load(Ordering::SeqCst) % 2;
        &self.slots[idx]
    }

    /// Уровень баса 0..1 для звучащего сейчас фрагмента активного слота
    pub fn bass_level(&self) -> f32 {
        let active = self.active();
        let pipeline = active.play.pipeline();
        if pipeline.current_state() != gstreamer::State::Playing {
            return 0.0;
        }
        let Some(now) = pipeline.current_running_time() else { return 0.0 };
        let Ok(mut q) = active.bass.lock() else { return 0.0 };
        while q.get(1).is_some_and(|&(t, _)| t <= now.nseconds()) {
            q.pop_front();
        }
        q.front().filter(|&&(t, _)| t <= now.nseconds()).map_or(0.0, |&(_, v)| v)
    }

    /// Регистрирует обработчик окончания воспроизведения трека (EndOfStream).
    /// Срабатывает только если EOS произошел на активном в данный момент слоте.
    pub fn set_on_eos<F: Fn() + Send + Sync + 'static>(&self, callback: F) {
        let callback = Arc::new(callback);
        for (idx, slot) in self.slots.iter().enumerate() {
            let cb = callback.clone();
            let active_slot = self.active_slot.clone();
            let fading_gen = self.fading_gen.clone();
            let is_playing = slot.is_playing.clone();
            slot._adapter.connect_end_of_stream(move |_| {
                if active_slot.load(Ordering::SeqCst) % 2 == idx {
                    if fading_gen.load(Ordering::SeqCst) != 0 {
                        eprintln!(
                            "[gst slot {}] EOS во время ожидания кроссфейда (помечаем исходящий слот остановленным)",
                            idx
                        );
                        if let Ok(mut lp) = is_playing.lock() {
                            *lp = false;
                        }
                        return;
                    }
                    eprintln!("[gst slot {}] конец трека (EOS)", idx);
                    cb();
                } else {
                    eprintln!("[gst slot {}] конец трека (EOS) на неактивном слоте (игнорируем)", idx);
                }
            });
        }
    }

    /// Регистрирует обработчик ошибок воспроизведения активного слота GStreamer.
    pub fn set_on_error<F: Fn(String) + Send + Sync + 'static>(&self, callback: F) {
        if let Ok(mut guard) = self.on_error.lock() {
            *guard = Some(Arc::new(callback));
        }
    }

    /// Регистрирует обработчик прогресса буферизации (`0..=100`) активного или стартующего слота.
    pub fn set_on_buffering<F: Fn(u8) + Send + Sync + 'static>(&self, callback: F) {
        if let Ok(mut guard) = self.on_buffering.lock() {
            *guard = Some(Arc::new(callback));
        }
    }

    /// Предварительно подготавливает и буферизует следующий трек в неактивный слот GStreamer (Prebuffering / Preroll).
    /// Поток подключается по сети, заполняет внутренний буфер и декодирует начало в состоянии PAUSED с нулевой громкостью.
    pub fn preroll_next_url(&self, url: &str) {
        let inactive_idx = 1 - (self.active_slot.load(Ordering::SeqCst) % 2);
        let slot = &self.slots[inactive_idx];

        let is_busy = self.fading_gen.load(Ordering::SeqCst) != 0
            || slot.play.pipeline().current_state() == gstreamer::State::Playing
            || slot.is_playing.lock().map(|l| *l).unwrap_or(false);

        if is_busy {
            if let Ok(mut pending) = self.pending_preroll.lock() {
                *pending = Some(url.to_string());
            }
            eprintln!("[audio] Преролл отложен до освобождения слота {}: {}", inactive_idx, url);
            return;
        }

        preroll_slot(
            &slot.play,
            &slot.volume,
            &slot.current_uri,
            &slot.is_prerolled,
            &slot.slot_gen,
            &slot.last_error,
            inactive_idx,
            url,
        );
    }

    /// Готовит отложенный преролл (если он есть) в освободившийся неактивный слот.
    /// Вызывается из конца сводки и из жёстких переключений трека.
    fn flush_pending_preroll(&self) {
        let inactive_idx = 1 - (self.active_slot.load(Ordering::SeqCst) % 2);
        let slot = &self.slots[inactive_idx];
        let uris = PendingPreroll {
            pending: &self.pending_preroll,
            playing_uri: &self.active().current_uri,
            slot_play: &slot.play,
            slot_volume: &slot.volume,
            slot_uri: &slot.current_uri,
            slot_prerolled: &slot.is_prerolled,
            slot_gen: &slot.slot_gen,
            slot_last_error: &slot.last_error,
            slot_idx: inactive_idx,
        };
        uris.flush();
    }

    /// Загружает и начинает воспроизведение трека по прямой ссылке в текущий слот (без кроссфейда)
    pub fn play_url(&self, url: &str) {
        self.play_url_internal(url, None, None);
    }

    /// Загружает и начинает воспроизведение трека, уведомляя `on_outcome` о фактическом старте или сбое GStreamer.
    pub fn play_url_with_outcome<F: FnOnce(StartOutcome) + Send + 'static>(
        &self,
        url: &str,
        on_outcome: F,
    ) {
        self.play_url_internal(url, None, Some(Box::new(on_outcome)));
    }

    fn play_url_internal(
        &self,
        url: &str,
        fade_in_ms: Option<u64>,
        on_outcome: Option<Box<dyn FnOnce(StartOutcome) + Send>>,
    ) {
        if let Some(ms) = fade_in_ms {
            eprintln!(
                "[audio] play_url_fade_in ({} ms) {}",
                ms,
                url.chars().take(90).collect::<String>()
            );
        } else {
            eprintln!("[audio] play_url {}", url.chars().take(90).collect::<String>());
        }
        self.fade_gen.fetch_add(1, Ordering::SeqCst);
        // Жёсткое переключение: прежней сводки больше нет, отложенный преролл можно выполнять.
        self.fading_gen.store(0, Ordering::SeqCst);
        let curr_idx = self.active_slot.load(Ordering::SeqCst) % 2;
        let inactive_idx = 1 - curr_idx;

        let slot = &self.slots[inactive_idx];
        let is_match = {
            let cur_match = slot.current_uri.lock().map(|cur| cur.as_deref() == Some(url)).unwrap_or(false);
            cur_match && slot.is_prerolled.load(Ordering::SeqCst)
        };

        let target_idx = if is_match {
            let old_idx = self.active_slot.swap(inactive_idx, Ordering::SeqCst);
            self.slots[old_idx].bump_gen();
            self.slots[old_idx].clear_error();
            self.slots[old_idx].play.stop();
            self.slots[old_idx].is_prerolled.store(false, Ordering::SeqCst);
            if let Ok(mut lock) = self.slots[old_idx].is_playing.lock() {
                *lock = false;
            }
            inactive_idx
        } else {
            self.slots[inactive_idx].bump_gen();
            self.slots[inactive_idx].clear_error();
            self.slots[inactive_idx].play.stop();
            slot.is_prerolled.store(false, Ordering::SeqCst);
            if let Ok(mut lock) = self.slots[inactive_idx].is_playing.lock() {
                *lock = false;
            }
            curr_idx
        };

        let target = &self.slots[target_idx];
        let our_slot_gen = target.bump_gen();
        target.clear_error();
        let wait_handle = target.wait_handle(is_match);
        if on_outcome.is_some() {
            target.start_wait_gen.store(our_slot_gen, Ordering::SeqCst);
        } else {
            target.start_wait_gen.store(0, Ordering::SeqCst);
        }

        target.is_prerolled.store(false, Ordering::SeqCst);
        let initial_vol = if fade_in_ms.is_some() {
            0.0
        } else {
            self.user_volume()
        };
        target.set_slot_volume(initial_vol);
        configure_pipeline_for_url(&target.play.pipeline(), url);
        if let Ok(mut cur) = target.current_uri.lock() {
            *cur = Some(url.to_string());
        }
        if !is_match {
            target.play.set_uri(Some(url));
        }
        target.play.play();
        if let Ok(mut lock) = target.is_playing.lock() {
            *lock = true;
        }
        if is_match {
            eprintln!(
                "[audio] Мгновенный 0-мс старт предбуферизованного слота {}{}",
                target_idx,
                if fade_in_ms.is_some() { " с fade-in" } else { "" }
            );
        }
        self.flush_pending_preroll();
        if let Some(ms) = fade_in_ms {
            self.fade_in(ms);
        }

        if let Some(cb) = on_outcome {
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                let play = wait_handle.play.clone();
                let slot_gen = target.slot_gen.clone();
                let start_wait_gen = target.start_wait_gen.clone();
                let is_playing = target.is_playing.clone();
                handle.spawn(async move {
                    let res = wait_for_slot_ready(
                        &wait_handle,
                        15_000,
                        || slot_gen.load(Ordering::SeqCst) != our_slot_gen,
                    )
                    .await;
                    if start_wait_gen.load(Ordering::SeqCst) == our_slot_gen {
                        start_wait_gen.store(0, Ordering::SeqCst);
                    }
                    match res {
                        WaitSlotResult::Playing => cb(StartOutcome::Started),
                        WaitSlotResult::Failed(reason) => {
                            if slot_gen.load(Ordering::SeqCst) == our_slot_gen {
                                play.stop();
                                if let Ok(mut lp) = is_playing.lock() {
                                    *lp = false;
                                }
                            }
                            cb(StartOutcome::Failed(reason));
                        }
                        WaitSlotResult::Timeout(state) => {
                            if matches!(state, gstreamer::State::Paused | gstreamer::State::Playing) {
                                cb(StartOutcome::Started);
                            } else {
                                if slot_gen.load(Ordering::SeqCst) == our_slot_gen {
                                    play.stop();
                                    if let Ok(mut lp) = is_playing.lock() {
                                        *lp = false;
                                    }
                                }
                                cb(StartOutcome::Failed(
                                    "Таймаут буферизации потока (15 с)".to_string(),
                                ));
                            }
                        }
                        WaitSlotResult::Cancelled => cb(StartOutcome::Cancelled),
                    }
                });
            } else {
                target.start_wait_gen.store(0, Ordering::SeqCst);
                cb(StartOutcome::Started);
            }
        }
    }

    /// Переключает воспроизведение активного слота на указанный URL с сохранением позиции,
    /// не сбрасывая неактивный слот, если в нём уже подготовлен преролл следующего трека.
    pub fn play_url_at_ms(&self, url: &str, position_ms: u64) {
        eprintln!(
            "[audio] play_url_at_ms ({} ms) {}",
            position_ms,
            url.chars().take(90).collect::<String>()
        );
        self.fade_gen.fetch_add(1, Ordering::SeqCst);
        self.fading_gen.store(0, Ordering::SeqCst);

        let active_idx = self.active_slot.load(Ordering::SeqCst) % 2;
        let inactive_idx = 1 - active_idx;
        let inactive = &self.slots[inactive_idx];
        // Неактивный слот глушим только если он реально играет и не хранит готовый преролл
        if !inactive.is_prerolled.load(Ordering::SeqCst) {
            let inactive_playing = inactive.is_playing.lock().map(|l| *l).unwrap_or(false)
                || inactive.play.pipeline().current_state() == gstreamer::State::Playing;
            if inactive_playing {
                inactive.bump_gen();
                inactive.play.stop();
                if let Ok(mut lock) = inactive.is_playing.lock() {
                    *lock = false;
                }
            }
        }

        let active = &self.slots[active_idx];
        let target_gen = active.bump_gen();
        active.clear_error();
        active.start_wait_gen.store(0, Ordering::SeqCst);
        active.is_prerolled.store(false, Ordering::SeqCst);
        // Сбрасываем текущий зависший конвейер активного слота, чтобы GStreamer переоткрыл поток заново
        active.play.stop();
        configure_pipeline_for_url(&active.play.pipeline(), url);

        if let Ok(mut cur) = active.current_uri.lock() {
            *cur = Some(url.to_string());
        }
        if let Ok(mut lock) = active.is_playing.lock() {
            *lock = true;
        }

        let target_vol = self.user_volume();
        if position_ms == 0 {
            active.set_slot_volume(target_vol);
            active.play.set_uri(Some(url));
            active.play.play();
            return;
        }

        // Выставляем 0.0 на время буферизации и seek, чтобы не было слышно скачка с 0:00
        active.set_slot_volume(0.0);
        active.play.set_uri(Some(url));
        active.play.pause();

        let play = active.play.clone();
        let volume_el = active.volume.clone();
        let slot_gen = active.slot_gen.clone();
        let is_playing = active.is_playing.clone();
        let user_volume_arc = self.user_volume.clone();

        let target_url = url.to_string();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                use gtk4::glib::prelude::ObjectExt;
                let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_millis(3000);
                let step = tokio::time::Duration::from_millis(15);
                loop {
                    tokio::time::sleep(step).await;
                    let still_active = slot_gen.load(Ordering::SeqCst) == target_gen
                        && is_playing.lock().map(|l| *l).unwrap_or(false);
                    if !still_active {
                        return;
                    }
                    let pipeline = play.pipeline();
                    let uri_matches = if pipeline.find_property("current-uri").is_some() {
                        pipeline
                            .property::<Option<String>>("current-uri")
                            .as_deref()
                            == Some(target_url.as_str())
                    } else {
                        true
                    };
                    let state = pipeline.current_state();
                    if uri_matches
                        && matches!(state, gstreamer::State::Paused | gstreamer::State::Playing)
                    {
                        play.seek(gstreamer::ClockTime::from_mseconds(position_ms));
                        let vol = user_volume_arc.lock().map(|l| *l).unwrap_or(target_vol);
                        set_element_volume(&volume_el, vol);
                        play.play();
                        return;
                    }
                    if tokio::time::Instant::now() >= deadline {
                        let vol = user_volume_arc.lock().map(|l| *l).unwrap_or(target_vol);
                        set_element_volume(&volume_el, vol);
                        play.play();
                        return;
                    }
                }
            });
        } else {
            // Запасной путь на случай синхронного вызова вне Tokio runtime
            let vol = user_volume_arc.lock().map(|l| *l).unwrap_or(target_vol);
            set_element_volume(&volume_el, vol);
            play.play();
        }
    }

    /// Загружает трек и начинает воспроизведение с плавным нарастанием громкости (Fade-In) от 0.0 до user_volume
    pub fn play_url_fade_in(&self, url: &str, duration_ms: u64) {
        self.play_url_internal(url, Some(duration_ms), None);
    }

    /// Загружает трек с плавным нарастанием громкости и уведомляет о старте или ошибке GStreamer.
    pub fn play_url_fade_in_with_outcome<F: FnOnce(StartOutcome) + Send + 'static>(
        &self,
        url: &str,
        duration_ms: u64,
        on_outcome: F,
    ) {
        self.play_url_internal(url, Some(duration_ms), Some(Box::new(on_outcome)));
    }

    /// Настоящий двухканальный кроссфейд: старый трек плавно затухает (1.0 -> 0.0),
    /// а новый трек параллельно нарастает (0.0 -> 1.0) в течение duration_ms,
    /// оба трека звучат ОДНОВРЕМЕННО без промежуточной паузы и тишины!
    pub fn crossfade_to_url(&self, url: &str, duration_ms: u64) {
        self.crossfade_to_url_internal(url, duration_ms, None);
    }

    /// Двухканальный кроссфейд с уведомлением `on_outcome` о фактическом старте нового слота или его ошибке.
    pub fn crossfade_to_url_with_outcome<F: FnOnce(StartOutcome) + Send + 'static>(
        &self,
        url: &str,
        duration_ms: u64,
        on_outcome: F,
    ) {
        self.crossfade_to_url_internal(url, duration_ms, Some(Box::new(on_outcome)));
    }

    fn crossfade_to_url_internal(
        &self,
        url: &str,
        duration_ms: u64,
        on_outcome: Option<Box<dyn FnOnce(StartOutcome) + Send>>,
    ) {
        eprintln!(
            "[audio] crossfade_to_url ({} ms) {}",
            duration_ms,
            url.chars().take(90).collect::<String>()
        );

        let current_idx = self.active_slot.load(Ordering::SeqCst) % 2;
        let next_idx = 1 - current_idx;

        let generation_id = self.fade_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let fade_gen = self.fade_gen.clone();
        if duration_ms > 0 {
            self.fading_gen.store(generation_id, Ordering::SeqCst);
        } else {
            self.fading_gen.store(0, Ordering::SeqCst);
            self.active_slot.store(next_idx, Ordering::SeqCst);
        }

        let current_slot = &self.slots[current_idx];
        let next_slot = &self.slots[next_idx];
        let outgoing_slot_gen = current_slot.slot_gen.load(Ordering::SeqCst);
        let our_next_gen = next_slot.bump_gen();
        next_slot.clear_error();
        let is_already_prerolled = {
            let cur_match = next_slot.current_uri.lock().map(|cur| cur.as_deref() == Some(url)).unwrap_or(false);
            cur_match && next_slot.is_prerolled.load(Ordering::SeqCst)
        };
        let wait_handle = next_slot.wait_handle(is_already_prerolled);
        if duration_ms > 0 || on_outcome.is_some() {
            next_slot.start_wait_gen.store(our_next_gen, Ordering::SeqCst);
        } else {
            next_slot.start_wait_gen.store(0, Ordering::SeqCst);
        }

        configure_pipeline_for_url(&next_slot.play.pipeline(), url);

        if is_already_prerolled {
            eprintln!("[audio] мгновенный старт предзагруженного трека из слота {} (0 мс задержки)!", next_idx);
            next_slot.set_slot_volume(0.0);
            next_slot.play.play();
        } else {
            next_slot.play.stop();
            next_slot.set_slot_volume(0.0);
            if let Ok(mut cur) = next_slot.current_uri.lock() {
                *cur = Some(url.to_string());
            }
            next_slot.play.set_uri(Some(url));
            next_slot.play.play();
        }
        next_slot.is_prerolled.store(false, Ordering::SeqCst);
        if let Ok(mut lock) = next_slot.is_playing.lock() {
            *lock = true;
        }

        let user_volume_arc = self.user_volume.clone();
        let user_vol = self.user_volume();

        let current_play = current_slot.play.clone();
        let current_volume = current_slot.volume.clone();
        let next_play = wait_handle.play.clone();
        let next_volume = next_slot.volume.clone();
        let current_is_playing = current_slot.is_playing.clone();
        let current_slot_gen = current_slot.slot_gen.clone();
        let next_is_playing = next_slot.is_playing.clone();
        let next_slot_gen = next_slot.slot_gen.clone();
        let next_start_wait_gen = next_slot.start_wait_gen.clone();

        if duration_ms == 0 {
            current_play.stop();
            if let Ok(mut lock) = current_is_playing.lock() {
                *lock = false;
            }
            set_element_volume(&next_volume, user_vol);
            self.flush_pending_preroll();
            if let Some(cb) = on_outcome {
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        let res = wait_for_slot_ready(
                            &wait_handle,
                            15_000,
                            || next_slot_gen.load(Ordering::SeqCst) != our_next_gen,
                        )
                        .await;
                        if next_start_wait_gen.load(Ordering::SeqCst) == our_next_gen {
                            next_start_wait_gen.store(0, Ordering::SeqCst);
                        }
                        match res {
                            WaitSlotResult::Playing => cb(StartOutcome::Started),
                            WaitSlotResult::Failed(reason) => {
                                if next_slot_gen.load(Ordering::SeqCst) == our_next_gen {
                                    next_play.stop();
                                    if let Ok(mut lp) = next_is_playing.lock() {
                                        *lp = false;
                                    }
                                }
                                cb(StartOutcome::Failed(reason));
                            }
                            WaitSlotResult::Timeout(st) => {
                                if matches!(st, gstreamer::State::Paused | gstreamer::State::Playing) {
                                    cb(StartOutcome::Started);
                                } else {
                                    cb(StartOutcome::Failed(
                                        "Таймаут буферизации потока (15 с)".to_string(),
                                    ));
                                }
                            }
                            WaitSlotResult::Cancelled => cb(StartOutcome::Cancelled),
                        }
                    });
                } else {
                    next_start_wait_gen.store(0, Ordering::SeqCst);
                    cb(StartOutcome::Started);
                }
            }
            return;
        }

        let start_current_vol = current_slot.slot_volume().min(user_vol);
        let active_slot_arc = self.active_slot.clone();
        let fading_gen = self.fading_gen.clone();
        let pending_preroll = self.pending_preroll.clone();
        let current_uri = current_slot.current_uri.clone();
        let current_prerolled = current_slot.is_prerolled.clone();
        let current_last_error = current_slot.last_error.clone();
        let next_uri = next_slot.current_uri.clone();
        let global_on_error = self.on_error.clone();

        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            active_slot_arc.store(next_idx, Ordering::SeqCst);
            next_start_wait_gen.store(0, Ordering::SeqCst);
            self.fading_gen.store(0, Ordering::SeqCst);
            if let Some(cb) = on_outcome {
                cb(StartOutcome::Started);
            }
            return;
        };

        handle.spawn(async move {
            let stop_outgoing_if_unclaimed = || {
                // Если слот уже перехватило следующее переключение или преролл, не останавливаем чужой запуск.
                if current_slot_gen.load(Ordering::SeqCst) == outgoing_slot_gen {
                    current_play.stop();
                    if let Ok(mut lock) = current_is_playing.lock() {
                        *lock = false;
                    }
                }
            };

            let notify_failed_start = |reason: String, cb: Option<Box<dyn FnOnce(StartOutcome) + Send>>| {
                if let Some(cb) = cb {
                    cb(StartOutcome::Failed(reason));
                } else if let Some(err_cb) = global_on_error.lock().ok().and_then(|g| g.clone()) {
                    err_cb(reason);
                }
            };

            // Ждем, пока новый поток сбуферизируется и физически перейдет в Playing
            // (либо сразу прерываемся при ошибке GStreamer / EOS / откате в Null/Ready без ожидания 15 с).
            let wait_res = wait_for_slot_ready(
                &wait_handle,
                15_000,
                || fade_gen.load(Ordering::SeqCst) != generation_id
                    || next_slot_gen.load(Ordering::SeqCst) != our_next_gen,
            )
            .await;

            if next_start_wait_gen.load(Ordering::SeqCst) == our_next_gen {
                next_start_wait_gen.store(0, Ordering::SeqCst);
            }

            let mut outcome_cb = on_outcome;

            match wait_res {
                WaitSlotResult::Cancelled => {
                    if fade_gen.load(Ordering::SeqCst) != generation_id {
                        stop_outgoing_if_unclaimed();
                    }
                    if fading_gen.load(Ordering::SeqCst) == generation_id {
                        fading_gen.store(0, Ordering::SeqCst);
                    }
                    if let Some(cb) = outcome_cb.take() {
                        cb(StartOutcome::Cancelled);
                    }
                    return;
                }
                WaitSlotResult::Failed(reason) => {
                    eprintln!(
                        "[audio] crossfade прерван из-за ошибки нового слота {}: {}",
                        next_idx, reason
                    );
                    if next_slot_gen.load(Ordering::SeqCst) == our_next_gen {
                        next_play.stop();
                        if let Ok(mut lock) = next_is_playing.lock() {
                            *lock = false;
                        }
                    }
                    let current_alive = current_slot_gen.load(Ordering::SeqCst) == outgoing_slot_gen
                        && current_is_playing.lock().map(|l| *l).unwrap_or(false)
                        && current_play.pipeline().current_state() != gstreamer::State::Null;
                    if !current_alive {
                        stop_outgoing_if_unclaimed();
                    }
                    if fading_gen.load(Ordering::SeqCst) == generation_id {
                        fading_gen.store(0, Ordering::SeqCst);
                    }
                    notify_failed_start(reason, outcome_cb.take());
                    return;
                }
                WaitSlotResult::Timeout(state) => {
                    if matches!(state, gstreamer::State::Paused | gstreamer::State::Playing) {
                        active_slot_arc.store(next_idx, Ordering::SeqCst);
                        stop_outgoing_if_unclaimed();
                        if fade_gen.load(Ordering::SeqCst) == generation_id {
                            eprintln!("[audio] новый поток не перешел в Playing за 15 с — переключаем без сводки");
                            let vol = user_volume_arc.lock().map(|l| *l).unwrap_or(1.0);
                            set_element_volume(&next_volume, vol);
                            if fading_gen.load(Ordering::SeqCst) == generation_id {
                                fading_gen.store(0, Ordering::SeqCst);
                            }
                            if let Some(cb) = outcome_cb.take() {
                                cb(StartOutcome::Started);
                            }
                            PendingPreroll {
                                pending: &pending_preroll,
                                playing_uri: &next_uri,
                                slot_play: &current_play,
                                slot_volume: &current_volume,
                                slot_uri: &current_uri,
                                slot_prerolled: &current_prerolled,
                                slot_gen: &current_slot_gen,
                                slot_last_error: &current_last_error,
                                slot_idx: current_idx,
                            }
                            .flush();
                        }
                    } else {
                        let reason = format!(
                            "таймаут буферизации нового потока за 15 с (state={state:?})"
                        );
                        eprintln!(
                            "[audio] crossfade прерван из-за ошибки нового слота {}: {}",
                            next_idx, reason
                        );
                        if next_slot_gen.load(Ordering::SeqCst) == our_next_gen {
                            next_play.stop();
                            if let Ok(mut lock) = next_is_playing.lock() {
                                *lock = false;
                            }
                        }
                        if fading_gen.load(Ordering::SeqCst) == generation_id {
                            fading_gen.store(0, Ordering::SeqCst);
                        }
                        notify_failed_start(reason, outcome_cb.take());
                    }
                    return;
                }
                WaitSlotResult::Playing => {
                    active_slot_arc.store(next_idx, Ordering::SeqCst);
                    if let Some(cb) = outcome_cb.take() {
                        cb(StartOutcome::Started);
                    }
                }
            }

            const STEP_MS: u64 = 25;
            let steps = (duration_ms / STEP_MS).max(1);
            let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(STEP_MS));

            for step in 1..=steps {
                interval.tick().await;
                if fade_gen.load(Ordering::SeqCst) != generation_id {
                    // Фейд был перехвачен новым переключением или остановкой
                    stop_outgoing_if_unclaimed();
                    return;
                }
                let t = step as f64 / steps as f64;
                let angle = t * std::f64::consts::FRAC_PI_2;
                let out_factor = angle.cos();
                let in_factor = angle.sin();

                let cur_user_vol = user_volume_arc.lock().map(|l| *l).unwrap_or(1.0);
                let current_v = (start_current_vol * out_factor).min(cur_user_vol);
                let next_v = cur_user_vol * in_factor;

                if current_slot_gen.load(Ordering::SeqCst) == outgoing_slot_gen {
                    set_element_volume(&current_volume, current_v);
                }
                set_element_volume(&next_volume, next_v);
            }

            stop_outgoing_if_unclaimed();
            if fade_gen.load(Ordering::SeqCst) == generation_id {
                let cur_user_vol = user_volume_arc.lock().map(|l| *l).unwrap_or(1.0);
                set_element_volume(&next_volume, cur_user_vol);
                if fading_gen.load(Ordering::SeqCst) == generation_id {
                    fading_gen.store(0, Ordering::SeqCst);
                }
                // Сводка доиграла, исходящий слот освободился — самое время прогреть следующий трек.
                PendingPreroll {
                    pending: &pending_preroll,
                    playing_uri: &next_uri,
                    slot_play: &current_play,
                    slot_volume: &current_volume,
                    slot_uri: &current_uri,
                    slot_prerolled: &current_prerolled,
                    slot_gen: &current_slot_gen,
                    slot_last_error: &current_last_error,
                    slot_idx: current_idx,
                }
                .flush();
            }
        });
    }

    /// Останавливает оба слота конвейера и отменяет активный fade
    pub fn stop(&self) {
        self.fade_gen.fetch_add(1, Ordering::SeqCst);
        // Сводки больше нет: снимаем блокировку преролла и выбрасываем отложенный URL —
        // после остановки его всё равно придётся запрашивать заново.
        self.fading_gen.store(0, Ordering::SeqCst);
        if let Ok(mut pending) = self.pending_preroll.lock() {
            *pending = None;
        }
        for slot in &self.slots {
            slot.bump_gen();
            slot.start_wait_gen.store(0, Ordering::SeqCst);
            slot.clear_error();
            slot.play.stop();
            slot.is_prerolled.store(false, Ordering::SeqCst);
            if let Ok(mut cur) = slot.current_uri.lock() {
                *cur = None;
            }
            if let Ok(mut lock) = slot.is_playing.lock() {
                *lock = false;
            }
        }
    }

    /// Возобновляет воспроизведение активного слота немедленно (на установленной громкости)
    pub fn play(&self) {
        self.fade_gen.fetch_add(1, Ordering::SeqCst);
        self.fading_gen.store(0, Ordering::SeqCst);
        let active = self.active();
        active.set_slot_volume(self.user_volume());
        active.play.play();
        if let Ok(mut lock) = active.is_playing.lock() {
            *lock = true;
        }
        self.flush_pending_preroll();
    }

    /// Возобновляет воспроизведение с мягким нарастанием громкости за duration_ms
    pub fn play_soft(&self, duration_ms: u64) {
        let active = self.active();
        if let Ok(mut lock) = active.is_playing.lock() {
            *lock = true;
        }
        active.set_slot_volume(0.0);
        active.play.play();
        self.fade_in(duration_ms);
    }

    /// Приостанавливает воспроизведение немедленно, не пробуждая неактивный остановленный слот
    pub fn pause(&self) {
        self.fade_gen.fetch_add(1, Ordering::SeqCst);
        let active_idx = self.active_slot.load(Ordering::SeqCst) % 2;
        for (idx, slot) in self.slots.iter().enumerate() {
            slot.bump_gen();
            let was_playing = if let Ok(mut lock) = slot.is_playing.lock() {
                let prev = *lock;
                *lock = false;
                prev
            } else {
                false
            };
            let pipeline_state = slot.play.pipeline().current_state();
            if should_pause_slot(idx, active_idx, was_playing, pipeline_state) {
                slot.play.pause();
            }
        }
    }

    /// Приостанавливает воспроизведение с мягким затуханием за duration_ms перед реальной паузой
    pub fn pause_soft(&self, duration_ms: u64) {
        let active_idx = self.active_slot.load(Ordering::SeqCst) % 2;
        let mut was_playing = [false; 2];
        for (idx, slot) in self.slots.iter().enumerate() {
            if let Ok(mut lock) = slot.is_playing.lock() {
                was_playing[idx] = *lock;
                *lock = false;
            }
        }
        let play0 = self.slots[0].play.clone();
        let play1 = self.slots[1].play.clone();
        let is_playing0 = self.slots[0].is_playing.clone();
        let is_playing1 = self.slots[1].is_playing.clone();
        self.fade_volume_internal(0.0, duration_ms, Some(Box::new(move || {
            let cur_is_playing0 = is_playing0.lock().map(|l| *l).unwrap_or(false);
            if !cur_is_playing0
                && should_pause_slot(0, active_idx, was_playing[0], play0.pipeline().current_state())
            {
                play0.pause();
            }
            let cur_is_playing1 = is_playing1.lock().map(|l| *l).unwrap_or(false);
            if !cur_is_playing1
                && should_pause_slot(1, active_idx, was_playing[1], play1.pipeline().current_state())
            {
                play1.pause();
            }
        })));
    }

    /// Переключает состояние воспроизведения:
    /// при паузе мягко затухает за 250 мс перед реальной паузой,
    /// при пуске мягко нарастает за 250 мс.
    pub fn toggle_play(&self) -> bool {
        let current = self.is_playing();
        if current {
            self.pause_soft(250);
            false
        } else {
            self.play_soft(250);
            true
        }
    }

    /// Проверяет, играет ли сейчас активный слот
    pub fn is_playing(&self) -> bool {
        self.active().is_playing.lock().map(|l| *l).unwrap_or(false)
    }

    /// Возвращает текущий URI активного слота
    pub fn active_uri(&self) -> Option<String> {
        self.active()
            .current_uri
            .lock()
            .ok()
            .and_then(|uri| uri.clone())
    }

    /// Проверяет, активна ли сейчас двухканальная сводка (кроссфейд)
    pub fn is_fading(&self) -> bool {
        self.fading_gen.load(Ordering::SeqCst) != 0
    }

    /// Перемотка к позиции в секундах
    pub fn seek_seconds(&self, seconds: u64) {
        let time = gstreamer::ClockTime::from_seconds(seconds);
        self.active().play.seek(time);
    }

    /// Перемотка с точностью до миллисекунды (переход к строке текста песни)
    pub fn seek_ms(&self, ms: u64) {
        let time = gstreamer::ClockTime::from_mseconds(ms);
        self.active().play.seek(time);
    }

    /// Докуда загружен текущий поток активного слота, доля 0..1
    pub fn buffered_fraction(&self) -> Option<f64> {
        let mut q = gstreamer::query::Buffering::new(gstreamer::Format::Percent);
        if !self.active().play.pipeline().query(&mut q) {
            return None;
        }
        let max = 1_000_000.0; // GST_FORMAT_PERCENT_MAX: доли заданы в миллионных
        let value = |v: gstreamer::GenericFormattedValue| v.value().max(0) as f64;
        let (_, range_stop, _) = q.range();
        let stop = q.ranges().map(|(_, stop)| value(stop)).fold(value(range_stop), f64::max);
        (stop > 0.0).then(|| (stop / max).clamp(0.0, 1.0))
    }

    /// Текущая позиция воспроизведения в миллисекундах активного слота
    pub fn position_ms(&self) -> Option<u64> {
        if let Some(ms) = std::env::var("VOLNA_DEMO_POS_MS").ok().and_then(|v| v.parse().ok()) {
            return Some(ms);
        }
        self.active().play.position().map(|t| t.mseconds())
    }

    /// Текущая позиция воспроизведения в секундах активного слота
    pub fn position_seconds(&self) -> Option<u64> {
        self.active().play.position().map(|t| t.seconds())
    }

    /// Полная длительность трека активного слота в секундах
    pub fn duration_seconds(&self) -> Option<u64> {
        self.active().play.duration().map(|t| t.seconds())
    }

    /// Полная длительность трека активного слота в миллисекундах
    pub fn duration_ms(&self) -> Option<u64> {
        self.active().play.duration().map(|t| t.mseconds())
    }

    /// Плавная интерполяция громкости через короткие шаги (25 мс) в фоновом потоке Tokio
    pub fn fade_volume(&self, target: f64, duration_ms: u64) {
        self.fade_volume_internal(target, duration_ms, None);
    }

    /// Внутренний метод плавного изменения громкости активного слота с коллбэком
    fn fade_volume_internal(
        &self,
        target: f64,
        duration_ms: u64,
        on_done: Option<Box<dyn FnOnce() + Send + 'static>>,
    ) {
        let target = target.clamp(0.0, 1.0);
        let generation_id = self.fade_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let fade_gen = self.fade_gen.clone();
        let volume_el = self.active().volume.clone();
        let start_vol = element_volume(&volume_el);

        if duration_ms == 0 || (start_vol - target).abs() < 1e-4 {
            set_element_volume(&volume_el, target);
            if let Some(done) = on_done {
                done();
            }
            return;
        }

        tokio::spawn(async move {
            const STEP_MS: u64 = 25;
            let steps = (duration_ms / STEP_MS).max(1);
            let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(STEP_MS));

            for step in 1..=steps {
                interval.tick().await;
                if fade_gen.load(Ordering::SeqCst) != generation_id {
                    return;
                }
                let progress = step as f64 / steps as f64;
                let current = start_vol + (target - start_vol) * progress;
                set_element_volume(&volume_el, current);
            }

            if fade_gen.load(Ordering::SeqCst) == generation_id {
                set_element_volume(&volume_el, target);
                if let Some(done) = on_done {
                    done();
                }
            }
        });
    }

    /// Плавный старт: стартует с громкости 0.0 и плавно наращивает до user_volume
    pub fn fade_in(&self, duration_ms: u64) {
        self.active().set_slot_volume(0.0);
        let target = self.user_volume();
        self.fade_volume(target, duration_ms);
    }

    /// Плавное затухание: плавно уменьшает громкость активного слота до 0.0
    pub fn fade_out(&self, duration_ms: u64) {
        self.fade_volume(0.0, duration_ms);
    }

    /// Асинхронное затухание громкости до 0.0 с ожиданием завершения
    pub async fn fade_out_async(&self, duration_ms: u64) {
        self.fade_volume_async(0.0, duration_ms).await;
    }

    /// Асинхронное нарастание громкости от 0.0 до user_volume с ожиданием завершения
    pub async fn fade_in_async(&self, duration_ms: u64) {
        self.active().set_slot_volume(0.0);
        let target = self.user_volume();
        self.fade_volume_async(target, duration_ms).await;
    }

    /// Асинхронная интерполяция громкости до target с шагом 25 мс
    pub async fn fade_volume_async(&self, target: f64, duration_ms: u64) {
        let target = target.clamp(0.0, 1.0);
        let generation_id = self.fade_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let fade_gen = self.fade_gen.clone();
        let volume_el = self.active().volume.clone();
        let start_vol = element_volume(&volume_el);

        if duration_ms == 0 || (start_vol - target).abs() < 1e-4 {
            set_element_volume(&volume_el, target);
            return;
        }

        const STEP_MS: u64 = 25;
        let steps = (duration_ms / STEP_MS).max(1);
        let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(STEP_MS));

        for step in 1..=steps {
            interval.tick().await;
            if fade_gen.load(Ordering::SeqCst) != generation_id {
                return;
            }
            let progress = step as f64 / steps as f64;
            let current = start_vol + (target - start_vol) * progress;
            set_element_volume(&volume_el, current);
        }

        if fade_gen.load(Ordering::SeqCst) == generation_id {
            set_element_volume(&volume_el, target);
        }
    }

    /// Установка громкости (от 0.0 до 1.0). Сохраняет в user_volume и устанавливает громкость активного слота
    pub fn set_volume(&self, volume: f64) {
        let v = volume.clamp(0.0, 1.0);
        if let Ok(mut lock) = self.user_volume.lock() {
            *lock = v;
        }
        self.active().set_slot_volume(v);
    }

    /// Целевая пользовательская громкость (по умолчанию 1.0 или заданная пользователем)
    pub fn user_volume(&self) -> f64 {
        self.user_volume.lock().map(|l| *l).unwrap_or(1.0)
    }

    /// Получение текущей пользовательской громкости
    pub fn volume(&self) -> f64 {
        self.user_volume()
    }

    /// Фактическая громкость программного элемента `volume` активного слота
    pub fn raw_volume(&self) -> f64 {
        self.active().slot_volume()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bass_meter() {
        let band_hz = 22_050.0 / SPECTRUM_BANDS as f32; // 44.1 кГц: полосы 1 и 2 = 64 и 107 Гц
        let quiet = [SPECTRUM_THRESHOLD_DB; 8];
        let mut kick = quiet;
        kick[1..3].copy_from_slice(&[-14.0, -16.0]);
        let mut bassline = quiet;
        bassline[1..3].copy_from_slice(&[-23.0, -25.0]);
        let mut highs = quiet;
        highs[3..].fill(-10.0); // от 150 Гц и выше — не бас
        let mut m = BassMeter::new();

        for _ in 0..30 {
            assert_eq!(m.push(&quiet, band_hz), 0.0);
        }
        assert_eq!(m.push(&highs, band_hz), 0.0);

        // 4 такта по ~500 мс: кик (2 сообщения) + ровный бас на ~9 дБ тише
        for _ in 0..4 {
            assert!(m.push(&kick, band_hz) > 0.7, "кик должен бить сразу");
            m.push(&kick, band_hz);
            assert!(m.push(&bassline, band_hz) > 0.4, "спад плавный, не мгновенный");
            let mut last = 1.0;
            for _ in 0..12 {
                last = m.push(&bassline, band_hz);
            }
            assert!(last < 0.25, "между ударами бас не должен светить постоянно: {last}");
        }

        for _ in 0..15 {
            m.push(&quiet, band_hz);
        }
        assert!(m.level < 0.05);
    }

    #[test]
    fn test_audio_backend_dual_slots() {
        let backend = match AudioBackend::new() {
            Ok(b) => b,
            Err(_) => return, // Если нет GStreamer в окружении тестирования
        };

        // Изначально активен слот 0
        assert_eq!(backend.active_slot.load(Ordering::SeqCst), 0);
        assert_eq!(backend.is_playing(), false);

        // crossfade_to_url переключает active_slot на противоположный (0 -> 1)
        backend.crossfade_to_url("file:///tmp/dummy1.mp3", 0);
        assert_eq!(backend.active_slot.load(Ordering::SeqCst), 1);

        // Следующий crossfade_to_url переключает обратно (1 -> 0)
        backend.crossfade_to_url("file:///tmp/dummy2.mp3", 0);
        assert_eq!(backend.active_slot.load(Ordering::SeqCst), 0);

        std::mem::forget(backend);
    }

    #[test]
    fn test_slot_volume_isolated_from_play_mixer() {
        let backend = match AudioBackend::new() {
            Ok(b) => b,
            Err(_) => return,
        };

        // У каждого слота свой audio-sink и audio-filter (bass_probe), а флаг soft-volume снят
        for slot in &backend.slots {
            let pipe = slot.play.pipeline();
            assert!(pipe.property::<Option<gstreamer::Element>>("audio-sink").is_some());
            assert!(pipe.property::<Option<gstreamer::Element>>("audio-filter").is_some());
            let flags = format!("{:?}", pipe.property_value("flags"));
            assert!(!flags.contains("soft-volume"), "не должно быть soft-volume: {flags}");
            assert!((slot.play.volume() - 1.0).abs() < 1e-6, "системный Play::volume должен оставаться 1.0");
        }

        // Регулировка громкости меняет программный volume-элемент, не трогая Play::volume
        backend.set_volume(0.35);
        assert!((backend.volume() - 0.35).abs() < 1e-6);
        assert!((backend.raw_volume() - 0.35).abs() < 1e-6);
        assert!((backend.active().play.volume() - 1.0).abs() < 1e-6);

        // Преролл в неактивный слот выставляет 0.0 только на программный volume-элемент неактивного слота
        let next_url = "file:///tmp/preroll_test.mp3";
        backend.preroll_next_url(next_url);
        assert!((backend.slots[1].slot_volume() - 0.0).abs() < 1e-6);
        assert!((backend.slots[1].play.volume() - 1.0).abs() < 1e-6);
        assert!((backend.slots[0].slot_volume() - 0.35).abs() < 1e-6);

        // Мгновенный старт преролленного трека восстанавливает user_volume (0.35), не 0.0
        backend.play_url(next_url);
        assert_eq!(backend.active_slot.load(Ordering::SeqCst), 1);
        assert!((backend.raw_volume() - 0.35).abs() < 1e-6);
        assert!((backend.active().play.volume() - 1.0).abs() < 1e-6);

        // Выход за границы 0..1 корректно ограничивается
        backend.set_volume(1.5);
        assert!((backend.raw_volume() - 1.0).abs() < 1e-6);
        backend.set_volume(-0.4);
        assert!((backend.raw_volume() - 0.0).abs() < 1e-6);
        assert!((backend.active().play.volume() - 1.0).abs() < 1e-6);

        std::mem::forget(backend);
    }

    #[test]
    fn test_configure_pipeline_flags_have_no_soft_volume() {
        let backend = match AudioBackend::new() {
            Ok(b) => b,
            Err(_) => return,
        };
        let pipe = backend.slots[0].play.pipeline();

        configure_pipeline_for_url(&pipe, "https://example.com/track/index.m3u8");
        let hls_flags = format!("{:?}", pipe.property_value("flags"));
        assert!(!hls_flags.contains("soft-volume"), "HLS flags: {hls_flags}");

        configure_pipeline_for_url(&pipe, "https://example.com/track/audio.flac");
        let prog_flags = format!("{:?}", pipe.property_value("flags"));
        assert!(!prog_flags.contains("soft-volume"), "progressive flags: {prog_flags}");

        assert_eq!(
            pipeline_buffering_config("file:///home/user/.cache/volna/audio/test.opus"),
            ("audio", -1i64, -1i32)
        );
        assert_eq!(
            pipeline_buffering_config("https://rr2---sn-u15hn5-cc.googlevideo.com/videoplayback?itag=251"),
            ("audio+download", 2_000_000_000i64, 512 * 1024i32)
        );
        assert_eq!(
            pipeline_buffering_config("https://example.com/track/audio.flac"),
            ("audio+download", 2_000_000_000i64, 512 * 1024i32)
        );
        if pipe.find_property("ring-buffer-max-size").is_some() {
            assert_eq!(
                pipe.property::<u64>("ring-buffer-max-size"),
                0u64,
                "ring-buffer-max-size должен быть 0 (отключен), чтобы не обрывать потоки >8 МБ"
            );
        }

        std::mem::forget(backend);
    }

    #[test]
    fn test_reset_system_sink_volume_heals_muted_sink() {
        if gstreamer::init().is_err() {
            return;
        }
        let Ok(vol_el) = gstreamer::ElementFactory::make("volume").build() else {
            return;
        };
        vol_el.set_property("volume", 0.0f64);
        vol_el.set_property("mute", true);

        reset_system_sink_volume(&vol_el);
        assert!((vol_el.property::<f64>("volume") - 1.0).abs() < 1e-6);
        assert!(!vol_el.property::<bool>("mute"));

        // Элемент без свойств volume/mute не должен вызывать панику
        if let Ok(fake) = gstreamer::ElementFactory::make("fakesink").build() {
            reset_system_sink_volume(&fake);
        }
    }

    #[test]
    fn test_fade_in_and_crossfade_zero_duration_keep_play_volume_unity() {
        let backend = match AudioBackend::new() {
            Ok(b) => b,
            Err(_) => return,
        };
        backend.set_volume(0.6);

        backend.play_url_fade_in("file:///tmp/fade_in_dummy.mp3", 0);
        assert!((backend.raw_volume() - 0.6).abs() < 1e-6);
        assert!((backend.active().play.volume() - 1.0).abs() < 1e-6);

        backend.crossfade_to_url("file:///tmp/crossfade_dummy.mp3", 0);
        assert_eq!(backend.active_slot.load(Ordering::SeqCst), 1);
        assert!((backend.raw_volume() - 0.6).abs() < 1e-6);
        assert!((backend.slots[0].play.volume() - 1.0).abs() < 1e-6);
        assert!((backend.slots[1].play.volume() - 1.0).abs() < 1e-6);

        std::mem::forget(backend);
    }

    #[test]
    fn test_should_pause_slot() {
        // Активный слот всегда должен ставиться на паузу
        assert!(should_pause_slot(0, 0, false, gstreamer::State::Null));
        assert!(should_pause_slot(1, 1, false, gstreamer::State::Ready));

        // Неактивный остановленный слот будить нельзя
        assert!(!should_pause_slot(1, 0, false, gstreamer::State::Null));
        assert!(!should_pause_slot(1, 0, false, gstreamer::State::Ready));
        assert!(!should_pause_slot(1, 0, false, gstreamer::State::Paused));

        // Но если неактивный слот ещё играл (уходящий слот сводки) — его нужно поставить на паузу
        assert!(should_pause_slot(1, 0, true, gstreamer::State::Paused));
        assert!(should_pause_slot(1, 0, false, gstreamer::State::Playing));
    }

    #[test]
    fn test_pause_does_not_wake_inactive_stopped_slot() {
        let backend = match AudioBackend::new() {
            Ok(b) => b,
            Err(_) => return,
        };

        backend.play_url("file:///tmp/active_pause_test.mp3");
        std::thread::sleep(std::time::Duration::from_millis(30));

        // Неактивный слот 1 остановлен; pause() и pause_soft(0) не должны переводить его в Paused/Playing
        backend.pause();
        std::thread::sleep(std::time::Duration::from_millis(30));

        let (_, state, pending) = backend.slots[1].play.pipeline().state(gstreamer::ClockTime::ZERO);
        assert!(
            !matches!(state, gstreamer::State::Paused | gstreamer::State::Playing),
            "неактивный остановленный слот не должен переходить в {state:?}"
        );
        assert!(
            !matches!(pending, gstreamer::State::Paused | gstreamer::State::Playing),
            "неактивный остановленный слот не должен планировать переход в {pending:?}"
        );
        assert!(!backend.is_playing());

        backend.play();
        backend.pause_soft(0);
        std::thread::sleep(std::time::Duration::from_millis(30));

        let (_, state_soft, pending_soft) = backend.slots[1].play.pipeline().state(gstreamer::ClockTime::ZERO);
        assert!(
            !matches!(state_soft, gstreamer::State::Paused | gstreamer::State::Playing),
            "pause_soft не должен будить неактивный слот ({state_soft:?})"
        );
        assert!(
            !matches!(pending_soft, gstreamer::State::Paused | gstreamer::State::Playing),
            "pause_soft не должен планировать переход неактивного слота в {pending_soft:?}"
        );

        std::mem::forget(backend);
    }

    #[tokio::test]
    async fn test_active_uri_and_play_url_at_ms_preserves_preroll() {
        let backend = match AudioBackend::new() {
            Ok(b) => b,
            Err(_) => return,
        };
        backend.set_volume(0.75);

        assert_eq!(backend.active_uri(), None);
        assert!(!backend.is_fading());

        backend.play_url("https://example.com/stream.mp3");
        assert_eq!(
            backend.active_uri().as_deref(),
            Some("https://example.com/stream.mp3")
        );

        let preroll_uri = "file:///tmp/preroll_keep.mp3";
        backend.preroll_next_url(preroll_uri);
        assert!(backend.slots[1].is_prerolled.load(Ordering::SeqCst));

        // Переключение при position_ms == 0 обновляет активный URI и сохраняет преролл в слоте 1
        backend.play_url_at_ms("file:///tmp/cached_zero.mp3", 0);
        assert_eq!(
            backend.active_uri().as_deref(),
            Some("file:///tmp/cached_zero.mp3")
        );
        assert!(backend.is_playing());
        assert!((backend.raw_volume() - 0.75).abs() < 1e-6);
        assert!(backend.slots[1].is_prerolled.load(Ordering::SeqCst));
        assert_eq!(
            backend.slots[1].current_uri.lock().unwrap().as_deref(),
            Some(preroll_uri)
        );

        // Переключение с position_ms > 0 ставит громкость 0.0 до готовности seek и тоже сохраняет преролл
        backend.play_url_at_ms("file:///tmp/cached_seek.mp3", 12_345);
        assert_eq!(
            backend.active_uri().as_deref(),
            Some("file:///tmp/cached_seek.mp3")
        );
        assert!(backend.is_playing());
        assert!((backend.raw_volume() - 0.0).abs() < 1e-6);
        assert!(backend.slots[1].is_prerolled.load(Ordering::SeqCst));
        assert_eq!(
            backend.slots[1].current_uri.lock().unwrap().as_deref(),
            Some(preroll_uri)
        );

        std::mem::forget(backend);
    }

    #[tokio::test]
    async fn test_slot_error_cleared_on_restart_and_crossfade_aborts_fast() {
        let backend = match AudioBackend::new() {
            Ok(b) => b,
            Err(_) => return,
        };

        // 1. Имитируем ошибку на слоте 1 и проверяем, что play_url очищает last_error при новом запуске
        {
            let mut guard = backend.slots[0].last_error.lock().unwrap();
            *guard = Some("старая ошибка".to_string());
        }
        backend.play_url("file:///tmp/does_not_exist_12345.mp3");
        assert_eq!(*backend.slots[0].last_error.lock().unwrap(), None);

        // 2. Слот 0 активен (допустим, играет), запускаем crossfade_to_url_with_outcome на несуществующий файл/схему:
        // ожидание должно прерваться по ошибке GStreamer / Stopped сильно быстрее 15 секунд,
        // а active_slot не должен остаться на сломанном слоте 1.
        backend.active_slot.store(0, Ordering::SeqCst);
        if let Ok(mut lp) = backend.slots[0].is_playing.lock() {
            *lp = true;
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let started = std::time::Instant::now();
        backend.crossfade_to_url_with_outcome("file:///tmp/definitely_missing_volna_test.mp3", 1500, move |outcome| {
            let _ = tx.send(outcome);
        });

        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), rx)
            .await
            .expect("кроссфейд не должен ждать 15 с при ошибке нового слота")
            .expect("канал результата не должен закрыться");
        assert!(
            matches!(outcome, StartOutcome::Failed(_)),
            "ожидалась StartOutcome::Failed, получено: {outcome:?}"
        );
        assert!(started.elapsed().as_secs() < 5);
        assert_eq!(backend.active_slot.load(Ordering::SeqCst), 0);

        std::mem::forget(backend);
    }

    #[test]
    fn test_should_emit_buffering_active_vs_starting_vs_preroll() {
        // Активный играющий слот 0 -> true
        assert!(should_emit_buffering(0, 0, true, 3, 0));
        // Остановленный активный слот (после stop) -> false
        assert!(!should_emit_buffering(0, 0, false, 3, 0));
        // Стартующий слот 1 во время кроссфейда (active_slot еще 0, но start_wait_gen == slot_gen) -> true
        assert!(should_emit_buffering(1, 0, true, 5, 5));
        // Фоновый преролл на неактивном слоте 1 (start_wait_gen устарел или 0) -> false
        assert!(!should_emit_buffering(1, 0, false, 6, 5));
        assert!(!should_emit_buffering(1, 0, false, 6, 0));
    }

    #[test]
    fn test_slot_error_callback_deduplicates_bus_and_adapter_signals() {
        let backend = match AudioBackend::new() {
            Ok(b) => b,
            Err(_) => return,
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_cb = calls.clone();
        backend.set_on_error(move |_| {
            calls_cb.fetch_add(1, Ordering::SeqCst);
        });

        let slot = &backend.slots[0];
        let ctx = SlotErrorCtx {
            index: 0,
            last_error: slot.last_error.clone(),
            error_gen: slot.error_gen.clone(),
            is_prerolled: slot.is_prerolled.clone(),
            is_playing: slot.is_playing.clone(),
            slot_gen: slot.slot_gen.clone(),
            start_wait_gen: slot.start_wait_gen.clone(),
            reported_error_gen: slot.reported_error_gen.clone(),
            active_slot: backend.active_slot.clone(),
            on_error: backend.on_error.clone(),
        };

        let g1 = slot.bump_gen();
        slot.clear_error();
        // Имитация двойной доставки одной ошибки: сначала sync-message шины, затем PlaySignalAdapter
        ctx.record("bus", "HTTP 403".to_string(), None);
        ctx.record("adapter", "HTTP 403".to_string(), Some("details".to_string()));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(slot.last_error.lock().unwrap().as_deref(), Some("HTTP 403"));
        assert_eq!(slot.reported_error_gen.load(Ordering::SeqCst), g1);

        // При новом запуске (новое slot_gen + clear_error) глобальный callback снова срабатывает один раз
        let g2 = slot.bump_gen();
        slot.clear_error();
        assert_ne!(g1, g2);
        ctx.record("bus", "Network error".to_string(), None);
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // Если запущено ожидание старта (start_wait_gen == cur_gen), глобальный callback подавляется в пользу waiter'а,
        // а отложенная доставка того же события из адаптера уже после выхода waiter'а не вызывает дубль.
        let g3 = slot.bump_gen();
        slot.clear_error();
        slot.start_wait_gen.store(g3, Ordering::SeqCst);
        let gen_before = slot.error_gen.load(Ordering::SeqCst);
        ctx.record("bus", "Early failure".to_string(), None);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(slot.error_gen.load(Ordering::SeqCst) > gen_before);
        assert_eq!(slot.last_error.lock().unwrap().as_deref(), Some("Early failure"));

        slot.start_wait_gen.store(0, Ordering::SeqCst);
        ctx.record("adapter", "Early failure".to_string(), None);
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        std::mem::forget(backend);
    }

    #[test]
    fn test_start_progress_tracker_ignores_early_stop_and_detects_eos_or_stalled_stop() {
        let t0 = tokio::time::Instant::now();
        let d20 = tokio::time::Duration::from_millis(20);
        let d40 = tokio::time::Duration::from_millis(40);
        let d100 = tokio::time::Duration::from_millis(100);

        // 1. Ранний PLAY_STATE_STOPPED (до движения вверх от нового URI) не срывает старт даже спустя >40 мс
        let mut tracker = StartProgressTracker::new(false);
        assert_eq!(
            tracker.step(
                t0,
                gstreamer::State::Null,
                false,
                None,
                true,
                PLAY_STATE_STOPPED,
                false,
                false,
            ),
            StartPollSignal::Pending
        );
        assert_eq!(
            tracker.step(
                t0 + d100,
                gstreamer::State::Null,
                false,
                None,
                true,
                PLAY_STATE_STOPPED,
                false,
                false,
            ),
            StartPollSignal::Pending
        );

        // 2. После движения вверх откат в Null/Ready даёт Failed спустя 40 мс (но не на 20 мс)
        assert_eq!(
            tracker.step(
                t0 + d100 + d20,
                gstreamer::State::Paused,
                false,
                None,
                true,
                PLAY_STATE_PAUSED,
                true,
                false,
            ),
            StartPollSignal::Pending
        );
        assert_eq!(
            tracker.step(
                t0 + d100 + d40,
                gstreamer::State::Ready,
                false,
                None,
                true,
                PLAY_STATE_STOPPED,
                false,
                false,
            ),
            StartPollSignal::Pending
        );
        assert_eq!(
            tracker.step(
                t0 + d100 + d40 + d20,
                gstreamer::State::Null,
                false,
                None,
                true,
                PLAY_STATE_STOPPED,
                false,
                false,
            ),
            StartPollSignal::Pending
        );
        assert!(matches!(
            tracker.step(
                t0 + d100 + d40 + d40,
                gstreamer::State::Null,
                false,
                None,
                true,
                PLAY_STATE_STOPPED,
                false,
                false,
            ),
            StartPollSignal::Failed(msg) if msg.contains("State::Stopped")
        ));

        // 3. Немедленный EOS до начала воспроизведения прерывает старт сразу без ожидания 40 мс
        let mut eos_tracker = StartProgressTracker::new(false);
        assert!(matches!(
            eos_tracker.step(
                t0,
                gstreamer::State::Ready,
                false,
                None,
                false,
                PLAY_STATE_STOPPED,
                false,
                true,
            ),
            StartPollSignal::Failed(msg) if msg.contains("EOS")
        ));
    }

    #[test]
    fn test_configure_http_source_sets_retries_and_youtube_headers() {
        if gstreamer::init().is_err() {
            return;
        }
        let Ok(source) = gstreamer::ElementFactory::make("souphttpsrc").build() else {
            return;
        };

        configure_http_source(
            &source,
            Some("https://rr3---sn-u15hn5-cc.googlevideo.com/videoplayback?itag=251"),
        );

        assert_eq!(source.property::<i32>("retries"), 5);
        assert_eq!(source.property::<u32>("timeout"), 15);
        assert!(source.property::<bool>("keep-alive"));
        let headers = source.property::<gstreamer::Structure>("extra-headers");
        assert_eq!(
            headers.get::<&str>("Referer").ok(),
            Some("https://www.youtube.com/")
        );
        assert_eq!(
            headers.get::<&str>("Origin").ok(),
            Some("https://www.youtube.com")
        );

        // Для не-googlevideo URI extra-headers не должны выставляться
        let Ok(non_yt_source) = gstreamer::ElementFactory::make("souphttpsrc").build() else {
            return;
        };
        configure_http_source(&non_yt_source, Some("https://music.yandex.ru/track.mp3"));
        assert_eq!(non_yt_source.property::<i32>("retries"), 5);
        assert!(
            non_yt_source
                .property::<Option<gstreamer::Structure>>("extra-headers")
                .is_none()
        );
    }
}
