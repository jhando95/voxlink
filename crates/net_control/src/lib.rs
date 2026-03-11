use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use shared_types::SignalMessage;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::Message;

type WsTx = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    Message,
>;

/// Max queued audio frames before dropping oldest. At 50fps, 200 = ~4 seconds.
/// Prevents unbounded memory growth if consumer falls behind.
const AUDIO_QUEUE_CAPACITY: usize = 200;

pub struct NetworkClient {
    ws_tx: Arc<Mutex<Option<WsTx>>>,
    signal_rx: mpsc::UnboundedReceiver<SignalMessage>,
    /// Raw binary audio frames — bounded channel to prevent OOM on slow consumers.
    audio_rx: mpsc::Receiver<Vec<u8>>,
    signal_tx_internal: mpsc::UnboundedSender<SignalMessage>,
    audio_tx_internal: mpsc::Sender<Vec<u8>>,
    connected: Arc<std::sync::atomic::AtomicBool>,
    server_url: Arc<Mutex<Option<String>>>,
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
            signal_tx_internal: sig_tx,
            audio_tx_internal: audio_tx,
            connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            server_url: Arc::new(Mutex::new(None)),
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
        let connected = self.connected.clone();

        tokio::spawn(async move {
            while let Some(msg) = rx.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(signal) = serde_json::from_str::<SignalMessage>(&text) {
                            let _ = sig_tx.send(signal);
                        }
                    }
                    Ok(Message::Binary(data)) => {
                        // Validate header, then pass raw bytes through bounded channel.
                        // try_send drops frames when consumer falls behind, preventing OOM.
                        if data.len() >= 2 {
                            let id_len = data[0] as usize;
                            if data.len() > 1 + id_len {
                                let _ = audio_tx.try_send(data.into());
                            }
                        }
                    }
                    Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {} // keepalive
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
        if let Some(tx) = self.ws_tx.lock().await.as_mut() {
            tx.send(Message::Binary(data.to_vec().into())).await?;
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

    pub async fn disconnect(&mut self) {
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
    if raw.len() < 2 {
        return None;
    }
    let id_len = raw[0] as usize;
    if raw.len() <= 1 + id_len {
        return None;
    }
    let sender_id = std::str::from_utf8(&raw[1..1 + id_len]).ok()?;
    let audio_data = &raw[1 + id_len..];
    Some((sender_id, audio_data))
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
            msg.strip_prefix("VOXLINK_SERVER:")
                .map(|s| s.to_string())
        }
        _ => None,
    }
}
