use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use shared_types::SignalMessage;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::Message;

type WsTx = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;

/// Max queued audio frames before dropping oldest. At 50fps, 200 = ~4 seconds.
/// Prevents unbounded memory growth if consumer falls behind.
const AUDIO_QUEUE_CAPACITY: usize = 200;
const SCREEN_QUEUE_CAPACITY: usize = 2;

pub struct NetworkClient {
    ws_tx: Arc<Mutex<Option<WsTx>>>,
    signal_rx: mpsc::UnboundedReceiver<SignalMessage>,
    /// Raw binary audio frames — bounded channel to prevent OOM on slow consumers.
    audio_rx: mpsc::Receiver<Vec<u8>>,
    /// Raw binary screen frames — bounded to keep only near-latest data.
    screen_rx: mpsc::Receiver<Vec<u8>>,
    signal_tx_internal: mpsc::UnboundedSender<SignalMessage>,
    audio_tx_internal: mpsc::Sender<Vec<u8>>,
    screen_tx_internal: mpsc::Sender<Vec<u8>>,
    connected: Arc<std::sync::atomic::AtomicBool>,
    server_url: Arc<Mutex<Option<String>>>,
    /// Last measured round-trip time in milliseconds (-1 = unknown).
    last_ping_ms: Arc<std::sync::atomic::AtomicI32>,
    /// Timestamp of last sent ping (for measuring RTT).
    ping_sent_at: Arc<Mutex<Option<std::time::Instant>>>,
    /// UDP socket for audio transport (None = not available, use WebSocket).
    udp_socket: Arc<Mutex<Option<Arc<tokio::net::UdpSocket>>>>,
    /// Whether UDP transport is active and ready for sending.
    udp_active: Arc<std::sync::atomic::AtomicBool>,
    /// UDP session token (8 bytes, set by server UdpReady response).
    udp_token: Arc<Mutex<Option<[u8; 8]>>>,
}

impl Default for NetworkClient {
    fn default() -> Self {
        Self::new()
    }
}

