use gstreamer::prelude::*;
use gstreamer_play::{Play, PlaySignalAdapter};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
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

/// Слот плеера GStreamer для двухканального воспроизведения и бесшовного кроссфейда
struct PlayerSlot {
    play: Play,
    _adapter: PlaySignalAdapter,
    is_playing: Arc<Mutex<bool>>,
    bass: BassQueue,
}

impl PlayerSlot {
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
        if pipeline.find_property("flags").is_some() {
            pipeline.set_property_from_str("flags", "audio+soft-volume+download");
        }

        let bass = BassQueue::default();
        if let Err(e) = attach_bass_analyzer(&play.pipeline(), bass.clone()) {
            eprintln!("Анализатор баса недоступен для слота {}: {}", index, e);
        }

        let is_playing = Arc::new(Mutex::new(false));

        Ok(Self {
            play,
            _adapter: adapter,
            is_playing,
            bass,
        })
    }
}

/// Аудио-бэкенд на базе двух слотов GStreamer Play для одновременного воспроизведения и настоящей сводки (кроссфейда)
pub struct AudioBackend {
    slots: [PlayerSlot; 2],
    active_slot: Arc<AtomicUsize>,
    user_volume: Arc<Mutex<f64>>,
    fade_gen: Arc<AtomicU64>,
}

impl AudioBackend {
    /// Инициализирует аудио бэкенд с двумя независимыми слотами плеера
    pub fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        gstreamer::init()?;
        let slot0 = PlayerSlot::new(0)?;
        let slot1 = PlayerSlot::new(1)?;

        let initial_vol = slot0.play.volume().clamp(0.0, 1.0);
        let user_volume = Arc::new(Mutex::new(if initial_vol <= 0.0 { 1.0 } else { initial_vol }));
        let fade_gen = Arc::new(AtomicU64::new(0));
        let active_slot = Arc::new(AtomicUsize::new(0));

