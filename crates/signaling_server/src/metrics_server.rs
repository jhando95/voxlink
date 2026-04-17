use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use crate::types::State;

pub(crate) struct ServerMetrics {
    pub(crate) connection_attempts_total: AtomicU64,
    pub(crate) active_connections: AtomicU64,
    pub(crate) websocket_handshake_failures_total: AtomicU64,
    pub(crate) signaling_messages_total: AtomicU64,
    pub(crate) malformed_signaling_messages_total: AtomicU64,
    pub(crate) signaling_rate_limited_total: AtomicU64,
    pub(crate) auth_success_total: AtomicU64,
    pub(crate) auth_failure_total: AtomicU64,
    pub(crate) audio_frames_in_total: AtomicU64,
    pub(crate) audio_frames_out_total: AtomicU64,
    pub(crate) screen_frames_in_total: AtomicU64,
    pub(crate) screen_frames_out_total: AtomicU64,
    pub(crate) udp_frames_in_total: AtomicU64,
    pub(crate) udp_frames_out_total: AtomicU64,
    pub(crate) started_at: Instant,
}

impl Default for ServerMetrics {
    fn default() -> Self {
        Self {
            connection_attempts_total: AtomicU64::new(0),
            active_connections: AtomicU64::new(0),
            websocket_handshake_failures_total: AtomicU64::new(0),
            signaling_messages_total: AtomicU64::new(0),
            malformed_signaling_messages_total: AtomicU64::new(0),
            signaling_rate_limited_total: AtomicU64::new(0),
            auth_success_total: AtomicU64::new(0),
            auth_failure_total: AtomicU64::new(0),
            audio_frames_in_total: AtomicU64::new(0),
            audio_frames_out_total: AtomicU64::new(0),
            screen_frames_in_total: AtomicU64::new(0),
            screen_frames_out_total: AtomicU64::new(0),
            udp_frames_in_total: AtomicU64::new(0),
            udp_frames_out_total: AtomicU64::new(0),
            started_at: Instant::now(),
        }
    }
}

pub(crate) async fn run_metrics_server(state: State, metrics: std::sync::Arc<ServerMetrics>, addr: String, tls_enabled: bool) {
    let listener = match TcpListener::bind(&addr).await {
        Ok(listener) => listener,
        Err(e) => {
            log::error!("Metrics endpoint unavailable on {addr}: {e}");
            return;
        }
    };

    log::info!("Metrics endpoint listening on http://{addr}");
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            continue;
        };
        let state = state.clone();
        let metrics = metrics.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            let read =
                tokio::time::timeout(std::time::Duration::from_secs(2), stream.read(&mut buf))
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..read]);
            let is_health = request.starts_with("GET /healthz ");
            let body = if is_health {
                "ok\n".to_string()
            } else {
                render_metrics(&state, &metrics, tls_enabled).await
            };
            let content_type = if is_health {
                "text/plain; charset=utf-8"
            } else {
                "text/plain; version=0.0.4; charset=utf-8"
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        });
    }
}

