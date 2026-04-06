use anyhow::Result;
use audio_core::AudioEngine;
use net_control::NetworkClient;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};

/// Max Opus frame size in bytes (1024 is generous; typical voice frames are ~100-500 bytes).
const POOL_BUF_CAPACITY: usize = 1024;
/// Number of pre-allocated buffers in the pool. Must exceed the channel capacity (8)
/// so the capture callback always has a buffer available without allocating.
const POOL_SIZE: usize = 12;

pub struct MediaSession {
    audio: Arc<Mutex<AudioEngine>>,
    network: Arc<Mutex<NetworkClient>>,
    dropped_frames: Arc<AtomicU64>,
}

impl MediaSession {
    pub fn new(
        audio: Arc<Mutex<AudioEngine>>,
        network: Arc<Mutex<NetworkClient>>,
        dropped_frames: Arc<AtomicU64>,
    ) -> Self {
        Self {
            audio,
            network,
            dropped_frames,
        }
    }

    /// Wire audio capture -> dedicated sender task -> network.
    /// Uses a pre-allocated buffer pool for zero-allocation frame transfer.
    pub async fn start(&self) -> Result<()> {
        log::info!("MediaSession::start — wiring audio capture to network");

        // Bounded channel: backpressure after 8 frames (~160ms). Drops on overflow.
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(8);

        // Buffer return channel — sender task returns used buffers here for reuse.
        // Use std::sync::mpsc so the audio callback can try_recv without async.
        let (ret_tx, ret_rx) = std::sync::mpsc::channel::<Vec<u8>>();

        // Pre-allocate the buffer pool and seed the return channel
        for _ in 0..POOL_SIZE {
            let _ = ret_tx.send(Vec::with_capacity(POOL_BUF_CAPACITY));
        }

        // Single sender task — acquires network lock per frame with timeout
        let net = self.network.clone();
        let send_dropped = self.dropped_frames.clone();
        tokio::spawn(async move {
            let mut sent: u64 = 0;
            while let Some(data) = rx.recv().await {
                let Ok(net) = tokio::time::timeout(Duration::from_millis(100), net.lock()).await
                else {
                    // Network lock contended — skip this frame rather than blocking audio
                    send_dropped.fetch_add(1, Ordering::Relaxed);
                    // Return buffer to pool even on skip
                    let _ = ret_tx.send(data);
                    continue;
                };
                match net.send_audio(&data).await {
                    Ok(()) => {
                        sent += 1;
                        if sent == 1 {
                            log::info!("First audio frame sent to server ({} bytes)", data.len());
                        }
                    }
                    Err(e) => {
                        if sent == 0 {
                            log::error!("Failed to send first audio frame: {e}");
                        } else {
                            log::warn!("Failed to send audio: {e}");
                        }
                    }
                }
                // Return buffer to pool for reuse (clear but keep allocation)
                let _ = ret_tx.send(data);
            }
            log::info!("Audio sender task ended (sent {sent} frames total)");
        });

        // Audio capture callback receives &[u8] — takes a pooled buffer, copies data, sends it.
        // In steady state this is zero-allocation: buffers cycle through pool → channel → pool.
        let dropped = self.dropped_frames.clone();
        let audio = self.audio.lock().await;
        audio.set_on_encoded_frame(move |encoded_data: &[u8]| {
            // Try to get a recycled buffer from the pool (non-blocking)
            let mut buf = match ret_rx.try_recv() {
                Ok(mut b) => {
                    b.clear();
                    b
                }
                // Pool exhausted (all buffers in flight) — allocate as fallback.
                // This should be rare: only happens if consumer is >POOL_SIZE frames behind.
                Err(_) => Vec::with_capacity(POOL_BUF_CAPACITY),
            };
            buf.extend_from_slice(encoded_data);

            // try_send: if channel is full, drop the frame (better than blocking audio thread)
            if tx.try_send(buf).is_err() {
                dropped.fetch_add(1, Ordering::Relaxed);
            }
        });

        log::info!("MediaSession::start — audio pipeline fully wired");
        Ok(())
    }
}