        Ok(Self {
            slots: [slot0, slot1],
            active_slot,
            user_volume,
            fade_gen,
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

    /// Загружает и начинает воспроизведение трека по прямой ссылке в текущий слот (без кроссфейда)
    pub fn play_url(&self, url: &str) {
        eprintln!("[audio] play_url {}", url.chars().take(90).collect::<String>());
        self.fade_gen.fetch_add(1, Ordering::SeqCst);
        let curr_idx = self.active_slot.load(Ordering::SeqCst) % 2;
        let other_idx = 1 - curr_idx;

        self.slots[other_idx].play.stop();
        if let Ok(mut lock) = self.slots[other_idx].is_playing.lock() {
            *lock = false;
        }

        let user_vol = self.user_volume();
        let current = &self.slots[curr_idx];
        current.play.set_volume(user_vol);
        current.play.set_uri(Some(url));
        current.play.play();
        if let Ok(mut lock) = current.is_playing.lock() {
            *lock = true;
        }
    }

    /// Загружает трек и начинает воспроизведение с плавным нарастанием громкости (Fade-In) от 0.0 до user_volume
    pub fn play_url_fade_in(&self, url: &str, duration_ms: u64) {
        eprintln!("[audio] play_url_fade_in ({} ms) {}", duration_ms, url.chars().take(90).collect::<String>());
        self.fade_gen.fetch_add(1, Ordering::SeqCst);
        let curr_idx = self.active_slot.load(Ordering::SeqCst) % 2;
        let other_idx = 1 - curr_idx;

        self.slots[other_idx].play.stop();
        if let Ok(mut lock) = self.slots[other_idx].is_playing.lock() {
            *lock = false;
        }

        let current = &self.slots[curr_idx];
        current.play.set_volume(0.0);
        current.play.set_uri(Some(url));
        current.play.play();
        if let Ok(mut lock) = current.is_playing.lock() {
            *lock = true;
        }

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

        let current_slot = &self.slots[current_idx];
        let next_slot = &self.slots[next_idx];

        // Подготавливаем следующий слот с нулевой громкостью и запускаем воспроизведение
        next_slot.play.set_volume(0.0);
        next_slot.play.set_uri(Some(url));
        next_slot.play.play();
        if let Ok(mut lock) = next_slot.is_playing.lock() {
            *lock = true;
        }

        // Мгновенно переключаем активный слот на новый трек, чтобы position_ms() и UI
        // не считывали устаревшую позицию конца предыдущего трека (устраняет дергание полосы
        // прогресса и ложные срабатывания автокроссфейда).
        self.active_slot.store(next_idx, Ordering::SeqCst);

        let user_volume_arc = self.user_volume.clone();
        let user_vol = self.user_volume();

        let current_play = current_slot.play.clone();
        let next_play = next_slot.play.clone();
        let current_is_playing = current_slot.is_playing.clone();

        let generation_id = self.fade_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let fade_gen = self.fade_gen.clone();

        if duration_ms == 0 {
            current_play.stop();
            if let Ok(mut lock) = current_is_playing.lock() {
                *lock = false;
            }
            next_play.set_volume(user_vol);
            return;
        }

        let start_current_vol = current_play.volume().min(user_vol);

        tokio::spawn(async move {
            // Ждем, пока новый поток сбуферизируется и физически перейдет в Playing
            let wait_start = tokio::time::Instant::now();
            let mut started = false;
            loop {
                if fade_gen.load(Ordering::SeqCst) != generation_id {
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
                eprintln!("[audio] новый поток не перешел в Playing за 15 с, отменяем затухание старого");
                return;
            }

            const STEP_MS: u64 = 25;
            let steps = (duration_ms / STEP_MS).max(1);
            let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(STEP_MS));

            for step in 1..=steps {
                interval.tick().await;
                if fade_gen.load(Ordering::SeqCst) != generation_id {
                    // Фейд был перехвачен новым переключением или остановкой
                    return;
                }
                let t = step as f64 / steps as f64;
                let angle = t * std::f64::consts::FRAC_PI_2;
                let out_factor = angle.cos();
                let in_factor = angle.sin();

                let cur_user_vol = user_volume_arc.lock().map(|l| *l).unwrap_or(1.0);
                let current_v = (start_current_vol * out_factor).min(cur_user_vol);
                let next_v = cur_user_vol * in_factor;

                current_play.set_volume(current_v.clamp(0.0, 1.0));
                next_play.set_volume(next_v.clamp(0.0, 1.0));
            }

            if fade_gen.load(Ordering::SeqCst) == generation_id {
                current_play.stop();
                if let Ok(mut lock) = current_is_playing.lock() {
                    *lock = false;
                }
                let cur_user_vol = user_volume_arc.lock().map(|l| *l).unwrap_or(1.0);
                next_play.set_volume(cur_user_vol);
            }
        });
    }

    /// Останавливает оба слота конвейера и отменяет активный fade
    pub fn stop(&self) {
        self.fade_gen.fetch_add(1, Ordering::SeqCst);
        for slot in &self.slots {
            slot.play.stop();
            if let Ok(mut lock) = slot.is_playing.lock() {
                *lock = false;
            }
        }
    }

    /// Возобновляет воспроизведение активного слота немедленно (на установленной громкости)
    pub fn play(&self) {
        self.fade_gen.fetch_add(1, Ordering::SeqCst);
        let active = self.active();
        active.play.set_volume(self.user_volume());
        active.play.play();
        if let Ok(mut lock) = active.is_playing.lock() {
            *lock = true;
        }
    }

    /// Возобновляет воспроизведение с мягким нарастанием громкости за duration_ms
    pub fn play_soft(&self, duration_ms: u64) {
        let active = self.active();
        if let Ok(mut lock) = active.is_playing.lock() {
            *lock = true;
        }
        active.play.set_volume(0.0);
        active.play.play();
        self.fade_in(duration_ms);
    }

    /// Приостанавливает воспроизведение немедленно на обоих слотах
    pub fn pause(&self) {
        self.fade_gen.fetch_add(1, Ordering::SeqCst);
        for slot in &self.slots {
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
        self.active().play.seek(gstreamer::ClockTime::from_mseconds(ms));
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
        let play = self.active().play.clone();
        let start_vol = play.volume();

        if duration_ms == 0 || (start_vol - target).abs() < 1e-4 {
            play.set_volume(target);
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
                play.set_volume(current.clamp(0.0, 1.0));
            }

            if fade_gen.load(Ordering::SeqCst) == generation_id {
                play.set_volume(target);
                if let Some(done) = on_done {
                    done();
                }
            }
        });
    }

    /// Плавный старт: стартует с громкости 0.0 и плавно наращивает до user_volume
    pub fn fade_in(&self, duration_ms: u64) {
        self.active().play.set_volume(0.0);
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
        self.active().play.set_volume(0.0);
        let target = self.user_volume();
        self.fade_volume_async(target, duration_ms).await;
    }

    /// Асинхронная интерполяция громкости до target с шагом 25 мс
    pub async fn fade_volume_async(&self, target: f64, duration_ms: u64) {
        let target = target.clamp(0.0, 1.0);
        let generation_id = self.fade_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let fade_gen = self.fade_gen.clone();
        let play = self.active().play.clone();
        let start_vol = play.volume();

        if duration_ms == 0 || (start_vol - target).abs() < 1e-4 {
            play.set_volume(target);
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
            play.set_volume(current.clamp(0.0, 1.0));
        }

        if fade_gen.load(Ordering::SeqCst) == generation_id {
            play.set_volume(target);
        }
    }

    /// Установка громкости (от 0.0 до 1.0). Сохраняет в user_volume и отменяет активный fade
    pub fn set_volume(&self, volume: f64) {
        let v = volume.clamp(0.0, 1.0);
        if let Ok(mut lock) = self.user_volume.lock() {
            *lock = v;
        }
        self.fade_gen.fetch_add(1, Ordering::SeqCst);
        self.active().play.set_volume(v);
    }

    /// Целевая пользовательская громкость (по умолчанию 1.0 или заданная пользователем)
    pub fn user_volume(&self) -> f64 {
        self.user_volume.lock().map(|l| *l).unwrap_or(1.0)
    }

    /// Получение текущей пользовательской громкости
    pub fn volume(&self) -> f64 {
        self.user_volume()
    }

    /// Фактическая громкость движка GStreamer активного слота
    pub fn raw_volume(&self) -> f64 {
        self.active().play.volume()
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
}