pub(crate) async fn render_metrics(state: &State, metrics: &ServerMetrics, tls_enabled: bool) -> String {
    let s = state.read().await;
    let active_rooms = s.rooms.len();
    let active_spaces = s.spaces.len();
    let connected_peers = s.peers.len();
    let udp_sessions = s.udp_sessions.len();
    let total_room_peers: usize = s.rooms.values().map(|r| r.peer_ids.len()).sum();
    let max_room_peers: usize = s
        .rooms
        .values()
        .map(|r| r.peer_ids.len())
        .max()
        .unwrap_or(0);
    let total_space_members: usize = s.spaces.values().map(|sp| sp.member_ids.len()).sum();
    drop(s);

    let uptime_secs = metrics.started_at.elapsed().as_secs();

    format!(
        concat!(
            "# TYPE voxlink_connection_attempts_total counter\n",
            "voxlink_connection_attempts_total {}\n",
            "# TYPE voxlink_active_connections gauge\n",
            "voxlink_active_connections {}\n",
            "# TYPE voxlink_websocket_handshake_failures_total counter\n",
            "voxlink_websocket_handshake_failures_total {}\n",
            "# TYPE voxlink_signaling_messages_total counter\n",
            "voxlink_signaling_messages_total {}\n",
            "# TYPE voxlink_malformed_signaling_messages_total counter\n",
            "voxlink_malformed_signaling_messages_total {}\n",
            "# TYPE voxlink_signaling_rate_limited_total counter\n",
            "voxlink_signaling_rate_limited_total {}\n",
            "# TYPE voxlink_auth_success_total counter\n",
            "voxlink_auth_success_total {}\n",
            "# TYPE voxlink_auth_failure_total counter\n",
            "voxlink_auth_failure_total {}\n",
            "# TYPE voxlink_audio_frames_in_total counter\n",
            "voxlink_audio_frames_in_total {}\n",
            "# TYPE voxlink_audio_frames_out_total counter\n",
            "voxlink_audio_frames_out_total {}\n",
            "# TYPE voxlink_screen_frames_in_total counter\n",
            "voxlink_screen_frames_in_total {}\n",
            "# TYPE voxlink_screen_frames_out_total counter\n",
            "voxlink_screen_frames_out_total {}\n",
            "# TYPE voxlink_udp_frames_in_total counter\n",
            "voxlink_udp_frames_in_total {}\n",
            "# TYPE voxlink_udp_frames_out_total counter\n",
            "voxlink_udp_frames_out_total {}\n",
            "# TYPE voxlink_active_rooms gauge\n",
            "voxlink_active_rooms {}\n",
            "# TYPE voxlink_active_spaces gauge\n",
            "voxlink_active_spaces {}\n",
            "# TYPE voxlink_connected_peers gauge\n",
            "voxlink_connected_peers {}\n",
            "# TYPE voxlink_udp_sessions gauge\n",
            "voxlink_udp_sessions {}\n",
            "# TYPE voxlink_total_room_peers gauge\n",
            "voxlink_total_room_peers {}\n",
            "# TYPE voxlink_max_room_peers gauge\n",
            "voxlink_max_room_peers {}\n",
            "# TYPE voxlink_total_space_members gauge\n",
            "voxlink_total_space_members {}\n",
            "# TYPE voxlink_uptime_seconds gauge\n",
            "voxlink_uptime_seconds {}\n",
            "# TYPE voxlink_tls_enabled gauge\n",
            "voxlink_tls_enabled {}\n",
        ),
        metrics.connection_attempts_total.load(Ordering::Relaxed),
        metrics.active_connections.load(Ordering::Relaxed),
        metrics
            .websocket_handshake_failures_total
            .load(Ordering::Relaxed),
        metrics.signaling_messages_total.load(Ordering::Relaxed),
        metrics
            .malformed_signaling_messages_total
            .load(Ordering::Relaxed),
        metrics.signaling_rate_limited_total.load(Ordering::Relaxed),
        metrics.auth_success_total.load(Ordering::Relaxed),
        metrics.auth_failure_total.load(Ordering::Relaxed),
        metrics.audio_frames_in_total.load(Ordering::Relaxed),
        metrics.audio_frames_out_total.load(Ordering::Relaxed),
        metrics.screen_frames_in_total.load(Ordering::Relaxed),
        metrics.screen_frames_out_total.load(Ordering::Relaxed),
        metrics.udp_frames_in_total.load(Ordering::Relaxed),
        metrics.udp_frames_out_total.load(Ordering::Relaxed),
        active_rooms,
        active_spaces,
        connected_peers,
        udp_sessions,
        total_room_peers,
        max_room_peers,
        total_space_members,
        uptime_secs,
        if tls_enabled { 1 } else { 0 },
    )
}
