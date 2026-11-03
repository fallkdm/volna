use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use super::provider::{MusicProvider, ProviderId};
use super::ytmusic::YtMusicClient;

#[derive(Debug, Clone, Copy)]
struct ProviderState {
    active_id: ProviderId,
    generation: u64,
}

/// Снимок активного провайдера для асинхронного запроса.
///
/// Ответ можно применять, только пока `is_current(id, generation)` возвращает true.
pub struct ProviderSession {
    pub id: ProviderId,
    pub generation: u64,
    pub provider: Arc<dyn MusicProvider>,
}

/// Менеджер активного музыкального провайдера
#[derive(Clone)]
pub struct ProviderManager {
    state: Arc<RwLock<ProviderState>>,
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
            state: Arc::new(RwLock::new(ProviderState {
                active_id: def_id,
                generation: 0,
            })),
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
            state: Arc::new(RwLock::new(ProviderState {
                active_id: def_id,
                generation: 0,
            })),
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

    /// Удаляет провайдер после выхода; активного провайдера удалять нельзя.
    pub fn unregister(&self, id: ProviderId) -> bool {
        if self.active_id() == id {
            return false;
        }
        self.providers
            .write()
            .map(|mut providers| providers.remove(&id).is_some())
            .unwrap_or(false)
    }

    /// Возвращает ID текущего активного провайдера
    pub fn active_id(&self) -> ProviderId {
        self.state
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .active_id
    }

    /// Атомарно фиксирует провайдера и поколение перед асинхронной операцией.
    pub fn session(&self) -> ProviderSession {
        let state = *self.state.read().unwrap_or_else(|error| error.into_inner());
        let provider = self
            .providers
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .get(&state.active_id)
            .cloned()
            .expect("active provider must be registered");
        ProviderSession {
            id: state.active_id,
            generation: state.generation,
            provider,
        }
    }

    /// Отклоняет ответы прошлых переключений, включая последовательность VK → Yandex → VK.
    pub fn is_current(&self, id: ProviderId, generation: u64) -> bool {
        let state = self.state.read().unwrap_or_else(|error| error.into_inner());
        state.active_id == id && state.generation == generation
    }

    /// Возвращает текущего активного провайдера
    pub fn current(&self) -> Arc<dyn MusicProvider> {
        self.session().provider
    }

    /// Переключает активного провайдера на указанный ID, возвращает true при успехе
    pub fn switch_provider(&self, id: ProviderId) -> bool {
        let has_provider = self
            .providers
            .read()
            .map(|providers| providers.contains_key(&id))
            .unwrap_or(false);

        if has_provider {
            let mut state = self
                .state
                .write()
                .unwrap_or_else(|error| error.into_inner());
            state.active_id = id;
            state.generation = state.generation.wrapping_add(1);
            return true;
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
    use crate::api::vk::provider::VkProvider;

    #[test]
    fn test_provider_manager_auto_registers_ytmusic() {
        let ya = Arc::new(YandexMusicClient::new(None)) as Arc<dyn MusicProvider>;
        let pm = ProviderManager::new(ya);

        assert_eq!(pm.active_id(), ProviderId::Yandex);
        assert_eq!(pm.current().display_name(), "Яндекс Музыка");

        assert!(pm.get(ProviderId::YouTubeMusic).is_some());
        assert_eq!(
            pm.get(ProviderId::YouTubeMusic).unwrap().display_name(),
            "YouTube Music"
        );

        assert!(pm.switch_provider(ProviderId::YouTubeMusic));
        assert_eq!(pm.active_id(), ProviderId::YouTubeMusic);
        assert_eq!(pm.current().display_name(), "YouTube Music");
    }

    #[test]
    fn stale_sessions_are_rejected_even_after_returning_to_the_same_provider() {
        let vk =
            Arc::new(VkProvider::new("contract-test-token", 1).unwrap()) as Arc<dyn MusicProvider>;
        let yandex = Arc::new(YandexMusicClient::new(None)) as Arc<dyn MusicProvider>;
        let pm = ProviderManager::with_yandex_and_ytmusic(yandex, Arc::new(YtMusicClient::new()));
        pm.register(vk);
        assert!(pm.switch_provider(ProviderId::Vk));

        let original = pm.session();
        assert!(pm.is_current(original.id, original.generation));
        assert!(pm.switch_provider(ProviderId::Yandex));
        assert!(!pm.is_current(original.id, original.generation));
        assert!(pm.switch_provider(ProviderId::Vk));
        assert_eq!(pm.active_id(), ProviderId::Vk);
        assert!(!pm.is_current(original.id, original.generation));

        let latest = pm.session();
        assert!(pm.is_current(latest.id, latest.generation));
        assert_ne!(original.generation, latest.generation);
    }
}
