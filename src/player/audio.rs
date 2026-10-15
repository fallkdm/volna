use gstreamer::prelude::*;
use gstreamer_play::{Play, PlaySignalAdapter};
use std::collections::VecDeque;
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

/// Аудио-бэкенд на базе GStreamer Play с поддержкой сигналов воспроизведения
pub struct AudioBackend {
    play: Play,
    _adapter: PlaySignalAdapter,
    is_playing: Arc<Mutex<bool>>,
    bass: BassQueue,
}

impl AudioBackend {
    /// Инициализирует аудио бэкенд и адаптер сигналов
    pub fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        gstreamer::init()?;
        let play = Play::new(None::<gstreamer_play::PlayVideoRenderer>);
        let adapter = PlaySignalAdapter::new(&play);
        let is_playing = Arc::new(Mutex::new(false));
        let bass = BassQueue::default();
        // Без анализатора просто играем дальше: орб останется спокойным
        if let Err(e) = attach_bass_analyzer(&play.pipeline(), bass.clone()) {
            eprintln!("Анализатор баса недоступен (нужен плагин spectrum из gst-plugins-good): {}", e);
        }

        Ok(Self {
            play,
            _adapter: adapter,
            is_playing,
            bass,
        })
    }

    /// Уровень баса 0..1 для звучащего сейчас фрагмента: spectrum стоит до очередей playsink
    /// и опережает звук ~на 1–1.5 с, поэтому берём значение по running-time конвейера
    pub fn bass_level(&self) -> f32 {
        // На паузе часы идут, а данные в буфере стоят — читаем только в настоящем PLAYING
        // (флаг is_playing ставится раньше, когда base_time ещё старый)
        let pipeline = self.play.pipeline();
        if pipeline.current_state() != gstreamer::State::Playing {
            return 0.0;
        }
        let Some(now) = pipeline.current_running_time() else { return 0.0 };
        let Ok(mut q) = self.bass.lock() else { return 0.0 };
        while q.get(1).is_some_and(|&(t, _)| t <= now.nseconds()) {
            q.pop_front();
        }
        q.front().filter(|&&(t, _)| t <= now.nseconds()).map_or(0.0, |&(_, v)| v)
    }

    /// Регистрирует обработчик окончания воспроизведения трека (EndOfStream)
    pub fn set_on_eos<F: Fn() + Send + Sync + 'static>(&self, callback: F) {
        self._adapter.connect_end_of_stream(move |_| {
            callback();
        });
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

    /// Текущая позиция воспроизведения в миллисекундах (для точной синхронизации караоке)
    pub fn position_ms(&self) -> Option<u64> {
        self.play.position().map(|t| t.mseconds())
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
}
