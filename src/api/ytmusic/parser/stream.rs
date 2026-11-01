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
