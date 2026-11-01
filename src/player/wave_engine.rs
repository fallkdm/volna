use std::collections::{HashSet, VecDeque};
use crate::api::models::Track;
use crate::api::ytmusic::parser::is_clean_music_track;

/// Максимальное количество запоминаемых недавних треков
const MAX_RECENT_HISTORY: usize = 100;

/// Через сколько сыгранных с одного сида треков Волна переезжает на свежий сид.
/// Слишком мало — дёрганое звучание, слишком много — поджанровый пузырь.
pub const SEED_DRIFT_AFTER_TRACKS: usize = 12;

/// Режимы работы алгоритмической волны
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaveMode {
    /// «Моя Волна» / SmartMix — персональный поток
    SmartMix,
    /// Волна по заданному артисту
    ByArtist,
    /// Волна по заданному треку
    ByTrack,
}

impl Default for WaveMode {
    fn default() -> Self {
        Self::SmartMix
    }
}

/// Алгоритмический движок «Моя Волна» (Smart Wave Engine)
#[derive(Debug, Clone)]
pub struct WaveEngine {
    /// Последние сыгранные идентификаторы треков (до 100 штук) для предотвращения повторов
    pub recent_history: VecDeque<String>,
    /// Идентификаторы любимых артистов пользователя
    pub seed_artists: HashSet<String>,
    /// Режим работы волны
    pub mode: WaveMode,
    /// Сколько треков сыграно с момента последней смены сида (для Seed Drift)
    pub played_since_seed: usize,
    /// Номер переезда на свежий сид: чётные/нечётные сиды берутся из разных мест очереди
    pub drift_serial: u64,
}

impl Default for WaveEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl WaveEngine {
    /// Создает новый экземпляр WaveEngine с режимом по умолчанию
    pub fn new() -> Self {
        Self {
            recent_history: VecDeque::with_capacity(MAX_RECENT_HISTORY),
            seed_artists: HashSet::new(),
            mode: WaveMode::SmartMix,
            played_since_seed: 0,
            drift_serial: 0,
        }
    }

    /// Создает WaveEngine с указанным режимом
    pub fn with_mode(mode: WaveMode) -> Self {
        Self {
            recent_history: VecDeque::with_capacity(MAX_RECENT_HISTORY),
            seed_artists: HashSet::new(),
            mode,
            played_since_seed: 0,
            drift_serial: 0,
        }
    }

    /// Регистрирует сыгранный трек в историю недавних треков
    pub fn record_played(&mut self, track_id: &str) {
        let id = track_id.trim();
        if id.is_empty() {
            return;
        }

        // Если трек уже был в recent_history, удаляем старое вхождение, чтобы обновить позицию в конце очереди
        if let Some(pos) = self.recent_history.iter().position(|x| x == id) {
            self.recent_history.remove(pos);
        }

        self.recent_history.push_back(id.to_string());
        while self.recent_history.len() > MAX_RECENT_HISTORY {
            self.recent_history.pop_front();
        }
        self.played_since_seed = self.played_since_seed.saturating_add(1);
    }

    /// Треки, которые Волна выбросила из хвоста очереди (Seed Drift), тоже не должны
    /// вернуться в следующей порции — но счётчик сыгранного они не увеличивают.
    pub fn remember_dropped<T: AsRef<str>>(&mut self, ids: &[T]) {
        for id in ids {
            let id = id.as_ref().trim();
            if id.is_empty() {
                continue;
            }
            if let Some(pos) = self.recent_history.iter().position(|x| x == id) {
                self.recent_history.remove(pos);
            }
            self.recent_history.push_back(id.to_string());
            while self.recent_history.len() > MAX_RECENT_HISTORY {
                self.recent_history.pop_front();
            }
        }
    }

    /// Пора ли переезжать на свежий сид (Seed Drift)
    pub fn seed_drift_due(&self) -> bool {
        self.played_since_seed >= SEED_DRIFT_AFTER_TRACKS
    }

    /// Волна перешла на новый сид — счётчик сыгранного начинается заново
    pub fn mark_seed_started(&mut self) {
        self.played_since_seed = 0;
    }

    /// Отмечает переезд на свежий сид и возвращает его номер.
    ///
    /// Номер нужен, чтобы чередовать источники сида: нечётный переезд стартует не от
    /// текущего трека, а от дальнего края очереди — иначе Волна ходит по кругу одного
    /// и того же поджанрового пузыря.
    pub fn note_seed_drift(&mut self) -> u64 {
        self.drift_serial = self.drift_serial.wrapping_add(1);
        self.drift_serial
    }

    /// Фильтрует список кандидатов для волны:
    /// - исключает треки из `recent_history`;
    /// - отсекает треки через `is_clean_music_track`;
    /// - дедуплицирует по `id`.
    pub fn filter_candidates(&self, candidates: Vec<Track>) -> Vec<Track> {
        let history_set: HashSet<&str> = self.recent_history.iter().map(|s| s.as_str()).collect();
        let mut seen = HashSet::new();
        let mut result = Vec::new();

        for track in candidates {
            if track.id.is_empty() {
                continue;
            }
            if history_set.contains(track.id.as_str()) {
                continue;
            }
            if !is_clean_music_track(&track) {
                continue;
            }
            if seen.insert(track.id.clone()) {
                result.push(track);
            }
        }

        result
    }