impl NetworkClient {
    pub fn new() -> Self {
        let (sig_tx, sig_rx) = mpsc::unbounded_channel();
        let (audio_tx, audio_rx) = mpsc::channel(AUDIO_QUEUE_CAPACITY);
        let (screen_tx, screen_rx) = mpsc::channel(SCREEN_QUEUE_CAPACITY);
        Self {
            ws_tx: Arc::new(Mutex::new(None)),
            signal_rx: sig_rx,
            audio_rx,
            screen_rx,
            signal_tx_internal: sig_tx,
            audio_tx_internal: audio_tx,
            screen_tx_internal: screen_tx,
            connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            server_url: Arc::new(Mutex::new(None)),
            last_ping_ms: Arc::new(std::sync::atomic::AtomicI32::new(-1)),
            ping_sent_at: Arc::new(Mutex::new(None)),
            udp_socket: Arc::new(Mutex::new(None)),
            udp_active: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            udp_token: Arc::new(Mutex::new(None)),
        }
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub async fn connect(&mut self, server_url: &str) -> Result<()> {
        *self.server_url.lock().await = Some(server_url.to_string());
        self.connect_internal(server_url).await
    }

    async fn connect_internal(&mut self, server_url: &str) -> Result<()> {
        let (ws, _) = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio_tungstenite::connect_async(server_url),
        )
        .await
        .map_err(|_| anyhow::anyhow!("Connection timed out (5s)"))?
        .context("Failed to connect to signaling server")?;

        let (tx, mut rx) = ws.split();
        *self.ws_tx.lock().await = Some(tx);
        self.connected
            .store(true, std::sync::atomic::Ordering::Relaxed);

        log::info!("Connected to signaling server at {server_url}");

        let sig_tx = self.signal_tx_internal.clone();
        let audio_tx = self.audio_tx_internal.clone();
        let screen_tx = self.screen_tx_internal.clone();
        let connected = self.connected.clone();
        let last_ping_ms_rx = self.last_ping_ms.clone();
        let ping_sent_at_rx = self.ping_sent_at.clone();

        tokio::spawn(async move {
            while let Some(msg) = rx.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(signal) = serde_json::from_str::<SignalMessage>(&text) {
                            let _ = sig_tx.send(signal);
                        }
                    }
                    Ok(Message::Binary(data)) => {
                        // Validate header, then pass media through bounded channels.
                        // try_send drops frames when consumers fall behind, preventing OOM.
                        if data.len() >= 3 {
                            match data[0] {
                                shared_types::MEDIA_PACKET_AUDIO => {
                                    let id_len = data[1] as usize;
                                    if data.len() > 2 + id_len {
                                        let _ = audio_tx.try_send(data.into());
                                    }
                                }
                                shared_types::MEDIA_PACKET_SCREEN => {
                                    let id_len = data[1] as usize;
                                    if data.len() > 2 + id_len {
                                        let _ = screen_tx.try_send(data.into());
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Ok(Message::Ping(_)) => {} // server keepalive
                    Ok(Message::Pong(_)) => {
                        // Measure RTT from our last sent Ping
                        if let Some(sent) = ping_sent_at_rx.lock().await.take() {
                            let rtt = sent.elapsed().as_millis() as i32;
                            last_ping_ms_rx.store(rtt, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    Ok(Message::Close(_)) => break,
                    Err(e) => {
                        log::warn!("WebSocket error: {e}");
                        break;
                    }
                    _ => {}
                }
            }
            connected.store(false, std::sync::atomic::Ordering::Relaxed);
            log::info!("Disconnected from signaling server");
        });

        Ok(())
    }

    pub async fn try_reconnect(&mut self) -> Result<bool> {
        // Clear stale UDP state before reconnecting — old token is invalid
        self.udp_active
            .store(false, std::sync::atomic::Ordering::Release);
        *self.udp_socket.lock().await = None;
        *self.udp_token.lock().await = None;

        let url = self.server_url.lock().await.clone();
        match url {
            Some(u) => {
                self.connect_internal(&u).await?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    pub async fn send_signal(&self, msg: &SignalMessage) -> Result<()> {
        let json = serde_json::to_string(msg)?;
        if let Some(tx) = self.ws_tx.lock().await.as_mut() {
            tx.send(Message::Text(json.into())).await?;
        } else {
            anyhow::bail!("Not connected to server");
        }
        Ok(())
    }

    pub async fn send_audio(&self, data: &[u8]) -> Result<()> {
        // Prefer UDP when available — lower latency, no head-of-line blocking
        if self.udp_active.load(std::sync::atomic::Ordering::Acquire) {
            if let Some(ref token) = *self.udp_token.lock().await {
                if let Some(ref socket) = *self.udp_socket.lock().await {
                    // UDP packet: [token(8)][MEDIA_PACKET_AUDIO(1)][opus_data]
                    // Use stack buffer for common case (most Opus frames < 500 bytes)
                    let pkt_len = 8 + 1 + data.len();
                    if pkt_len <= 512 {
                        let mut buf = [0u8; 512];
                        buf[..8].copy_from_slice(token);
                        buf[8] = shared_types::MEDIA_PACKET_AUDIO;
                        buf[9..pkt_len].copy_from_slice(data);
                        let _ = socket.send(&buf[..pkt_len]).await;
                    } else {
                        let mut packet = Vec::with_capacity(pkt_len);
                        packet.extend_from_slice(token);
                        packet.push(shared_types::MEDIA_PACKET_AUDIO);
                        packet.extend_from_slice(data);
                        let _ = socket.send(&packet).await;
                    }
                    return Ok(());
                }
            }
        }

        // Fallback: WebSocket
        if let Some(tx) = self.ws_tx.lock().await.as_mut() {
            let pkt_len = 1 + data.len();
            let packet = if pkt_len <= 512 {
                let mut buf = vec![0u8; pkt_len];
                buf[0] = shared_types::MEDIA_PACKET_AUDIO;
                buf[1..].copy_from_slice(data);
                buf
            } else {
                let mut p = Vec::with_capacity(pkt_len);
                p.push(shared_types::MEDIA_PACKET_AUDIO);
                p.extend_from_slice(data);
                p
            };
            tx.send(Message::Binary(packet.into())).await?;
        }
        Ok(())
    }

    pub async fn send_screen_frame(&self, data: &[u8]) -> Result<()> {
        if let Some(tx) = self.ws_tx.lock().await.as_mut() {
            let mut packet = Vec::with_capacity(data.len() + 1);
            packet.push(shared_types::MEDIA_PACKET_SCREEN);
            packet.extend_from_slice(data);
            tx.send(Message::Binary(packet.into())).await?;
        }
        Ok(())
    }

    pub fn try_recv_signal(&mut self) -> Option<SignalMessage> {
        self.signal_rx.try_recv().ok()
    }

    /// Receive a raw binary audio frame. Call `parse_audio_frame()` to extract
    /// sender_id and audio data as zero-copy slices.
    pub fn try_recv_audio(&mut self) -> Option<Vec<u8>> {
        self.audio_rx.try_recv().ok()
    }

    pub fn try_recv_screen_frame(&mut self) -> Option<Vec<u8>> {
        self.screen_rx.try_recv().ok()
    }

    /// Send a WebSocket Ping to measure latency. Call `ping_ms()` to read the result.
    pub async fn send_ping(&self) {
        if let Some(tx) = self.ws_tx.lock().await.as_mut() {
            *self.ping_sent_at.lock().await = Some(std::time::Instant::now());
            let _ = tx.send(Message::Ping(vec![].into())).await;
        }
    }

    /// Last measured round-trip time in milliseconds, or -1 if unknown.
    pub fn ping_ms(&self) -> i32 {
        self.last_ping_ms.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Set up UDP transport after receiving UdpReady from server.
    /// Connects a UDP socket to the server and sends a hello packet with the session token.
    pub async fn setup_udp(&self, token_hex: &str, udp_port: u16) -> Result<()> {
        // Parse token from hex
        let token = hex_decode_8(token_hex)
            .ok_or_else(|| anyhow::anyhow!("Invalid UDP token hex: {token_hex}"))?;

        // Derive server host from the WebSocket URL
        let server_host = {
            let url = self.server_url.lock().await;
            let url = url.as_ref().context("Not connected")?;
            // Extract host from ws://host:port or wss://host:port
            let without_proto = url
                .strip_prefix("wss://")
                .or_else(|| url.strip_prefix("ws://"))
                .unwrap_or(url);
            let host = without_proto.split(':').next().unwrap_or("127.0.0.1");
            host.to_string()
        };

        let udp_target = format!("{server_host}:{udp_port}");
        let socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
        socket.connect(&udp_target).await?;

        // Send hello packet (just the token) to register our address with the server
        socket.send(&token).await?;

        let socket = Arc::new(socket);

        // Start UDP receive task — pushes incoming audio to the same bounded channel
        let audio_tx = self.audio_tx_internal.clone();
        let recv_socket = socket.clone();
        let udp_active = self.udp_active.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 2 + 256 + shared_types::MAX_AUDIO_FRAME_SIZE];
            loop {
                match recv_socket.recv(&mut buf).await {
                    Ok(len) if len >= 3 => {
                        // Same frame format as WebSocket: [type][id_len][sender_id][audio_data]
                        if buf[0] == shared_types::MEDIA_PACKET_AUDIO {
                            let id_len = buf[1] as usize;
                            if len > 2 + id_len {
                                let _ = audio_tx.try_send(buf[..len].to_vec());
                            }
                        }
                    }
                    Ok(_) => {} // Too short or keepalive response, ignore
                    Err(e) => {
                        log::warn!("UDP recv error: {e}");
                        break;
                    }
                }
            }
            udp_active.store(false, std::sync::atomic::Ordering::Relaxed);
            log::info!("UDP receive task ended");
        });

        // Store socket and token BEFORE setting udp_active and spawning tasks
        // that read them, to prevent races where send_audio() or keepalive sees
        // udp_active=true but socket/token are still None.
        *self.udp_socket.lock().await = Some(socket.clone());
        *self.udp_token.lock().await = Some(token);
        self.udp_active
            .store(true, std::sync::atomic::Ordering::Release);

        // Start UDP keepalive task — prevents NAT mapping from expiring
        let keepalive_socket = socket;
        let keepalive_active = self.udp_active.clone();
        tokio::spawn(async move {
            let interval = std::time::Duration::from_secs(shared_types::UDP_KEEPALIVE_INTERVAL_SECS);
            let keepalive_packet = [token[0], token[1], token[2], token[3],
                                    token[4], token[5], token[6], token[7],
                                    shared_types::UDP_KEEPALIVE];
            loop {
                tokio::time::sleep(interval).await;
                if !keepalive_active.load(std::sync::atomic::Ordering::Acquire) {
                    break;
                }
                if keepalive_socket.send(&keepalive_packet).await.is_err() {
                    break;
                }
            }
        });

        log::info!("UDP audio transport active → {udp_target}");

        Ok(())
    }

    /// Request UDP transport from the server (sends RequestUdp signal).
    pub async fn request_udp(&self) -> Result<()> {
        self.send_signal(&SignalMessage::RequestUdp).await
    }

    /// Whether UDP transport is currently active.
    pub fn is_udp_active(&self) -> bool {
        self.udp_active
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub async fn disconnect(&mut self) {
        // Shut down UDP
        self.udp_active
            .store(false, std::sync::atomic::Ordering::Relaxed);
        *self.udp_socket.lock().await = None;
        *self.udp_token.lock().await = None;

        if let Some(mut tx) = self.ws_tx.lock().await.take() {
            let _ = tx.close().await;
        }
        self.connected
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Parse a raw binary audio frame into (sender_id, audio_data) slices.
/// Zero-allocation: returns references into the original buffer.
#[inline]
pub fn parse_audio_frame(raw: &[u8]) -> Option<(&str, &[u8])> {
    if raw.len() < 3 || raw[0] != shared_types::MEDIA_PACKET_AUDIO {
        return None;
    }
    let id_len = raw[1] as usize;
    if raw.len() <= 2 + id_len {
        return None;
    }
    let sender_id = std::str::from_utf8(&raw[2..2 + id_len]).ok()?;
    let audio_data = &raw[2 + id_len..];
    Some((sender_id, audio_data))
}

#[inline]
pub fn parse_screen_frame(raw: &[u8]) -> Option<(&str, &[u8])> {
    if raw.len() < 3 || raw[0] != shared_types::MEDIA_PACKET_SCREEN {
        return None;
    }
    let id_len = raw[1] as usize;
    if raw.len() <= 2 + id_len {
        return None;
    }
    let sender_id = std::str::from_utf8(&raw[2..2 + id_len]).ok()?;
    let frame_data = &raw[2 + id_len..];
    Some((sender_id, frame_data))
}

fn hex_decode_8(hex: &str) -> Option<[u8; 8]> {
    if hex.len() != 16 {
        return None;
    }
    let mut out = [0u8; 8];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_decode_8_valid() {
        let result = hex_decode_8("0123456789abcdef");
        assert_eq!(result, Some([0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef]));
    }

    #[test]
    fn hex_decode_8_invalid_length() {
        assert_eq!(hex_decode_8("0123"), None);
        assert_eq!(hex_decode_8("0123456789abcdef00"), None);
        assert_eq!(hex_decode_8(""), None);
    }

    #[test]
    fn hex_decode_8_invalid_chars() {
        assert_eq!(hex_decode_8("zzzzzzzzzzzzzzzz"), None);
    }

    #[test]
    fn parse_audio_frame_valid() {
        let mut frame = vec![shared_types::MEDIA_PACKET_AUDIO, 2]; // type, id_len=2
        frame.extend_from_slice(b"p1");
        frame.extend_from_slice(&[0xAA, 0xBB]);
        let (sender, data) = parse_audio_frame(&frame).unwrap();
        assert_eq!(sender, "p1");
        assert_eq!(data, &[0xAA, 0xBB]);
    }

    #[test]
    fn udp_active_default_false() {
        let client = NetworkClient::new();
        assert!(!client.is_udp_active());
    }

    #[test]
    fn parse_audio_frame_too_short() {
        assert!(parse_audio_frame(&[]).is_none());
        assert!(parse_audio_frame(&[0x01]).is_none());
        assert!(parse_audio_frame(&[0x01, 0x02]).is_none());
    }

    #[test]
    fn parse_audio_frame_wrong_type() {
        let frame = vec![0xFF, 2, b'p', b'1', 0xAA];
        assert!(parse_audio_frame(&frame).is_none());
    }

    #[test]
    fn parse_audio_frame_id_len_exceeds_data() {
        // id_len says 10 but only 2 bytes of id follow
        let frame = vec![shared_types::MEDIA_PACKET_AUDIO, 10, b'p', b'1'];
        assert!(parse_audio_frame(&frame).is_none());
    }

    #[test]
    fn parse_audio_frame_empty_audio_data() {
        // Valid header but audio data is exactly 0 bytes
        let frame = vec![shared_types::MEDIA_PACKET_AUDIO, 2, b'p', b'1'];
        // id_len=2, total len=4, 2+id_len=4, so len <= 2+id_len → None
        assert!(parse_audio_frame(&frame).is_none());
    }

    #[test]
    fn parse_audio_frame_single_byte_audio() {
        let frame = vec![shared_types::MEDIA_PACKET_AUDIO, 2, b'p', b'1', 0xFF];
        let (sender, data) = parse_audio_frame(&frame).unwrap();
        assert_eq!(sender, "p1");
        assert_eq!(data, &[0xFF]);
    }

    #[test]
    fn parse_screen_frame_valid() {
        let mut frame = vec![shared_types::MEDIA_PACKET_SCREEN, 3];
        frame.extend_from_slice(b"abc");
        frame.extend_from_slice(&[0x01, 0x02, 0x03]);
        let (sender, data) = parse_screen_frame(&frame).unwrap();
        assert_eq!(sender, "abc");
        assert_eq!(data, &[0x01, 0x02, 0x03]);
    }

    #[test]
    fn parse_screen_frame_invalid() {
        assert!(parse_screen_frame(&[]).is_none());
        assert!(parse_screen_frame(&[shared_types::MEDIA_PACKET_AUDIO, 1, b'x', 0xFF]).is_none());
    }

    #[test]
    fn hex_decode_8_uppercase() {
        let result = hex_decode_8("0123456789ABCDEF");
        assert_eq!(result, Some([0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF]));
    }

    #[test]
    fn network_client_default_state() {
        let client = NetworkClient::new();
        assert!(!client.is_connected());
        assert!(!client.is_udp_active());
        assert_eq!(client.ping_ms(), -1);
    }

    // ─── Connection lifecycle tests ───

    #[tokio::test]
    async fn connect_to_invalid_server_fails() {
        let mut client = NetworkClient::new();
        let result = client.connect("ws://127.0.0.1:1").await;
        assert!(result.is_err(), "Connecting to invalid server should fail");
        assert!(!client.is_connected());
    }

    #[tokio::test]
    async fn try_reconnect_without_prior_connect() {
        let mut client = NetworkClient::new();
        // No prior connect means no stored URL
        let result = client.try_reconnect().await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false, "Should return false when no URL stored");
    }

    #[tokio::test]
    async fn disconnect_when_not_connected() {
        let mut client = NetworkClient::new();
        // Should not panic
        client.disconnect().await;
        assert!(!client.is_connected());
        assert!(!client.is_udp_active());
    }

    #[tokio::test]
    async fn send_signal_when_not_connected() {
        let client = NetworkClient::new();
        let msg = shared_types::SignalMessage::LeaveRoom;
        let result = client.send_signal(&msg).await;
        assert!(result.is_err(), "Sending when not connected should fail");
    }

    #[tokio::test]
    async fn send_audio_when_not_connected() {
        let client = NetworkClient::new();
        // Should not panic, just silently succeed (no-op)
        let result = client.send_audio(&[0xAA, 0xBB]).await;
        assert!(result.is_ok(), "send_audio with no connection should not error");
    }

    #[test]
    fn try_recv_signal_empty() {
        let mut client = NetworkClient::new();
        assert!(client.try_recv_signal().is_none());
    }

    #[test]
    fn try_recv_audio_empty() {
        let mut client = NetworkClient::new();
        assert!(client.try_recv_audio().is_none());
    }

    #[test]
    fn try_recv_screen_empty() {
        let mut client = NetworkClient::new();
        assert!(client.try_recv_screen_frame().is_none());
    }

    // ─── Frame parsing edge cases ───

    #[test]
    fn parse_audio_frame_max_id_len() {
        // id_len=255 (max u8), but data is short
        let frame = vec![shared_types::MEDIA_PACKET_AUDIO, 255, b'x'];
        assert!(parse_audio_frame(&frame).is_none());
    }

    #[test]
    fn parse_audio_frame_zero_id_len() {
        // id_len=0 means sender_id is empty string
        let frame = vec![shared_types::MEDIA_PACKET_AUDIO, 0, 0xAA];
        let result = parse_audio_frame(&frame);
        assert!(result.is_some());
        let (sender, data) = result.unwrap();
        assert_eq!(sender, "");
        assert_eq!(data, &[0xAA]);
    }

    #[test]
    fn parse_audio_frame_large_payload() {
        let mut frame = vec![shared_types::MEDIA_PACKET_AUDIO, 3];
        frame.extend_from_slice(b"abc");
        frame.extend_from_slice(&vec![0xBB; 1024]);
        let (sender, data) = parse_audio_frame(&frame).unwrap();
        assert_eq!(sender, "abc");
        assert_eq!(data.len(), 1024);
    }

    #[test]
    fn parse_screen_frame_zero_id_len() {
        let frame = vec![shared_types::MEDIA_PACKET_SCREEN, 0, 0x01];
        let result = parse_screen_frame(&frame);
        assert!(result.is_some());
        let (sender, data) = result.unwrap();
        assert_eq!(sender, "");
        assert_eq!(data, &[0x01]);
    }

    // ─── Hex decode edge cases ───

    #[test]
    fn hex_decode_8_mixed_case() {
        let result = hex_decode_8("aAbBcCdDeEfF0011");
        assert_eq!(result, Some([0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x00, 0x11]));
    }
}

/// Discover a Voxlink server on the local network via UDP broadcast.
/// Returns the server URL (e.g., "ws://192.168.1.5:9090") or None if not found.
pub async fn discover_lan_server() -> Option<String> {
    use tokio::net::UdpSocket;

    let socket = UdpSocket::bind("0.0.0.0:0").await.ok()?;
    socket.set_broadcast(true).ok()?;

    // Send discovery request
    socket
        .send_to(b"VOXLINK_DISCOVER", "255.255.255.255:9091")
        .await
        .ok()?;

    // Wait up to 2 seconds for response
    let mut buf = [0u8; 256];
    match tokio::time::timeout(
        std::time::Duration::from_secs(2),
        socket.recv_from(&mut buf),
    )
    .await
    {
        Ok(Ok((len, _src))) => {
            let msg = std::str::from_utf8(&buf[..len]).ok()?;
            msg.strip_prefix("VOXLINK_SERVER:").map(|s| s.to_string())
        }
        _ => None,
    }
}
