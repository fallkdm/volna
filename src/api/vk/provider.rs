use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use async_trait::async_trait;

use crate::api::models::{Album, Playlist, SearchResult, SearchSection, Track};
use crate::api::provider::{FeedbackEvent, MusicProvider, ProviderId, StationBatch, StationChip};

use super::client::{VkAudioId, VkCaptchaHandler, VkClient, VkError};
use super::models::{VkAudioDto, VkLyricsDto, VkPlaylistDto, VkStreamMixSettingsDto};

const VK_PAGE_SIZE: u32 = 100;
const VK_MIX_BATCH_SIZE: u32 = 30;
pub const VK_MIX_STATION_ID: &str = "vk:mix";
const VK_MIX_OPTION_PREFIX: &str = "vk:mix:";

/// Адаптер VK X `audio.*` к общему интерфейсу музыкального провайдера «Волны».
pub struct VkProvider {
    client: VkClient,
    owner_id: i64,
    stream_urls: Arc<RwLock<HashMap<String, String>>>,
}

impl VkProvider {
    pub fn new(token: impl Into<String>, owner_id: i64) -> Result<Self, VkError> {
        Ok(Self {
            client: VkClient::new(token)?,
            owner_id,
            stream_urls: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub fn from_client(client: VkClient, owner_id: i64) -> Self {
        Self {
            client,
            owner_id,
            stream_urls: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn client(&self) -> &VkClient {
        &self.client
    }

    pub fn user_id(&self) -> i64 {
        self.owner_id
    }

    pub fn set_captcha_handler(&self, handler: VkCaptchaHandler) {
        self.client.set_captcha_handler(handler);
    }
}

fn boxed(error: VkError) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(error)
}

fn deduplicate_tracks(items: Vec<VkAudioDto>) -> Vec<Track> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|audio| audio.is_playable() && seen.insert(audio.track_id()))
        .map(VkAudioDto::into_track)
        .collect()
}

fn into_playlist(playlist: VkPlaylistDto) -> Playlist {
    let id = format!("vk:{}_{}", playlist.owner_id, playlist.id);
    Playlist {
        uid: id.clone(),
        kind: id,
        title: playlist.title,
        track_count: playlist.count.and_then(|count| u32::try_from(count).ok()),
        og_image: playlist
            .photo
            .and_then(|photo| photo.best_url().map(str::to_owned)),
    }
}

fn mix_chips(settings: VkStreamMixSettingsDto) -> Vec<StationChip> {
    settings
        .categories
        .into_iter()
        .flat_map(|category| {
            let category_title = category.title;
            category.options.into_iter().map(move |option| {
                let option_title = option.title.unwrap_or_else(|| option.option_id.clone());
                let label = match category_title.as_deref().map(str::trim) {
                    Some(category) if !category.is_empty() => {
                        format!("{category}: {option_title}")
                    }
                    _ => option_title,
                };
                StationChip {
                    label,
                    playlist_id: format!("{VK_MIX_OPTION_PREFIX}{}", option.option_id),
                }
            })
        })
        .collect()
}

fn mix_request(station_id: &str) -> (&str, Vec<String>) {
    if let Some(settings) = station_id.strip_prefix("vk:mix?settings=") {
        return (
            "vk",
            settings
                .split(',')
                .map(str::trim)
                .filter(|option| !option.is_empty())
                .map(str::to_owned)
                .collect(),
        );
    }
    match station_id.strip_prefix(VK_MIX_OPTION_PREFIX) {
        Some(option) if !option.is_empty() => ("vk", vec![option.to_owned()]),
        _ => ("vk", Vec::new()),
    }
}

/// Переводит миллисекундные timestamps VK в обычный LRC.
fn lyrics_to_string(lyrics: VkLyricsDto) -> Option<String> {
    if !lyrics.timestamps.is_empty() {
        let mut result = String::new();
        for timestamp in lyrics.timestamps {
            let total_seconds = timestamp.begin / 1000;
            let minutes = total_seconds / 60;
            let seconds = total_seconds % 60;
            let hundredths = (timestamp.begin % 1000) / 10;
            result.push_str(&format!(
                "[{minutes:02}:{seconds:02}.{hundredths:02}]{}\n",
                timestamp.line
            ));
        }
        return Some(result);
    }

    lyrics
        .text
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

#[async_trait]
impl MusicProvider for VkProvider {
    fn provider_id(&self) -> ProviderId {
        ProviderId::Vk
    }

    fn display_name(&self) -> &'static str {
        "VK Музыка"
    }

    fn is_authenticated(&self) -> bool {
        self.client.is_authenticated()
    }

    async fn get_stream_url(
        &self,
        track_id: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        if let Ok(cache) = self.stream_urls.read() {
            if let Some(url) = cache.get(track_id) {
                return Ok(url.clone());
            }
        }
        self.client.get_stream_url(track_id).await.map_err(boxed)
    }

    async fn search(
        &self,
        query: &str,
    ) -> Result<SearchResult, Box<dyn std::error::Error + Send + Sync>> {
        let query_clean = query.trim();
        if query_clean.is_empty() {
            return Ok(SearchResult::default());
        }

        match self.client.search_audio(query_clean, 0, 50).await {
            Ok(response) => {
                if let Ok(mut cache) = self.stream_urls.write() {
                    for audio in &response.items {
                        if !audio.is_playable() {
                            continue;
                        }
                        if let Some(url) = audio.stream_url() {
                            cache.insert(audio.track_id(), url.to_owned());
                        }
                    }
                }

                let tracks = deduplicate_tracks(response.items.clone());

                let mut artist_counts: HashMap<(Option<i64>, String), (usize, Option<String>)> =
                    HashMap::new();

                for audio in &response.items {
                    let audio_cover = audio
                        .album
                        .as_ref()
                        .and_then(|alb| alb.thumb.as_ref())
                        .and_then(|th| th.best_url())
                        .map(str::to_owned);

                    if let Some(ref main_artists) = audio.main_artists {
                        if !main_artists.is_empty() {
                            for a in main_artists {
                                let name = a.name.trim();
                                if !name.is_empty() {
                                    let vk_artist_id =
                                        a.id.as_deref().and_then(|id_str| id_str.parse::<i64>().ok());
                                    let entry = artist_counts
                                        .entry((vk_artist_id, name.to_string()))
                                        .or_insert((0, None));
                                    entry.0 += 1;
                                    if entry.1.is_none() && audio_cover.is_some() {
                                        entry.1 = audio_cover.clone();
                                    }
                                }
                            }
                        } else {
                            for part in audio.artist.split([',', '&', '/']) {
                                let name = part.trim();
                                if !name.is_empty() {
                                    let entry = artist_counts
                                        .entry((None, name.to_string()))
                                        .or_insert((0, None));
                                    entry.0 += 1;
                                    if entry.1.is_none() && audio_cover.is_some() {
                                        entry.1 = audio_cover.clone();
                                    }
                                }
                            }
                        }
                    } else {
                        for part in audio.artist.split([',', '&', '/']) {
                            let name = part.trim();
                            if !name.is_empty() {
                                let entry = artist_counts
                                    .entry((None, name.to_string()))
                                    .or_insert((0, None));
                                entry.0 += 1;
                                if entry.1.is_none() && audio_cover.is_some() {
                                    entry.1 = audio_cover.clone();
                                }
                            }
                        }
                    }
                }

                let query_lower = query_clean.to_lowercase();
                let mut artist_list: Vec<((Option<i64>, String), (usize, Option<String>))> =
                    artist_counts.into_iter().collect();

                artist_list.sort_by(|((_, name_a), (count_a, _)), ((_, name_b), (count_b, _))| {
                    let rank_a = if name_a.eq_ignore_ascii_case(query_clean)
                        || name_a.to_lowercase() == query_lower
                    {
                        0
                    } else if name_a.to_lowercase().contains(&query_lower) {
                        1
                    } else {
                        2
                    };
                    let rank_b = if name_b.eq_ignore_ascii_case(query_clean)
                        || name_b.to_lowercase() == query_lower
                    {
                        0
                    } else if name_b.to_lowercase().contains(&query_lower) {
                        1
                    } else {
                        2
                    };

                    rank_a
                        .cmp(&rank_b)
                        .then_with(|| count_b.cmp(count_a))
                        .then_with(|| name_a.cmp(name_b))
                });

                let artists: Vec<crate::api::models::Artist> = artist_list
                    .into_iter()
                    .take(10)
                    .map(|((vk_artist_id, name), (_count, cover))| {
                        let id = match vk_artist_id {
                            Some(id) => format!("vk:{}", id),
                            None => {
                                format!("vk:name:{}", glib::Uri::escape_string(&name, None, true))
                            }
                        };
                        let cover = cover.map(|uri| crate::api::models::ArtistCover { uri: Some(uri) });
                        crate::api::models::Artist { id, name, cover }
                    })
                    .collect();

                let mut albums_map: HashMap<(i64, i64), (Album, usize)> = HashMap::new();
                for audio in &response.items {
                    if let Some(alb) = &audio.album {
                        let key = (alb.owner_id.unwrap_or(self.owner_id), alb.id);
                        let entry = albums_map
                            .entry(key)
                            .or_insert_with(|| {
                                let mut album_obj = alb.clone().into_album();
                                let is_unnamed = album_obj
                                    .title
                                    .as_deref()
                                    .map_or(true, |t| t.trim().is_empty() || t.trim().eq_ignore_ascii_case("unnamed"));
                                if is_unnamed && !audio.title.trim().is_empty() {
                                    album_obj.title = Some(audio.title.trim().to_string());
                                }
                                (album_obj, 0)
                            });
                        let is_unnamed = entry
                            .0
                            .title
                            .as_deref()
                            .map_or(true, |t| t.trim().is_empty() || t.trim().eq_ignore_ascii_case("unnamed"));
                        if is_unnamed && !audio.title.trim().is_empty() {
                            entry.0.title = Some(audio.title.trim().to_string());
                        }
                        entry.1 += 1;
                    }
                }

                let mut albums_sorted: Vec<(Album, usize)> = albums_map.into_values().collect();
                albums_sorted.sort_by(|a, b| b.1.cmp(&a.1));
                let albums: Vec<Album> =
                    albums_sorted.into_iter().take(12).map(|(a, _)| a).collect();

                let mut playlists = Vec::new();
                if let Ok(pl_res) = self.client.search_playlists(query_clean, 10).await {
                    for pl in pl_res.items {
                        if pl.owner_id < 0 {
                            playlists.push(into_playlist(pl));
                        }
                    }
                }

                Ok(SearchResult {
                    tracks: if tracks.is_empty() {
                        None
                    } else {
                        Some(SearchSection { results: tracks })
                    },
                    artists: if artists.is_empty() {
                        None
                    } else {
                        Some(SearchSection { results: artists })
                    },
                    albums: if albums.is_empty() {
                        None
                    } else {
                        Some(SearchSection { results: albums })
                    },
                    playlists: if playlists.is_empty() {
                        None
                    } else {
                        Some(SearchSection { results: playlists })
                    },
                })
            }
            Err(VkError::Api { code: 3, .. }) | Err(VkError::Api { code: 15, .. }) => {
                // Если официальный поиск в каталоге VK недоступен для текущего токена:
                // 1) Ищем среди плейлистов пользователя
                let mut found_playlists = Vec::new();
                if let Ok(page) = self.client.get_playlists(self.owner_id, 0, 50).await {
                    let query_lower = query_clean.to_lowercase();
                    for p in page.items {
                        if p.title.to_lowercase().contains(&query_lower) {
                            found_playlists.push(into_playlist(p));
                        }
                    }
                }

                let mut seen_ids = HashSet::new();
                let mut relevant_tracks = Vec::new();
                let mut other_tracks = Vec::new();
                let query_lower = query_clean.to_lowercase();

                // 2) Ищем среди любимых треков пользователя
                if let Ok(liked) = self.get_liked_tracks().await {
                    for t in liked {
                        let matches_artist = t
                            .artists
                            .iter()
                            .any(|a| a.name.to_lowercase().contains(&query_lower));
                        let matches_title = t.title.to_lowercase().contains(&query_lower);
                        if matches_artist || matches_title {
                            if seen_ids.insert(t.id.clone()) {
                                relevant_tracks.push(t);
                            }
                        }
                    }
                }

                // 3) Глобальный поиск треков по ВКонтакте через newsfeed.search
                if let Ok(audios) = self.client.search_newsfeed_audios(query_clean, 100).await {
                    if let Ok(mut cache) = self.stream_urls.write() {
                        for a in &audios {
                            if !a.is_playable() {
                                continue;
                            }
                            if let Some(url) = a.stream_url() {
                                cache.insert(a.track_id(), url.to_owned());
                            }
                        }
                    }

                    for a in audios {
                        if !a.is_playable() {
                            continue;
                        }
                        let track_id = a.track_id();
                        if seen_ids.insert(track_id) {
                            let matches_artist = a.artist.to_lowercase().contains(&query_lower);
                            let matches_title = a.title.to_lowercase().contains(&query_lower);
                            let track = a.into_track();
                            if matches_artist || matches_title {
                                relevant_tracks.push(track);
                            } else {
                                other_tracks.push(track);
                            }
                        }
                    }
                }

                // Сначала релевантные по исполнителю/названию, затем остальные
                relevant_tracks.extend(other_tracks);

                Ok(SearchResult {
                    tracks: Some(SearchSection {
                        results: relevant_tracks,
                    }),
                    playlists: if !found_playlists.is_empty() {
                        Some(SearchSection {
                            results: found_playlists,
                        })
                    } else {
                        None
                    },
                    ..Default::default()
                })
            }
            Err(e) => Err(boxed(e)),
        }
    }

    async fn get_liked_tracks(
        &self,
    ) -> Result<Vec<Track>, Box<dyn std::error::Error + Send + Sync>> {
        let mut offset = 0_u32;
        let mut seen = HashSet::new();
        let mut tracks = Vec::new();

        loop {
            let page = match self.client.get_liked_audio(Some(self.owner_id), offset, VK_PAGE_SIZE).await {
                Ok(page) => page,
                Err(VkError::Api { code: 3, .. }) | Err(VkError::Api { code: 15, .. }) => {
                    eprintln!("[vk] audio.get недоступен для текущего токена (код 3/15). Для загрузки «Моих треков» необходим токен официального приложения.");
                    return Ok(tracks);
                }
                Err(e) => return Err(boxed(e)),
            };

            let page_len = page.items.len();
            let total = page.count;
            let mut new_items = 0_usize;

            if let Ok(mut cache) = self.stream_urls.write() {
                for audio in &page.items {
                    if !audio.is_playable() {
                        continue;
                    }
                    if let Some(url) = audio.stream_url() {
                        cache.insert(audio.track_id(), url.to_owned());
                    }
                }
            }

            for audio in page.items {
                if !audio.is_playable() {
                    continue;
                }
                if seen.insert(audio.track_id()) {
                    tracks.push(audio.into_track());
                    new_items += 1;
                }
            }

            offset = offset.saturating_add(page_len as u32);
            if page_len < VK_PAGE_SIZE as usize
                || offset as u64 >= total
                || (page_len > 0 && new_items == 0)
            {
                break;
            }
        }

        Ok(tracks)
    }

    async fn set_track_like(
        &self,
        track_id: &str,
        liked: bool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.client
            .set_track_like(track_id, liked)
            .await
            .map_err(boxed)
    }

    async fn set_track_dislike(
        &self,
        track_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.client
            .set_track_dislike(track_id, true)
            .await
            .map_err(boxed)
    }

    async fn get_liked_albums(
        &self,
    ) -> Result<Vec<Album>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Vec::new())
    }

