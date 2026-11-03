mod api {
    pub mod models {
        include!("../src/api/models.rs");
    }

    pub mod vk {
        pub mod models {
            include!("../src/api/vk/models.rs");
        }
    }
}

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::api::models::TrackSource;
use crate::api::vk::models::{
    VkApiResponse, VkAudioDto, VkAudioListDto, VkAuthResponse, VkLyricsDto, VkPlaylistDto,
    VkStreamMixAudiosDto, VkStreamMixSettingsDto,
};

fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/vk")
        .join(name);
    std::fs::read_to_string(path).expect("VK fixture must be readable")
}

fn parse_fixture<T: DeserializeOwned>(name: &str) -> T {
    serde_json::from_str(&fixture(name)).expect("VK fixture must match its DTO")
}

#[test]
fn auth_success_is_parsed_and_debug_redacts_token() {
    let parsed: VkAuthResponse = parse_fixture("auth_success.json");
    let VkAuthResponse::Success(success) = parsed else {
        panic!("expected successful OAuth response");
    };

    assert_eq!(success.access_token, "vk1.a.SANITIZED_CONTRACT_FIXTURE");
    assert_eq!(success.expires_in, Some(0));
    assert_eq!(success.user_id, 12_345_678);
    assert_eq!(success.email.as_deref(), Some("listener@example.invalid"));

    let debug = format!("{success:?}");
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains("SANITIZED_CONTRACT_FIXTURE"));
}

#[test]
fn auth_two_factor_supports_sms_app_and_numeric_sid() {
    let parsed: VkAuthResponse = parse_fixture("auth_2fa_required.json");
    let VkAuthResponse::TwoFactorRequired(required) = parsed else {
        panic!("expected two-factor response");
    };
    assert_eq!(required.error, "need_validation");
    assert_eq!(required.validation_type, "2fa_sms");
    assert_eq!(required.phone_mask.as_deref(), Some("+7 *** *** 45 67"));
    assert_eq!(required.validation_sid.as_deref(), Some("987654321"));

    let mut app_response: Value = serde_json::from_str(&fixture("auth_2fa_required.json"))
        .expect("2FA fixture must be valid JSON");
    app_response["validation_type"] = Value::String("2fa_app".to_owned());
    app_response["validation_sid"] = Value::Number(987_654_321.into());
    let parsed: VkAuthResponse = serde_json::from_value(app_response)
        .expect("TOTP response and numeric validation SID must parse");
    let VkAuthResponse::TwoFactorRequired(required) = parsed else {
        panic!("expected app two-factor response");
    };
    assert_eq!(required.validation_type, "2fa_app");
    assert_eq!(required.validation_sid.as_deref(), Some("987654321"));
}

#[test]
fn auth_captcha_contains_sid_and_image() {
    let parsed: VkAuthResponse = parse_fixture("auth_captcha_required.json");
    let VkAuthResponse::CaptchaRequired(required) = parsed else {
        panic!("expected captcha response");
    };

    assert_eq!(required.error, "need_captcha");
    assert_eq!(required.captcha_sid, "1234567890");
    assert_eq!(
        required.captcha_img,
        "https://api.example.invalid/captcha.php?sid=1234567890"
    );
}

#[test]
fn audio_library_maps_mp3_hls_and_unavailable_tracks() {
    let parsed: VkApiResponse<VkAudioListDto> = parse_fixture("audio_get_response.json");
    let VkApiResponse::Response { response } = parsed else {
        panic!("expected audio list response");
    };
    assert_eq!(response.count, 3);
    assert_eq!(response.items.len(), 3);

    let tracks: Vec<_> = response
        .items
        .into_iter()
        .map(VkAudioDto::into_track)
        .collect();

    let mp3 = &tracks[0];
    assert_eq!(mp3.id, "vk:1234567_456240123");
    assert_eq!(mp3.source, TrackSource::Vk);
    assert_eq!(mp3.title, "Direct MP3");
    assert_eq!(mp3.version.as_deref(), Some("Contract Mix"));
    assert_eq!(mp3.duration_ms, Some(184_000));
    assert_eq!(mp3.available, Some(true));
    assert_eq!(mp3.artists_string(), "Example Artist, Second Artist");
    assert_eq!(mp3.artists[1].id, "1002");
    assert!(mp3.is_explicit());
    assert_eq!(mp3.albums.len(), 1);
    assert_eq!(mp3.albums[0].id, "vk:1234567_123456");
    assert_eq!(
        mp3.cover_url(600).as_deref(),
        Some("https://images.example.invalid/cover-600.jpg")
    );

    let hls = &tracks[1];
    assert_eq!(hls.id, "vk:1234567_456240124");
    assert_eq!(hls.available, Some(true));
    assert_eq!(hls.artists_string(), "HLS Artist");
    assert!(hls.albums.is_empty());
    assert_eq!(hls.duration_ms, Some(201_000));

    let unavailable = &tracks[2];
    assert_eq!(unavailable.id, "vk:-2000001_456240125");
    assert_eq!(unavailable.available, Some(false));
    assert_eq!(unavailable.artists_string(), "Unavailable Artist");
    assert!(unavailable.albums.is_empty());
}

