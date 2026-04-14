use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use shared_types::SignalMessage;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::Message;

type WsTx = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;

/// Max queued audio frames before dropping oldest. At 50fps, 200 = ~4 seconds.
/// Prevents unbounded memory growth if consumer falls behind.
const AUDIO_QUEUE_CAPACITY: usize = 200;
const SCREEN_CHUNK_REASSEMBLY_TIMEOUT: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScreenChunkCounters {
    pub chunk_packets: u64,
    pub frames_completed: u64,
    pub frames_superseded: u64,
    pub frames_timed_out: u64,
}

#[derive(Debug, Default)]
struct ScreenChunkCounterSet {
    chunk_packets: AtomicU64,
    frames_completed: AtomicU64,
    frames_superseded: AtomicU64,
    frames_timed_out: AtomicU64,
}

impl ScreenChunkCounterSet {
    fn record_chunk_packet(&self) {
        self.chunk_packets.fetch_add(1, Ordering::Relaxed);
    }

    fn record_completed_frame(&self) {
        self.frames_completed.fetch_add(1, Ordering::Relaxed);
    }

    fn record_superseded_frame(&self) {
        self.frames_superseded.fetch_add(1, Ordering::Relaxed);
    }

    fn record_timed_out_frame(&self) {
        self.frames_timed_out.fetch_add(1, Ordering::Relaxed);
    }

    fn swap(&self) -> ScreenChunkCounters {
        ScreenChunkCounters {
            chunk_packets: self.chunk_packets.swap(0, Ordering::Relaxed),
            frames_completed: self.frames_completed.swap(0, Ordering::Relaxed),
            frames_superseded: self.frames_superseded.swap(0, Ordering::Relaxed),
            frames_timed_out: self.frames_timed_out.swap(0, Ordering::Relaxed),
        }
    }
}

#[derive(Debug)]
struct ScreenChunkAssembler {
    sequence: u32,
    chunk_count: u16,
    received_chunks: u16,
    total_len: usize,
    last_chunk_at: Instant,
    chunks: Vec<Option<Vec<u8>>>,
}

impl ScreenChunkAssembler {
    fn new(meta: shared_types::ScreenChunkMetadata) -> Self {
        let now = Instant::now();
        Self {
            sequence: meta.sequence,
            chunk_count: meta.chunk_count,
            received_chunks: 0,
            total_len: 0,
            last_chunk_at: now,
            chunks: vec![None; meta.chunk_count as usize],
        }
    }

    fn store_chunk(&mut self, meta: shared_types::ScreenChunkMetadata, chunk: &[u8]) -> bool {
        if self.sequence != meta.sequence || self.chunk_count != meta.chunk_count {
            return false;
        }
        self.last_chunk_at = Instant::now();
        let slot = &mut self.chunks[meta.chunk_index as usize];
        if slot.is_none() {
            self.received_chunks = self.received_chunks.saturating_add(1);
            self.total_len = self.total_len.saturating_add(chunk.len());
            if self.total_len > shared_types::MAX_SCREEN_FRAME_SIZE {
                return false;
            }
            *slot = Some(chunk.to_vec());
        }
        true
    }

    fn is_complete(&self) -> bool {
        self.received_chunks == self.chunk_count
    }

    fn is_timed_out(&self, now: Instant) -> bool {
        now.duration_since(self.last_chunk_at) >= SCREEN_CHUNK_REASSEMBLY_TIMEOUT
    }

    fn into_frame(self, sender_id: &str) -> Option<Vec<u8>> {
        if !self.is_complete() || sender_id.len() > u8::MAX as usize {
            return None;
        }
        let mut frame = Vec::with_capacity(2 + sender_id.len() + self.total_len);
        frame.push(shared_types::MEDIA_PACKET_SCREEN);
        frame.push(sender_id.len() as u8);
        frame.extend_from_slice(sender_id.as_bytes());
        for chunk in self.chunks {
            frame.extend_from_slice(chunk?.as_slice());
        }
        Some(frame)
    }
}

struct ScreenChunkFrame<'a> {
    sender_id: &'a str,
    metadata: shared_types::ScreenChunkMetadata,
    chunk_data: &'a [u8],
}