    async fn get_playlists(
        &self,
    ) -> Result<Vec<Playlist>, Box<dyn std::error::Error + Send + Sync>> {
        if self.owner_id == 0 {
            return Err(boxed(VkError::InvalidInput(
                "Для загрузки плейлистов VK требуется повторный вход".to_owned(),
            )));
        }

        let mut offset = 0_u32;
        let mut playlists = Vec::new();
        loop {
            let page = self
                .client
                .get_playlists(self.owner_id, offset, VK_PAGE_SIZE)
                .await
                .map_err(boxed)?;
            let page_len = page.items.len();
            let total = page.count;
            playlists.extend(page.items.into_iter().map(into_playlist));
            offset = offset.saturating_add(page_len as u32);
            if page_len < VK_PAGE_SIZE as usize || offset as u64 >= total {
                break;
            }
        }
        Ok(playlists)
    }

    async fn get_playlist_tracks(
        &self,
        playlist: &Playlist,
    ) -> Result<Vec<Track>, Box<dyn std::error::Error + Send + Sync>> {
        let id = VkAudioId::parse(&playlist.uid).map_err(boxed)?;
        if id.audio_id == 7 || playlist.title == "Мне нравится" {
            let liked = self.get_liked_tracks().await?;
            if !liked.is_empty() {
                return Ok(liked);
            }
        }

        // 1. Пробуем получить треки напрямую через audio.get(owner_id, playlist_id)
        let mut audios = self
            .client
            .get_playlist_audios(id.owner_id, id.audio_id, None)
            .await
            .unwrap_or_default();

        // 2. Если пусто, пробуем запросить метаданные плейлиста для получения access_key (если плейлист закрытый/сообщества)
        if audios.is_empty() {
            if let Ok(pl_meta) = self.client.get_playlist_by_id(id.owner_id, id.audio_id).await {
                if !pl_meta.audios.is_empty() {
                    audios = pl_meta.audios;
                } else if let Some(ref key) = pl_meta.access_key {
                    if let Ok(items) = self.client.get_playlist_audios(id.owner_id, id.audio_id, Some(key)).await {
                        audios = items;
                    }
                }
            }
        }

        if !audios.is_empty() {
            if let Ok(mut cache) = self.stream_urls.write() {
                for audio in &audios {
                    if !audio.is_playable() {
                        continue;
                    }
                    if let Some(url) = audio.stream_url() {
                        cache.insert(audio.track_id(), url.to_owned());
                    }
                }
            }
            return Ok(deduplicate_tracks(audios));
        }

        if id.audio_id == 7 || playlist.title.contains("Мне нравится") {
            return self.get_liked_tracks().await;
        }
        Ok(Vec::new())
    }