#[test]
fn audio_get_by_id_batch_preserves_order_and_refreshed_urls() {
    let parsed: VkApiResponse<Vec<VkAudioDto>> = parse_fixture("audio_get_by_id_batch.json");
    let VkApiResponse::Response { response } = parsed else {
        panic!("expected batch audio response");
    };

    assert_eq!(response.len(), 2);
    assert_eq!(response[0].track_id(), "vk:1234567_456240123");
    assert_eq!(
        response[0].stream_url(),
        Some("https://audio.example.invalid/refreshed/direct.mp3")
    );
    assert_eq!(response[1].track_id(), "vk:1234567_456240124");
    assert_eq!(
        response[1].stream_url(),
        Some("https://audio.example.invalid/refreshed/index.m3u8")
    );
}

#[test]
fn stream_mix_settings_expose_server_category_and_option_ids() {
    let parsed: VkApiResponse<VkStreamMixSettingsDto> = parse_fixture("stream_mix_settings.json");
    let VkApiResponse::Response { response } = parsed else {
        panic!("expected stream mix settings response");
    };
    assert_eq!(response.categories.len(), 4);

    let mood = response
        .categories
        .iter()
        .find(|category| category.category_id == "mood")
        .expect("mood category must exist");
    assert!(
        mood.options
            .iter()
            .any(|option| option.option_id == "active")
    );

    let language = response
        .categories
        .iter()
        .find(|category| category.category_id == "language")
        .expect("language category must exist");
    assert!(
        language
            .options
            .iter()
            .any(|option| option.option_id == "without-words")
    );
}

#[test]
fn stream_mix_batch_preserves_opaque_context() {
    let parsed: VkApiResponse<VkStreamMixAudiosDto> = parse_fixture("stream_mix_audios.json");
    let VkApiResponse::Response { response } = parsed else {
        panic!("expected stream mix audios response");
    };

    assert_eq!(response.items.len(), 2);
    assert_eq!(
        response.context.as_deref(),
        Some("eyJjb250cmFjdF9maXh0dXJlIjp0cnVlfQ==")
    );
    assert_eq!(response.items[0].track_id(), "vk:1234567_456240200");
    assert_eq!(response.items[1].track_id(), "vk:1234567_456240201");
}

#[test]
fn lyrics_timestamps_keep_milliseconds_and_line_order() {
    let parsed: VkApiResponse<VkLyricsDto> = parse_fixture("lyrics_timestamps.json");
    let VkApiResponse::Response { response } = parsed else {
        panic!("expected lyrics response");
    };

    assert_eq!(response.timestamps.len(), 2);
    assert_eq!(response.timestamps[0].begin, 12_400);
    assert_eq!(response.timestamps[0].end, Some(15_800));
    assert_eq!(response.timestamps[0].line, "Первая строка песни");
    assert_eq!(response.timestamps[1].begin, 16_000);
    assert_eq!(response.timestamps[1].line, "Вторая строка песни");
}

#[test]
fn api_error_supports_auth_revocation_and_captcha() {
    let revoked: VkApiResponse<VkAudioListDto> = serde_json::from_str(
        r#"{"error":{"error_code":5,"error_msg":"User authorization failed"}}"#,
    )
    .expect("auth revocation must parse");
    let VkApiResponse::Error { error } = revoked else {
        panic!("expected API error");
    };
    assert_eq!(error.error_code, 5);
    assert_eq!(error.captcha_sid, None);

    let captcha: VkApiResponse<VkAudioListDto> = serde_json::from_str(
        r#"{"error":{"error_code":14,"error_msg":"Captcha needed","captcha_sid":1234567890,"captcha_img":"https://api.example.invalid/captcha.jpg"}}"#,
    )
    .expect("API captcha must parse");
    let VkApiResponse::Error { error } = captcha else {
        panic!("expected captcha API error");
    };
    assert_eq!(error.error_code, 14);
    assert_eq!(error.captcha_sid.as_deref(), Some("1234567890"));
    assert_eq!(
        error.captcha_img.as_deref(),
        Some("https://api.example.invalid/captcha.jpg")
    );
}

#[test]
fn playlist_is_a_separate_dto_and_supports_cover_fallback() {
    let playlist: VkPlaylistDto = serde_json::from_str(
        r#"{
            "id": 77,
            "owner_id": 1234567,
            "title": "Contract Playlist",
            "count": 12,
            "photo": {"photo_300": "https://images.example.invalid/playlist-300.jpg"}
        }"#,
    )
    .expect("playlist DTO must parse");

    assert_eq!(playlist.id, 77);
    assert_eq!(playlist.owner_id, 1_234_567);
    assert_eq!(playlist.count, Some(12));
    assert_eq!(
        playlist.photo.as_ref().and_then(|photo| photo.best_url()),
        Some("https://images.example.invalid/playlist-300.jpg")
    );
}
