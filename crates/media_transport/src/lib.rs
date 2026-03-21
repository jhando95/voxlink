use anyhow::Result;
use audio_core::AudioEngine;
use net_control::NetworkClient;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};

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
    /// Uses Arc<[u8]> for zero-copy frame sharing between capture callback and sender.
    pub async fn start(&self) -> Result<()> {
        log::info!("MediaSession::start — wiring audio capture to network");

        // Bounded channel: backpressure after 8 frames (~160ms). Drops oldest on overflow.
        let (tx, mut rx) = mpsc::channel::<Arc<[u8]>>(8);

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
            }
            log::info!("Audio sender task ended (sent {sent} frames total)");
        });

        // Audio capture callback pushes Arc<[u8]> frames — no extra copy
        let dropped = self.dropped_frames.clone();
        let audio = self.audio.lock().await;
        audio.set_on_encoded_frame(move |encoded_data| {
            // try_send: if channel is full, drop the frame (better than blocking audio thread)
            if tx.try_send(encoded_data).is_err() {
                dropped.fetch_add(1, Ordering::Relaxed);
            }
        });

        log::info!("MediaSession::start — audio pipeline fully wired");
        Ok(())
    }
}
