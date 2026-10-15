//! История поиска: результаты, которые пользователь открыл (трек, исполнитель, альбом, плейлист).

use serde::{Deserialize, Serialize};

use super::auth::{load_config, save_config};
use super::models::{Album, Artist, Playlist, Track};

const LIMIT: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "item", rename_all = "lowercase")]
pub enum HistoryItem {
    Track(Track),
    Artist(Artist),
    Album(Album),
    Playlist(Playlist),
}

impl HistoryItem {
    /// Ключ для сравнения: тип и идентификатор
    fn key(&self) -> String {
        match self {
            Self::Track(t) => format!("track:{}", t.id),
            Self::Artist(a) => format!("artist:{}", a.id),
            Self::Album(a) => format!("album:{}", a.id),
            Self::Playlist(p) => format!("playlist:{}:{}", p.uid, p.kind),
        }
    }
}

/// Новый элемент в начало: повтор переезжает наверх, лишнее отбрасывается
fn with_item(mut list: Vec<HistoryItem>, item: HistoryItem) -> Vec<HistoryItem> {
    let key = item.key();
    list.retain(|i| i.key() != key);
    list.insert(0, item);
    list.truncate(LIMIT);
    list
}

/// Сохранённая история, свежие первыми; записи, которые не удалось разобрать, пропускаются
pub fn load_history() -> Vec<HistoryItem> {
    let values = load_config().map(|c| c.history_items).unwrap_or_default();
    values.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect()
}

pub fn push_history(item: HistoryItem) {
    let mut config = load_config().unwrap_or_default();
    let list: Vec<HistoryItem> = config.history_items.drain(..).filter_map(|v| serde_json::from_value(v).ok()).collect();
    config.history_items = with_item(list, item).iter().filter_map(|i| serde_json::to_value(i).ok()).collect();
    let _ = save_config(&config);
}

/// Запоминает последний проигранный трек
pub fn save_last_track(track: &Track) {
    let mut config = load_config().unwrap_or_default();
    config.last_track = serde_json::to_value(track).ok();
    let _ = save_config(&config);
}

/// Последний проигранный трек, если он сохранён и читается
pub fn load_last_track() -> Option<Track> {
    let value = load_config().ok()?.last_track?;
    serde_json::from_value(value).ok()
}

pub fn clear_history() {
    let mut config = load_config().unwrap_or_default();
    config.history_items.clear();
    let _ = save_config(&config);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artist(id: &str) -> HistoryItem {
        HistoryItem::Artist(Artist { id: id.into(), name: id.into(), cover: None })
    }

    #[test]
    fn repeat_moves_to_front_and_list_is_capped() {
        let list = with_item(vec![artist("a"), artist("b")], artist("b"));
        assert_eq!(list.iter().map(|i| i.key()).collect::<Vec<_>>(), ["artist:b", "artist:a"]);
        let many: Vec<_> = (0..30).map(|i| artist(&i.to_string())).collect();
        assert_eq!(with_item(many, artist("new")).len(), LIMIT);
    }

    #[test]
    fn item_survives_json_roundtrip() {
        let v = serde_json::to_value(artist("42")).unwrap();
        assert_eq!(serde_json::from_value::<HistoryItem>(v).unwrap().key(), "artist:42");
    }
}
