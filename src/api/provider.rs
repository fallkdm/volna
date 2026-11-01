use async_trait::async_trait;
use crate::api::client::YandexMusicClient;
use crate::api::models::{Album, Playlist, SearchResult, Track};

/// Идентификатор музыкального сервиса / провайдера
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderId {
    Yandex,
    YouTubeMusic,
    Vk,
}

/// Чип выдачи станции: подпись и вариант сида, который она открывает
///
/// У Яндекс.Волны это фильтры Ротора в ответе станции, у YouTube Music —
/// кнопки над очередью («Знакомое», «Редкие треки», «Металл»…). У YT такой чип
/// задаёт не фильтр поверх текущего микса, а другой его сид-вариант: подстановка
/// `playlistId` чипа в запрос `next` меняет саму выдачу.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StationChip {
    /// Подпись чипа как её отдаёт сервис
    pub label: String,
    /// Идентификатор варианта станции (`playlistId` вида `RDAT…` для YT Music)
    pub playlist_id: String,
}

/// Порция треков станции (радио / волны)
#[derive(Debug, Clone)]
pub struct StationBatch {
    pub batch_id: String,
    pub tracks: Vec<Track>,
    /// Чипы, которые сервис предлагает вместе с этой порцией (может быть пусто)
    pub chips: Vec<StationChip>,
}

/// События обратной связи воспроизведения для радиостанции
#[derive(Debug, Clone)]
pub enum FeedbackEvent {
    TrackStarted {
        track_id: String,
        station_id: String,
        batch_id: Option<String>,
    },
    TrackFinished {
        track_id: String,
        station_id: String,
        duration_secs: u32,
        batch_id: Option<String>,
    },
    Skip {
        track_id: String,
        station_id: String,
        position_secs: u32,
        batch_id: Option<String>,
    },
}

/// Унифицированный архитектурный интерфейс музыкального провайдера
#[async_trait]
pub trait MusicProvider: Send + Sync {
    /// Идентификатор провайдера
    fn provider_id(&self) -> ProviderId;

    /// Человекочитаемое название сервиса
    fn display_name(&self) -> &'static str;

    /// Авторизован ли пользователь в данном провайдере
    fn is_authenticated(&self) -> bool;

    /// Получение прямой ссылки на аудиопоток трека
    async fn get_stream_url(&self, track_id: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;

    /// Поиск треков, альбомов и плейлистов
    async fn search(&self, query: &str) -> Result<SearchResult, Box<dyn std::error::Error + Send + Sync>>;

    /// Получение списка треков из медиатеки («Любимые»)
    async fn get_liked_tracks(&self) -> Result<Vec<Track>, Box<dyn std::error::Error + Send + Sync>>;

    /// Установка / снятие лайка у трека
    async fn set_track_like(&self, track_id: &str, liked: bool) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    /// Установка дизлайка у трека
    async fn set_track_dislike(&self, track_id: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    /// Получение любимых альбомов
    async fn get_liked_albums(&self) -> Result<Vec<Album>, Box<dyn std::error::Error + Send + Sync>>;

    /// Получение плейлистов пользователя
    async fn get_playlists(&self) -> Result<Vec<Playlist>, Box<dyn std::error::Error + Send + Sync>>;

    /// Получение порции треков станции
    async fn get_station_tracks(&self, station_id: &str, batch_id: Option<&str>) -> Result<StationBatch, Box<dyn std::error::Error + Send + Sync>>;

    /// Отправка фидбека станции (старт, завершение, скип)
    async fn send_feedback(&self, event: FeedbackEvent) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    /// Получение текста песни трека (LRC / синхронизированного или обычного)
    async fn get_track_lyrics(&self, track: &Track) -> Option<String>;

    /// Официальное общее количество треков в медиатеке (если известно из метаданных сервиса)
    async fn get_liked_total_count(&self) -> Option<u32> {
        None
    }
}

#[async_trait]
impl MusicProvider for YandexMusicClient {
    fn provider_id(&self) -> ProviderId {
        ProviderId::Yandex
    }

    fn display_name(&self) -> &'static str {
        "Яндекс Музыка"
    }

    fn is_authenticated(&self) -> bool {
        self.has_token()
    }

    async fn get_stream_url(&self, track_id: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let (url, _) = self
            .get_track_direct_url(track_id)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
        Ok(url)
    }

    async fn search(&self, query: &str) -> Result<SearchResult, Box<dyn std::error::Error + Send + Sync>> {
        self.search(query)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }

    async fn get_liked_tracks(&self) -> Result<Vec<Track>, Box<dyn std::error::Error + Send + Sync>> {
        let uid = self
            .get_uid()
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
        self.get_liked_tracks(&uid)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }

    async fn set_track_like(&self, track_id: &str, liked: bool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let uid = self
            .get_uid()
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
        self.set_track_like(&uid, track_id, liked)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }

    async fn set_track_dislike(&self, track_id: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let uid = self
            .get_uid()
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
        self.dislike_track(&uid, track_id)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }

    async fn get_liked_albums(&self) -> Result<Vec<Album>, Box<dyn std::error::Error + Send + Sync>> {
        let uid = self
            .get_uid()
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
        self.get_liked_albums(&uid)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }

    async fn get_playlists(&self) -> Result<Vec<Playlist>, Box<dyn std::error::Error + Send + Sync>> {
        let uid = self
            .get_uid()
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
        self.get_user_playlists(&uid)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }

    async fn get_station_tracks(&self, station_id: &str, _batch_id: Option<&str>) -> Result<StationBatch, Box<dyn std::error::Error + Send + Sync>> {
        let res = self
            .get_station_tracks(station_id)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
        let tracks = res.sequence.into_iter().map(|item| item.track).collect();
        Ok(StationBatch {
            batch_id: res.batch_id.unwrap_or_default(),
            tracks,
            chips: Vec::new(),
        })
    }

    async fn send_feedback(&self, event: FeedbackEvent) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match event {
            FeedbackEvent::TrackStarted { track_id, station_id, batch_id } => {
                self.feedback_track_started(&station_id, &track_id, batch_id)
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
            }
            FeedbackEvent::TrackFinished { track_id, station_id, duration_secs, batch_id } => {
                self.feedback_track_finished(&station_id, &track_id, duration_secs as f64, batch_id)
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
            }
            FeedbackEvent::Skip { track_id, station_id, position_secs, batch_id } => {
                self.feedback_skip(&station_id, &track_id, position_secs as f64, batch_id)
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
            }
        }
    }

    async fn get_track_lyrics(&self, track: &Track) -> Option<String> {
        let artist = track.artists_string();
        let duration_secs = track.duration_ms.map(|ms| ms / 1000);
        self.get_track_lyrics(&track.id, &artist, &track.title, duration_secs)
            .await
            .ok()
            .flatten()
    }
}
