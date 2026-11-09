use volna::api;
use volna::api::models::TrackSource;
use volna::player::{lookup_registered_artist_in_map, parse_artist_id, PlaySource, SourceTarget};
use volna::ui::collection::{format_listeners_label, group_digits, ListenersMetric};
use std::collections::HashMap;

#[test]
fn test_artist_registry_isolation() {
    let mut registry: HashMap<(TrackSource, String), String> = HashMap::new();
    let id_ya: String = "123".into();
    let id_vk: String = "123".into();

    registry.insert((TrackSource::Yandex, id_ya.clone()), "ArtistA".into());
    registry.insert((TrackSource::Vk, id_vk.clone()), "ArtistB".into());

    assert_eq!(
        registry.get(&(TrackSource::Yandex, id_ya)).map(|s| s.as_str()),
        Some("ArtistA")
    );
    assert_eq!(
        registry.get(&(TrackSource::Vk, id_vk)).map(|s| s.as_str()),
        Some("ArtistB")
    );
}

#[test]
fn test_parse_artist_id_prefixes() {
    // 1. vk:name:... с URL-encoded именем
    let (id, source) = parse_artist_id(
        "vk:name:%D0%A2%D1%80%D0%B8%20%D0%B4%D0%BD%D1%8F%20%D0%B4%D0%BE%D0%B6%D0%B4%D1%8F",
        None,
    )
    .expect("vk:name: parse error");
    assert_eq!(source, TrackSource::Vk);
    assert_eq!(id, "Три дня дождя");

    // 2. ya:123
    let (id, source) = parse_artist_id("ya:123", None).expect("ya: parse error");
    assert_eq!(source, TrackSource::Yandex);
    assert_eq!(id, "123");

    // 3. yt:UC...
    let (id, source) = parse_artist_id("yt:UC1234567890", None).expect("yt: parse error");
    assert_eq!(source, TrackSource::YouTubeMusic);
    assert_eq!(id, "UC1234567890");

    // 4. Чистый UC...
    let (id, source) = parse_artist_id("UCabcdef12345", None).expect("pure UC parse error");
    assert_eq!(source, TrackSource::YouTubeMusic);
    assert_eq!(id, "UCabcdef12345");

    // 5. Чистый 123 без fallback -> ошибка
    let err = parse_artist_id("123", None);
    assert!(err.is_err());
    assert_eq!(err.unwrap_err(), "Источник артиста не определён");

    // 6. Чистый 123 с fallback_source Yandex
    let (id, source) = parse_artist_id("123", Some(TrackSource::Yandex)).expect("fallback numeric parse error");
    assert_eq!(source, TrackSource::Yandex);
    assert_eq!(id, "123");

    // 7. Отказ для unknown без fallback
    let err = parse_artist_id("unknown", None);
    assert!(err.is_err());
    assert_eq!(err.unwrap_err(), "Источник артиста не определён");

    // 8. С fallback_source
    let (id, source) = parse_artist_id("unknown", Some(TrackSource::Vk)).expect("fallback parse error");
    assert_eq!(source, TrackSource::Vk);
    assert_eq!(id, "unknown");
}

