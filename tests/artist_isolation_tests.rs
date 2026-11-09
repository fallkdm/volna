use volna::api;
use volna::api::models::TrackSource;
use volna::player::{lookup_registered_artist_in_map, parse_artist_id, PlaySource, SourceTarget};
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
            artist: "Radiohead".into(),
            title: "Karma Police".into(),
            subtitle: None,
            duration: 260,
            url: Some("https://example.com/audio1.mp3".into()),
            audio_streams: None,
            lyrics_id: None,
            album: Some(api::vk::models::VkAlbumDto {
                id: 1,
                owner_id: Some(100),
                title: "OK Computer".into(),
                thumb: None,
            }),
            main_artists: Some(vec![api::vk::models::VkArtistDto {
                id: None,
                name: "Radiohead".into(),
            }]),
            is_explicit: None,
        },
        api::vk::models::VkAudioDto {
            id: 2,
            owner_id: 200,
            artist: "Muse".into(),
            title: "Plug in Baby".into(),
            subtitle: None,
            duration: 220,
            url: Some("https://example.com/audio2.mp3".into()),
            audio_streams: None,
            lyrics_id: None,
            album: Some(api::vk::models::VkAlbumDto {
                id: 2,
                owner_id: Some(200),
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

    let (tracks, albums) = api::vk::provider::filter_artist_items("Radiohead", 100, &items);

    assert_eq!(albums.len(), 1);
    assert_eq!(albums[0].title.as_deref(), Some("OK Computer"));
    assert_eq!(albums[0].id, "vk:100_1");
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].title, "Karma Police");
}

