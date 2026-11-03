use std::collections::HashSet;

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
}

impl VkProvider {
    pub fn new(token: impl Into<String>, owner_id: i64) -> Result<Self, VkError> {
        Ok(Self {
            client: VkClient::new(token)?,
            owner_id,
        })
    }

    pub fn from_client(client: VkClient, owner_id: i64) -> Self {
        Self { client, owner_id }
    }

    pub fn client(&self) -> &VkClient {
        &self.client
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
        .filter(|audio| seen.insert(audio.track_id()))
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
        self.client.get_stream_url(track_id).await.map_err(boxed)
    }

    async fn search(
        &self,
        query: &str,
    ) -> Result<SearchResult, Box<dyn std::error::Error + Send + Sync>> {
        let response = self
            .client
            .search_audio(query, 0, 50)
            .await
            .map_err(boxed)?;
        Ok(SearchResult {
            tracks: Some(SearchSection {
                results: deduplicate_tracks(response.items),
            }),
            ..Default::default()
        })
    }

    async fn get_liked_tracks(
        &self,
    ) -> Result<Vec<Track>, Box<dyn std::error::Error + Send + Sync>> {
        let mut offset = 0_u32;
        let mut seen = HashSet::new();
        let mut tracks = Vec::new();

        loop {
            let page = self
                .client
                .get_liked_audio(offset, VK_PAGE_SIZE)
                .await
                .map_err(boxed)?;
            let page_len = page.items.len();
            let total = page.count;
            let mut new_items = 0_usize;

            for audio in page.items {
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
        let playlist = self
            .client
            .get_playlist_by_id(id.owner_id, id.audio_id)
            .await
            .map_err(boxed)?;
        Ok(deduplicate_tracks(playlist.audios))
    }

    async fn get_station_tracks(
        &self,
        station_id: &str,
        batch_id: Option<&str>,
    ) -> Result<StationBatch, Box<dyn std::error::Error + Send + Sync>> {
        let (mix_id, settings) = mix_request(station_id);
        let chips = if batch_id.is_none() {
            mix_chips(self.client.get_stream_mix_settings().await.map_err(boxed)?)
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
                .get_stream_mix_audios(mix_id, &settings, context.as_deref(), VK_MIX_BATCH_SIZE)
                .await
                .map_err(boxed)?;
            let next_context = response.context;
            for audio in response.items {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::vk::models::{
        VkLyricTimestampDto, VkStreamMixSettingsCategoryDto, VkStreamMixSettingsOptionDto,
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
}
