//! Извлечение URL аудиопотока из ответа player-эндпоинта.

/// Извлечение прямого аудиопотока из ответа `/youtubei/v1/player`
pub fn parse_stream_url(json: &serde_json::Value) -> Result<String, String> {
    if let Some(status) = json
        .get("playabilityStatus")
        .and_then(|s| s.get("status"))
        .and_then(|s| s.as_str())
        && status != "OK"
    {
        let reason = json
            .get("playabilityStatus")
            .and_then(|s| s.get("reason"))
            .and_then(|r| r.as_str())
            .unwrap_or(status);
        return Err(format!("Воспроизведение недоступно: {}", reason));
    }

    let empty_vec = Vec::new();
    let streaming_data = json.get("streamingData");
    let adaptive_formats = streaming_data
        .and_then(|sd| sd.get("adaptiveFormats"))
        .and_then(|af| af.as_array())
        .unwrap_or(&empty_vec);

    let formats = streaming_data
        .and_then(|sd| sd.get("formats"))
        .and_then(|f| f.as_array())
        .unwrap_or(&empty_vec);

    struct Candidate<'a> {
        url: &'a str,
        itag: u64,
        mime_type: &'a str,
        bitrate: u64,
    }

    let mut candidates: Vec<Candidate> = Vec::new();
    for f in adaptive_formats.iter().chain(formats.iter()) {
        if let Some(url) = f.get("url").and_then(|u| u.as_str())
            && !url.is_empty()
        {
            let itag = f.get("itag").and_then(|i| i.as_u64()).unwrap_or(0);
            let mime_type = f.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
            let bitrate = f
                .get("bitrate")
                .and_then(|b| b.as_u64())
                .or_else(|| f.get("averageBitrate").and_then(|b| b.as_u64()))
                .unwrap_or(0);
            candidates.push(Candidate {
                url,
                itag,
                mime_type,
                bitrate,
            });
        }
    }

    if candidates.is_empty() {
        return Err("Не найдено прямых аудиопотоков в ответе InnerTube".to_string());
    }

    // 1. Приоритет: Opus 160 kbps (audio/webm; codecs="opus" / itag 251)
    if let Some(c) = candidates
        .iter()
        .filter(|c| {
            c.itag == 251 || (c.mime_type.contains("audio/webm") && c.mime_type.contains("opus"))
        })
        .max_by_key(|c| c.bitrate)
    {
        return Ok(c.url.to_string());
    }

    // 2. Фолбэк: AAC 128 kbps (audio/mp4; codecs="mp4a.40.2" / itag 140)
    if let Some(c) = candidates
        .iter()
        .filter(|c| {
            c.itag == 140 || (c.mime_type.contains("audio/mp4") && c.mime_type.contains("mp4a"))
        })
        .max_by_key(|c| c.bitrate)
    {
        return Ok(c.url.to_string());
    }

    // 3. Фолбэк: любой аудиопоток
    if let Some(c) = candidates
        .iter()
        .filter(|c| c.mime_type.starts_with("audio/"))
        .max_by_key(|c| c.bitrate)
    {
        return Ok(c.url.to_string());
    }

    // 4. Фолбэк: любой формат с максимальным битрейтом
    if let Some(c) = candidates.iter().max_by_key(|c| c.bitrate) {
        return Ok(c.url.to_string());
    }

    Err("Не удалось выбрать подходящий аудиопоток".to_string())
}

/// Длительность трека из ответа `/player` (`videoDetails.lengthSeconds`).
/// Поиск YouTube Music без фильтра «Песни» приходит без длительности в строке результата,
/// а для проверки текста песни она нужна — берём её из того же ответа, что и поток.
pub fn parse_video_length_secs(json: &serde_json::Value) -> Option<u64> {
    let text = |v: Option<&serde_json::Value>| -> Option<String> {
        match v? {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            _ => None,
        }
    };

    text(
        json.get("videoDetails")
            .and_then(|d| d.get("lengthSeconds")),
    )
    .or_else(|| {
        text(
            json.get("microformat")
                .and_then(|m| m.get("playerMicroformatRenderer"))
                .and_then(|m| m.get("lengthSeconds")),
        )
    })
    .and_then(|s| s.trim().parse::<u64>().ok())
    .filter(|s| *s > 0)
}

// ───────────────────────── Видеопоток (клипы на фоне Волны) ─────────────────────────

/// Фактически выбранный видеоформат — для телеметрии `clip_format`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFormatInfo {
    pub itag: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

impl VideoFormatInfo {
    /// Строка для `volna cmd clip status`, например `itag 135 480x854`
    pub fn label(&self) -> String {
        match (self.width, self.height) {
            (Some(w), Some(h)) => format!("itag {} {}x{}", self.itag, w, h),
            _ => format!("itag {}", self.itag),
        }
    }
}

