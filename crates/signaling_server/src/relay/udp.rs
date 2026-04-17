use std::sync::Arc;
use std::sync::atomic::Ordering;
use rand::rngs::OsRng;
use rand::RngCore;
use tokio::net::UdpSocket;
use shared_types::SignalMessage;
use crate::types::{Peer, State};
use crate::metrics_server::ServerMetrics;
use crate::relay::{audio::relay_audio_udp, screen::{relay_screen_chunk, relay_screen_udp}};
use crate::UDP_PORT;

type Metrics = Arc<ServerMetrics>;

pub(crate) async fn handle_request_udp(state: &State, peer_id: &str) {
    let udp_port = UDP_PORT.load(Ordering::Relaxed);
    if udp_port == 0 {
        // UDP not enabled on this server
        let s = state.read().await;
        if let Some(peer) = s.peers.get(peer_id).cloned() {
            drop(s);
            crate::send_to(&peer, &SignalMessage::UdpUnavailable).await;
        }
        return;
    }

    // Generate 8 random bytes as session token
    let mut token_bytes = [0u8; 8];
    OsRng.fill_bytes(&mut token_bytes);
    let token_hex = hex_encode(&token_bytes);

    {
        let mut s = state.write().await;
        // Remove any existing token for this peer
        s.udp_sessions.retain(|_, pid| pid != peer_id);
        s.udp_sessions.insert(token_bytes, peer_id.to_string());
    }

    let s = state.read().await;
    if let Some(peer) = s.peers.get(peer_id).cloned() {
        drop(s);
        crate::send_to(
            &peer,
            &SignalMessage::UdpReady {
                token: token_hex,
                port: udp_port,
            },
        )
        .await;
        log::info!("UDP session created for peer {peer_id} on port {udp_port}");
    }
}

pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Run the UDP media relay socket. Receives UDP packets, maps session tokens to peers,
/// and relays supported media to room peers via UDP with WebSocket fallback.
pub(crate) async fn run_udp_relay(state: State, metrics: Metrics, udp_socket: Arc<UdpSocket>) {
    log::info!("UDP media relay started on {:?}", udp_socket.local_addr());

    // Pre-allocate receive buffer for the largest supported UDP media payload.
    let mut buf = vec![0u8; 8 + 1 + shared_types::MAX_UDP_MEDIA_PAYLOAD_SIZE];

    // Per-loop reusable buffers for media relay (avoids alloc per frame)
    let mut relay_buf: Vec<u8> = Vec::with_capacity(shared_types::MAX_UDP_MEDIA_PAYLOAD_SIZE + 300);
    let mut room_peers_buf: Vec<Arc<Peer>> = Vec::with_capacity(20);

    loop {
        let (len, src_addr) = match udp_socket.recv_from(&mut buf).await {
            Ok(r) => r,
            Err(e) => {
                log::warn!("UDP recv error: {e}");
                continue;
            }
        };

        let t0 = std::time::Instant::now();

        // Minimum packet: 8-byte session token.
        if len < shared_types::UDP_SESSION_TOKEN_LEN {
            metrics.udp_invalid_packets_total.fetch_add(1, Ordering::Relaxed);
            continue;
        }

        let token: [u8; 8] = match buf[..8].try_into() {
            Ok(t) => t,
            Err(_) => {
                metrics.udp_invalid_packets_total.fetch_add(1, Ordering::Relaxed);
                continue;
            }
        };
        let packet_type = if len > shared_types::UDP_SESSION_TOKEN_LEN {
            Some(buf[8])
        } else {
            None
        };

        // Single state read: look up peer by token + register/update UDP address
        let peer_id = {
            let s = state.read().await;
            let pid = match s.udp_sessions.get(&token) {
                Some(pid) => pid.clone(),
                None => {
                    metrics.udp_invalid_packets_total.fetch_add(1, Ordering::Relaxed);
                    continue; // Unknown token, silently drop
                }
            };
            // Register/update the peer's UDP address on first packet (or address change)
            if let Some(peer) = s.peers.get(&pid) {
                let current = peer.udp_addr.read().map(|a| *a).unwrap_or(None);
                if current != Some(src_addr) {
                    log::info!("UDP peer {pid} registered at {src_addr}");
                    if let Ok(mut addr) = peer.udp_addr.write() {
                        *addr = Some(src_addr);
                    }
                }
            }
            pid
        };

        // Bare token packet: registration only.
        let Some(packet_type) = packet_type else {
            continue;
        };

        // Keepalive: just refreshes the address mapping above, no relay needed
        if packet_type == shared_types::UDP_KEEPALIVE {
            continue;
        }

        if len < 10 {
            metrics.udp_invalid_packets_total.fetch_add(1, Ordering::Relaxed);
            continue;
        }

        metrics.udp_frames_in_total.fetch_add(1, Ordering::Relaxed);
        match packet_type {
            shared_types::MEDIA_PACKET_AUDIO => {
                let audio_data = &buf[9..len];
                relay_audio_udp(
                    &state,
                    &metrics,
                    &peer_id,
                    audio_data,
                    &udp_socket,
                    &mut relay_buf,
                    &mut room_peers_buf,
                )
                .await;
            }
            shared_types::MEDIA_PACKET_SCREEN => {
                let screen_data = &buf[9..len];
                relay_screen_udp(
                    &state,
                    &metrics,
                    &peer_id,
                    screen_data,
                    &udp_socket,
                    &mut relay_buf,
                    &mut room_peers_buf,
                )
                .await;
            }
            shared_types::MEDIA_PACKET_SCREEN_CHUNK => {
                let screen_data = &buf[9..len];
                relay_screen_chunk(&state, &metrics, &peer_id, screen_data).await;
            }
            _ => {
                metrics.udp_invalid_packets_total.fetch_add(1, Ordering::Relaxed);
            }
        }

        metrics.udp_relay_latency.observe(t0.elapsed().as_secs_f64());
    }
}
