use super::*;

use serde_json::json;

#[test]
fn test_parse_duration_ms() {
    assert_eq!(parse_duration_ms("3:45"), Some(225_000));
    assert_eq!(parse_duration_ms("0:45"), Some(45_000));
    assert_eq!(parse_duration_ms("1:02:30"), Some(3_750_000));
    assert_eq!(parse_duration_ms("45"), Some(45_000));
    assert_eq!(parse_duration_ms("invalid"), None);
}

#[test]
fn test_extract_best_thumbnail() {
    let val = json!({
        "thumbnails": [
            { "url": "//example.com/small.jpg", "width": 60, "height": 60 },
            { "url": "https://example.com/large.jpg", "width": 540, "height": 540 },
            { "url": "https://example.com/med.jpg", "width": 240, "height": 240 }
        ]
    });
    assert_eq!(
        extract_best_thumbnail(&val),
        Some("https://example.com/large.jpg".to_string())
    );

    let proto_val = json!([
        { "url": "//example.com/test.jpg", "width": 100 }
    ]);
    assert_eq!(
        extract_best_thumbnail(&proto_val),
        Some("https://example.com/test.jpg".to_string())
    );

    // Вложенный путь musicResponsiveListItemRenderer -> thumbnail -> musicThumbnailRenderer -> thumbnail -> thumbnails
    let nested_responsive = json!({
        "thumbnail": {
            "musicThumbnailRenderer": {
                "thumbnail": {
                    "thumbnails": [
                        { "url": "https://lh3.googleusercontent.com/small.jpg", "width": 60, "height": 60 },
                        { "url": "https://lh3.googleusercontent.com/best.jpg", "width": 544, "height": 544 }
                    ]
                }
            }
        }
    });
    assert_eq!(
        extract_best_thumbnail(&nested_responsive),
        Some("https://lh3.googleusercontent.com/best.jpg".to_string())
    );

    // Прямой musicThumbnailRenderer
    let direct_mtr = json!({
        "thumbnail": {
            "thumbnails": [
                { "url": "https://yt3.googleusercontent.com/avatar.jpg", "width": 300, "height": 300 }
            ]
        }
    });
    assert_eq!(
        extract_best_thumbnail(&direct_mtr),
        Some("https://yt3.googleusercontent.com/avatar.jpg".to_string())
    );
}