    async fn get_album_tracks(
        &self,
        album_id: &str,
    ) -> Result<Vec<Track>, Box<dyn std::error::Error + Send + Sync>> {
        let clean_id = album_id.strip_prefix("vk:").unwrap_or(album_id);
        let clean_id = clean_id.strip_prefix("album:").unwrap_or(clean_id);
        let (owner_id, aid) = if let Some((o, a)) = clean_id.rsplit_once('_') {
            let o = o.parse::<i64>().map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            let a = a.parse::<i64>().map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            (o, a)
        } else if let Ok(a) = clean_id.parse::<i64>() {
            (self.owner_id, a)
        } else {
            return Err("Некорректный ID альбома VK".into());
        };

        let audios = self.client.get_album_audios(owner_id, aid).await.map_err(boxed)?;
        if let Ok(mut cache) = self.stream_urls.write() {
            for audio in &audios {
                if !audio.is_playable() {
                    continue;
                }
                if let Some(url) = audio.stream_url() {
                    cache.insert(audio.track_id(), url.to_owned());
                }
            }
        }
        Ok(deduplicate_tracks(audios))
    }

    async fn get_station_tracks(
        &self,
        station_id: &str,
        batch_id: Option<&str>,
    ) -> Result<StationBatch, Box<dyn std::error::Error + Send + Sync>> {
        let (mix_id, settings) = mix_request(station_id);
        let chips = if batch_id.is_none() {
            self.client
                .get_stream_mix_settings()
                .await
                .ok()
                .map(mix_chips)
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let mut context = batch_id.map(str::to_owned);
        let mut seen = HashSet::new();
        let mut tracks = Vec::new();

        // Пустой результат или порция из одних дублей не завершает VK Mix сразу:
        // делаем не более трёх дозапросов, каждый раз возвращая серверу context как есть.
        for _ in 0..=3 {
            let response = self
                .client
                .get_stream_mix_audios(
                    mix_id,
                    &settings,
                    context.as_deref(),
                    None,
                    VK_MIX_BATCH_SIZE,
                )
                .await
                .map_err(boxed)?;
            let next_context = response.context;
            if let Ok(mut cache) = self.stream_urls.write() {
                for audio in &response.items {
                    if !audio.is_playable() {
                        continue;
                    }
                    if let Some(url) = audio.stream_url() {
                        cache.insert(audio.track_id(), url.to_owned());
                    }
                }
            }
            for audio in response.items {
                if !audio.is_playable() {
                    continue;
                }
                if seen.insert(audio.track_id()) {
                    tracks.push(audio.into_track());
                }
            }
            if !tracks.is_empty() {
                return Ok(StationBatch {
                    batch_id: next_context.unwrap_or_default(),
                    tracks,
                    chips,
                });
            }
            if next_context.is_none() {
                break;
            }
            context = next_context;
        }

        Ok(StationBatch {
            batch_id: context.unwrap_or_default(),
            tracks,
            chips,
        })
    }

    async fn send_feedback(
        &self,
        _event: FeedbackEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // VK Mix не получает выдуманные события Rotor: поддерживаем только подтверждённые audio.*-вызовы.
        Ok(())
    }

    async fn get_track_lyrics(&self, track: &Track) -> Option<String> {
        if let Some(lyrics_id) = self
            .client
            .get_audio_by_id(&[track.id.clone()])
            .await
            .ok()
            .and_then(|items| {
                items
                    .into_iter()
                    .find(|audio| audio.track_id() == track.id)
                    .and_then(|audio| audio.lyrics_id)
            })
            && let Ok(lyrics) = self.client.get_lyrics(&track.id, lyrics_id).await
            && let Some(text) = lyrics_to_string(lyrics)
        {
            return Some(text);
        }

        let artist = track.artists_string();
        let duration = track.duration_ms.map(|duration| duration / 1000);
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .ok()?;
        crate::api::lrclib::get_cached_or_fetch_lrclib(
            &http,
            &track.id,
            &artist,
            &track.title,
            duration,
        )
        .await
    }

    async fn get_artist_brief(
        &self,
        artist: &crate::api::models::Artist,
    ) -> Result<crate::api::models::ArtistBrief, Box<dyn std::error::Error + Send + Sync>> {
        let artist_name = artist.name.trim();
        let tracks_res = self.client.search_audio(artist_name, 0, 40).await.ok();
        let raw_items = tracks_res.map(|r| r.items).unwrap_or_default();

        let (top_tracks, albums) = filter_artist_items(artist_name, self.owner_id, &raw_items);

        let mut enriched_albums = Vec::new();
        for mut album in albums {
            let is_unnamed = album
                .title
                .as_deref()
                .map_or(true, |t| t.trim().is_empty() || t.trim().eq_ignore_ascii_case("unnamed"));
            if is_unnamed {
                let clean_id = album.id.strip_prefix("vk:").unwrap_or(&album.id);
                if let Some((o_str, a_str)) = clean_id.split_once('_') {
                    if let (Ok(o), Ok(a)) = (o_str.parse::<i64>(), a_str.parse::<i64>()) {
                        if let Ok(pl) = self.client.get_playlist_by_id(o, a).await {
                            let pl_title = pl.title.trim();
                            if !pl_title.is_empty() && !pl_title.eq_ignore_ascii_case("unnamed") {
                                album.title = Some(pl_title.to_string());
                                if album.og_image.is_none() {
                                    album.og_image = pl.photo.and_then(|p| p.best_url().map(str::to_owned));
                                }
                            }
                        }
                    }
                }
            }
            enriched_albums.push(album);
        }

        let mut playlists = Vec::new();
        if let Ok(pl_res) = self.client.search_playlists(artist_name, 10).await {
            for pl in pl_res.items {
                if pl.owner_id < 0 && pl.title.trim().eq_ignore_ascii_case(artist_name) {
                    playlists.push(into_playlist(pl));
                }
            }
        }

        let real_id = if artist.id.starts_with("vk:") {
            artist.id.clone()
        } else if !artist_name.is_empty() {
            format!("vk:name:{}", glib::Uri::escape_string(artist_name, None, true))
        } else {
            artist.id.clone()
        };

        Ok(crate::api::models::ArtistBrief {
            artist: crate::api::models::Artist {
                id: real_id,
                name: artist_name.to_string(),
                cover: None,
            },
            popular_tracks: top_tracks,
            albums: enriched_albums,
            playlists,
            all_covers: Vec::new(),
            stats: None,
            background_video_url: None,
        })
    }
}

pub fn matches_artist_name(raw_artist: &str, target_artist: &str) -> bool {
    let target = target_artist.trim();
    if target.is_empty() {
        return false;
    }
    if raw_artist.trim().eq_ignore_ascii_case(target) {
        return true;
    }
    let target_lower = target.to_lowercase();

    // Разделение по коллаборациям: ',', '&', '/', "feat.", "ft."
    for part in raw_artist.split(|c| c == ',' || c == '&' || c == '/') {
        let trimmed = part.trim();
        if trimmed.eq_ignore_ascii_case(target) {
            return true;
        }

        let lower = trimmed.to_lowercase();
        let stripped = if let Some(rest) = lower.strip_prefix("feat. ") {
            Some(rest)
        } else if let Some(rest) = lower.strip_prefix("feat ") {
            Some(rest)
        } else if let Some(rest) = lower.strip_prefix("ft. ") {
            Some(rest)
        } else if let Some(rest) = lower.strip_prefix("ft ") {
            Some(rest)
        } else {
            None
        };
        if let Some(rest) = stripped {
            if rest.trim() == target_lower {
                return true;
            }
        }

        // Поддержка "Artist feat. Target" и "Artist ft. Target"
        for delimiter in &[" feat. ", " ft. ", " feat ", " ft "] {
            for sub in lower.split(delimiter) {
                if sub.trim() == target_lower {
                    return true;
                }
            }
        }
    }
    false
}

pub fn filter_artist_items(
    artist_name: &str,
    fallback_owner: i64,
    items: &[VkAudioDto],
) -> (Vec<Track>, Vec<Album>) {
    let mut top_tracks = Vec::new();
    let mut albums_map: std::collections::HashMap<(i64, i64), (Album, usize)> =
        std::collections::HashMap::new();

    for item in items {
        let matches = item.main_artists.as_ref().map_or(false, |artists| {
            artists.iter().any(|a| a.name.trim().eq_ignore_ascii_case(artist_name))
        }) || matches_artist_name(&item.artist, artist_name);
        if !matches {
            continue;
        }
        if let Some(alb) = &item.album {
            let key = (alb.owner_id.unwrap_or(fallback_owner), alb.id);
            let entry = albums_map
                .entry(key)
                .or_insert_with(|| {
                    let mut album_obj = alb.clone().into_album();
                    let is_unnamed = album_obj
                        .title
                        .as_deref()
                        .map_or(true, |t| t.trim().is_empty() || t.trim().eq_ignore_ascii_case("unnamed"));
                    if is_unnamed && !item.title.trim().is_empty() {
                        album_obj.title = Some(item.title.trim().to_string());
                    }
                    (album_obj, 0)
                });
            let is_unnamed = entry
                .0
                .title
                .as_deref()
                .map_or(true, |t| t.trim().is_empty() || t.trim().eq_ignore_ascii_case("unnamed"));
            if is_unnamed && !item.title.trim().is_empty() {
                entry.0.title = Some(item.title.trim().to_string());
            }
            entry.1 += 1;
        }
        if top_tracks.len() < 10 && item.is_playable() {
            top_tracks.push(item.clone().into_track());
        }
    }

    let mut albums_sorted: Vec<(Album, usize)> = albums_map.into_values().collect();
    albums_sorted.sort_by(|a, b| b.1.cmp(&a.1));
    let albums: Vec<Album> = albums_sorted.into_iter().take(12).map(|(a, _)| a).collect();

    (top_tracks, albums)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::vk::models::{
        VkLyricTimestampDto, VkNewsfeedSearchResponse, VkStreamMixSettingsCategoryDto,
        VkStreamMixSettingsOptionDto,
    };

    #[test]
    fn deduplication_uses_composite_vk_id() {
        let audio = |id, owner_id| VkAudioDto {
            id,
            owner_id,
            artist: "Artist".to_owned(),
            title: "Track".to_owned(),
            subtitle: None,
            duration: 180,
            url: Some("https://audio.example.invalid/track.mp3".to_owned()),
            audio_streams: None,
            lyrics_id: None,
            album: None,
            main_artists: None,
            is_explicit: None,
        };
        let tracks = deduplicate_tracks(vec![
            audio(10, 20),
            audio(10, 20),
            audio(10, 21),
            audio(11, 20),
        ]);
        let ids: Vec<_> = tracks.into_iter().map(|track| track.id).collect();
        assert_eq!(ids, ["vk:20_10", "vk:21_10", "vk:20_11"]);
    }

    #[test]
    fn vk_lyrics_keep_millisecond_precision_in_lrc() {
        let lyrics = VkLyricsDto {
            timestamps: vec![
                VkLyricTimestampDto {
                    begin: 65_432,
                    end: Some(67_000),
                    line: "First line".to_owned(),
                },
                VkLyricTimestampDto {
                    begin: 125_009,
                    end: None,
                    line: "Second line".to_owned(),
                },
            ],
            text: None,
        };
        assert_eq!(
            lyrics_to_string(lyrics).as_deref(),
            Some("[01:05.43]First line\n[02:05.00]Second line\n")
        );
    }

    #[test]
    fn mix_option_is_a_seed_and_context_is_not_parsed() {
        assert_eq!(mix_request(VK_MIX_STATION_ID), ("vk", Vec::new()));
        assert_eq!(
            mix_request("vk:mix:discovery"),
            ("vk", vec!["discovery".to_owned()])
        );
        assert_eq!(mix_request("vk:mix:"), ("vk", Vec::new()));
        assert_eq!(
            mix_request("vk:mix?settings=calm,instrumental,ru"),
            (
                "vk",
                vec![
                    "calm".to_owned(),
                    "instrumental".to_owned(),
                    "ru".to_owned()
                ]
            )
        );

        let chips = mix_chips(VkStreamMixSettingsDto {
            categories: vec![VkStreamMixSettingsCategoryDto {
                category_id: "mood".to_owned(),
                title: Some("Настроение".to_owned()),
                options: vec![VkStreamMixSettingsOptionDto {
                    option_id: "calm".to_owned(),
                    title: Some("Спокойное".to_owned()),
                }],
            }],
        });
        assert_eq!(chips[0].label, "Настроение: Спокойное");
        assert_eq!(chips[0].playlist_id, "vk:mix:calm");
    }

    #[test]
    fn newsfeed_search_extracts_audios() {
        let json = r#"{
            "items": [
                {
                    "attachments": [
                        {
                            "type": "audio",
                            "audio": {
                                "id": 100,
                                "owner_id": 200,
                                "artist": "Три дня дождя",
                                "title": "Прощание",
                                "duration": 236,
                                "url": "https://example.com/stream.m3u8"
                            }
                        }
                    ]
                }
            ]
        }"#;
        let resp: VkNewsfeedSearchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.items.len(), 1);
        assert_eq!(resp.items[0].attachments.len(), 1);
        let audio = resp.items[0].attachments[0].audio.as_ref().unwrap();
        assert_eq!(audio.artist, "Три дня дождя");
        assert_eq!(audio.track_id(), "vk:200_100");
    }
}