/// Результат выбора видеоформата: URL может отсутствовать (только `signatureCipher`),
/// но метаданные битрейта при этом известны и нужны отсеву арт-треков.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoStreamSelection {
    /// Прямой URL формата, если он есть в ответе InnerTube.
    pub url: Option<String>,
    /// Формат именно того кандидата, чей URL выбран.
    pub format: Option<VideoFormatInfo>,
    /// Первый известный битрейт среди кандидатов в порядке приоритета.
    /// Может относиться не к тому формату, чей URL в итоге выбран.
    pub best_known_bitrate_bps: Option<u64>,
}

/// Ошибка выбора видеоформата — отдельный диагноз, не сетевой таймаут.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoStreamError {
    Playability(String),
    NoH264,
}

struct VideoCandidate {
    url: Option<String>,
    itag: u64,
    width: Option<u32>,
    height: Option<u32>,
    bitrate_bps: Option<u64>,
}

/// Приоритет формата: 480p, 360p, другой H.264 шириной до 480, затем 720p.
fn video_rank(itag: u64, width: Option<u32>) -> Option<u8> {
    match itag {
        135 => Some(0),
        134 => Some(1),
        136 => Some(3),
        _ if width.is_some_and(|w| w <= 480) => Some(2),
        _ => None,
    }
}

/// Выбор video-only H.264 потока для фонового клипа.
///
/// AV1/VP9 и muxed-форматы (`itag 18`, `avc1` + `mp4a`) не рассматриваются:
/// первые плохо декодируются программно на слабых машинах, второй даёт второй звук.
pub fn select_video_stream(
    json: &serde_json::Value,
) -> Result<VideoStreamSelection, VideoStreamError> {
    if let Some(status) = json
        .get("playabilityStatus")
        .and_then(|s| s.get("status"))
        .and_then(|s| s.as_str())
        && status != "OK"
    {
        let reason = json
            .get("playabilityStatus")
            .and_then(|s| s.get("reason"))
            .and_then(|r| r.as_str())
            .unwrap_or(status);
        return Err(VideoStreamError::Playability(reason.to_string()));
    }

    let duration = parse_video_length_secs(json);
    let empty = Vec::new();
    let formats = json
        .get("streamingData")
        .and_then(|sd| sd.get("adaptiveFormats"))
        .and_then(|af| af.as_array())
        .unwrap_or(&empty);

    let mut candidates: Vec<VideoCandidate> = Vec::new();
    for f in formats {
        let mime = f.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
        if !mime.starts_with("video/mp4") || !mime.contains("avc1") || mime.contains("mp4a") {
            continue;
        }
        let itag = f.get("itag").and_then(|i| i.as_u64()).unwrap_or(0);
        if itag == 18 {
            continue;
        }
        let width = f
            .get("width")
            .and_then(|w| w.as_u64())
            .map(|w| w as u32);
        let height = f
            .get("height")
            .and_then(|h| h.as_u64())
            .map(|h| h as u32);
        if video_rank(itag, width).is_none() {
            continue;
        }
        let content_length = f
            .get("contentLength")
            .and_then(|c| c.as_str().and_then(|s| s.trim().parse::<u64>().ok()).or_else(|| c.as_u64()));
        let bitrate_bps = match (content_length, duration) {
            (Some(len), Some(secs)) if secs > 0 => Some(len.saturating_mul(8) / secs),
            _ => None,
        };
        let url = f
            .get("url")
            .and_then(|u| u.as_str())
            .filter(|u| !u.is_empty())
            .map(str::to_string);
        candidates.push(VideoCandidate { url, itag, width, height, bitrate_bps });
    }

    if candidates.is_empty() {
        return Err(VideoStreamError::NoH264);
    }

    let mut order: Vec<usize> = (0..candidates.len()).collect();
    order.sort_by_key(|&i| {
        let c = &candidates[i];
        (video_rank(c.itag, c.width).unwrap_or(u8::MAX), c.itag)
    });

    let best_known_bitrate_bps = order.iter().find_map(|&i| candidates[i].bitrate_bps);

    let chosen = order
        .iter()
        .copied()
        .find(|&i| candidates[i].url.is_some());
    let (url, format) = match chosen {
        Some(i) => {
            let c = &candidates[i];
            (
                c.url.clone(),
                Some(VideoFormatInfo {
                    itag: c.itag.to_string(),
                    width: c.width,
                    height: c.height,
                }),
            )
        }
        None => (None, None),
    };

    Ok(VideoStreamSelection {
        url,
        format,
        best_known_bitrate_bps,
    })
}

#[cfg(test)]
mod video_tests {
    use super::*;
    use serde_json::json;

    fn player(formats: Vec<serde_json::Value>) -> serde_json::Value {
        json!({
            "playabilityStatus": { "status": "OK" },
            "videoDetails": { "lengthSeconds": "200" },
            "streamingData": { "adaptiveFormats": formats }
        })
    }

    fn fmt(itag: u64, width: u32, height: u32, len: u64) -> serde_json::Value {
        json!({
            "itag": itag,
            "mimeType": "video/mp4; codecs=\"avc1.4D401E\"",
            "width": width,
            "height": height,
            "contentLength": len.to_string(),
            "url": format!("https://rr1---sn-test.googlevideo.com/videoplayback?itag={itag}")
        })
    }

