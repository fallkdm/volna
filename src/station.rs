//! Яндекс Станция в локальной сети: поиск по mDNS, токен Glagol, защищённое WebSocket-соединение
//! и команды управления (воспроизвести трек, пауза, громкость).
//!
//! Сертификат станции самоподписанный; приложение принимает только тот, что Яндекс отдаёт в списке устройств.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerifier};
use rustls::pki_types::{pem::PemObject, CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::api::client::YandexMusicClient;

const SERVICE: &str = "_yandexio._tcp.local.";

/// Станция, найденная в сети и сопоставленная с устройством аккаунта
#[derive(Debug, Clone)]
pub struct StationDevice {
    pub device_id: String,
    pub platform: String,
    pub name: String,
    pub addr: SocketAddr,
    /// Сертификат станции (PEM) из списка устройств Яндекса
    pub cert_pem: String,
}

/// Ищет станции аккаунта в локальной сети (до `wait` секунд)
pub async fn find_stations(client: &YandexMusicClient, wait: Duration) -> Result<Vec<StationDevice>, String> {
    let accounts = client.glagol_devices().await.map_err(|e| e.to_string())?;
    if accounts.is_empty() {
        return Ok(Vec::new());
    }
    let found = tokio::task::spawn_blocking(move || discover(wait)).await.map_err(|e| e.to_string())??;
    Ok(accounts
        .into_iter()
        .filter_map(|a| {
            let addr = found.iter().find(|(id, _)| *id == a.id)?.1;
            Some(StationDevice { device_id: a.id, platform: a.platform, name: a.name, addr, cert_pem: a.cert_pem })
        })
        .collect())
}

/// mDNS: `deviceId` → адрес. Блокирует поток на время поиска.
fn discover(wait: Duration) -> Result<Vec<(String, SocketAddr)>, String> {
    let daemon = mdns_sd::ServiceDaemon::new().map_err(|e| e.to_string())?;
    let rx = daemon.browse(SERVICE).map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + wait;
    let mut out: Vec<(String, SocketAddr)> = Vec::new();
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        let Ok(event) = rx.recv_timeout(left) else { break };
        if let mdns_sd::ServiceEvent::ServiceResolved(info) = event {
            let id = info.get_property_val_str("deviceId").unwrap_or_default().to_string();
            let ip = info.get_addresses_v4().into_iter().next().map(|a| IpAddr::V4(a));
            if let (false, Some(ip)) = (id.is_empty(), ip) {
                if !out.iter().any(|(i, _)| *i == id) {
                    out.push((id, SocketAddr::new(ip, info.get_port())));
                }
            }
        }
    }
    let _ = daemon.shutdown();
    Ok(out)
}

/// Принимает единственный сертификат: тот, что станция получила от Яндекса
#[derive(Debug)]
struct PinnedCert {
    der: CertificateDer<'static>,
    algs: rustls::crypto::WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for PinnedCert {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if end_entity.as_ref() == self.der.as_ref() {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("сертификат станции не совпал с ожидаемым".into()))
        }
    }

    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algs)
    }

    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

/// Состояние плеера станции из сообщений `state`
#[derive(Debug, Clone, Default)]
pub struct StationState {
    pub playing: bool,
    /// Громкость 0..1
    pub volume: f64,
    pub title: String,
    pub subtitle: String,
    pub duration: f64,
    pub progress: f64,
}

impl StationState {
    fn from_json(v: &Value) -> Option<Self> {
        let s = v.get("state")?;
        let p = &s["playerState"];
        Some(Self {
            playing: s["playing"].as_bool().unwrap_or(false),
            volume: s["volume"].as_f64().unwrap_or(0.0),
            title: p["title"].as_str().unwrap_or("").to_string(),
            subtitle: p["subtitle"].as_str().unwrap_or("").to_string(),
            duration: p["duration"].as_f64().unwrap_or(0.0),
            progress: p["progress"].as_f64().unwrap_or(0.0),
        })
    }
}

/// Живое соединение со станцией. Команды уходят через канал, состояние приходит в `states`.
pub struct StationSession {
    pub device: StationDevice,
    tx: mpsc::UnboundedSender<Value>,
}