pub struct NetworkClient {
    ws_tx: Arc<Mutex<Option<WsTx>>>,
    signal_rx: mpsc::UnboundedReceiver<SignalMessage>,
    /// Raw binary audio frames — bounded channel to prevent OOM on slow consumers.
    audio_rx: mpsc::Receiver<Vec<u8>>,
    /// Latest raw binary screen frame. Screen sharing is freshness-sensitive, so
    /// we overwrite stale pending frames instead of queueing them.
    screen_latest: Arc<std::sync::Mutex<Option<Vec<u8>>>>,
    screen_chunks: Arc<std::sync::Mutex<HashMap<String, ScreenChunkAssembler>>>,
    screen_chunk_counters: Arc<ScreenChunkCounterSet>,
    signal_tx_internal: mpsc::UnboundedSender<SignalMessage>,
    audio_tx_internal: mpsc::Sender<Vec<u8>>,
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
    /// Bandwidth tracking — lock-free counters for live display.
    bytes_sent: Arc<std::sync::atomic::AtomicU64>,
    bytes_recv: Arc<std::sync::atomic::AtomicU64>,
    packets_sent: Arc<std::sync::atomic::AtomicU64>,
    packets_recv: Arc<std::sync::atomic::AtomicU64>,
    next_screen_sequence: std::sync::atomic::AtomicU32,
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
        Self {
            ws_tx: Arc::new(Mutex::new(None)),
            signal_rx: sig_rx,
            audio_rx,
            screen_latest: Arc::new(std::sync::Mutex::new(None)),
            screen_chunks: Arc::new(std::sync::Mutex::new(HashMap::new())),
            screen_chunk_counters: Arc::new(ScreenChunkCounterSet::default()),
            signal_tx_internal: sig_tx,
            audio_tx_internal: audio_tx,
            connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            server_url: Arc::new(Mutex::new(None)),
            last_ping_ms: Arc::new(std::sync::atomic::AtomicI32::new(-1)),
            ping_sent_at: Arc::new(Mutex::new(None)),
            udp_socket: Arc::new(Mutex::new(None)),
            udp_active: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            udp_token: Arc::new(Mutex::new(None)),
            bytes_sent: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            bytes_recv: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            packets_sent: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            packets_recv: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            next_screen_sequence: std::sync::atomic::AtomicU32::new(0),
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
        let screen_latest = self.screen_latest.clone();
        let screen_chunks = self.screen_chunks.clone();
        let screen_chunk_counters = self.screen_chunk_counters.clone();
        let connected = self.connected.clone();
        let last_ping_ms_rx = self.last_ping_ms.clone();
        let ping_sent_at_rx = self.ping_sent_at.clone();
        let bytes_recv_rx = self.bytes_recv.clone();
        let packets_recv_rx = self.packets_recv.clone();

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
                                        bytes_recv_rx.fetch_add(
                                            data.len() as u64,
                                            std::sync::atomic::Ordering::Relaxed,
                                        );
                                        packets_recv_rx
                                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                        let _ = audio_tx.try_send(data.into());
                                    }
                                }
                                shared_types::MEDIA_PACKET_SCREEN => {
                                    let id_len = data[1] as usize;
                                    if data.len() > 2 + id_len {
                                        bytes_recv_rx.fetch_add(
                                            data.len() as u64,
                                            std::sync::atomic::Ordering::Relaxed,
                                        );
                                        packets_recv_rx
                                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                        store_latest_frame(screen_latest.as_ref(), data.into());
                                    }
                                }
                                shared_types::MEDIA_PACKET_SCREEN_CHUNK => {
                                    if let Some(frame) = absorb_screen_chunk(
                                        screen_chunks.as_ref(),
                                        screen_chunk_counters.as_ref(),
                                        &data,
                                    ) {
                                        bytes_recv_rx.fetch_add(
                                            data.len() as u64,
                                            std::sync::atomic::Ordering::Relaxed,
                                        );
                                        packets_recv_rx
                                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                        store_latest_frame(screen_latest.as_ref(), frame);
                                    } else if parse_screen_chunk_frame(&data).is_some() {
                                        bytes_recv_rx.fetch_add(
                                            data.len() as u64,
                                            std::sync::atomic::Ordering::Relaxed,
                                        );
                                        packets_recv_rx
                                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
        if let Ok(mut chunks) = self.screen_chunks.lock() {
            chunks.clear();
        }
        if let Ok(mut latest) = self.screen_latest.lock() {
            *latest = None;
        }

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
                    self.bytes_sent
                        .fetch_add(pkt_len as u64, std::sync::atomic::Ordering::Relaxed);
                    self.packets_sent
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
            self.bytes_sent
                .fetch_add(pkt_len as u64, std::sync::atomic::Ordering::Relaxed);
            self.packets_sent
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tx.send(Message::Binary(packet.into())).await?;
        }
        Ok(())
    }

    pub async fn send_screen_frame(&self, data: &[u8]) -> Result<()> {
        if self.udp_active.load(std::sync::atomic::Ordering::Acquire) {
            if let Some(ref token) = *self.udp_token.lock().await {
                if let Some(ref socket) = *self.udp_socket.lock().await {
                    if data.len() <= shared_types::MAX_UDP_SCREEN_CHUNK_SIZE {
                        let pkt_len = 8 + 1 + data.len();
                        let mut packet = Vec::with_capacity(pkt_len);
                        packet.extend_from_slice(token);
                        packet.push(shared_types::MEDIA_PACKET_SCREEN);
                        packet.extend_from_slice(data);
                        let _ = socket.send(&packet).await;
                        self.bytes_sent
                            .fetch_add(pkt_len as u64, std::sync::atomic::Ordering::Relaxed);
                        self.packets_sent
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        return Ok(());
                    }

                    let sequence = self.next_screen_sequence();
                    let chunk_count = data.len().div_ceil(shared_types::MAX_UDP_SCREEN_CHUNK_SIZE);
                    if chunk_count <= u16::MAX as usize {
                        for (chunk_index, chunk) in data
                            .chunks(shared_types::MAX_UDP_SCREEN_CHUNK_SIZE)
                            .enumerate()
                        {
                            let header = shared_types::encode_screen_chunk_metadata(
                                sequence,
                                chunk_index as u16,
                                chunk_count as u16,
                            );
                            let pkt_len = 8 + 1 + header.len() + chunk.len();
                            let mut packet = Vec::with_capacity(pkt_len);
                            packet.extend_from_slice(token);
                            packet.push(shared_types::MEDIA_PACKET_SCREEN_CHUNK);
                            packet.extend_from_slice(&header);
                            packet.extend_from_slice(chunk);
                            let _ = socket.send(&packet).await;
                            self.bytes_sent
                                .fetch_add(pkt_len as u64, std::sync::atomic::Ordering::Relaxed);
                            self.packets_sent
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                        return Ok(());
                    }
                }
            }
        }

        if let Some(tx) = self.ws_tx.lock().await.as_mut() {
            if data.len() <= shared_types::MAX_UDP_SCREEN_CHUNK_SIZE {
                let mut packet = Vec::with_capacity(data.len() + 1);
                packet.push(shared_types::MEDIA_PACKET_SCREEN);
                packet.extend_from_slice(data);
                self.bytes_sent
                    .fetch_add(packet.len() as u64, std::sync::atomic::Ordering::Relaxed);
                self.packets_sent
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                tx.send(Message::Binary(packet.into())).await?;
            } else {
                let sequence = self.next_screen_sequence();
                let chunk_count = data.len().div_ceil(shared_types::MAX_UDP_SCREEN_CHUNK_SIZE);
                if chunk_count <= u16::MAX as usize {
                    for (chunk_index, chunk) in data
                        .chunks(shared_types::MAX_UDP_SCREEN_CHUNK_SIZE)
                        .enumerate()
                    {
                        let header = shared_types::encode_screen_chunk_metadata(
                            sequence,
                            chunk_index as u16,
                            chunk_count as u16,
                        );
                        let mut packet = Vec::with_capacity(1 + header.len() + chunk.len());
                        packet.push(shared_types::MEDIA_PACKET_SCREEN_CHUNK);
                        packet.extend_from_slice(&header);
                        packet.extend_from_slice(chunk);
                        self.bytes_sent
                            .fetch_add(packet.len() as u64, std::sync::atomic::Ordering::Relaxed);
                        self.packets_sent
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        tx.send(Message::Binary(packet.into())).await?;
                    }
                }
            }
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
        self.screen_latest
            .lock()
            .ok()
            .and_then(|mut latest| latest.take())
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

        // Start UDP receive task — pushes incoming media to the same hot-path queues.
        let audio_tx = self.audio_tx_internal.clone();
        let screen_latest_udp = self.screen_latest.clone();
        let screen_chunks_udp = self.screen_chunks.clone();
        let screen_chunk_counters_udp = self.screen_chunk_counters.clone();
        let recv_socket = socket.clone();
        let udp_active = self.udp_active.clone();
        let bytes_recv_udp = self.bytes_recv.clone();
        let packets_recv_udp = self.packets_recv.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 2 + 256 + shared_types::MAX_UDP_MEDIA_PAYLOAD_SIZE];
            loop {
                match recv_socket.recv(&mut buf).await {
                    Ok(len) if len >= 3 => {
                        let packet_type = buf[0];
                        let id_len = buf[1] as usize;
                        if len > 2 + id_len {
                            bytes_recv_udp
                                .fetch_add(len as u64, std::sync::atomic::Ordering::Relaxed);
                            packets_recv_udp.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            match packet_type {
                                shared_types::MEDIA_PACKET_AUDIO => {
                                    let _ = audio_tx.try_send(buf[..len].to_vec());
                                }
                                shared_types::MEDIA_PACKET_SCREEN => {
                                    store_latest_frame(
                                        screen_latest_udp.as_ref(),
                                        buf[..len].to_vec(),
                                    );
                                }
                                shared_types::MEDIA_PACKET_SCREEN_CHUNK => {
                                    if let Some(frame) = absorb_screen_chunk(
                                        screen_chunks_udp.as_ref(),
                                        screen_chunk_counters_udp.as_ref(),
                                        &buf[..len],
                                    ) {
                                        store_latest_frame(screen_latest_udp.as_ref(), frame);
                                    }
                                }
                                _ => {}
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
            let interval =
                std::time::Duration::from_secs(shared_types::UDP_KEEPALIVE_INTERVAL_SECS);
            let keepalive_packet = [
                token[0],
                token[1],
                token[2],
                token[3],
                token[4],
                token[5],
                token[6],
                token[7],
                shared_types::UDP_KEEPALIVE,
            ];
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
        self.udp_active.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Snapshot and reset bandwidth counters. Returns (bytes_sent, bytes_recv).
    /// Designed for periodic 1-second sampling in the tick loop.
    pub fn swap_bandwidth_counters(&self) -> (u64, u64) {
        let sent = self
            .bytes_sent
            .swap(0, std::sync::atomic::Ordering::Relaxed);
        let recv = self
            .bytes_recv
            .swap(0, std::sync::atomic::Ordering::Relaxed);
        (sent, recv)
    }

    /// Read total packets sent/received (cumulative, not reset).
    pub fn packet_counts(&self) -> (u64, u64) {
        let sent = self.packets_sent.load(std::sync::atomic::Ordering::Relaxed);
        let recv = self.packets_recv.load(std::sync::atomic::Ordering::Relaxed);
        (sent, recv)
    }

    /// Remove stalled partial screen-share frames and return how many timed out.
    pub fn expire_stale_screen_chunks(&self) -> u64 {
        expire_stale_screen_chunks(
            self.screen_chunks.as_ref(),
            self.screen_chunk_counters.as_ref(),
        )
    }

    /// Snapshot and reset logical screen-chunk counters for the last sampling window.
    pub fn swap_screen_chunk_counters(&self) -> ScreenChunkCounters {
        self.screen_chunk_counters.swap()
    }

    pub async fn disconnect(&mut self) {
        // Shut down UDP
        self.udp_active
            .store(false, std::sync::atomic::Ordering::Relaxed);
        *self.udp_socket.lock().await = None;
        *self.udp_token.lock().await = None;
        if let Ok(mut chunks) = self.screen_chunks.lock() {
            chunks.clear();
        }
        if let Ok(mut latest) = self.screen_latest.lock() {
            *latest = None;
        }

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

fn parse_screen_chunk_frame(raw: &[u8]) -> Option<ScreenChunkFrame<'_>> {
    if raw.len() < 3 || raw[0] != shared_types::MEDIA_PACKET_SCREEN_CHUNK {
        return None;
    }
    let id_len = raw[1] as usize;
    if raw.len() <= 2 + id_len + shared_types::SCREEN_CHUNK_METADATA_LEN {
        return None;
    }
    let sender_id = std::str::from_utf8(&raw[2..2 + id_len]).ok()?;
    let (metadata, chunk_data) = shared_types::decode_screen_chunk_metadata(&raw[2 + id_len..])?;
    Some(ScreenChunkFrame {
        sender_id,
        metadata,
        chunk_data,
    })
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

fn store_latest_frame(slot: &std::sync::Mutex<Option<Vec<u8>>>, frame: Vec<u8>) {
    if let Ok(mut latest) = slot.lock() {
        *latest = Some(frame);
    }
}

fn next_sequence_is_newer(sequence: u32, current: u32) -> bool {
    sequence != current && sequence.wrapping_sub(current) < 0x8000_0000
}

fn expire_stale_screen_chunks(
    slot: &std::sync::Mutex<HashMap<String, ScreenChunkAssembler>>,
    counters: &ScreenChunkCounterSet,
) -> u64 {
    let now = Instant::now();
    let mut expired = 0u64;
    if let Ok(mut all) = slot.lock() {
        all.retain(|_, assembler| {
            if assembler.is_timed_out(now) {
                expired = expired.saturating_add(1);
                counters.record_timed_out_frame();
                false
            } else {
                true
            }
        });
    }
    expired
}

fn absorb_screen_chunk(
    slot: &std::sync::Mutex<HashMap<String, ScreenChunkAssembler>>,
    counters: &ScreenChunkCounterSet,
    raw: &[u8],
) -> Option<Vec<u8>> {
    let packet = parse_screen_chunk_frame(raw)?;
    counters.record_chunk_packet();
    let sender_key = packet.sender_id.to_string();
    let mut all = slot.lock().ok()?;
    let now = Instant::now();
    all.retain(|_, assembler| {
        if assembler.is_timed_out(now) {
            counters.record_timed_out_frame();
            false
        } else {
            true
        }
    });
    let replace = match all.get(&sender_key) {
        Some(existing) if existing.sequence == packet.metadata.sequence => {
            existing.chunk_count != packet.metadata.chunk_count
        }
        Some(existing) => next_sequence_is_newer(packet.metadata.sequence, existing.sequence),
        None => true,
    };

    if replace {
        if let Some(existing) = all.insert(
            sender_key.clone(),
            ScreenChunkAssembler::new(packet.metadata),
        ) {
            if !existing.is_complete() {
                counters.record_superseded_frame();
            }
        }
    } else if all
        .get(&sender_key)
        .map(|existing| existing.sequence != packet.metadata.sequence)
        .unwrap_or(false)
    {
        return None;
    }

    let complete = {
        let entry = all.get_mut(&sender_key)?;
        if !entry.store_chunk(packet.metadata, packet.chunk_data) {
            all.remove(&sender_key);
            return None;
        }
        entry.is_complete()
    };

    if !complete {
        return None;
    }

    let frame = all.remove(&sender_key)?.into_frame(packet.sender_id)?;
    counters.record_completed_frame();
    Some(frame)
}

#[cfg(test)]
fn build_screen_chunk_frame(
    sender_id: &str,
    metadata: shared_types::ScreenChunkMetadata,
    chunk_data: &[u8],
) -> Vec<u8> {
    let mut frame = Vec::with_capacity(
        2 + sender_id.len() + shared_types::SCREEN_CHUNK_METADATA_LEN + chunk_data.len(),
    );
    frame.push(shared_types::MEDIA_PACKET_SCREEN_CHUNK);
    frame.push(sender_id.len() as u8);
    frame.extend_from_slice(sender_id.as_bytes());
    frame.extend_from_slice(&shared_types::encode_screen_chunk_metadata(
        metadata.sequence,
        metadata.chunk_index,
        metadata.chunk_count,
    ));
    frame.extend_from_slice(chunk_data);
    frame
}

impl NetworkClient {
    fn next_screen_sequence(&self) -> u32 {
        self.next_screen_sequence
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .wrapping_add(1)
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
        .send_to(b"VOXLINK_DISCOVER", "255.255.255.255:9092")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_decode_8_valid() {
        let result = hex_decode_8("0123456789abcdef");
        assert_eq!(
            result,
            Some([0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef])
        );
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
    fn parse_screen_chunk_frame_valid() {
        let frame = build_screen_chunk_frame(
            "abc",
            shared_types::ScreenChunkMetadata {
                sequence: 9,
                chunk_index: 1,
                chunk_count: 3,
            },
            b"tail",
        );
        let parsed = parse_screen_chunk_frame(&frame).unwrap();
        assert_eq!(parsed.sender_id, "abc");
        assert_eq!(
            parsed.metadata,
            shared_types::ScreenChunkMetadata {
                sequence: 9,
                chunk_index: 1,
                chunk_count: 3,
            }
        );
        assert_eq!(parsed.chunk_data, b"tail");
    }

    #[test]
    fn absorb_screen_chunk_reassembles_frame() {
        let slot = std::sync::Mutex::new(HashMap::new());
        let counters = ScreenChunkCounterSet::default();
        let meta0 = shared_types::ScreenChunkMetadata {
            sequence: 77,
            chunk_index: 0,
            chunk_count: 2,
        };
        let meta1 = shared_types::ScreenChunkMetadata {
            sequence: 77,
            chunk_index: 1,
            chunk_count: 2,
        };
        let chunk0 = build_screen_chunk_frame("peer-1", meta0, b"hello ");
        let chunk1 = build_screen_chunk_frame("peer-1", meta1, b"world");

        assert!(absorb_screen_chunk(&slot, &counters, &chunk0).is_none());
        let frame = absorb_screen_chunk(&slot, &counters, &chunk1).unwrap();
        let (sender, payload) = parse_screen_frame(&frame).unwrap();
        assert_eq!(sender, "peer-1");
        assert_eq!(payload, b"hello world");
        assert_eq!(
            counters.swap(),
            ScreenChunkCounters {
                chunk_packets: 2,
                frames_completed: 1,
                frames_superseded: 0,
                frames_timed_out: 0,
            }
        );
    }

    #[test]
    fn absorb_screen_chunk_drops_stale_sequence_after_newer_frame_starts() {
        let slot = std::sync::Mutex::new(HashMap::new());
        let counters = ScreenChunkCounterSet::default();
        let old_chunk = build_screen_chunk_frame(
            "peer-1",
            shared_types::ScreenChunkMetadata {
                sequence: 11,
                chunk_index: 0,
                chunk_count: 2,
            },
            b"old-",
        );
        let new_chunk0 = build_screen_chunk_frame(
            "peer-1",
            shared_types::ScreenChunkMetadata {
                sequence: 12,
                chunk_index: 0,
                chunk_count: 2,
            },
            b"new-",
        );
        let stale_old_chunk1 = build_screen_chunk_frame(
            "peer-1",
            shared_types::ScreenChunkMetadata {
                sequence: 11,
                chunk_index: 1,
                chunk_count: 2,
            },
            b"frame",
        );
        let new_chunk1 = build_screen_chunk_frame(
            "peer-1",
            shared_types::ScreenChunkMetadata {
                sequence: 12,
                chunk_index: 1,
                chunk_count: 2,
            },
            b"frame",
        );

        assert!(absorb_screen_chunk(&slot, &counters, &old_chunk).is_none());
        assert!(absorb_screen_chunk(&slot, &counters, &new_chunk0).is_none());
        assert!(absorb_screen_chunk(&slot, &counters, &stale_old_chunk1).is_none());
        let frame = absorb_screen_chunk(&slot, &counters, &new_chunk1).unwrap();
        let (_, payload) = parse_screen_frame(&frame).unwrap();
        assert_eq!(payload, b"new-frame");
        assert_eq!(
            counters.swap(),
            ScreenChunkCounters {
                chunk_packets: 4,
                frames_completed: 1,
                frames_superseded: 1,
                frames_timed_out: 0,
            }
        );
    }

    #[test]
    fn expire_stale_screen_chunks_counts_timeouts() {
        let slot = std::sync::Mutex::new(HashMap::new());
        let counters = ScreenChunkCounterSet::default();
        let chunk = build_screen_chunk_frame(
            "peer-1",
            shared_types::ScreenChunkMetadata {
                sequence: 25,
                chunk_index: 0,
                chunk_count: 2,
            },
            b"partial",
        );

        assert!(absorb_screen_chunk(&slot, &counters, &chunk).is_none());
        std::thread::sleep(SCREEN_CHUNK_REASSEMBLY_TIMEOUT + Duration::from_millis(20));
        assert_eq!(expire_stale_screen_chunks(&slot, &counters), 1);
        assert_eq!(
            counters.swap(),
            ScreenChunkCounters {
                chunk_packets: 1,
                frames_completed: 0,
                frames_superseded: 0,
                frames_timed_out: 1,
            }
        );
    }

    #[test]
    fn hex_decode_8_uppercase() {
        let result = hex_decode_8("0123456789ABCDEF");
        assert_eq!(
            result,
            Some([0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF])
        );
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
        assert!(!result.unwrap(), "Should return false when no URL stored");
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
        assert!(
            result.is_ok(),
            "send_audio with no connection should not error"
        );
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

    #[test]
    fn store_latest_frame_overwrites_stale_screen_frame() {
        let slot = std::sync::Mutex::new(None);
        store_latest_frame(&slot, vec![1, 2, 3]);
        store_latest_frame(&slot, vec![4, 5, 6]);
        let latest = slot.lock().unwrap().clone();
        assert_eq!(latest, Some(vec![4, 5, 6]));
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
        assert_eq!(
            result,
            Some([0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x00, 0x11])
        );
    }
}
