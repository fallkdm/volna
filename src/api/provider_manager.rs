use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use super::provider::{MusicProvider, ProviderId};
use super::ytmusic::YtMusicClient;

/// Менеджер активного музыкального провайдера
#[derive(Clone)]
pub struct ProviderManager {
    active_id: Arc<RwLock<ProviderId>>,
    providers: Arc<RwLock<HashMap<ProviderId, Arc<dyn MusicProvider>>>>,
}

impl ProviderManager {
    /// Создаёт новый менеджер с провайдером по умолчанию и автоматически регистрирует YouTube Music
    pub fn new(default_provider: Arc<dyn MusicProvider>) -> Self {
        let def_id = default_provider.provider_id();
        let mut map = HashMap::new();
        map.insert(def_id, default_provider);
        if def_id != ProviderId::YouTubeMusic {
            let yt = Arc::new(YtMusicClient::new()) as Arc<dyn MusicProvider>;
            map.insert(ProviderId::YouTubeMusic, yt);
        }
        Self {
            active_id: Arc::new(RwLock::new(def_id)),
            providers: Arc::new(RwLock::new(map)),
        }
    }

    /// Создаёт менеджер с явно переданными провайдерами Яндекс Музыки и YouTube Music
    pub fn with_yandex_and_ytmusic(
        yandex: Arc<dyn MusicProvider>,
        ytmusic: Arc<dyn MusicProvider>,
    ) -> Self {
        let def_id = yandex.provider_id();
        let mut map = HashMap::new();
        map.insert(def_id, yandex);
        map.insert(ytmusic.provider_id(), ytmusic);
        Self {
            active_id: Arc::new(RwLock::new(def_id)),
            providers: Arc::new(RwLock::new(map)),
        }
    }

    /// Регистрирует нового музыкального провайдера
    pub fn register(&self, provider: Arc<dyn MusicProvider>) {
        let id = provider.provider_id();
        if let Ok(mut map) = self.providers.write() {
            map.insert(id, provider);
        }
    }

    /// Возвращает ID текущего активного провайдера
    pub fn active_id(&self) -> ProviderId {
        *self.active_id.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Возвращает текущего активного провайдера
    pub fn current(&self) -> Arc<dyn MusicProvider> {
        let id = self.active_id();
        if let Ok(map) = self.providers.read() {
            if let Some(provider) = map.get(&id) {
                return Arc::clone(provider);
            }
            if let Some(fallback) = map.values().next() {
                return Arc::clone(fallback);
            }
        }
        panic!("ProviderManager has no registered providers");
    }

    /// Переключает активного провайдера на указанный ID, возвращает true при успехе
    pub fn switch_provider(&self, id: ProviderId) -> bool {
        let has_provider = self
            .providers
            .read()
            .map(|m| m.contains_key(&id))
            .unwrap_or(false);

        if has_provider {
            if let Ok(mut active) = self.active_id.write() {
                *active = id;
                return true;
            }
        }
        false
    }

    /// Получает провайдера по ID, если он зарегистрирован
    pub fn get(&self, id: ProviderId) -> Option<Arc<dyn MusicProvider>> {
        self.providers.read().ok()?.get(&id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::client::YandexMusicClient;

    #[test]
    fn test_provider_manager_auto_registers_ytmusic() {
        let ya = Arc::new(YandexMusicClient::new(None)) as Arc<dyn MusicProvider>;
        let pm = ProviderManager::new(ya);

        assert_eq!(pm.active_id(), ProviderId::Yandex);
        assert_eq!(pm.current().display_name(), "Яндекс Музыка");

        assert!(pm.get(ProviderId::YouTubeMusic).is_some());
        assert_eq!(pm.get(ProviderId::YouTubeMusic).unwrap().display_name(), "YouTube Music");

        assert!(pm.switch_provider(ProviderId::YouTubeMusic));
        assert_eq!(pm.active_id(), ProviderId::YouTubeMusic);
        assert_eq!(pm.current().display_name(), "YouTube Music");
    }
}