#[test]
fn test_isolation_five_cases() {
    // 1. Yandex 123 + VK 123 не затирают друг друга в реестре
    let mut registry: HashMap<(TrackSource, String), String> = HashMap::new();
    registry.insert((TrackSource::Yandex, "123".into()), "Яндекс Артист".into());
    registry.insert((TrackSource::Vk, "123".into()), "ВК Артист".into());
    assert_eq!(
        registry.get(&(TrackSource::Yandex, "123".into())).map(|s| s.as_str()),
        Some("Яндекс Артист")
    );
    assert_eq!(
        registry.get(&(TrackSource::Vk, "123".into())).map(|s| s.as_str()),
        Some("ВК Артист")
    );

    // 2. Пустой VK id с именем vk:name:<encoded> парсится в декодированное имя и TrackSource::Vk
    let encoded_name = glib::Uri::escape_string("Три дня дождя", None, true);
    let raw_vk_id = format!("vk:name:{}", encoded_name);
    let (id, source) = parse_artist_id(&raw_vk_id, None).expect("case 2: vk:name parse error");
    assert_eq!(source, TrackSource::Vk);
    assert_eq!(id, "Три дня дождя");

    // 3. ya:123 парсится в чистый 123 и TrackSource::Yandex
    let (id, source) = parse_artist_id("ya:123", None).expect("case 3: ya:123 parse error");
    assert_eq!(source, TrackSource::Yandex);
    assert_eq!(id, "123");

    // 4. yt:UC... парсится в чистый UC... и TrackSource::YouTubeMusic
    let (id, source) = parse_artist_id("yt:UCabcdef12345", None).expect("case 4: yt:UC parse error");
    assert_eq!(source, TrackSource::YouTubeMusic);
    assert_eq!(id, "UCabcdef12345");

    // 5. Неизвестный numeric 123 без префикса и без fallback -> Err("Источник артиста не определён")
    let err = parse_artist_id("123", None);
    assert!(err.is_err());
    assert_eq!(err.unwrap_err(), "Источник артиста не определён");
}

#[test]
fn test_play_source_serde_backward_compat() {
    let old_json = r#"{"label": "Тест", "target": {"Artist": "123"}}"#;
    let parsed: PlaySource = serde_json::from_str(old_json).expect("десериализация старого формата");
    assert_eq!(parsed.label, "Тест");
    match parsed.target {
        Some(SourceTarget::Artist(id)) => assert_eq!(id, "123"),
        _ => panic!("Ожидался SourceTarget::Artist"),
    }
    assert_eq!(parsed.source, None);
    assert_eq!(parsed.artist_name, None);
}

#[test]
fn test_lookup_registered_artist_by_id() {
    let mut reg: HashMap<(TrackSource, String), String> = HashMap::new();

    // 1. Регистрация (TrackSource::Yandex, "123") -> lookup находит (Yandex, name)
    reg.insert((TrackSource::Yandex, "123".into()), "Radiohead".into());
    assert_eq!(
        lookup_registered_artist_in_map(&reg, "123"),
        Some((TrackSource::Yandex, "Radiohead".into()))
    );

    // 2. При коллизии (есть и Yandex "123", и VK "123") -> lookup возвращает None (отказ от угадывания)
    reg.insert((TrackSource::Vk, "123".into()), "Три дня дождя".into());
    assert_eq!(lookup_registered_artist_in_map(&reg, "123"), None);

    // 3. Для неизвестного ID -> возвращает None
    assert_eq!(lookup_registered_artist_in_map(&reg, "456"), None);
    assert_eq!(lookup_registered_artist_in_map(&reg, ""), None);
}