#[test]
fn test_parse_artist_browse() {
    let json = json!({
        "header": {
            "musicImmersiveHeaderRenderer": {
                "title": { "runs": [{ "text": "Король и Шут" }] },
                "thumbnail": {
                    "musicThumbnailRenderer": {
                        "thumbnail": {
                            "thumbnails": [
                                { "url": "https://lh3.googleusercontent.com/artist.jpg", "width": 500 }
                            ]
                        }
                    }
                }
            }
        },
        "contents": {
            "singleColumnBrowseResultsRenderer": {
                "tabs": [
                    {
                        "tabRenderer": {
                            "content": {
                                "sectionListRenderer": {
                                    "contents": [
                                        {
                                            "musicShelfRenderer": {
                                                "title": { "runs": [{ "text": "Песни" }] },
                                                "contents": [
                                                    {
                                                        "musicResponsiveListItemRenderer": {
                                                            "playlistItemData": { "videoId": "kish_track_1" },
                                                            "flexColumns": [
                                                                {
                                                                    "musicResponsiveListItemFlexColumnRenderer": {
                                                                        "text": { "runs": [{ "text": "Лесник" }] }
                                                                    }
                                                                },
                                                                {
                                                                    "musicResponsiveListItemFlexColumnRenderer": {
                                                                        "text": {
                                                                            "runs": [
                                                                                { "text": "Король и Шут" },
                                                                                { "text": " • " },
                                                                                { "text": "3:12" }
                                                                            ]
                                                                        }
                                                                    }
                                                                }
                                                            ]
                                                        }
                                                    }
                                                ]
                                            }
                                        },
                                        {
                                            "musicCarouselShelfRenderer": {
                                                "header": {
                                                    "musicCarouselShelfBasicHeaderRenderer": {
                                                        "title": { "runs": [{ "text": "Альбомы" }] }
                                                    }
                                                },
                                                "contents": [
                                                    {
                                                        "musicTwoRowItemRenderer": {
                                                            "title": { "runs": [{ "text": "Камнем по голове" }] },
                                                            "subtitle": { "runs": [{ "text": "1996" }] },
                                                            "navigationEndpoint": {
                                                                "browseEndpoint": { "browseId": "MPREb_album1" }
                                                            },
                                                            "thumbnailRenderer": {
                                                                "musicThumbnailRenderer": {
                                                                    "thumbnail": {
                                                                        "thumbnails": [
                                                                            { "url": "https://lh3.googleusercontent.com/album.jpg", "width": 300 }
                                                                        ]
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                ]
                                            }
                                        }
                                    ]
                                }
                            }
                        }
                    }
                ]
            }
        }
    });

    let brief = parse_artist_browse(&json, "UC_kish").expect("Should parse artist brief");
    assert_eq!(brief.artist.id, "UC_kish");
    assert_eq!(brief.artist.name, "Король и Шут");
    assert_eq!(
        brief.artist.cover_url(400),
        Some("https://lh3.googleusercontent.com/artist.jpg".to_string())
    );
    assert_eq!(brief.popular_tracks.len(), 1);
    assert_eq!(brief.popular_tracks[0].id, "kish_track_1");
    assert_eq!(brief.popular_tracks[0].title, "Лесник");
    assert_eq!(brief.albums.len(), 1);
    assert_eq!(brief.albums[0].id, "MPREb_album1");
    assert_eq!(brief.albums[0].title.as_deref(), Some("Камнем по голове"));
    assert_eq!(brief.albums[0].year, Some(1996));
}

#[test]
fn test_parse_artist_browse_ignores_banner() {
    let banner_json = json!({
        "header": {
            "musicImmersiveHeaderRenderer": {
                "title": { "runs": [{ "text": "Deftones" }] },
                "thumbnail": {
                    "musicThumbnailRenderer": {
                        "thumbnail": {
                            "thumbnails": [
                                { "url": "https://lh3.googleusercontent.com/banner=w540-h225", "width": 540, "height": 225 },
                                { "url": "https://lh3.googleusercontent.com/banner=w1080-h450", "width": 1080, "height": 450 }
                            ]
                        }
                    }
                }
            }
        }
    });

    let brief =
        parse_artist_browse(&banner_json, "UC_deftones").expect("Should parse artist brief");
    assert_eq!(brief.artist.name, "Deftones");
    assert_eq!(
        brief.artist.cover_url(400),
        None,
        "Wide header banner should not be used as artist avatar"
    );
}

#[test]
fn test_parse_album_tracks() {
    let json = json!({
        "header": {
            "musicDetailHeaderRenderer": {
                "title": { "runs": [{ "text": "Акустический альбом" }] },
                "thumbnail": {
                    "croppedSquareThumbnailRenderer": {
                        "thumbnail": {
                            "thumbnails": [
                                { "url": "https://lh3.googleusercontent.com/album_acoustic.jpg", "width": 500 }
                            ]
                        }
                    }
                }
            }
        },
        "contents": {
            "singleColumnBrowseResultsRenderer": {
                "tabs": [
                    {
                        "tabRenderer": {
                            "content": {
                                "sectionListRenderer": {
                                    "contents": [
                                        {
                                            "musicShelfRenderer": {
                                                "contents": [
                                                    {
                                                        "musicResponsiveListItemRenderer": {
                                                            "playlistItemData": { "videoId": "track_jump" },
                                                            "flexColumns": [
                                                                {
                                                                    "musicResponsiveListItemFlexColumnRenderer": {
                                                                        "text": { "runs": [{ "text": "Прыгну со скалы" }] }
                                                                    }
                                                                },
                                                                {
                                                                    "musicResponsiveListItemFlexColumnRenderer": {
                                                                        "text": {
                                                                            "runs": [
                                                                                { "text": "Король и Шут" },
                                                                                { "text": " • " },
                                                                                { "text": "3:15" }
                                                                            ]
                                                                        }
                                                                    }
                                                                }
                                                            ]
                                                        }
                                                    }
                                                ]
                                            }
                                        }
                                    ]
                                }
                            }
                        }
                    }
                ]
            }
        }
    });

    let tracks = parse_album_tracks(&json);
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].id, "track_jump");
    assert_eq!(tracks[0].title, "Прыгну со скалы");
    assert_eq!(
        tracks[0].cover_uri,
        Some("https://lh3.googleusercontent.com/album_acoustic.jpg".to_string())
    );
}

#[test]
fn test_parse_stream_url_priority() {
    let json = json!({
        "playabilityStatus": {
            "status": "OK"
        },
        "streamingData": {
            "adaptiveFormats": [
                {
                    "itag": 140,
                    "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"",
                    "bitrate": 128000,
                    "url": "https://googlevideo.com/aac"
                },
                {
                    "itag": 251,
                    "mimeType": "audio/webm; codecs=\"opus\"",
                    "bitrate": 160000,
                    "url": "https://googlevideo.com/opus"
                }
            ]
        }
    });

    let url = parse_stream_url(&json).expect("Stream url should be found");
    assert_eq!(url, "https://googlevideo.com/opus");
}

#[test]
fn test_parse_stream_url_fallback_aac() {
    let json = json!({
        "playabilityStatus": {
            "status": "OK"
        },
        "streamingData": {
            "adaptiveFormats": [
                {
                    "itag": 140,
                    "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"",
                    "bitrate": 128000,
                    "url": "https://googlevideo.com/aac"
                }
            ]
        }
    });

    let url = parse_stream_url(&json).expect("Stream url should be found");
    assert_eq!(url, "https://googlevideo.com/aac");
}

#[test]
fn test_parse_radio_batch() {
    let json = json!({
        "contents": {
            "singleColumnMusicWatchNextResultsRenderer": {
                "tabbedRenderer": {
                    "watchNextTabbedResultsRenderer": {
                        "tabs": [
                            {
                                "tabRenderer": {
                                    "content": {
                                        "musicQueueRenderer": {
                                            "content": {
                                                "playlistPanelRenderer": {
                                                    "contents": [
                                                        {
                                                            "playlistPanelVideoRenderer": {
                                                                "videoId": "vid123",
                                                                "title": { "runs": [{ "text": "Radio Track 1" }] },
                                                                "lengthText": { "runs": [{ "text": "2:30" }] },
                                                                "longBylineText": {
                                                                    "runs": [
                                                                        {
                                                                            "text": "Artist Radio",
                                                                            "navigationEndpoint": {
                                                                                "browseEndpoint": { "browseId": "UC1234" }
                                                                            }
                                                                        }
                                                                    ]
                                                                },
                                                                "thumbnail": {
                                                                    "thumbnails": [{ "url": "https://img.com/cover.jpg", "width": 300 }]
                                                                }
                                                            }
                                                        }
                                                    ],
                                                    "continuations": [
                                                        {
                                                            "nextRadioContinuationData": {
                                                                "continuation": "cont_token_xyz"
                                                            }
                                                        }
                                                    ]
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        ]
                    }
                }
            }
        }
    });

    let (tracks, continuation) = parse_radio_batch(&json);
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].id, "vid123");
    assert_eq!(tracks[0].title, "Radio Track 1");
    assert_eq!(tracks[0].duration_ms, Some(150_000));
    assert_eq!(tracks[0].artists.len(), 1);
    assert_eq!(tracks[0].artists[0].name, "Artist Radio");
    assert_eq!(tracks[0].source, TrackSource::YouTubeMusic);
    assert_eq!(continuation, Some("cont_token_xyz".to_string()));
}

#[test]
fn test_parse_search_results() {
    let json = json!({
        "contents": {
            "tabbedSearchResultsRenderer": {
                "tabs": [
                    {
                        "tabRenderer": {
                            "content": {
                                "sectionListRenderer": {
                                    "contents": [
                                        {
                                            "musicShelfRenderer": {
                                                "title": { "runs": [{ "text": "Песни" }] },
                                                "contents": [
                                                    {
                                                        "musicResponsiveListItemRenderer": {
                                                            "playlistItemData": { "videoId": "search_vid_1" },
                                                            "flexColumns": [
                                                                {
                                                                    "musicResponsiveListItemFlexColumnRenderer": {
                                                                        "text": { "runs": [{ "text": "Found Song" }] }
                                                                    }
                                                                },
                                                                {
                                                                    "musicResponsiveListItemFlexColumnRenderer": {
                                                                        "text": {
                                                                            "runs": [
                                                                                { "text": "Песня" },
                                                                                { "text": " • " },
                                                                                {
                                                                                    "text": "Singer",
                                                                                    "navigationEndpoint": {
                                                                                        "browseEndpoint": { "browseId": "UCsinger" }
                                                                                    }
                                                                                },
                                                                                { "text": " • " },
                                                                                { "text": "3:10" }
                                                                            ]
                                                                        }
                                                                    }
                                                                }
                                                            ]
                                                        }
                                                    }
                                                ]
                                            }
                                        }
                                    ]
                                }
                            }
                        }
                    }
                ]
            }
        }
    });

    let res = parse_search_results(&json);
    assert!(res.tracks.is_some());
    let tr_list = res.tracks.unwrap().results;
    assert_eq!(tr_list.len(), 1);
    assert_eq!(tr_list[0].id, "search_vid_1");
    assert_eq!(tr_list[0].title, "Found Song");
    assert_eq!(tr_list[0].artists[0].name, "Singer");
    assert_eq!(tr_list[0].duration_ms, Some(190_000));
    assert_eq!(tr_list[0].source, TrackSource::YouTubeMusic);
}

#[test]
fn test_parse_liked_tracks() {
    let json = json!({
        "contents": {
            "singleColumnBrowseResultsRenderer": {
                "tabs": [
                    {
                        "tabRenderer": {
                            "content": {
                                "sectionListRenderer": {
                                    "contents": [
                                        {
                                            "musicPlaylistShelfRenderer": {
                                                "contents": [
                                                    {
                                                        "musicResponsiveListItemRenderer": {
                                                            "playlistItemData": { "videoId": "liked_track_1" },
                                                            "flexColumns": [
                                                                {
                                                                    "musicResponsiveListItemFlexColumnRenderer": {
                                                                        "text": { "runs": [{ "text": "Liked Song" }] }
                                                                    }
                                                                },
                                                                {
                                                                    "musicResponsiveListItemFlexColumnRenderer": {
                                                                        "text": {
                                                                            "runs": [
                                                                                {
                                                                                    "text": "Favorite Artist",
                                                                                    "navigationEndpoint": {
                                                                                        "browseEndpoint": { "browseId": "UCfav" }
                                                                                    }
                                                                                },
                                                                                { "text": " • " },
                                                                                { "text": "2:45" }
                                                                            ]
                                                                        }
                                                                    }
                                                                }
                                                            ]
                                                        }
                                                    }
                                                ]
                                            }
                                        }
                                    ]
                                }
                            }
                        }
                    }
                ]
            }
        }
    });

    let liked = parse_liked_tracks(&json);
    assert_eq!(liked.len(), 1);
    assert_eq!(liked[0].id, "liked_track_1");
    assert_eq!(liked[0].title, "Liked Song");
    assert_eq!(liked[0].artists[0].name, "Favorite Artist");
    assert_eq!(liked[0].duration_ms, Some(165_000));
    assert_eq!(liked[0].source, TrackSource::YouTubeMusic);
}

#[test]
fn test_parse_playlists() {
    let json = json!({
        "contents": {
            "sectionListRenderer": {
                "contents": [
                    {
                        "gridRenderer": {
                            "items": [
                                {
                                    "musicTwoRowItemRenderer": {
                                        "title": { "runs": [{ "text": "My Workout Mix" }] },
                                        "subtitle": { "runs": [{ "text": "Плейлист • 50 треков" }] },
                                        "navigationEndpoint": {
                                            "browseEndpoint": { "browseId": "VLPL_workout_123" }
                                        },
                                        "thumbnailRenderer": {
                                            "musicThumbnailRenderer": {
                                                "thumbnail": {
                                                    "thumbnails": [{ "url": "https://img.com/pl.jpg", "width": 400 }]
                                                }
                                            }
                                        }
                                    }
                                }
                            ]
                        }
                    }
                ]
            }
        }
    });

    let pls = parse_playlists(&json);
    assert_eq!(pls.len(), 1);
    assert_eq!(pls[0].uid, "VLPL_workout_123");
    assert_eq!(pls[0].title, "My Workout Mix");
    assert_eq!(pls[0].og_image, Some("https://img.com/pl.jpg".to_string()));
}

#[test]
fn test_parse_albums() {
    let json = json!({
        "contents": {
            "sectionListRenderer": {
                "contents": [
                    {
                        "gridRenderer": {
                            "items": [
                                {
                                    "musicTwoRowItemRenderer": {
                                        "title": { "runs": [{ "text": "Random Access Memories" }] },
                                        "subtitle": {
                                            "runs": [
                                                { "text": "Альбом" },
                                                { "text": " • " },
                                                {
                                                    "text": "Daft Punk",
                                                    "navigationEndpoint": {
                                                        "browseEndpoint": { "browseId": "UCdaft" }
                                                    }
                                                },
                                                { "text": " • " },
                                                { "text": "2013" }
                                            ]
                                        },
                                        "navigationEndpoint": {
                                            "browseEndpoint": { "browseId": "MPREb_daft_ram" }
                                        },
                                        "thumbnailRenderer": {
                                            "musicThumbnailRenderer": {
                                                "thumbnail": {
                                                    "thumbnails": [{ "url": "https://img.com/ram.jpg", "width": 500 }]
                                                }
                                            }
                                        }
                                    }
                                }
                            ]
                        }
                    }
                ]
            }
        }
    });

    let albums = parse_albums(&json);
    assert_eq!(albums.len(), 1);
    assert_eq!(albums[0].id, "MPREb_daft_ram");
    assert_eq!(albums[0].title, Some("Random Access Memories".to_string()));
    assert_eq!(albums[0].year, Some(2013));
    assert_eq!(albums[0].artists.len(), 1);
    assert_eq!(albums[0].artists[0].name, "Daft Punk");
    assert_eq!(
        albums[0].cover_uri,
        Some("https://img.com/ram.jpg".to_string())
    );
}

#[test]
fn test_parse_search_card_shelf_and_dedup() {
    let json = json!({
        "contents": {
            "sectionListRenderer": {
                "contents": [
                    {
                        "musicCardShelfRenderer": {
                            "title": { "runs": [{ "text": "Queen", "navigationEndpoint": { "browseEndpoint": { "browseId": "UCqueen" } } }] },
                            "subtitle": { "runs": [{ "text": "Исполнитель • 40M подписчиков" }] },
                            "thumbnail": { "thumbnails": [{ "url": "https://img.com/queen.jpg", "width": 500 }] },
                            "contents": [
                                {
                                    "musicResponsiveListItemRenderer": {
                                        "playlistItemData": { "videoId": "bohemian_rhapsody" },
                                        "flexColumns": [
                                            {
                                                "musicResponsiveListItemFlexColumnRenderer": {
                                                    "text": { "runs": [{ "text": "Bohemian Rhapsody" }] }
                                                }
                                            },
                                            {
                                                "musicResponsiveListItemFlexColumnRenderer": {
                                                    "text": { "runs": [{ "text": "Queen" }, { "text": " • " }, { "text": "5:55" }] }
                                                }
                                            }
                                        ]
                                    }
                                }
                            ]
                        }
                    },
                    {
                        "itemSectionRenderer": {
                            "contents": [
                                // Дубликат того же трека
                                {
                                    "musicResponsiveListItemRenderer": {
                                        "playlistItemData": { "videoId": "bohemian_rhapsody" },
                                        "flexColumns": [
                                            {
                                                "musicResponsiveListItemFlexColumnRenderer": {
                                                    "text": { "runs": [{ "text": "Bohemian Rhapsody" }] }
                                                }
                                            }
                                        ]
                                    }
                                },
                                // Альбом без videoId
                                {
                                    "musicResponsiveListItemRenderer": {
                                        "navigationEndpoint": {
                                            "browseEndpoint": { "browseId": "MPRE_night_at_opera" }
                                        },
                                        "flexColumns": [
                                            {
                                                "musicResponsiveListItemFlexColumnRenderer": {
                                                    "text": { "runs": [{ "text": "A Night at the Opera" }] }
                                                }
                                            },
                                            {
                                                "musicResponsiveListItemFlexColumnRenderer": {
                                                    "text": { "runs": [{ "text": "Альбом • Queen • 1975" }] }
                                                }
                                            }
                                        ]
                                    }
                                },
                                // Плейлист без videoId
                                {
                                    "musicResponsiveListItemRenderer": {
                                        "navigationEndpoint": {
                                            "browseEndpoint": { "browseId": "VLPL_queen_best" }
                                        },
                                        "flexColumns": [
                                            {
                                                "musicResponsiveListItemFlexColumnRenderer": {
                                                    "text": { "runs": [{ "text": "Best of Queen" }] }
                                                }
                                            },
                                            {
                                                "musicResponsiveListItemFlexColumnRenderer": {
                                                    "text": { "runs": [{ "text": "Плейлист • 30 треков" }] }
                                                }
                                            }
                                        ]
                                    }
                                }
                            ]
                        }
                    }
                ]
            }
        }
    });

    let res = parse_search_results(&json);

    // Проверяем артиста из карточки
    assert!(res.artists.is_some());
    let artists = res.artists.unwrap().results;
    assert_eq!(artists.len(), 1);
    assert_eq!(artists[0].id, "UCqueen");
    assert_eq!(artists[0].name, "Queen");
    assert_eq!(
        artists[0].cover.as_ref().unwrap().uri.as_deref(),
        Some("https://img.com/queen.jpg")
    );

    // Проверяем треки с дедупликацией
    assert!(res.tracks.is_some());
    let tracks = res.tracks.unwrap().results;
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].id, "bohemian_rhapsody");
    assert_eq!(tracks[0].title, "Bohemian Rhapsody");

    // Проверяем альбом
    assert!(res.albums.is_some());
    let albums = res.albums.unwrap().results;
    assert_eq!(albums.len(), 1);
    assert_eq!(albums[0].id, "MPRE_night_at_opera");
    assert_eq!(albums[0].title.as_deref(), Some("A Night at the Opera"));

    // Проверяем плейлист
    assert!(res.playlists.is_some());
    let playlists = res.playlists.unwrap().results;
    assert_eq!(playlists.len(), 1);
    assert_eq!(playlists[0].uid, "VLPL_queen_best");
    assert_eq!(playlists[0].title, "Best of Queen");
}

#[test]
fn test_is_clean_music_track() {
    // Обычный трек 3 мин 20 сек
    let normal_track = Track {
        id: "track1".to_string(),
        title: "Shape of You".to_string(),
        artists: vec![Artist {
            id: "a1".to_string(),
            name: "Ed Sheeran".to_string(),
            cover: None,
        }],
        duration_ms: Some(200_000),
        ..Default::default()
    };
    assert!(is_clean_music_track(&normal_track));

    // Слишком короткий (звуковой эффект 15 сек)
    let short_track = Track {
        id: "short".to_string(),
        title: "Sound Effect".to_string(),
        duration_ms: Some(15_000),
        ..Default::default()
    };
    assert!(!is_clean_music_track(&short_track));

    // Слишком длинный (подкаст/аудиокнига 1 час)
    let long_track = Track {
        id: "long".to_string(),
        title: "Audio recording".to_string(),
        duration_ms: Some(3_600_000),
        ..Default::default()
    };
    assert!(!is_clean_music_track(&long_track));

    // Стоп-слова в названии
    for word in &[
        "Музыкальный Подкаст #1",
        "Joe Rogan Podcast",
        "Выпуск #42 с гостем",
        "Выпуск №5",
        "Эпизод 10",
        "Episode 4",
        "Интервью со звездой",
        "Утреннее Шоу на радио",
        "Talk show 2024",
        "Стрим игры",
        "Stream replay",
        "Реакция на клип",
        "Reaction to song",
        "Review of album",
        "Обзор техники",
        "Metallica - Full Album",
        "Тизер нового альбома",
        "Official Teaser",
        "Аудиокнига Война и мир",
        "Audiobook chapter 1",
    ] {
        let t = Track {
            id: "stop".to_string(),
            title: word.to_string(),
            duration_ms: Some(240_000),
            ..Default::default()
        };
        assert!(
            !is_clean_music_track(&t),
            "Должен был отсечься трек со стоп-словом: {}",
            word
        );
    }

    // Стоп-слово в артисте
    let artist_stop = Track {
        id: "artist_stop".to_string(),
        title: "Интересный выпуск".to_string(),
        artists: vec![Artist {
            id: "a2".to_string(),
            name: "Подкаст Клуб".to_string(),
            cover: None,
        }],
        duration_ms: Some(300_000),
        ..Default::default()
    };
    assert!(!is_clean_music_track(&artist_stop));
}

#[test]
fn test_parse_album_tracks_with_deep_header_and_cover_fallback() {
    let json = json!({
        "contents": {
            "twoColumnBrowseResultsRenderer": {
                "secondaryContents": {
                    "sectionListRenderer": {
                        "contents": [
                            {
                                "musicResponsiveHeaderRenderer": {
                                    "title": { "runs": [{ "text": "Deep Album" }] },
                                    "thumbnail": {
                                        "musicThumbnailRenderer": {
                                            "thumbnail": {
                                                "thumbnails": [
                                                    { "url": "//lh3.googleusercontent.com/album_cover.jpg", "width": 544, "height": 544 }
                                                ]
                                            }
                                        }
                                    }
                                }
                            }
                        ]
                    }
                }
            }
        },
        "musicShelfRenderer": {
            "contents": [
                {
                    "musicResponsiveListItemRenderer": {
                        "playlistItemData": { "videoId": "vid_1" },
                        "flexColumns": [
                            {
                                "musicResponsiveListItemFlexColumnRenderer": {
                                    "text": { "runs": [{ "text": "Track 1" }] }
                                }
                            }
                        ]
                    }
                },
                {
                    "musicResponsiveListItemRenderer": {
                        "playlistItemData": { "videoId": "vid_2" },
                        "flexColumns": [
                            {
                                "musicResponsiveListItemFlexColumnRenderer": {
                                    "text": { "runs": [{ "text": "Track 2" }] }
                                }
                            }
                        ]
                    }
                },
                // Дубликат первого трека
                {
                    "musicResponsiveListItemRenderer": {
                        "playlistItemData": { "videoId": "vid_1" },
                        "flexColumns": [
                            {
                                "musicResponsiveListItemFlexColumnRenderer": {
                                    "text": { "runs": [{ "text": "Track 1" }] }
                                }
                            }
                        ]
                    }
                }
            ]
        }
    });

    let tracks = parse_album_tracks(&json);
    assert_eq!(tracks.len(), 2, "Дубликат трека должен быть отсечен");
    assert_eq!(tracks[0].id, "vid_1");
    assert_eq!(tracks[0].title, "Track 1");
    assert_eq!(
        tracks[0].cover_uri.as_deref(),
        Some("https://lh3.googleusercontent.com/album_cover.jpg"),
        "Обложка альбома должна быть унаследована треком"
    );
    assert_eq!(
        tracks[1].cover_uri.as_deref(),
        Some("https://lh3.googleusercontent.com/album_cover.jpg")
    );
}

#[test]
fn test_parse_playlists_filters_system_buttons_and_extracts_meta() {
    let json = json!({
        "sectionListRenderer": {
            "contents": [
                {
                    "gridRenderer": {
                        "items": [
                            // Системная кнопка создания плейлиста
                            {
                                "musicTwoRowItemRenderer": {
                                    "title": { "runs": [{ "text": "Новый плейлист" }] },
                                    "navigationEndpoint": {
                                        "browseEndpoint": { "browseId": "FEmusic_new_playlist" }
                                    }
                                }
                            },
                            // Настоящий плейлист пользователя
                            {
                                "musicTwoRowItemRenderer": {
                                    "title": { "runs": [{ "text": "Мой Рок-плейлист" }] },
                                    "navigationEndpoint": {
                                        "browseEndpoint": { "browseId": "VLPL_my_rock" }
                                    },
                                    "thumbnailRenderer": {
                                        "musicThumbnailRenderer": {
                                            "thumbnail": {
                                                "thumbnails": [
                                                    { "url": "https://example.com/rock.jpg", "width": 400, "height": 400 }
                                                ]
                                            }
                                        }
                                    },
                                    "subtitle": {
                                        "runs": [{ "text": "45 треков" }]
                                    }
                                }
                            }
                        ]
                    }
                }
            ]
        }
    });

    let playlists = parse_playlists(&json);
    assert_eq!(playlists.len(), 1, "Системная кнопка должна быть отсечена");
    assert_eq!(playlists[0].uid, "VLPL_my_rock");
    assert_eq!(playlists[0].title, "Мой Рок-плейлист");
    assert_eq!(playlists[0].track_count, Some(45));
    assert_eq!(
        playlists[0].og_image.as_deref(),
        Some("https://example.com/rock.jpg")
    );
}

#[test]
fn test_parse_liked_tracks_from_shelf() {
    let json = json!({
        "musicPlaylistShelfRenderer": {
            "contents": [
                {
                    "musicResponsiveListItemRenderer": {
                        "playlistItemData": { "videoId": "liked_1" },
                        "flexColumns": [
                            {
                                "musicResponsiveListItemFlexColumnRenderer": {
                                    "text": { "runs": [{ "text": "Liked Track 1" }] }
                                }
                            },
                            {
                                "musicResponsiveListItemFlexColumnRenderer": {
                                    "text": { "runs": [{ "text": "Singer Name" }, { "text": " • " }, { "text": "3:15" }] }
                                }
                            }
                        ],
                        "thumbnail": {
                            "musicThumbnailRenderer": {
                                "thumbnail": {
                                    "thumbnails": [
                                        { "url": "https://img.com/liked1.jpg", "width": 200 }
                                    ]
                                }
                            }
                        }
                    }
                }
            ]
        }
    });

    let tracks = parse_liked_tracks(&json);
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].id, "liked_1");
    assert_eq!(tracks[0].title, "Liked Track 1");
    assert_eq!(
        tracks[0].cover_uri.as_deref(),
        Some("https://img.com/liked1.jpg")
    );
    assert_eq!(tracks[0].duration_ms, Some(195_000));
    assert_eq!(tracks[0].artists.len(), 1);
    assert_eq!(tracks[0].artists[0].name, "Singer Name");
}

#[test]
fn test_unescape_js_string() {
    let raw = r#"{"text":"\x48\x65\x6c\x6c\x6f \\\x22World\\\x22"}"#;
    let unescaped = unescape_js_string(raw);
    assert_eq!(unescaped, r#"{"text":"Hello \"World\""}"#);
}

#[test]
fn test_parse_tracks_from_html_regex_and_data() {
    let sample_html = r#"
        <script>
        initialData.push({path: '/browse', params: JSON.parse('{}'), data: '{\x22musicPlaylistShelfRenderer\x22:{\x22contents\x22:[{\x22musicResponsiveListItemRenderer\x22:{\x22playlistItemData\x22:{\x22videoId\x22:\x22GBRAnuT48qo\x22},\x22flexColumns\x22:[{\x22musicResponsiveListItemFlexColumnRenderer\x22:{\x22text\x22:{\x22runs\x22:[{\x22text\x22:\x22Happy Song\x22}]}}},{\x22musicResponsiveListItemFlexColumnRenderer\x22:{\x22text\x22:{\x22runs\x22:[{\x22text\x22:\x22Bring Me The Horizon\x22}]}}}]}}]}}'});
        </script>
        <!-- Regex fallback block -->
        <div>musicResponsiveListItemRenderer \x22videoId\x22:\x22hzRltUL5M3k\x22, \x22label\x22:\x22Change (In the House of Flies) - Deftones - 4 минуты 59 секунд – воспроизвести\x22</div>
        "#;

    let tracks = parse_tracks_from_html(sample_html);
    assert_eq!(tracks.len(), 2);
    assert_eq!(tracks[0].id, "GBRAnuT48qo");
    assert_eq!(tracks[0].title, "Happy Song");
    assert_eq!(tracks[0].artists[0].name, "Bring Me The Horizon");
    assert_eq!(
        tracks[0].cover_uri.as_deref(),
        Some("https://i.ytimg.com/vi/GBRAnuT48qo/hqdefault.jpg")
    );

    assert_eq!(tracks[1].id, "hzRltUL5M3k");
    assert_eq!(tracks[1].title, "Change (In the House of Flies)");
    assert_eq!(tracks[1].artists[0].name, "Deftones");
    assert_eq!(
        tracks[1].cover_uri.as_deref(),
        Some("https://i.ytimg.com/vi/hzRltUL5M3k/hqdefault.jpg")
    );
}

#[test]
fn test_parse_playlists_from_html_guide() {
    let sample_html = r#"
        <script>
        initialData.push({path: '/guide', params: JSON.parse('{}'), data: '{\x22items\x22:[{\x22guideEntryRenderer\x22:{\x22formattedTitle\x22:{\x22runs\x22:[{\x22text\x22:\x22TikTok Songs\x22}]},\x22navigationEndpoint\x22:{\x22browseEndpoint\x22:{\x22browseId\x22:\x22VLPLcHCtI7UKhDqwiMLD3A_nTUx7DJxKQt2c\x22}}}},{\x22guideEntryRenderer\x22:{\x22formattedTitle\x22:{\x22runs\x22:[{\x22text\x22:\x22Новый плейлист\x22}]},\x22navigationEndpoint\x22:{\x22browseEndpoint\x22:{\x22browseId\x22:\x22FEmusic_new_playlist\x22}}}}]}}'});
        </script>
        "#;

    let playlists = parse_playlists_from_html(sample_html);
    assert_eq!(playlists.len(), 1);
    assert_eq!(playlists[0].uid, "VLPLcHCtI7UKhDqwiMLD3A_nTUx7DJxKQt2c");
    assert_eq!(playlists[0].title, "TikTok Songs");
}

#[test]
fn test_parse_album_item_thumbnail_renderer() {
    let item = json!({
        "musicTwoRowItemRenderer": {
            "title": { "runs": [{ "text": "Hybrid Theory" }] },
            "subtitle": { "runs": [{ "text": "2000" }] },
            "navigationEndpoint": {
                "browseEndpoint": { "browseId": "MPREb_album_ht" }
            },
            "thumbnailRenderer": {
                "musicThumbnailRenderer": {
                    "thumbnail": {
                        "thumbnails": [
                            { "url": "https://lh3.googleusercontent.com/album_ht.jpg", "width": 544, "height": 544 }
                        ]
                    }
                }
            }
        }
    });

    let album = parse_album_item(&item).expect("Should parse album");
    assert_eq!(album.id, "MPREb_album_ht");
    assert_eq!(album.title.as_deref(), Some("Hybrid Theory"));
    assert_eq!(album.year, Some(2000));
    assert_eq!(
        album.cover_url(300),
        Some("https://lh3.googleusercontent.com/album_ht.jpg".to_string())
    );
}

#[test]
fn test_artist_browse_empty_duration_and_album_cover() {
    let json = json!({
        "header": {
            "musicImmersiveHeaderRenderer": {
                "title": { "runs": [{ "text": "Deftones" }] },
                "thumbnail": {
                    "musicThumbnailRenderer": {
                        "thumbnail": {
                            "thumbnails": [
                                { "url": "https://lh3.googleusercontent.com/deftones_artist.jpg", "width": 800 }
                            ]
                        }
                    }
                }
            }
        },
        "contents": {
            "singleColumnBrowseResultsRenderer": {
                "tabs": [
                    {
                        "tabRenderer": {
                            "content": {
                                "sectionListRenderer": {
                                    "contents": [
                                        {
                                            "musicShelfRenderer": {
                                                "title": { "runs": [{ "text": "Песни" }] },
                                                "contents": [
                                                    {
                                                        "musicResponsiveListItemRenderer": {
                                                            "playlistItemData": { "videoId": "deftones_song_1" },
                                                            "flexColumns": [
                                                                {
                                                                    "musicResponsiveListItemFlexColumnRenderer": {
                                                                        "text": { "runs": [{ "text": "Be Quiet and Drive" }] }
                                                                    }
                                                                },
                                                                {
                                                                    "musicResponsiveListItemFlexColumnRenderer": {
                                                                        "text": {
                                                                            "runs": [
                                                                                { "text": "Deftones" }
                                                                            ]
                                                                        }
                                                                    }
                                                                }
                                                            ]
                                                        }
                                                    }
                                                ]
                                            }
                                        },
                                        {
                                            "musicCarouselShelfRenderer": {
                                                "header": {
                                                    "musicCarouselShelfBasicHeaderRenderer": {
                                                        "title": { "runs": [{ "text": "Альбомы" }] }
                                                    }
                                                },
                                                "contents": [
                                                    {
                                                        "musicTwoRowItemRenderer": {
                                                            "title": { "runs": [{ "text": "Around the Fur" }] },
                                                            "subtitle": { "runs": [{ "text": "1997" }] },
                                                            "navigationEndpoint": {
                                                                "browseEndpoint": { "browseId": "MPREb_atf" }
                                                            },
                                                            "thumbnailRenderer": {
                                                                "musicThumbnailRenderer": {
                                                                    "thumbnail": {
                                                                        "thumbnails": [
                                                                            { "url": "https://lh3.googleusercontent.com/album_atf.jpg", "width": 500 }
                                                                        ]
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                ]
                                            }
                                        }
                                    ]
                                }
                            }
                        }
                    }
                ]
            }
        }
    });

    let brief = parse_artist_browse(&json, "UC_deftones").expect("Should parse artist brief");
    assert_eq!(brief.artist.id, "UC_deftones");
    assert_eq!(brief.artist.name, "Deftones");
    assert_eq!(
        brief.artist.cover_url(400),
        Some("https://lh3.googleusercontent.com/deftones_artist.jpg".to_string())
    );

    // Альбом из карусели получил обложку через thumbnailRenderer
    assert_eq!(brief.albums.len(), 1);
    assert_eq!(brief.albums[0].id, "MPREb_atf");
    assert_eq!(
        brief.albums[0].cover_url(300),
        Some("https://lh3.googleusercontent.com/album_atf.jpg".to_string())
    );

    // Популярный трек без указанной длительности в runs
    assert_eq!(brief.popular_tracks.len(), 1);
    assert_eq!(brief.popular_tracks[0].id, "deftones_song_1");
    assert_eq!(brief.popular_tracks[0].duration_ms, None);
    // Не должен возвращать "00:00"!
    assert_eq!(brief.popular_tracks[0].formatted_duration(), "");
    // Обложка трека унаследована от артиста
    assert_eq!(
        brief.popular_tracks[0].cover_url(300),
        Some("https://lh3.googleusercontent.com/deftones_artist.jpg".to_string())
    );
}

#[test]
fn test_parse_header_track_count() {
    let json = serde_json::json!({
        "contents": {
            "twoColumnBrowseResultsRenderer": {
                "tabs": [{
                    "tabRenderer": {
                        "content": {
                            "sectionListRenderer": {
                                "contents": [{
                                    "musicResponsiveHeaderRenderer": {
                                        "secondSubtitle": {
                                            "runs": [
                                                { "text": "325\u{00a0}треков" },
                                                { "text": " • " },
                                                { "text": "Больше 6 часов" }
                                            ]
                                        }
                                    }
                                }]
                            }
                        }
                    }
                }]
            }
        }
    });

    assert_eq!(parse_header_track_count(&json), Some(325));
}

#[test]
fn test_parse_header_track_count_from_html() {
    let html = r#"<script>initialData.push({path: '/browse', params: JSON.parse("null"), data: '{\x22contents\x22:{\x22twoColumnBrowseResultsRenderer\x22:{\x22tabs\x22:[{\x22tabRenderer\x22:{\x22content\x22:{\x22sectionListRenderer\x22:{\x22contents\x22:[{\x22musicResponsiveHeaderRenderer\x22:{\x22secondSubtitle\x22:{\x22runs\x22:[{\x22text\x22:\x22325 треков\x22}]}}}]}}}]}}}}'});</script>"#;
    assert_eq!(parse_header_track_count_from_html(html), Some(325));

    let html_fallback = r#"<div>musicResponsiveHeaderRenderer ... secondSubtitle: {"runs":[{"text":"325 треков"}]}</div>"#;
    assert_eq!(parse_header_track_count_from_html(html_fallback), Some(325));
}

// --- Извлечение аудиопотока (/youtubei/v1/player) ---

/// Формат потока для фикстур `parse_stream_url` — как в реальном `adaptiveFormats`.
fn stream_format(itag: u64, mime_type: &str, bitrate: u64) -> serde_json::Value {
    json!({
        "itag": itag,
        "mimeType": mime_type,
        "bitrate": bitrate,
        "url": format!("https://r1.googlevideo.com/videoplayback?itag={itag}"),
    })
}

/// Ответ player-эндпоинта с успешным статусом и списком адаптивных форматов.
fn player_json(adaptive_formats: Vec<serde_json::Value>) -> serde_json::Value {
    json!({
        "playabilityStatus": { "status": "OK" },
        "streamingData": { "adaptiveFormats": adaptive_formats },
    })
}

#[test]
fn test_parse_stream_url_reports_playability_reason() {
    // Текст ошибки показывается пользователю — держим его дословно
    let json = json!({
        "playabilityStatus": {
            "status": "LOGIN_REQUIRED",
            "reason": "Sign in to confirm you're not a bot"
        }
    });
    assert_eq!(
        parse_stream_url(&json).unwrap_err(),
        "Воспроизведение недоступно: Sign in to confirm you're not a bot"
    );

    // Без reason в сообщение попадает сам статус
    let json = json!({ "playabilityStatus": { "status": "ERROR" } });
    assert_eq!(
        parse_stream_url(&json).unwrap_err(),
        "Воспроизведение недоступно: ERROR"
    );

    // Статус OK сам по себе ничего не даёт: без форматов возвращается ошибка
    let json = json!({ "playabilityStatus": { "status": "OK" } });
    assert_eq!(
        parse_stream_url(&json).unwrap_err(),
        "Не найдено прямых аудиопотоков в ответе InnerTube"
    );
}

#[test]
fn test_parse_stream_url_ignores_formats_without_direct_url() {
    // signatureCipher и пустой url не годятся: нужен прямой адрес
    let json = json!({
        "playabilityStatus": { "status": "OK" },
        "streamingData": {
            "adaptiveFormats": [
                {
                    "itag": 251,
                    "mimeType": "audio/webm; codecs=\"opus\"",
                    "signatureCipher": "s=abc"
                },
                {
                    "itag": 140,
                    "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"",
                    "bitrate": 128000,
                    "url": ""
                }
            ]
        }
    });
    assert_eq!(
        parse_stream_url(&json).unwrap_err(),
        "Не найдено прямых аудиопотоков в ответе InnerTube"
    );
}

#[test]
fn test_parse_stream_url_prefers_opus_over_video() {
    // У видео-дорожки битрейт выше, но приоритет у Opus (itag 251)
    let json = player_json(vec![
        stream_format(137, "video/mp4; codecs=\"avc1.640028\"", 4_000_000),
        stream_format(251, "audio/webm; codecs=\"opus\"", 160_000),
    ]);
    assert_eq!(
        parse_stream_url(&json).unwrap(),
        "https://r1.googlevideo.com/videoplayback?itag=251"
    );
}

#[test]
fn test_parse_stream_url_picks_loudest_opus_and_falls_back_to_average_bitrate() {
    // Из нескольких Opus-дорожек выбирается самая битрейтная
    let json = player_json(vec![
        json!({
            "itag": 251,
            "mimeType": "audio/webm; codecs=\"opus\"",
            "bitrate": 130_000,
            "url": "https://r1.googlevideo.com/videoplayback?q=quiet",
        }),
        json!({
            "itag": 251,
            "mimeType": "audio/webm; codecs=\"opus\"",
            "bitrate": 160_000,
            "url": "https://r1.googlevideo.com/videoplayback?q=loud",
        }),
    ]);
    assert_eq!(
        parse_stream_url(&json).unwrap(),
        "https://r1.googlevideo.com/videoplayback?q=loud"
    );

    // Если bitrate нет, используется averageBitrate
    let json = player_json(vec![
        json!({
            "itag": 251,
            "mimeType": "audio/webm; codecs=\"opus\"",
            "averageBitrate": 160_000,
            "url": "https://r1.googlevideo.com/videoplayback?a=avg",
        }),
        json!({
            "itag": 251,
            "mimeType": "audio/webm; codecs=\"opus\"",
            "bitrate": 130_000,
            "url": "https://r1.googlevideo.com/videoplayback?a=bit",
        }),
    ]);
    assert_eq!(
        parse_stream_url(&json).unwrap(),
        "https://r1.googlevideo.com/videoplayback?a=avg"
    );
}

#[test]
fn test_parse_stream_url_falls_back_to_aac_then_any_audio() {
    // Без Opus выбирается AAC (itag 140)
    let json = player_json(vec![
        stream_format(137, "video/mp4; codecs=\"avc1\"", 3_000_000),
        stream_format(140, "audio/mp4; codecs=\"mp4a.40.2\"", 129_000),
    ]);
    assert_eq!(
        parse_stream_url(&json).unwrap(),
        "https://r1.googlevideo.com/videoplayback?itag=140"
    );

    // Любой audio/* подходит, если нет ни Opus, ни AAC
    let json = player_json(vec![json!({
        "itag": 250,
        "mimeType": "audio/webm; codecs=\"vorbis\"",
        "bitrate": 64_000,
        "url": "https://r1.googlevideo.com/videoplayback?codec=vorbis",
    })]);
    assert_eq!(
        parse_stream_url(&json).unwrap(),
        "https://r1.googlevideo.com/videoplayback?codec=vorbis"
    );

    // Последний фолбэк: формат без audio в mimeType — берётся самый битрейтный
    let json = player_json(vec![
        stream_format(137, "video/mp4; codecs=\"avc1\"", 3_000_000),
        stream_format(248, "video/webm; codecs=\"vp9\"", 1_500_000),
    ]);
    assert_eq!(
        parse_stream_url(&json).unwrap(),
        "https://r1.googlevideo.com/videoplayback?itag=137"
    );
}

#[test]
fn test_parse_stream_url_reads_muxed_formats_array() {
    // `formats` (мультиплекс) просматривается наравне с `adaptiveFormats`
    let json = json!({
        "streamingData": {
            "formats": [{
                "itag": 18,
                "mimeType": "video/mp4; codecs=\"avc1, mp4a.40.2\"",
                "bitrate": 96_000,
                "url": "https://r1.googlevideo.com/videoplayback?itag=18",
            }]
        }
    });
    assert_eq!(
        parse_stream_url(&json).unwrap(),
        "https://r1.googlevideo.com/videoplayback?itag=18"
    );
}

// --- Разбор JS-строк и полей из HTML ---

#[test]
fn test_extract_js_string_literal_quotes_and_escapes() {
    // Ведущие пробельные символы пропускаются
    assert_eq!(
        extract_js_string_literal("  \n\t'/browse', params: 1"),
        Some("/browse")
    );
    // Двойные кавычки поддерживаются наравне с одинарными
    assert_eq!(extract_js_string_literal("\"VLPL123\""), Some("VLPL123"));
    // Экранированная кавычка не закрывает литерал
    assert_eq!(extract_js_string_literal(r"'it\'s'"), Some(r"it\'s"));
    // Пустой литерал — валидная строка
    assert_eq!(extract_js_string_literal("''"), Some(""));
    // Незакрытый литерал, не-строка и пустой ввод дают None
    assert_eq!(extract_js_string_literal("'abc"), None);
    assert_eq!(extract_js_string_literal("path: /browse"), None);
    assert_eq!(extract_js_string_literal("   "), None);
}

#[test]
fn test_extract_field_value_supported_shapes() {
    // Экранированная форма из `data: '...'`
    assert_eq!(
        extract_field_value(r#"..."\x22videoId\x22:\x22dQw4w9WgXcQ\x22..."#, "videoId").as_deref(),
        Some("dQw4w9WgXcQ")
    );
    // Обычный JSON без пробела и с пробелом после двоеточия
    assert_eq!(
        extract_field_value(r#""videoId":"dQw4w9WgXcQ""#, "videoId").as_deref(),
        Some("dQw4w9WgXcQ")
    );
    assert_eq!(
        extract_field_value(r#""videoId": "dQw4w9WgXcQ""#, "videoId").as_deref(),
        Some("dQw4w9WgXcQ")
    );
    // Форма без экранирования открывающей кавычки ключа
    assert_eq!(
        extract_field_value(r#"videoId\x22:\x22dQw4w9WgXcQ"#, "videoId").as_deref(),
        Some("dQw4w9WgXcQ")
    );
    // Пустое значение и отсутствующее поле — None
    assert_eq!(extract_field_value(r#""videoId":"""#, "videoId"), None);
    assert_eq!(extract_field_value("no fields here", "videoId"), None);
}

#[test]
fn test_extract_track_label_splits_title_and_artist() {
    // Полный label из accessibility-атрибута
    let chunk =
        r#"{"label":"Happy Song - Bring Me The Horizon - 3 минуты 57 секунд – воспроизвести"}"#;
    assert_eq!(
        extract_track_label(chunk),
        Some(("Happy Song".to_string(), "Bring Me The Horizon".to_string()))
    );

    // Экранированная форма и английский суффикс "play"
    let chunk = r#"label\x22:\x22Change - Deftones - play"#;
    assert_eq!(
        extract_track_label(chunk),
        Some(("Change".to_string(), "Deftones".to_string()))
    );

    // Дефис без пробелов — запасной вариант разбиения
    let chunk = r#""label":"Song-Deftones""#;
    assert_eq!(
        extract_track_label(chunk),
        Some(("Song".to_string(), "Deftones".to_string()))
    );

    // Разделителя нет — трек не распознан
    assert_eq!(
        extract_track_label(r#""label":"JustASong – воспроизвести""#),
        None
    );
    assert_eq!(extract_track_label("no label at all"), None);
}

#[test]
fn test_extract_duration_from_chunk() {
    // Обычная и экранированная форма lengthText
    assert_eq!(
        extract_duration_from_chunk(r#"{"lengthText":{"runs":[{"text":"3:45"}]}}"#),
        Some(225_000)
    );
    assert_eq!(
        extract_duration_from_chunk(r#"\x22lengthText\x22:{\x22runs\x22:[{\x22text\x22:\x2201:02:03\x22}]}"#),
        Some(3_723_000)
    );

    // lengthText нет — длительность берём словами из accessibility label
    assert_eq!(
        extract_duration_from_chunk(
            r#"{"label":"Sad Song - Tokio Hotel - 1 час 2 минуты 3 секунды – воспроизвести"}"#
        ),
        Some(3_723_000)
    );
    assert_eq!(
        extract_duration_from_chunk(r#""label":"Song - Artist - 4 minutes 12 seconds - play""#),
        Some(252_000)
    );

    // Ни времени, ни подписи — None (а не ноль)
    assert_eq!(extract_duration_from_chunk("no duration here"), None);
}

#[test]
fn test_parse_duration_text_units() {
    assert_eq!(parse_duration_text("3 минуты 57 секунд"), Some(237_000));
    assert_eq!(parse_duration_text("45 секунд"), Some(45_000));
    assert_eq!(parse_duration_text("2 hours 5 minutes"), Some(7_500_000));
    assert_eq!(parse_duration_text("no numbers here"), None);
    assert_eq!(parse_duration_text("3 tracks"), None);
}

#[test]
fn test_parse_tracks_and_continuation_from_html() {
    // Один проход отдаёт и треки из initialData, и токен продолжения
    let html = r#"<script>initialData.push({path: '/browse', params: JSON.parse('{}'), data: '{\x22musicPlaylistShelfRenderer\x22:{\x22contents\x22:[{\x22musicResponsiveListItemRenderer\x22:{\x22playlistItemData\x22:{\x22videoId\x22:\x22dQw4w9WgXcQ\x22},\x22flexColumns\x22:[{\x22musicResponsiveListItemFlexColumnRenderer\x22:{\x22text\x22:{\x22runs\x22:[{\x22text\x22:\x22Never Gonna Give You Up\x22}]}}},{\x22musicResponsiveListItemFlexColumnRenderer\x22:{\x22text\x22:{\x22runs\x22:[{\x22text\x22:\x22Rick Astley\x22}]}}}]}}],\x22continuations\x22:[{\x22nextContinuationData\x22:{\x22continuation\x22:\x22CBQSTOKEN_1234567890\x22}}]}}'});</script>"#;

    let (tracks, continuation) = parse_tracks_and_continuation_from_html(html);
    assert_eq!(continuation.as_deref(), Some("CBQSTOKEN_1234567890"));
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].id, "dQw4w9WgXcQ");
    assert_eq!(tracks[0].title, "Never Gonna Give You Up");
    assert_eq!(tracks[0].artists[0].name, "Rick Astley");
    assert_eq!(
        tracks[0].cover_uri.as_deref(),
        Some("https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg")
    );
    // В этом фрагменте lengthText нет, а label с длительностью есть не всегда
    assert_eq!(tracks[0].duration_ms, None);
}

// --- Поиск идентификаторов и текста в ответах InnerTube ---

#[test]
fn test_find_video_id_supported_shapes() {
    // Все пять форм videoId, встречающихся в рендерерах
    assert_eq!(
        find_video_id(&json!({ "videoId": "dQw4w9WgXcQ" })).as_deref(),
        Some("dQw4w9WgXcQ")
    );
    assert_eq!(
        find_video_id(&json!({ "playlistItemData": { "videoId": "dQw4w9WgXcQ" } })).as_deref(),
        Some("dQw4w9WgXcQ")
    );
    assert_eq!(
        find_video_id(&json!({ "watchEndpoint": { "videoId": "dQw4w9WgXcQ" } })).as_deref(),
        Some("dQw4w9WgXcQ")
    );
    assert_eq!(
        find_video_id(
            &json!({ "navigationEndpoint": { "watchEndpoint": { "videoId": "dQw4w9WgXcQ" } } })
        )
        .as_deref(),
        Some("dQw4w9WgXcQ")
    );
    assert_eq!(
        find_video_id(&json!({ "onTap": { "watchEndpoint": { "videoId": "dQw4w9WgXcQ" } } }))
            .as_deref(),
        Some("dQw4w9WgXcQ")
    );

    // Пустой идентификатор отбрасывается
    assert_eq!(find_video_id(&json!({ "videoId": "" })), None);
    assert_eq!(find_video_id(&json!({ "title": "без id" })), None);
}

#[test]
fn test_find_browse_id_supported_shapes() {
    // Прямой browseId и browseEndpoint
    assert_eq!(
        find_browse_id(&json!({ "browseId": "UCartist123" })).as_deref(),
        Some("UCartist123")
    );
    assert_eq!(
        find_browse_id(&json!({ "browseEndpoint": { "browseId": "UCartist123" } })).as_deref(),
        Some("UCartist123")
    );
    // Цепочки navigationEndpoint/onTap
    assert_eq!(
        find_browse_id(
            &json!({ "navigationEndpoint": { "browseEndpoint": { "browseId": "UCartist123" } } })
        )
        .as_deref(),
        Some("UCartist123")
    );
    assert_eq!(
        find_browse_id(&json!({ "onTap": { "browseEndpoint": { "browseId": "UCartist123" } } }))
            .as_deref(),
        Some("UCartist123")
    );
    assert_eq!(
        find_browse_id(&json!({
            "onTap": { "navigationEndpoint": { "browseEndpoint": { "browseId": "UCartist123" } } }
        }))
        .as_deref(),
        Some("UCartist123")
    );

    // browseId спрятанный в runs первой колонки списка
    let json = json!({
        "flexColumns": [{
            "musicResponsiveListItemFlexColumnRenderer": {
                "text": { "runs": [{
                    "text": "Deftones",
                    "navigationEndpoint": { "browseEndpoint": { "browseId": "UCdeftones" } }
                }] }
            }
        }]
    });
    assert_eq!(find_browse_id(&json).as_deref(), Some("UCdeftones"));

    // browseId в заголовке карточки
    let json = json!({
        "header": {
            "musicCardShelfHeaderBasicRenderer": {
                "browseId": "UCcard123"
            }
        }
    });
    assert_eq!(find_browse_id(&json).as_deref(), Some("UCcard123"));

    assert_eq!(find_browse_id(&json!({ "browseId": "" })), None);
}

#[test]
fn test_extract_text_supported_shapes() {
    // Простая строка, simpleText и склейка runs
    assert_eq!(
        extract_text(&json!("raw text")),
        Some("raw text".to_string())
    );
    assert_eq!(
        extract_text(&json!({ "simpleText": "Simple" })),
        Some("Simple".to_string())
    );
    assert_eq!(
        extract_text(&json!({ "runs": [{ "text": "Never " }, { "text": "Gonna" }] })),
        Some("Never Gonna".to_string())
    );
    // Пустой runs и неподходящий тип — None
    assert_eq!(extract_text(&json!({ "runs": [] })), None);
    assert_eq!(extract_text(&json!(42)), None);
}

// --- Обложки: пропорции и выбор квадратной ---

#[test]
fn test_is_banner_thumbnail_detects_wide_images() {
    // Широкоформатный баннер по числовым полям
    assert!(is_banner_thumbnail(
        &json!({ "width": 1920, "height": 540 })
    ));
    // Квадрат баннером не считается
    assert!(!is_banner_thumbnail(
        &json!({ "width": 544, "height": 544 })
    ));

    // Если ширины/высоты нет, пропорции берутся из параметров URL
    assert!(is_banner_thumbnail(
        &json!({ "url": "https://lh3.googleusercontent.com/banner=w1920-h1080-l90-rj" })
    ));
    assert!(!is_banner_thumbnail(
        &json!({ "url": "https://lh3.googleusercontent.com/cover=w1060-h1060-l90-rj" })
    ));
}

#[test]
fn test_extract_square_thumbnail_skips_banner() {
    // Баннер больше по площади, но квадратная обложка должна победить
    let json = json!({
        "thumbnails": [
            {
                "url": "https://lh3.googleusercontent.com/banner.jpg=w1920-h540",
                "width": 1920,
                "height": 540
            },
            {
                "url": "https://lh3.googleusercontent.com/cover.jpg=w544-h544",
                "width": 544,
                "height": 544
            }
        ]
    });
    assert_eq!(
        extract_square_thumbnail(&json).as_deref(),
        Some("https://lh3.googleusercontent.com/cover.jpg=w544-h544")
    );
    // extract_best_thumbnail баннеры не фильтрует — это его штатное поведение
    assert_eq!(
        extract_best_thumbnail(&json).as_deref(),
        Some("https://lh3.googleusercontent.com/banner.jpg=w1920-h540")
    );
}

// --- Радио и очередь воспроизведения ---

#[test]
fn test_find_continuation_token_supported_shapes() {
    // Четыре формы токена, встречающиеся в ответах InnerTube
    assert_eq!(
        find_continuation_token(&json!({ "nextContinuationData": { "continuation": "TOKEN_A" } }))
            .as_deref(),
        Some("TOKEN_A")
    );
    assert_eq!(
        find_continuation_token(
            &json!({ "nextRadioContinuationData": { "continuation": "TOKEN_B" } })
        )
        .as_deref(),
        Some("TOKEN_B")
    );
    assert_eq!(
        find_continuation_token(&json!({ "continuationCommand": { "token": "TOKEN_C" } }))
            .as_deref(),
        Some("TOKEN_C")
    );
    assert_eq!(
        find_continuation_token(&json!({
            "continuationItemRenderer": {
                "continuationEndpoint": { "continuationCommand": { "token": "TOKEN_D" } }
            }
        }))
        .as_deref(),
        Some("TOKEN_D")
    );

    // Токен находится и внутри массива
    let json = json!({
        "contents": [
            { "x": 1 },
            { "nextContinuationData": { "continuation": "TOKEN_E" } }
        ]
    });
    assert_eq!(find_continuation_token(&json).as_deref(), Some("TOKEN_E"));

    assert_eq!(find_continuation_token(&json!({ "contents": [] })), None);
}

#[test]
fn test_parse_playlist_panel_video() {
    let renderer = json!({
        "videoId": "abcDEF12345",
        "title": { "runs": [{ "text": "Minerva" }] },
        "lengthText": { "runs": [{ "text": "4:32" }] },
        "longBylineText": {
            "runs": [{
                "text": "Deftones",
                "navigationEndpoint": { "browseEndpoint": { "browseId": "UCdeftones" } }
            }]
        },
        "thumbnail": {
            "thumbnails": [{
                "url": "https://lh3.googleusercontent.com/minerva.jpg",
                "width": 544,
                "height": 544
            }]
        },
        "badges": [{
            "musicInlineBadgeRenderer": { "icon": { "iconType": "MUSIC_EXPLICIT_BADGE" } }
        }]
    });

    let track = parse_playlist_panel_video(&renderer).expect("трек должен распарситься");
    assert_eq!(track.id, "abcDEF12345");
    assert_eq!(track.title, "Minerva");
    assert_eq!(track.duration_ms, Some(272_000));
    assert_eq!(track.artists[0].name, "Deftones");
    // Обложка с googleusercontent не грузится напрямую — подменяется на i.ytimg.com
    assert_eq!(
        track.cover_uri.as_deref(),
        Some("https://i.ytimg.com/vi/abcDEF12345/hqdefault.jpg")
    );
    assert_eq!(track.content_warning.as_deref(), Some("explicit"));
    assert_eq!(track.available, Some(true));
    assert_eq!(track.source, TrackSource::YouTubeMusic);

    // Обёртка playlistPanelVideoRenderer разворачивается
    let wrapped = json!({ "playlistPanelVideoRenderer": renderer });
    assert_eq!(
        parse_playlist_panel_video(&wrapped).map(|t| t.id),
        Some("abcDEF12345".to_string())
    );

    // Готовая обложка i.ytimg.com остаётся как есть
    let with_own_cover = json!({
        "videoId": "abcDEF12345",
        "thumbnail": {
            "thumbnails": [{
                "url": "https://i.ytimg.com/vi/abcDEF12345/mqdefault.jpg",
                "width": 320,
                "height": 180
            }]
        }
    });
    assert_eq!(
        parse_playlist_panel_video(&with_own_cover)
            .and_then(|t| t.cover_uri)
            .as_deref(),
        Some("https://i.ytimg.com/vi/abcDEF12345/mqdefault.jpg")
    );

    // Без videoId трек не создаётся
    assert!(parse_playlist_panel_video(&json!({ "title": { "simpleText": "x" } })).is_none());
}

// --- Continuation-токен из сырого HTML (фолбэк) ---

#[test]
fn test_extract_continuation_from_raw_html() {
    // Канонический JSON из инлайн-скрипта
    let html = r#"<script>var data = {"nextContinuationData":{"continuation":"CBQSCOPL_TOKEN_1234567890"}};</script>"#;
    assert_eq!(
        extract_continuation_from_raw_html(html).as_deref(),
        Some("CBQSCOPL_TOKEN_1234567890")
    );

    // Тот же токен в экранированном виде (data: '...')
    let html = r#"<script>data: '{\x22nextContinuationData\x22:{\x22continuation\x22:\x22CBQSESCAPED_TOKEN_0987654321\x22}}'</script>"#;
    assert_eq!(
        extract_continuation_from_raw_html(html).as_deref(),
        Some("CBQSESCAPED_TOKEN_0987654321")
    );

    // Пробел после двоеточия не мешает
    let html = r#"{"nextContinuationData": {"continuation": "CBQSSPACED_TOKEN_5555555555"}}"#;
    assert_eq!(
        extract_continuation_from_raw_html(html).as_deref(),
        Some("CBQSSPACED_TOKEN_5555555555")
    );

    // Слишком короткое значение — это не токен
    assert_eq!(
        extract_continuation_from_raw_html(r#"{"nextContinuationData":{"continuation":"short"}}"#),
        None
    );

    // Без nextContinuationData токена нет
    assert_eq!(extract_continuation_from_raw_html("<html></html>"), None);
}
