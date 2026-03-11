use anyhow::Result;
use audio_core::AudioEngine;
use net_control::NetworkClient;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
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
        Self { audio, network, dropped_frames }
    }

    /// Wire audio capture -> dedicated sender task -> network.
    /// Uses Arc<[u8]> for zero-copy frame sharing between capture callback and sender.
    pub async fn start(&self) -> Result<()> {
        // Bounded channel: backpressure after 8 frames (~160ms). Drops oldest on overflow.
        let (tx, mut rx) = mpsc::channel::<Arc<[u8]>>(8);

        // Single sender task — one lock acquisition per batch, not per frame
        let net = self.network.clone();
        tokio::spawn(async move {
            while let Some(data) = rx.recv().await {
                let net = net.lock().await;
                if let Err(e) = net.send_audio(&data).await {
                    log::warn!("Failed to send audio: {e}");
                }
            }
        });

        // Audio capture callback pushes Arc<[u8]> frames — no extra copy
        let dropped = self.dropped_frames.clone();
        {
            let audio = self.audio.lock().await;
            audio.set_on_encoded_frame(move |encoded_data| {
                // try_send: if channel is full, drop the frame (better than blocking audio thread)
                if tx.try_send(encoded_data).is_err() {
                    dropped.fetch_add(1, Ordering::Relaxed);
                }
            });
        }

        Ok(())
    }
}
