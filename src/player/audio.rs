use gstreamer::prelude::*;
use gstreamer_play::{Play, PlaySignalAdapter};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

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

    fn new(index: usize) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let play = Play::new(None::<gstreamer_play::PlayVideoRenderer>);
        let adapter = PlaySignalAdapter::new(&play);
        adapter.connect_error(move |_, err, details| {
            eprintln!("[gst slot {}] ошибка: {} ({:?})", index, err, details.map(|d| d.to_string()));
        });
        adapter.connect_warning(move |_, err, _| {
            eprintln!("[gst slot {}] предупреждение: {}", index, err);
        });
        adapter.connect_state_changed(move |_, state| {
            eprintln!("[gst slot {}] состояние: {:?}", index, state);
        });
        adapter.connect_buffering(move |_, percent| {
            if percent % 25 == 0 {
                eprintln!("[gst slot {}] буфер {}%", index, percent);
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
            let _ = pipeline.set_property("buffer-duration", 10_000_000_000i64); // 10 секунд буфера
        }
        if pipeline.find_property("buffer-size").is_some() {
            let _ = pipeline.set_property("buffer-size", 4 * 1024 * 1024i32); // 4 МБ
        }
        if pipeline.find_property("ring-buffer-max-size").is_some() {
            let _ = pipeline.set_property("ring-buffer-max-size", 8 * 1024 * 1024u64); // 8 МБ
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

        if let Some(bus) = pipeline.bus() {
            let pipe_weak = pipeline.downgrade();
            bus.set_sync_handler(move |_, msg| {
                if let gstreamer::MessageView::Buffering(b) = msg.view() {
                    if b.percent() < 100 {
                        if let Some(pipe) = pipe_weak.upgrade() {
                            if pipe.current_state() == gstreamer::State::Playing {
                                return gstreamer::BusSyncReply::Drop;
                            }
                        }
                    }
                }
                gstreamer::BusSyncReply::Pass
            });
        }

        pipeline.connect("source-setup", false, |values| {
            if let Some(source) = values.get(1).and_then(|v| v.get::<gstreamer::Element>().ok()) {
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
            }
            None
        });

        let bass = BassQueue::default();
        if let Err(e) = attach_bass_analyzer(&play.pipeline(), bass.clone()) {
            eprintln!("Анализатор баса недоступен для слота {}: {}", index, e);
        }

        let is_playing = Arc::new(Mutex::new(false));
        let current_uri = Arc::new(Mutex::new(None));
        let is_prerolled = Arc::new(AtomicBool::new(false));
        let slot_gen = Arc::new(AtomicU64::new(0));

        Ok(Self {
            play,
            volume,
            _adapter: adapter,
            is_playing,
            bass,
            current_uri,
            is_prerolled,
            slot_gen,
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

fn configure_pipeline_for_url(pipeline: &gstreamer::Element, url: &str) {
    if is_hls_url(url) {
        // Для HLS плейлистов флаги без buffering и без download, чтобы playbin3
        // не создавал паразитный queue2 с жестким ожиданием 10 секунд
        if pipeline.find_property("flags").is_some() {
            pipeline.set_property_from_str("flags", "audio");
        }
        if pipeline.find_property("buffer-duration").is_some() {
            let _ = pipeline.set_property("buffer-duration", -1i64);
        }
        if pipeline.find_property("buffer-size").is_some() {
            let _ = pipeline.set_property("buffer-size", -1i32);
        }
    } else {
        // Для прогрессивных MP3/FLAC/Opus/AAC используем download и 10s буфер
        if pipeline.find_property("flags").is_some() {
            pipeline.set_property_from_str("flags", "audio+download");
        }
        if pipeline.find_property("buffer-duration").is_some() {
            let _ = pipeline.set_property("buffer-duration", 10_000_000_000i64);
        }
        if pipeline.find_property("buffer-size").is_some() {
            let _ = pipeline.set_property("buffer-size", 4 * 1024 * 1024i32);
        }
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
    idx: usize,
    url: &str,
) {
    if let Ok(cur) = current_uri.lock() {
        if cur.as_deref() == Some(url) && is_prerolled.load(Ordering::SeqCst) {
            return;
        }
    }
    slot_gen.fetch_add(1, Ordering::SeqCst);
    configure_pipeline_for_url(&play.pipeline(), url);
    play.stop();
    set_element_volume(volume, 0.0);
    play.set_uri(Some(url));
    play.pause(); // Начинает сетевую буферизацию до 100% без воспроизведения звука
    is_prerolled.store(true, Ordering::SeqCst);
    if let Ok(mut cur) = current_uri.lock() {
        *cur = Some(url.to_string());
    }
    eprintln!("[audio] Аппаратная предбуферизация следующего трека в слот {}: {}", idx, url);
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
}

impl AudioBackend {
    /// Инициализирует аудио бэкенд с двумя независимыми слотами плеера
    pub fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        gstreamer::init()?;
        let slot0 = PlayerSlot::new(0)?;
        let slot1 = PlayerSlot::new(1)?;

        let initial_vol = crate::api::auth::load_volume();
        slot0.set_slot_volume(initial_vol);
        slot1.set_slot_volume(0.0);
        let user_volume = Arc::new(Mutex::new(initial_vol));
        let fade_gen = Arc::new(AtomicU64::new(0));
        let fading_gen = Arc::new(AtomicU64::new(0));
        let active_slot = Arc::new(AtomicUsize::new(0));

        Ok(Self {
            slots: [slot0, slot1],
            active_slot,
            user_volume,
            fade_gen,
            fading_gen,
            pending_preroll: Arc::new(Mutex::new(None)),
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
            slot._adapter.connect_end_of_stream(move |_| {
                if active_slot.load(Ordering::SeqCst) == idx {
                    eprintln!("[gst slot {}] конец трека (EOS)", idx);
                    cb();
                } else {
                    eprintln!("[gst slot {}] конец трека (EOS) на неактивном слоте (игнорируем)", idx);
                }
            });
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
            slot_idx: inactive_idx,
        };
        uris.flush();
    }

    /// Загружает и начинает воспроизведение трека по прямой ссылке в текущий слот (без кроссфейда)
    pub fn play_url(&self, url: &str) {
        eprintln!("[audio] play_url {}", url.chars().take(90).collect::<String>());
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

        if is_match {
            let old_idx = self.active_slot.swap(inactive_idx, Ordering::SeqCst);
            self.slots[old_idx].bump_gen();
            self.slots[old_idx].play.stop();
            self.slots[old_idx].is_prerolled.store(false, Ordering::SeqCst);
            if let Ok(mut lock) = self.slots[old_idx].is_playing.lock() {
                *lock = false;
            }
            let vol = self.user_volume();
            self.slots[inactive_idx].bump_gen();
            self.slots[inactive_idx].is_prerolled.store(false, Ordering::SeqCst);
            self.slots[inactive_idx].set_slot_volume(vol);
            configure_pipeline_for_url(&self.slots[inactive_idx].play.pipeline(), url);
            self.slots[inactive_idx].play.play();
            if let Ok(mut lock) = self.slots[inactive_idx].is_playing.lock() {
                *lock = true;
            }
            eprintln!("[audio] Мгновенный 0-мс старт предбуферизованного слота {}", inactive_idx);
            self.flush_pending_preroll();
            return;
        }

        self.slots[inactive_idx].bump_gen();
        self.slots[inactive_idx].play.stop();
        slot.is_prerolled.store(false, Ordering::SeqCst);
        if let Ok(mut lock) = self.slots[inactive_idx].is_playing.lock() {
            *lock = false;
        }

        let current = &self.slots[curr_idx];
        current.bump_gen();
        current.is_prerolled.store(false, Ordering::SeqCst);
        let vol = self.user_volume();
        current.set_slot_volume(vol);
        current.play.set_uri(Some(url));
        configure_pipeline_for_url(&current.play.pipeline(), url);
        current.play.play();
        if let Ok(mut cur) = current.current_uri.lock() {
            *cur = Some(url.to_string());
        }
        if let Ok(mut lock) = current.is_playing.lock() {
            *lock = true;
        }
        self.flush_pending_preroll();
    }

    /// Загружает трек и начинает воспроизведение с плавным нарастанием громкости (Fade-In) от 0.0 до user_volume
    pub fn play_url_fade_in(&self, url: &str, duration_ms: u64) {
        eprintln!("[audio] play_url_fade_in ({} ms) {}", duration_ms, url.chars().take(90).collect::<String>());
        self.fade_gen.fetch_add(1, Ordering::SeqCst);
        self.fading_gen.store(0, Ordering::SeqCst);
        let curr_idx = self.active_slot.load(Ordering::SeqCst) % 2;
        let inactive_idx = 1 - curr_idx;

        let slot = &self.slots[inactive_idx];
        let is_match = {
            let cur_match = slot.current_uri.lock().map(|cur| cur.as_deref() == Some(url)).unwrap_or(false);
            cur_match && slot.is_prerolled.load(Ordering::SeqCst)
        };

        if is_match {
            let old_idx = self.active_slot.swap(inactive_idx, Ordering::SeqCst);
            self.slots[old_idx].bump_gen();
            self.slots[old_idx].play.stop();
            self.slots[old_idx].is_prerolled.store(false, Ordering::SeqCst);
            if let Ok(mut lock) = self.slots[old_idx].is_playing.lock() {
                *lock = false;
            }
            self.slots[inactive_idx].bump_gen();
            self.slots[inactive_idx].is_prerolled.store(false, Ordering::SeqCst);
            self.slots[inactive_idx].set_slot_volume(0.0);
            configure_pipeline_for_url(&self.slots[inactive_idx].play.pipeline(), url);
            self.slots[inactive_idx].play.play();
            if let Ok(mut lock) = self.slots[inactive_idx].is_playing.lock() {
                *lock = true;
            }
            eprintln!("[audio] Мгновенный 0-мс старт предбуферизованного слота {} с fade-in", inactive_idx);
            self.flush_pending_preroll();
            self.fade_in(duration_ms);
            return;
        }

        self.slots[inactive_idx].bump_gen();
        self.slots[inactive_idx].play.stop();
        slot.is_prerolled.store(false, Ordering::SeqCst);
        if let Ok(mut lock) = self.slots[inactive_idx].is_playing.lock() {
            *lock = false;
        }

        let current = &self.slots[curr_idx];
        current.bump_gen();
        current.is_prerolled.store(false, Ordering::SeqCst);
        current.set_slot_volume(0.0);
        current.play.set_uri(Some(url));
        configure_pipeline_for_url(&current.play.pipeline(), url);
        current.play.play();
        if let Ok(mut cur) = current.current_uri.lock() {
            *cur = Some(url.to_string());
        }
        if let Ok(mut lock) = current.is_playing.lock() {
            *lock = true;
        }

        self.flush_pending_preroll();
        self.fade_in(duration_ms);
    }

    /// Настоящий двухканальный кроссфейд: старый трек плавно затухает (1.0 -> 0.0),
    /// а новый трек параллельно нарастает (0.0 -> 1.0) в течение duration_ms,
    /// оба трека звучат ОДНОВРЕМЕННО без промежуточной паузы и тишины!
    pub fn crossfade_to_url(&self, url: &str, duration_ms: u64) {
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
        }
        self.active_slot.store(next_idx, Ordering::SeqCst);

        let current_slot = &self.slots[current_idx];
        let next_slot = &self.slots[next_idx];
        let outgoing_slot_gen = current_slot.slot_gen.load(Ordering::SeqCst);
        next_slot.bump_gen();

        configure_pipeline_for_url(&next_slot.play.pipeline(), url);

        let is_already_prerolled = {
            let cur_match = next_slot.current_uri.lock().map(|cur| cur.as_deref() == Some(url)).unwrap_or(false);
            cur_match && next_slot.is_prerolled.load(Ordering::SeqCst)
        };

        if is_already_prerolled {
            eprintln!("[audio] мгновенный старт предзагруженного трека из слота {} (0 мс задержки)!", next_idx);
            next_slot.set_slot_volume(0.0);
            next_slot.play.play();
        } else {
            next_slot.play.stop();
            next_slot.set_slot_volume(0.0);
            next_slot.play.set_uri(Some(url));
            next_slot.play.play();
            if let Ok(mut cur) = next_slot.current_uri.lock() {
                *cur = Some(url.to_string());
            }
        }
        next_slot.is_prerolled.store(false, Ordering::SeqCst);
        if let Ok(mut lock) = next_slot.is_playing.lock() {
            *lock = true;
        }

        let user_volume_arc = self.user_volume.clone();
        let user_vol = self.user_volume();

        let current_play = current_slot.play.clone();
        let current_volume = current_slot.volume.clone();
        let next_play = next_slot.play.clone();
        let next_volume = next_slot.volume.clone();
        let current_is_playing = current_slot.is_playing.clone();
        let current_slot_gen = current_slot.slot_gen.clone();

        if duration_ms == 0 {
            current_play.stop();
            if let Ok(mut lock) = current_is_playing.lock() {
                *lock = false;
            }
            set_element_volume(&next_volume, user_vol);
            self.flush_pending_preroll();
            return;
        }

        let start_current_vol = current_slot.slot_volume().min(user_vol);
        let fading_gen = self.fading_gen.clone();
        let pending_preroll = self.pending_preroll.clone();
        let current_uri = current_slot.current_uri.clone();
        let current_prerolled = current_slot.is_prerolled.clone();
        let next_uri = next_slot.current_uri.clone();

        tokio::spawn(async move {
            let stop_outgoing_if_unclaimed = || {
                // Если слот уже перехватило следующее переключение или преролл, не останавливаем чужой запуск.
                if current_slot_gen.load(Ordering::SeqCst) == outgoing_slot_gen {
                    current_play.stop();
                    if let Ok(mut lock) = current_is_playing.lock() {
                        *lock = false;
                    }
                }
            };

            // Ждем, пока новый поток сбуферизируется и физически перейдет в Playing
            let wait_start = tokio::time::Instant::now();
            let mut started = false;
            loop {
                if fade_gen.load(Ordering::SeqCst) != generation_id {
                    stop_outgoing_if_unclaimed();
                    return;
                }
                if next_play.pipeline().current_state() == gstreamer::State::Playing {
                    started = true;
                    break;
                }
                // Ждём буферизацию до 15 секунд; если поток не заиграл — не глушим старый трек
                if wait_start.elapsed().as_millis() > 15_000 {
                    break;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
            }

            if !started {
                // Новый поток так и не заиграл: вместо тишины отдаём трек без сводки, иначе
                // active_slot уже указывает на молчащий слот, а UI считает, что играет он.
                stop_outgoing_if_unclaimed();
                if fade_gen.load(Ordering::SeqCst) == generation_id {
                    eprintln!("[audio] новый поток не перешел в Playing за 15 с — переключаем без сводки");
                    let vol = user_volume_arc.lock().map(|l| *l).unwrap_or(1.0);
                    set_element_volume(&next_volume, vol);
                    if fading_gen.load(Ordering::SeqCst) == generation_id {
                        fading_gen.store(0, Ordering::SeqCst);
                    }
                    PendingPreroll {
                        pending: &pending_preroll,
                        playing_uri: &next_uri,
                        slot_play: &current_play,
                        slot_volume: &current_volume,
                        slot_uri: &current_uri,
                        slot_prerolled: &current_prerolled,
                        slot_gen: &current_slot_gen,
                        slot_idx: current_idx,
                    }
                    .flush();
                }
                return;
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

    /// Приостанавливает воспроизведение немедленно на обоих слотах
    pub fn pause(&self) {
        self.fade_gen.fetch_add(1, Ordering::SeqCst);
        for slot in &self.slots {
            slot.bump_gen();
            slot.play.pause();
            if let Ok(mut lock) = slot.is_playing.lock() {
                *lock = false;
            }
        }
    }

    /// Приостанавливает воспроизведение с мягким затуханием за duration_ms перед реальной паузой
    pub fn pause_soft(&self, duration_ms: u64) {
        for slot in &self.slots {
            if let Ok(mut lock) = slot.is_playing.lock() {
                *lock = false;
            }
        }
        let play0 = self.slots[0].play.clone();
        let play1 = self.slots[1].play.clone();
        let is_playing0 = self.slots[0].is_playing.clone();
        let is_playing1 = self.slots[1].is_playing.clone();
        self.fade_volume_internal(0.0, duration_ms, Some(Box::new(move || {
            if !is_playing0.lock().map(|l| *l).unwrap_or(false) {
                play0.pause();
            }
            if !is_playing1.lock().map(|l| *l).unwrap_or(false) {
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
}