    /// Возвращает `true`, если до конца очереди осталось меньше 3 треков
    pub fn refill_needed(&self, current_queue_len: usize, current_index: usize) -> bool {
        let remaining = current_queue_len.saturating_sub(current_index + 1);
        remaining < 3
    }

    /// Добавляет артиста в список seed-артистов
    pub fn add_seed_artist(&mut self, artist: String) {
        self.seed_artists.insert(artist);
    }

    /// Устанавливает режим работы волны
    pub fn set_mode(&mut self, mode: WaveMode) {
        self.mode = mode;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_track(id: &str, title: &str, duration_ms: Option<u64>) -> Track {
        Track {
            id: id.to_string(),
            title: title.to_string(),
            duration_ms,
            ..Default::default()
        }
    }

    #[test]
    fn test_record_played_and_limit() {
        let mut engine = WaveEngine::new();
        for i in 0..120 {
            engine.record_played(&format!("track_{}", i));
        }
        assert_eq!(engine.recent_history.len(), 100);
        assert_eq!(engine.recent_history.front().unwrap(), "track_20");
        assert_eq!(engine.recent_history.back().unwrap(), "track_119");
    }

    #[test]
    fn test_filter_candidates_excludes_history_and_dirty_and_duplicates() {
        let mut engine = WaveEngine::new();
        engine.record_played("played_1");

        let candidates = vec![
            make_track("played_1", "Already Played Track", Some(180_000)),
            make_track("good_1", "Great Song", Some(200_000)),
            make_track("podcast_1", "Daily Podcast Episode #4", Some(3_600_000)), // dirty
            make_track("good_1", "Duplicate of Great Song", Some(200_000)), // duplicate id
            make_track("good_2", "Another Music Piece", Some(150_000)),
        ];

        let filtered = engine.filter_candidates(candidates);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].id, "good_1");
        assert_eq!(filtered[1].id, "good_2");
    }

    #[test]
    fn test_refill_needed() {
        let engine = WaveEngine::new();
        // 5 треков, индекс 2 -> осталось 2 трека (индексы 3, 4) -> < 3 -> true
        assert!(engine.refill_needed(5, 2));
        // 5 треков, индекс 3 -> остался 1 трек (индекс 4) -> true
        assert!(engine.refill_needed(5, 3));
        // 5 треков, индекс 4 -> осталось 0 треков -> true
        assert!(engine.refill_needed(5, 4));
        // 5 треков, индекс 1 -> осталось 3 трека (индексы 2, 3, 4) -> false
        assert!(!engine.refill_needed(5, 1));
        // 10 треков, индекс 0 -> осталось 9 треков -> false
        assert!(!engine.refill_needed(10, 0));
    }

    #[test]
    fn test_seed_drift_counts_only_real_tracks() {
        let mut engine = WaveEngine::new();
        assert!(!engine.seed_drift_due());

        // Пустые идентификаторы — это не сыгранные треки, счётчик не двигают
        engine.record_played("");
        engine.record_played("   ");
        assert_eq!(engine.played_since_seed, 0);
        assert!(!engine.seed_drift_due());

        for i in 0..SEED_DRIFT_AFTER_TRACKS - 1 {
            engine.record_played(&format!("t{}", i));
        }
        assert!(!engine.seed_drift_due(), "до порога переезжать рано");

        engine.record_played("last_one");
        assert!(engine.seed_drift_due(), "на пороге пора менять сид");
    }

    #[test]
    fn test_mark_seed_started_resets_drift() {
        let mut engine = WaveEngine::new();
        for i in 0..SEED_DRIFT_AFTER_TRACKS + 5 {
            engine.record_played(&format!("t{}", i));
        }
        assert!(engine.seed_drift_due());

        engine.mark_seed_started();
        assert!(!engine.seed_drift_due());
        assert_eq!(engine.played_since_seed, 0);
        // История сыгранного при смене сида не сбрасывается — иначе вернутся те же треки
        assert_eq!(engine.recent_history.len(), SEED_DRIFT_AFTER_TRACKS + 5);
    }

    #[test]
    fn test_remember_dropped_does_not_count_as_played() {
        let mut engine = WaveEngine::new();
        engine.remember_dropped(&["dropped_a", "dropped_b", "dropped_a", "  "]);

        // Выброшенный хвост уходит в историю (без дублей) ...
        assert_eq!(
            engine.recent_history.iter().cloned().collect::<Vec<_>>(),
            vec!["dropped_b".to_string(), "dropped_a".to_string()]
        );
        // ... но сыгранными эти треки не считаются
        assert_eq!(engine.played_since_seed, 0);
        assert!(!engine.seed_drift_due());
    }
}