impl StationSession {
    /// Подключается к станции и возвращает сессию и поток обновлений состояния
    pub async fn connect(client: &YandexMusicClient, device: StationDevice) -> Result<(Self, mpsc::UnboundedReceiver<StationState>), String> {
        let token = client.glagol_token(&device.device_id, &device.platform).await.map_err(|e| e.to_string())?;
        let der = CertificateDer::from_pem_slice(device.cert_pem.as_bytes()).map_err(|e| format!("сертификат станции: {e}"))?;
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = Arc::new(PinnedCert { der, algs: provider.signature_verification_algorithms });
        let config = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth();

        let tcp = tokio::time::timeout(Duration::from_secs(5), tokio::net::TcpStream::connect(device.addr))
            .await
            .map_err(|_| "станция не отвечает".to_string())?
            .map_err(|e| e.to_string())?;
        let tls = tokio_rustls::TlsConnector::from(Arc::new(config))
            .connect(ServerName::try_from("localhost").map_err(|e| e.to_string())?, tcp)
            .await
            .map_err(|e| format!("TLS: {e}"))?;
        let (ws, _) = tokio_tungstenite::client_async(format!("wss://{}", device.addr), tls).await.map_err(|e| format!("WebSocket: {e}"))?;
        let (mut sink, mut stream) = ws.split();

        let (tx, mut cmd_rx) = mpsc::unbounded_channel::<Value>();
        let (state_tx, state_rx) = mpsc::unbounded_channel();

        // запись: команды и периодический ping, чтобы станция не закрыла соединение
        tokio::spawn(async move {
            // станция присылает состояние только в ответ на запрос: опрашиваем раз в секунду
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {
                    cmd = cmd_rx.recv() => {
                        let Some(payload) = cmd else { break };
                        let msg = json!({
                            "conversationToken": token,
                            "id": uuid::Uuid::new_v4().to_string(),
                            "sentTime": chrono::Utc::now().timestamp_millis(),
                            "payload": payload,
                        });
                        if sink.send(Message::text(msg.to_string())).await.is_err() { break }
                    }
                    _ = tick.tick() => {
                        let msg = json!({
                            "conversationToken": token,
                            "id": uuid::Uuid::new_v4().to_string(),
                            "sentTime": chrono::Utc::now().timestamp_millis(),
                            "payload": {"command": "ping"},
                        });
                        if sink.send(Message::text(msg.to_string())).await.is_err() { break }
                    }
                }
            }
        });
        // чтение: состояние плеера; обрыв закрывает канал состояний
        tokio::spawn(async move {
            while let Some(Ok(msg)) = stream.next().await {
                if let Message::Text(t) = msg {
                    if let Some(st) = serde_json::from_str::<Value>(&t).ok().and_then(|v| StationState::from_json(&v)) {
                        if state_tx.send(st).is_err() { break }
                    }
                }
            }
        });

        let session = Self { device, tx };
        session.command(json!({"command": "ping"}));
        Ok((session, state_rx))
    }

    pub fn command(&self, payload: Value) {
        let _ = self.tx.send(payload);
    }

    /// Включает трек на станции
    pub fn play_track(&self, track_id: &str) {
        self.command(json!({"command": "playMusic", "id": track_id, "type": "track"}));
    }

    pub fn play(&self) {
        self.command(json!({"command": "play"}));
    }

    pub fn pause(&self) {
        self.command(json!({"command": "stop"}));
    }

    pub fn next(&self) {
        self.command(json!({"command": "next"}));
    }

    pub fn prev(&self) {
        self.command(json!({"command": "prev"}));
    }

    /// Громкость 0..1
    pub fn set_volume(&self, volume: f64) {
        self.command(json!({"command": "setVolume", "volume": volume.clamp(0.0, 1.0)}));
    }

    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_is_parsed_from_message() {
        let v = json!({"state": {"playing": true, "volume": 0.4, "playerState": {"title": "T", "subtitle": "S", "duration": 100.0, "progress": 12.5}}});
        let s = StationState::from_json(&v).unwrap();
        assert!(s.playing && s.title == "T" && s.subtitle == "S" && (s.volume - 0.4).abs() < 1e-9 && s.progress == 12.5);
        assert!(StationState::from_json(&json!({"error": "x"})).is_none());
    }

    /// Реальная станция в сети: `cargo test station_live -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn station_live() {
        let token = crate::api::auth::load_token().expect("нет токена");
        let client = YandexMusicClient::new(Some(token));
        let stations = find_stations(&client, Duration::from_secs(4)).await.unwrap();
        eprintln!("станции: {:?}", stations.iter().map(|s| (&s.name, s.addr)).collect::<Vec<_>>());
        let dev = stations.into_iter().next().expect("станция не найдена");
        let (_session, mut rx) = StationSession::connect(&client, dev).await.unwrap();
        let st = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await.expect("нет состояния").expect("закрыто");
        eprintln!("состояние: {:?}", st);
    }

    /// Команды доходят до станции: громкость меняется на один шаг (10%) и возвращается
    #[tokio::test]
    #[ignore]
    async fn station_volume_roundtrip() {
        let token = crate::api::auth::load_token().expect("нет токена");
        let client = YandexMusicClient::new(Some(token));
        let dev = find_stations(&client, Duration::from_secs(4)).await.unwrap().into_iter().next().expect("станция не найдена");
        let (session, mut rx) = StationSession::connect(&client, dev).await.unwrap();
        let first = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await.unwrap().unwrap();
        let target = if first.volume > 0.5 { first.volume - 0.1 } else { first.volume + 0.1 };
        session.set_volume(target);
        let mut seen = None;
        for _ in 0..10 {
            let Ok(Some(st)) = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await else { break };
            if (st.volume - target).abs() < 0.02 {
                seen = Some(st.volume);
                break;
            }
        }
        session.set_volume(first.volume);
        eprintln!("было {} → поставили {} → станция сообщила {:?}", first.volume, target, seen);
        assert!(seen.is_some(), "станция не приняла команду громкости");
    }
}
