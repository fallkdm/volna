//! История поиска: результаты, которые пользователь открыл (трек, исполнитель, альбом, плейлист).

use serde::{Deserialize, Serialize};

use super::auth::{load_config, save_config};
use super::models::{Album, Artist, Playlist, Track};
use super::provider::ProviderId;

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

    /// Определяет сервис, к которому относится элемент
    pub fn provider(&self) -> ProviderId {
        match self {
            Self::Track(t) => {
                if t.source == crate::api::models::TrackSource::YouTubeMusic || t.id.len() == 11 {
                    ProviderId::YouTubeMusic
                } else {
                    ProviderId::Yandex
                }
            }
            Self::Artist(a) => {
                if a.id.starts_with("UC") {
                    ProviderId::YouTubeMusic
                } else {
                    ProviderId::Yandex
                }
            }
            Self::Album(a) => {
                if a.id.starts_with("MPRE") {
                    ProviderId::YouTubeMusic
                } else {
                    ProviderId::Yandex
                }
            }
            Self::Playlist(p) => {
                if p.kind.starts_with("VL") || p.kind.starts_with("PL") || p.uid.starts_with("VL") {
                    ProviderId::YouTubeMusic
                } else {
                    ProviderId::Yandex
                }
            }
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

/// Сохранённая история поиска для конкретного сервиса (Яндекс Музыка или YouTube Music)
pub fn load_history_for(provider: ProviderId) -> Vec<HistoryItem> {
    let mut config = load_config().unwrap_or_default();
    match provider {
        ProviderId::YouTubeMusic => {
            // Если в ytmusic_history_items пока пусто, мигрируем элементы YouTube Music и удаляем их из history_items
            if config.ytmusic_history_items.is_empty() {
                let mut migrated = Vec::new();
                config.history_items.retain(|v| {
                    if let Ok(item) = serde_json::from_value::<HistoryItem>(v.clone()) {
                        if item.provider() == ProviderId::YouTubeMusic {
                            migrated.push(v.clone());
                            return false;
                        }
                    }
                    true
                });
                if !migrated.is_empty() {
                    config.ytmusic_history_items = migrated;
                    let _ = save_config(&config);
                }
            }
            let mut list: Vec<HistoryItem> = config
                .ytmusic_history_items
                .into_iter()
                .filter_map(|v| serde_json::from_value(v).ok())
                .collect();
            list.truncate(LIMIT);
            list
        }
        ProviderId::Yandex => {
            config
                .history_items
                .into_iter()
                .filter_map(|v| serde_json::from_value::<HistoryItem>(v).ok())
                .filter(|it| it.provider() == ProviderId::Yandex)
                .take(LIMIT)
                .collect()
        }
        _ => Vec::new(),
    }
}

pub fn push_history_for(provider: ProviderId, item: HistoryItem) {
    let mut config = load_config().unwrap_or_default();
    match provider {
        ProviderId::YouTubeMusic => {
            let list: Vec<HistoryItem> = config
                .ytmusic_history_items
                .drain(..)
                .filter_map(|v| serde_json::from_value(v).ok())
                .collect();
            config.ytmusic_history_items = with_item(list, item)
                .iter()
                .filter_map(|i| serde_json::to_value(i).ok())
                .collect();
        }
        ProviderId::Yandex => {
            let list: Vec<HistoryItem> = config
                .history_items
                .drain(..)
                .filter_map(|v| serde_json::from_value::<HistoryItem>(v).ok())
                .filter(|it| it.provider() == ProviderId::Yandex)
                .collect();
            config.history_items = with_item(list, item)
                .iter()
                .filter_map(|i| serde_json::to_value(i).ok())
                .collect();
        }
        _ => {}
    }
    let _ = save_config(&config);
}

pub fn clear_history_for(provider: ProviderId) {
    let mut config = load_config().unwrap_or_default();
    match provider {
        ProviderId::YouTubeMusic => {
            config.ytmusic_history_items.clear();
            config.history_items.retain(|v| {
                if let Ok(item) = serde_json::from_value::<HistoryItem>(v.clone()) {
                    item.provider() != ProviderId::YouTubeMusic
                } else {
                    true
                }
            });
        }
        ProviderId::Yandex => {
            config.history_items.retain(|v| {
                if let Ok(item) = serde_json::from_value::<HistoryItem>(v.clone()) {
                    item.provider() != ProviderId::Yandex
                } else {
                    false
                }
            });
        }
        _ => {}
    }
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

/// Сохранённая история, свежие первыми (для обратной совместимости)
pub fn load_history() -> Vec<HistoryItem> {
    load_history_for(ProviderId::Yandex)
}

pub fn push_history(item: HistoryItem) {
    let provider = item.provider();
    push_history_for(provider, item);
}

pub fn clear_history() {
    clear_history_for(ProviderId::Yandex);
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