    #[test]
    fn prefers_480p_then_360p() {
        let json = player(vec![fmt(134, 640, 360, 8_000_000), fmt(135, 854, 480, 15_000_000)]);
        let sel = select_video_stream(&json).unwrap();
        assert!(sel.url.is_some());
        let format = sel.format.unwrap();
        assert_eq!(format.itag, "135");
        assert_eq!((format.width, format.height), (Some(854), Some(480)));
        // 15 МБ за 200 с ≈ 600 кбит/с
        assert_eq!(sel.best_known_bitrate_bps, Some(600_000));
    }

    #[test]
    fn falls_back_to_360p_and_then_720p() {
        let json = player(vec![fmt(134, 640, 360, 8_000_000)]);
        assert_eq!(select_video_stream(&json).unwrap().format.unwrap().itag, "134");

        let json = player(vec![fmt(136, 1280, 720, 27_000_000)]);
        assert_eq!(select_video_stream(&json).unwrap().format.unwrap().itag, "136");
    }

    #[test]
    fn vertical_480p_is_allowed() {
        let json = player(vec![fmt(135, 480, 854, 8_700_000)]);
        let format = select_video_stream(&json).unwrap().format.unwrap();
        assert_eq!(format.itag, "135");
        assert_eq!(format.label(), "itag 135 480x854");
    }

    #[test]
    fn muxed_and_audio_codecs_are_rejected() {
        let muxed = json!({
            "itag": 18,
            "mimeType": "video/mp4; codecs=\"avc1.42001E, mp4a.40.2\"",
            "width": 640, "height": 360, "url": "https://x/1"
        });
        let with_audio = json!({
            "itag": 135,
            "mimeType": "video/mp4; codecs=\"avc1.4D401E, mp4a.40.2\"",
            "width": 854, "height": 480, "url": "https://x/2"
        });
        assert_eq!(
            select_video_stream(&player(vec![muxed, with_audio])),
            Err(VideoStreamError::NoH264)
        );
    }

    #[test]
    fn av1_and_vp9_are_not_selected() {
        let av1 = json!({
            "itag": 399,
            "mimeType": "video/mp4; codecs=\"av01.0.08M.08\"",
            "width": 1920, "height": 1080, "url": "https://x/3"
        });
        let vp9 = json!({
            "itag": 248,
            "mimeType": "video/webm; codecs=\"vp9\"",
            "width": 1920, "height": 1080, "url": "https://x/4"
        });
        assert_eq!(
            select_video_stream(&player(vec![av1, vp9])),
            Err(VideoStreamError::NoH264)
        );
    }

    #[test]
    fn cipher_only_format_reports_no_url_but_keeps_bitrate() {
        let cipher = json!({
            "itag": 135,
            "mimeType": "video/mp4; codecs=\"avc1.4D401E\"",
            "width": 854, "height": 480,
            "contentLength": "15_000_000".replace('_', ""),
            "signatureCipher": "s=abc&url=https%3A%2F%2Fexample"
        });
        let sel = select_video_stream(&player(vec![cipher])).unwrap();
        assert!(sel.url.is_none());
        assert!(sel.format.is_none());
        assert_eq!(sel.best_known_bitrate_bps, Some(600_000));
    }

    #[test]
    fn bitrate_survives_when_chosen_format_has_no_content_length() {
        let mut no_len = fmt(135, 854, 480, 15_000_000);
        no_len.as_object_mut().unwrap().remove("contentLength");
        let with_len = fmt(134, 640, 360, 4_000_000); // 160 кбит/с
        let sel = select_video_stream(&player(vec![no_len, with_len])).unwrap();
        assert_eq!(sel.format.unwrap().itag, "135");
        assert_eq!(sel.best_known_bitrate_bps, Some(160_000));
    }

    #[test]
    fn playability_error_keeps_reason() {
        let json = json!({
            "playabilityStatus": { "status": "LOGIN_REQUIRED", "reason": "Sign in to confirm you’re not a bot" },
            "streamingData": {}
        });
        match select_video_stream(&json) {
            Err(VideoStreamError::Playability(reason)) => assert!(reason.contains("Sign in")),
            other => panic!("ожидалась Playability, получено {other:?}"),
        }
    }

    #[test]
    fn skips_candidate_without_url_and_takes_next_direct_one() {
        let mut cipher = fmt(135, 854, 480, 15_000_000);
        cipher.as_object_mut().unwrap().remove("url");
        cipher.as_object_mut().unwrap().insert("signatureCipher".into(), json!("s=abc"));
        let sel = select_video_stream(&player(vec![cipher, fmt(134, 640, 360, 8_000_000)])).unwrap();
        assert_eq!(sel.format.unwrap().itag, "134");
        assert!(sel.url.unwrap().contains("itag=134"));
    }
}