#[test]
fn test_vk_artist_brief_alien_album_filtered_out() {
    let items = vec![
        api::vk::models::VkAudioDto {
            id: 1,
            owner_id: 100,
            artist: "Plenka".into(),
            title: "Closed".into(),
            subtitle: None,
            duration: 180,
            url: Some("https://example.com/audio1.mp3".into()),
            audio_streams: None,
            lyrics_id: None,
            album: Some(api::vk::models::VkAlbumDto {
                id: 1,
                owner_id: Some(100),
                title: "Ptah".into(),
                thumb: None,
            }),
            main_artists: Some(vec![api::vk::models::VkArtistDto {
                id: None,
                name: "Plenka".into(),
            }]),
            is_explicit: None,
        },
        api::vk::models::VkAudioDto {
            id: 2,
            owner_id: 100,
            artist: "Plenka, Asenssia".into(),
            title: "Nightmare".into(),
            subtitle: None,
            duration: 200,
            url: Some("https://example.com/audio2.mp3".into()),
            audio_streams: None,
            lyrics_id: None,
            album: Some(api::vk::models::VkAlbumDto {
                id: 2,
                owner_id: Some(100),
                title: "Plenka & Asenssia".into(),
                thumb: None,
            }),
            main_artists: None,
            is_explicit: None,
        },
        api::vk::models::VkAudioDto {
            id: 3,
            owner_id: 200,
            artist: "Plenka 1987".into(),
            title: "Alien Track".into(),
            subtitle: None,
            duration: 220,
            url: Some("https://example.com/audio3.mp3".into()),
            audio_streams: None,
            lyrics_id: None,
            album: Some(api::vk::models::VkAlbumDto {
                id: 29202913,
                owner_id: Some(200),
                title: "Alien 1987 Album".into(),
                thumb: None,
            }),
            main_artists: None,
            is_explicit: None,
        },
        api::vk::models::VkAudioDto {
            id: 4,
            owner_id: 300,
            artist: "Muse".into(),
            title: "Plug in Baby".into(),
            subtitle: None,
            duration: 220,
            url: Some("https://example.com/audio4.mp3".into()),
            audio_streams: None,
            lyrics_id: None,
            album: Some(api::vk::models::VkAlbumDto {
                id: 4,
                owner_id: Some(300),
                title: "Origin of Symmetry".into(),
                thumb: None,
            }),
            main_artists: Some(vec![api::vk::models::VkArtistDto {
                id: None,
                name: "Muse".into(),
            }]),
            is_explicit: None,
        },
    ];

    let (tracks, albums) = api::vk::provider::filter_artist_items("Plenka", 100, &items);

    assert_eq!(tracks.len(), 2);
    assert_eq!(tracks[0].title, "Closed");
    assert_eq!(tracks[1].title, "Nightmare");

    assert_eq!(albums.len(), 2);
    assert!(albums.iter().any(|a| a.id == "vk:100_1"));
    assert!(albums.iter().any(|a| a.id == "vk:100_2"));
    assert!(!albums.iter().any(|a| a.id.contains("29202913")));
}

#[test]
fn test_matches_artist_name() {
    use api::vk::provider::matches_artist_name;

    assert!(matches_artist_name("Plenka", "Plenka"));
    assert!(matches_artist_name("Plenka, Asenssia", "Plenka"));
    assert!(!matches_artist_name("Plenka 1987", "Plenka"));
    assert!(!matches_artist_name("Radiohead Tribute", "Radiohead"));
    assert!(matches_artist_name("Thom Yorke feat. Plenka", "Plenka"));
}

#[test]
fn test_listeners_metric_formatting() {
    let monthly_text = format_listeners_label(5_510_287, ListenersMetric::Monthly);
    assert_eq!(monthly_text, "5\u{a0}510\u{a0}287 слушателей в месяц");
    assert!(monthly_text.contains("слушателей в месяц"));

    let lastfm_text = format_listeners_label(1_234_567, ListenersMetric::LastFm);
    assert_eq!(lastfm_text, "1\u{a0}234\u{a0}567 слушателей на Last.fm");
    assert!(lastfm_text.contains("слушателей на Last.fm"));

    assert_eq!(
        ListenersMetric::Monthly.format_label(999),
        "999 слушателей в месяц"
    );
    assert_eq!(
        ListenersMetric::LastFm.format_label(999),
        "999 слушателей на Last.fm"
    );
    assert_eq!(group_digits(10_000), "10\u{a0}000");
}

#[test]
fn test_vk_history_roundtrip() {
    use volna::api::history::{clear_history_for, load_history_for, push_history_for, HistoryItem};
    use volna::api::models::Artist;
    use volna::api::provider::ProviderId;

    clear_history_for(ProviderId::Vk);
    let artist_item = HistoryItem::Artist(Artist {
        id: "vk:123".into(),
        name: "VK Artist".into(),
        cover: None,
    });
    push_history_for(ProviderId::Vk, artist_item);

    let loaded = load_history_for(ProviderId::Vk);
    assert!(!loaded.is_empty(), "История VK должна содержать элементы");
    let found = loaded.iter().any(|item| match item {
        HistoryItem::Artist(a) => a.id == "vk:123" && a.name == "VK Artist",
        _ => false,
    });
    assert!(
        found,
        "Сохраненный артист с id vk:123 должен возвращаться load_history_for(ProviderId::Vk)"
    );

    clear_history_for(ProviderId::Vk);
    let after_clear = load_history_for(ProviderId::Vk);
    assert!(after_clear.is_empty(), "После clear_history_for история VK должна быть пуста");
}

