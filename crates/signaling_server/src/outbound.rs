//! Per-peer outbound write path.
//!
//! Each peer owns two bounded `tokio::sync::mpsc` channels — one for reliable
//! signaling and one for lossy media — and a single drain task that owns the
//! WebSocket sink. Senders never lock anything; they call `try_send` on the
//! appropriate channel and apply the lane's overflow policy:
//!
//! * **Media lane** (audio + screen frames): drop the new frame on Full and
//!   bump `ws_frames_dropped_lossy_total`. Acceptable for 20ms audio because
//!   the next frame is already on its way from the client.
//! * **Signaling lane** (JSON `SignalMessage`s): treat Full as a misbehaving
//!   or dead peer. Bump `ws_frames_dropped_overflow_total`, notify the
//!   `disconnect` watcher so the rx loop tears the peer down.
//!
//! The drain task biases signaling over media in its `select!` so backpressure
//! never starves Welcome / Error / Pong.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use futures_util::SinkExt;
use tokio::sync::{mpsc, Notify};
use tokio_tungstenite::tungstenite::{Bytes, Message};

use crate::metrics_server::ServerMetrics;
use crate::tls::ServerStream;

/// SplitSink half of a WebSocketStream<ServerStream>. The drain task is the
/// sole owner; no Mutex anywhere on the hot path.
pub(crate) type Tx =
    futures_util::stream::SplitSink<tokio_tungstenite::WebSocketStream<ServerStream>, Message>;

/// Bounded media-lane capacity. 32 frames ≈ 640ms of audio at 50fps.
///
/// This must absorb *ingress bursts*, not just consumer stalls: TCP delivery
/// bunches frames after a network hiccup, so the rx loop can hand the relay a
/// multi-frame burst faster than the drain task is scheduled to run
/// (empirically, capacity 8 dropped ~2/3 of a same-host 100-frame burst).
/// Worst-case recovery staleness for a stalled-then-revived peer is the full
/// queue (~640ms), which the client-side jitter buffer trims. Frames are
/// shared `Bytes`, so a full queue holds refcounts, not buffer copies.
pub(crate) const MEDIA_LANE_CAPACITY: usize = 32;

/// Bounded signaling-lane capacity. Large enough that any well-behaved peer
/// never approaches it (chat bursts, multi-peer space join fan-out), but
/// bounded so a hung client is detected within seconds.
pub(crate) const SIGNALING_LANE_CAPACITY: usize = 256;

/// Outbound payload variants. Pre-serializing signaling means the drain task
/// does zero serde work — the hot audio path stays at a `Bytes` refcount bump.
pub(crate) enum OutboundFrame {
    Signaling(String),
    Media(Bytes),
    Ping,
}

/// Spawn the per-peer drain task. Owns the sink for the rest of the peer's
/// lifetime. Returns the `JoinHandle` so the connection handler can `await`
/// it during teardown for a clean WebSocket close.
pub(crate) fn spawn_peer_drain(
    peer_id: String,
    sink: Tx,
    signaling_rx: mpsc::Receiver<OutboundFrame>,
    media_rx: mpsc::Receiver<OutboundFrame>,
    disconnect: Arc<Notify>,
    metrics: Arc<ServerMetrics>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(drain_loop(
        peer_id,
        sink,
        signaling_rx,
        media_rx,
        disconnect,
        metrics,
    ))
}

async fn drain_loop(
    peer_id: String,
    mut sink: Tx,
    mut signaling_rx: mpsc::Receiver<OutboundFrame>,
    mut media_rx: mpsc::Receiver<OutboundFrame>,
    disconnect: Arc<Notify>,
    metrics: Arc<ServerMetrics>,
) {
    loop {
        let frame = tokio::select! {
            biased; // signaling first — reliable lane wins ties
            Some(f) = signaling_rx.recv() => f,
            Some(f) = media_rx.recv() => f,
            else => break, // both senders dropped → peer gone
        };
        let msg = match frame {
            OutboundFrame::Signaling(json) => Message::Text(json.into()),
            OutboundFrame::Media(bytes) => Message::Binary(bytes),
            OutboundFrame::Ping => Message::Ping(Bytes::new()),
        };
        if let Err(e) = sink.send(msg).await {
            log::debug!("Drain: send failed for {peer_id}: {e}");
            metrics
                .ws_overflow_disconnects_total
                .fetch_add(1, Ordering::Relaxed);
            break;
        }
    }
    // Either both channels closed (peer being torn down) or sink errored.
    // Signal the rx loop to break so handle_disconnect runs.
    disconnect.notify_one();
}

/// Push a media frame onto a peer's media lane. Lossy: returns `false` and
/// bumps `ws_frames_dropped_lossy_total` on Full or Closed — never blocks,
/// never disconnects. Audio frames arrive at 50fps, so a single drop is
/// indistinguishable from a normal packet loss.
pub(crate) fn try_send_media(peer: &crate::types::Peer, bytes: Bytes, metrics: &ServerMetrics) {
    match peer.media_tx.try_send(OutboundFrame::Media(bytes)) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(_)) => {
            metrics
                .ws_frames_dropped_lossy_total
                .fetch_add(1, Ordering::Relaxed);
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            // Peer is being torn down; nothing to do.
        }
    }
}

/// Push a WebSocket Ping onto the signaling lane. Returns true if the channel
/// is still open; false (and a Pong-task exit signal) if the peer is gone.
pub(crate) fn try_send_ping(peer: &crate::types::Peer) -> bool {
    match peer.signaling_tx.try_send(OutboundFrame::Ping) {
        Ok(()) => true,
        Err(mpsc::error::TrySendError::Full(_)) => true, // drop the ping; lane is overflowing
        Err(mpsc::error::TrySendError::Closed(_)) => false,
    }
}
