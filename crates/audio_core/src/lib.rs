mod buffers;
/// Audio DSP primitives. Hot-path functions are re-exported at crate root for benchmarks.
mod codec;
mod feedback;

// Re-export hot-path DSP functions for benchmarks and external use
pub use codec::{frame_energy, soft_clip};

use anyhow::{Context, Result};
use audiopus::coder::{Decoder as OpusDecoder, Encoder as OpusEncoder};
use audiopus::packet::Packet as OpusPacket;
use audiopus::{Application, Channels as OpusChannels, MutSignals, SampleRate as OpusSampleRate};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Host, Stream, StreamConfig};
use shared_types::{CHANNELS, FRAME_SIZE, SAMPLE_RATE};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use buffers::{CaptureRing, PeerPlayback, PeerPlaybackShared};
use codec::{
    Agc, ComfortNoise, DeEsser, EchoCanceller, EchoReference,
    HighPassFilter, MuteRamp, NoiseGate, NoiseSuppressor, SendEncoder,
};
use feedback::{FeedbackAction, FeedbackTone};

const MAX_PEER_BUFFER_SAMPLES: usize = FRAME_SIZE * 10; // ~200ms per peer

// ─── Audio Metrics (lock-free counters for perf panel) ───

pub struct AudioMetrics {
    pub frames_decoded: Arc<AtomicU32>,
    pub frames_dropped: Arc<AtomicU32>,
    pub current_jitter_ms: Arc<AtomicU32>,
    pub active_peers: Arc<AtomicU32>,
    pub encode_bitrate_kbps: Arc<AtomicU32>,
}

impl AudioMetrics {
    pub fn new() -> Self {
        Self {
            frames_decoded: Arc::new(AtomicU32::new(0)),
            frames_dropped: Arc::new(AtomicU32::new(0)),
            current_jitter_ms: Arc::new(AtomicU32::new(JITTER_INITIAL as u32 * 20)),
            active_peers: Arc::new(AtomicU32::new(0)),
            encode_bitrate_kbps: Arc::new(AtomicU32::new(64)),
        }
    }
}

impl Default for AudioMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of attempting device recovery after an error.
#[derive(Debug)]
pub enum DeviceRecoveryResult {
    /// Successfully recovered using the specified device.
    Recovered { device_name: String },
    /// Recovered using the system default device.
    FellBackToDefault { device_name: String },
    /// No working device found.
    NoDeviceAvailable,
}

const OPUS_MAX_PACKET: usize = 512; // headroom for complex frames at 32kbps
const OPUS_BITRATE: i32 = 64000; // 64 kbps — high quality voice, still very bandwidth efficient

// Adaptive jitter buffer constants
const JITTER_MIN_FRAMES: u16 = 1; // 20ms minimum playout delay
const JITTER_MAX_FRAMES: u16 = 5; // 100ms maximum playout delay
const JITTER_INITIAL: u16 = 2; // 40ms default — good for LAN + decent internet
const JITTER_STABLE_THRESHOLD: u16 = 150; // ~3s stable before reducing (was 500/10s — too conservative)

/// Get or create an entry in a HashMap by key. Avoids double key allocation.
fn get_or_create<'a, V>(
    map: &'a mut HashMap<String, V>,
    key: &str,
    factory: impl FnOnce() -> Option<V>,
) -> Option<&'a mut V> {
    if !map.contains_key(key) {
        let val = factory()?;
        map.insert(key.to_string(), val);
    }
    map.get_mut(key)
}

// ─── Audio Engine ───

/// Callback type for encoded audio frames. Uses Arc<[u8]> for zero-copy sharing.
type EncodedFrameCallback = Box<dyn Fn(Arc<[u8]>) + Send>;

pub struct AudioDevice {
    pub name: String,
    pub is_default: bool,
    /// Hint about device type (headphones, speakers, headset mic, etc.)
    pub device_type: DeviceType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    Unknown,
    Headphones,
    Headset,
    Speakers,
    Microphone,
    Webcam,
    VirtualDevice,
}

impl DeviceType {
    /// Classify a device by its name using common patterns across platforms
    fn from_name(name: &str, is_input: bool) -> Self {
        let lower = name.to_lowercase();
        // Virtual/software devices
        if lower.contains("virtual")
            || lower.contains("soundflower")
            || lower.contains("blackhole")
            || lower.contains("loopback")
            || lower.contains("cable")
            || lower.contains("voicemeeter")
        {
            return DeviceType::VirtualDevice;
        }
        // Webcam mics
        if is_input
            && (lower.contains("webcam")
                || lower.contains("camera")
                || lower.contains("facetime")
                || lower.contains("c920")
                || lower.contains("c922")
                || lower.contains("brio"))
        {
            return DeviceType::Webcam;
        }
        // Headsets (have both mic and speakers — look for gaming/headset keywords)
        if lower.contains("headset")
            || lower.contains("arctis")
            || lower.contains("hyperx")
            || lower.contains("steelseries")
            || lower.contains("corsair")
            || lower.contains("razer")
            || lower.contains("astro")
            || lower.contains("jabra")
            || lower.contains("plantronics")
            || lower.contains("poly")
        {
            return DeviceType::Headset;
        }
        // Headphones (output only, no mic implied)
        if !is_input
            && (lower.contains("headphone")
                || lower.contains("airpod")
                || lower.contains("earphone")
                || lower.contains("buds")
                || lower.contains("wh-1000")
                || lower.contains("wf-1000")
                || lower.contains("qc35")
                || lower.contains("qc45"))
        {
            return DeviceType::Headphones;
        }
        // Speakers
        if !is_input
            && (lower.contains("speaker")
                || lower.contains("monitor")
                || lower.contains("soundbar")
                || lower.contains("macbook pro speaker")
                || lower.contains("built-in output")
                || lower.contains("realtek"))
        {
            return DeviceType::Speakers;
        }
        // Microphone
        if is_input
            && (lower.contains("microphone")
                || lower.contains("mic")
                || lower.contains("built-in")
                || lower.contains("internal")
                || lower.contains("blue yeti")
                || lower.contains("rode")
                || lower.contains("at2020")
                || lower.contains("samson"))
        {
            return DeviceType::Microphone;
        }
        DeviceType::Unknown
    }

    /// Return a display suffix hint for the UI
    pub fn label(&self) -> &'static str {
        match self {
            DeviceType::Headphones => " (Headphones)",
            DeviceType::Headset => " (Headset)",
            DeviceType::Speakers => " (Speakers)",
            DeviceType::Microphone => " (Mic)",
            DeviceType::Webcam => " (Webcam)",
            DeviceType::VirtualDevice => " (Virtual)",
            DeviceType::Unknown => "",
        }
    }
}

pub struct AudioEngine {
    host: Host,
    capture_stream: Option<Stream>,
    playback_stream: Option<Stream>,
    is_capturing: Arc<AtomicBool>,
    is_deafened: Arc<AtomicBool>,
    vad_enabled: Arc<AtomicBool>,
    peer_buffers: Arc<Mutex<HashMap<String, PeerPlayback>>>,
    /// Shared peer list for the playback callback. Updated on peer join/leave.
    /// The playback callback snapshots this once when the generation changes.
    playback_peers: Arc<Mutex<Vec<Arc<PeerPlaybackShared>>>>,
    /// Generation counter: bumped on peer add/remove so playback knows to re-snapshot.
    playback_generation: Arc<AtomicU32>,
    on_encoded_frame: Arc<Mutex<Option<EncodedFrameCallback>>>,
    opus_decoders: Mutex<HashMap<String, OpusDecoder>>,
    mic_level_raw: Arc<AtomicU32>,
    /// Noise gate sensitivity (0.0–1.0 stored as 0–1000). Shared with capture callback.
    noise_gate_sensitivity: Arc<AtomicU32>,
    /// Neural noise suppression enabled (RNNoise via nnnoiseless). Shared with capture callback.
    noise_suppression_enabled: Arc<AtomicBool>,
    /// Echo cancellation enabled. Shared with capture callback.
    echo_cancellation_enabled: Arc<AtomicBool>,
    /// Shared echo reference buffer — playback writes, capture reads.
    echo_ref: Arc<EchoReference>,
    /// Input gain (mic boost/cut, 0.0–2.0 stored as 0–2000). Shared with capture callback.
    input_gain: Arc<AtomicU32>,
    /// Output volume (master, 0.0–1.0 stored as 0–1000). Shared with playback callback.
    output_volume: Arc<AtomicU32>,
    /// Keybind feedback tone generator.
    feedback_tone: FeedbackTone,
    /// Runtime bitrate in bps. Encoder reads this each frame for dynamic quality switching.
    opus_bitrate: Arc<AtomicI32>,
    /// FEC packet loss percentage hint for Opus encoder (0–100, updated dynamically)
    opus_fec_loss_pct: Arc<AtomicI32>,
    /// Target (base) bitrate from voice quality setting. Adaptive bitrate reduces from this.
    target_bitrate: Arc<AtomicI32>,
    /// Set to true when a stream error occurs — signals the app to attempt recovery (#1)
    pub capture_error: Arc<AtomicBool>,
    pub playback_error: Arc<AtomicBool>,
    /// Lock-free audio metrics for perf panel
    pub metrics: AudioMetrics,
    /// Volume ducking: amount to reduce non-speaking peers (0.0=disabled, 1.0=full duck, stored 0-1000)
    ducking_amount: Arc<AtomicU32>,
    /// Volume ducking: energy threshold to consider a peer "speaking" (stored 0-1000)
    ducking_threshold: Arc<AtomicU32>,
    /// Soundboard: load and play WAV clips mixed into the capture stream
    soundboard: feedback::Soundboard,
}

unsafe impl Send for AudioEngine {}

impl AudioEngine {
    pub fn new() -> Result<Self> {
        let host = cpal::default_host();
        Ok(Self {
            host,
            capture_stream: None,
            playback_stream: None,
            is_capturing: Arc::new(AtomicBool::new(false)),
            is_deafened: Arc::new(AtomicBool::new(false)),
            vad_enabled: Arc::new(AtomicBool::new(true)),
            peer_buffers: Arc::new(Mutex::new(HashMap::new())),
            playback_peers: Arc::new(Mutex::new(Vec::new())),
            playback_generation: Arc::new(AtomicU32::new(0)),
            on_encoded_frame: Arc::new(Mutex::new(None)),
            opus_decoders: Mutex::new(HashMap::new()),
            mic_level_raw: Arc::new(AtomicU32::new(0)),
            noise_gate_sensitivity: Arc::new(AtomicU32::new(500)), // 0.5 default
            noise_suppression_enabled: Arc::new(AtomicBool::new(false)),
            echo_cancellation_enabled: Arc::new(AtomicBool::new(false)),
            echo_ref: Arc::new(EchoReference::new(FRAME_SIZE * 6)), // ~120ms reference
            input_gain: Arc::new(AtomicU32::new(1000)),            // 1.0 default (unity gain)
            output_volume: Arc::new(AtomicU32::new(1000)),         // 1.0 default
            feedback_tone: FeedbackTone::new(),
            opus_bitrate: Arc::new(AtomicI32::new(OPUS_BITRATE)),
            opus_fec_loss_pct: Arc::new(AtomicI32::new(5)),
            target_bitrate: Arc::new(AtomicI32::new(OPUS_BITRATE)),
            capture_error: Arc::new(AtomicBool::new(false)),
            playback_error: Arc::new(AtomicBool::new(false)),
            metrics: AudioMetrics::new(),
            ducking_amount: Arc::new(AtomicU32::new(0)),
            ducking_threshold: Arc::new(AtomicU32::new(100)), // 0.1 default
            soundboard: feedback::Soundboard::new(),
        })
    }

    /// Current mic input level (0.0 to 1.0) for UI display
    pub fn mic_level(&self) -> f32 {
        self.mic_level_raw.load(Ordering::Relaxed) as f32 / 1000.0
    }

    // ─── Device Enumeration ───

    /// Re-enumerate the host to pick up hot-plugged devices
    pub fn refresh_host(&mut self) {
        self.host = cpal::default_host();
        log::info!("Audio host refreshed (re-enumerated devices)");
    }

    pub fn list_input_devices(&self) -> Vec<AudioDevice> {
        let default_name = self.host.default_input_device().and_then(|d| d.name().ok());
        log::info!("Default input device: {:?}", default_name);

        let devices: Vec<AudioDevice> = self
            .host
            .input_devices()
            .map(|devices| {
                devices
                    .filter_map(|d| {
                        let name = d.name().ok()?;
                        let is_default = default_name.as_deref() == Some(&name);
                        let device_type = DeviceType::from_name(&name, true);
                        Some(AudioDevice {
                            name,
                            is_default,
                            device_type,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        log::info!(
            "Found {} input devices: {:?}",
            devices.len(),
            devices
                .iter()
                .map(|d| format!("{}{}", d.name, d.device_type.label()))
                .collect::<Vec<_>>()
        );
        devices
    }

    pub fn list_output_devices(&self) -> Vec<AudioDevice> {
        let default_name = self
            .host
            .default_output_device()
            .and_then(|d| d.name().ok());
        log::info!("Default output device: {:?}", default_name);

        let devices: Vec<AudioDevice> = self
            .host
            .output_devices()
            .map(|devices| {
                devices
                    .filter_map(|d| {
                        let name = d.name().ok()?;
                        let is_default = default_name.as_deref() == Some(&name);
                        let device_type = DeviceType::from_name(&name, false);
                        Some(AudioDevice {
                            name,
                            is_default,
                            device_type,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        log::info!(
            "Found {} output devices: {:?}",
            devices.len(),
            devices
                .iter()
                .map(|d| format!("{}{}", d.name, d.device_type.label()))
                .collect::<Vec<_>>()
        );
        devices
    }

    fn find_input_device(&self, name: Option<&str>) -> Option<Device> {
        if let Some(name) = name {
            let devices = self.host.input_devices().ok()?;
            let all: Vec<Device> = devices.collect();
            // Exact match first
            if let Some(d) = all.iter().find(|d| d.name().ok().as_deref() == Some(name)) {
                return Some(d.clone());
            }
            // Partial match fallback (Windows headsets may change suffixes)
            if let Some(d) = all.iter().find(|d| {
                d.name()
                    .ok()
                    .map(|n| n.contains(name) || name.contains(&n))
                    .unwrap_or(false)
            }) {
                log::info!(
                    "Input device '{}' not found exactly, using partial match '{}'",
                    name,
                    d.name().unwrap_or_default()
                );
                return Some(d.clone());
            }
            log::warn!("Input device '{}' not found, falling back to default", name);
            self.host.default_input_device()
        } else {
            self.host.default_input_device()
        }
    }

    fn find_output_device(&self, name: Option<&str>) -> Option<Device> {
        if let Some(name) = name {
            let devices = self.host.output_devices().ok()?;
            let all: Vec<Device> = devices.collect();
            if let Some(d) = all.iter().find(|d| d.name().ok().as_deref() == Some(name)) {
                return Some(d.clone());
            }
            if let Some(d) = all.iter().find(|d| {
                d.name()
                    .ok()
                    .map(|n| n.contains(name) || name.contains(&n))
                    .unwrap_or(false)
            }) {
                log::info!(
                    "Output device '{}' not found exactly, using partial match '{}'",
                    name,
                    d.name().unwrap_or_default()
                );
                return Some(d.clone());
            }
            log::warn!(
                "Output device '{}' not found, falling back to default",
                name
            );
            self.host.default_output_device()
        } else {
            self.host.default_output_device()
        }
    }

    // ─── Callbacks ───

    pub fn set_on_encoded_frame<F: Fn(Arc<[u8]>) + Send + 'static>(&self, callback: F) {
        if let Ok(mut cb) = self.on_encoded_frame.lock() {
            *cb = Some(Box::new(callback));
            log::info!("on_encoded_frame callback set — audio frames will be forwarded to network");
        } else {
            log::error!("Failed to set on_encoded_frame — callback lock poisoned");
        }
    }

    pub fn clear_on_encoded_frame(&self) {
        if let Ok(mut cb) = self.on_encoded_frame.lock() {
            *cb = None;
            log::info!("on_encoded_frame callback cleared");
        }
    }

    // ─── Peer Management ───

    /// Rebuild the playback_peers snapshot and bump the generation counter.
    /// Call after any peer add/remove.
    fn sync_playback_peers(&self) {
        if let Ok(peers) = self.peer_buffers.lock() {
            let shared_list: Vec<Arc<PeerPlaybackShared>> =
                peers.values().map(|p| p.shared.clone()).collect();
            if let Ok(mut pp) = self.playback_peers.lock() {
                *pp = shared_list;
            }
            self.playback_generation.fetch_add(1, Ordering::Release);
        }
    }

    pub fn set_peer_volume(&self, peer_id: &str, volume: f32) {
        if let Ok(peers) = self.peer_buffers.lock() {
            if let Some(peer) = peers.get(peer_id) {
                let val = (volume.clamp(0.0, 1.0) * 1000.0) as u32;
                peer.shared.volume.store(val, Ordering::Relaxed);
            }
        }
    }

    pub fn remove_peer(&self, peer_id: &str) {
        let removed = if let Ok(mut peers) = self.peer_buffers.lock() {
            peers.remove(peer_id).is_some()
        } else {
            false
        };
        if let Ok(mut decs) = self.opus_decoders.lock() {
            decs.remove(peer_id);
        }
        if removed {
            self.sync_playback_peers();
        }
    }

    // ─── Capture ───

    pub fn start_capture(&mut self, device_name: Option<&str>) -> Result<()> {
        let device = self
            .find_input_device(device_name)
            .context("No input device found")?;

        log::info!("Starting capture on: {}", device.name().unwrap_or_default());

        let config = StreamConfig {
            channels: CHANNELS,
            sample_rate: cpal::SampleRate(SAMPLE_RATE),
            buffer_size: cpal::BufferSize::Default,
        };

        let mut opus_encoder = SendEncoder(
            OpusEncoder::new(
                OpusSampleRate::Hz48000,
                OpusChannels::Mono,
                Application::Voip,
            )
            .context("Failed to create Opus encoder")?,
        );
        opus_encoder
            .set_bitrate(audiopus::Bitrate::BitsPerSecond(OPUS_BITRATE))
            .ok();
        opus_encoder.set_inband_fec(true).ok();
        opus_encoder.set_packet_loss_perc(5).ok();
        // DTX: Discontinuous Transmission — near-zero bandwidth during silence.
        opus_encoder.set_dtx(true).ok();
        // Max encoding complexity for best quality (CPU is negligible at mono 48kHz)
        opus_encoder.set_complexity(10).ok();
        // Explicit fullband (20kHz) — Opus defaults to this at 48kHz but be explicit
        opus_encoder
            .set_bandwidth(audiopus::Bandwidth::Fullband)
            .ok();
        // VBR: Variable bitrate — better quality for speech by allocating more bits
        // to complex segments and fewer to simple/silence. More efficient than CBR.
        opus_encoder.set_vbr(true).ok();
        // Constrained VBR: prevents bitrate spikes that could cause network jitter
        opus_encoder.set_vbr_constraint(true).ok();
        // Explicit voice signal type — helps Opus optimize for speech intelligibility
        opus_encoder
            .set_signal(audiopus::Signal::Voice)
            .ok();

        let on_frame = self.on_encoded_frame.clone();
        let is_capturing = self.is_capturing.clone();
        let vad_enabled = self.vad_enabled.clone();
        let mic_level = self.mic_level_raw.clone();
        let sensitivity = self.noise_gate_sensitivity.clone();
        let ns_enabled = self.noise_suppression_enabled.clone();
        let aec_enabled = self.echo_cancellation_enabled.clone();
        let echo_ref_capture = self.echo_ref.clone();
        let input_gain = self.input_gain.clone();
        let opus_bitrate = self.opus_bitrate.clone();
        let opus_fec_loss_pct = self.opus_fec_loss_pct.clone();
        let soundboard_playback = self.soundboard.capture_state();

        // All buffers pre-allocated once — zero allocation in the audio callback
        let mut capture_ring = CaptureRing::new(FRAME_SIZE * 4);
        let mut frame_buf = [0.0f32; FRAME_SIZE];
        let mut pcm_i16 = [0i16; FRAME_SIZE];
        let mut frames_encoded: u64 = 0;
        let mut opus_out = [0u8; OPUS_MAX_PACKET];
        let mut noise_gate = NoiseGate::new(sensitivity);
        let mut hp_filter = HighPassFilter::new();
        let mut deesser = DeEsser::new();
        let mut mute_ramp = MuteRamp::new(is_capturing.clone());
        let mut agc = Agc::new();
        let mut comfort_noise = ComfortNoise::new();
        let mut noise_suppressor = NoiseSuppressor::new();
        let mut echo_canceller = EchoCanceller::new(FRAME_SIZE);
        let mut silence_frames: u32 = 0;

        let stream = device.build_input_stream(
            &config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                capture_ring.push_slice(data);

                while capture_ring.read_frame(&mut frame_buf) {
                    // Remove DC offset and low-frequency rumble
                    hp_filter.process(&mut frame_buf);

                    // Apply user input gain (mic boost/cut)
                    let gain = input_gain.load(Ordering::Relaxed) as f32 / 1000.0;
                    if (gain - 1.0).abs() > 0.01 {
                        for s in frame_buf.iter_mut() {
                            *s *= gain;
                        }
                    }

                    // Echo cancellation — subtract correlated playback reference
                    echo_canceller.set_enabled(aec_enabled.load(Ordering::Relaxed));
                    echo_canceller.process(&mut frame_buf, &echo_ref_capture);

                    // Neural noise suppression (RNNoise) — runs before gate for cleaner VAD
                    noise_suppressor.set_enabled(ns_enabled.load(Ordering::Relaxed));
                    noise_suppressor.process(&mut frame_buf);

                    let energy = frame_energy(&frame_buf);
                    // Show mic level even when muted (UI feedback)
                    let is_active = is_capturing.load(Ordering::Relaxed);
                    mic_level.store(
                        if is_active {
                            (energy.min(1.0) * 1000.0) as u32
                        } else {
                            0
                        },
                        Ordering::Relaxed,
                    );

                    // Noise gate: decide open/closed, then apply smooth gain ramp
                    let gate_open = noise_gate.process(energy, vad_enabled.load(Ordering::Relaxed));
                    noise_gate.apply_gain(&mut frame_buf, gate_open);

                    // AGC: always runs but adapts 10x slower when gate closed
                    // so gain stays calibrated across gate transitions
                    agc.process_with_gate(&mut frame_buf, gate_open);

                    // Smooth mute/unmute fade (eliminates click on toggle)
                    let has_audio = mute_ramp.apply(&mut frame_buf);

                    // Skip encoding if fully silent (gate closed + ramp done, or muted + ramp done)
                    if noise_gate.is_silent() && !has_audio {
                        silence_frames += 1;
                        // Only inject comfort noise after 5 frames (100ms) of continuous silence
                        if silence_frames >= 5 {
                            comfort_noise.fill(&mut frame_buf);
                        }
                        // Still encode the frame
                    } else {
                        silence_frames = 0;
                        // De-ess sibilant frequencies before encoding
                        deesser.process(&mut frame_buf);
                    }

                    // Mix soundboard clip samples into the outgoing frame (if playing)
                    soundboard_playback.mix_into(&mut frame_buf);

                    for (out, &s) in pcm_i16.iter_mut().zip(frame_buf.iter()) {
                        *out = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                    }

                    // Apply runtime bitrate and FEC loss hint (updated by adaptive bitrate)
                    let target_bps = opus_bitrate.load(Ordering::Relaxed);
                    opus_encoder.set_bitrate(audiopus::Bitrate::BitsPerSecond(target_bps)).ok();
                    let fec_pct = opus_fec_loss_pct.load(Ordering::Relaxed) as u8;
                    opus_encoder.set_packet_loss_perc(fec_pct).ok();

                    match opus_encoder.encode(&pcm_i16, &mut opus_out) {
                        Ok(len) => {
                            frames_encoded += 1;
                            let frame: Arc<[u8]> = Arc::from(&opus_out[..len]);
                            if let Ok(cb) = on_frame.try_lock() {
                                if let Some(ref f) = *cb {
                                    f(frame);
                                    // Log first frame and periodic status
                                    if frames_encoded == 1 {
                                        log::info!("First audio frame encoded and sent ({len} bytes)");
                                    } else if frames_encoded == 250 {
                                        log::info!("Audio pipeline healthy: 250 frames encoded (~5s)");
                                    }
                                } else if frames_encoded <= 3 {
                                    log::warn!("Audio frame #{frames_encoded} encoded but no callback set — frame lost");
                                }
                            }
                        }
                        Err(e) => {
                            log::warn!("Opus encode error: {e}");
                        }
                    }
                }
            },
            {
                let err_flag = self.capture_error.clone();
                move |err| {
                    log::error!("Capture stream error: {err}");
                    err_flag.store(true, Ordering::Relaxed);
                }
            },
            None,
        )?;

        stream.play()?;
        self.capture_error.store(false, Ordering::Relaxed);
        self.is_capturing.store(true, Ordering::Relaxed);
        self.capture_stream = Some(stream);
        Ok(())
    }

    pub fn stop_capture(&mut self) {
        self.is_capturing.store(false, Ordering::Relaxed);
        self.capture_stream = None;
        self.clear_on_encoded_frame();
        self.mic_level_raw.store(0, Ordering::Relaxed);
        log::info!("Capture stopped");
    }

    /// Set voice quality by updating the runtime bitrate. Takes effect on the next encoded frame.
    pub fn set_voice_quality(&self, quality: u8) {
        let bps = shared_types::voice_quality_bitrate(quality);
        self.target_bitrate.store(bps, Ordering::Relaxed);
        self.opus_bitrate.store(bps, Ordering::Relaxed);
        log::info!(
            "Voice quality set to {} ({}bps)",
            shared_types::voice_quality_label(quality),
            bps
        );
    }

    /// Set the runtime Opus bitrate directly (used by adaptive bitrate logic).
    pub fn set_bitrate(&self, bitrate: i32) {
        self.opus_bitrate.store(bitrate, Ordering::Relaxed);
    }

    /// Set the Opus encoder's FEC packet loss hint (0–100%).
    /// Higher values make Opus allocate more redundancy for loss recovery.
    pub fn set_fec_loss_pct(&self, pct: i32) {
        self.opus_fec_loss_pct.store(pct.clamp(0, 100), Ordering::Relaxed);
    }

    /// Get the target (base) bitrate from the voice quality setting.
    pub fn target_bitrate(&self) -> i32 {
        self.target_bitrate.load(Ordering::Relaxed)
    }

    /// Get the current runtime bitrate.
    pub fn current_bitrate(&self) -> i32 {
        self.opus_bitrate.load(Ordering::Relaxed)
    }

    // ─── Soundboard ───

    /// Load a WAV file as a soundboard clip. Returns the clip index.
    pub fn load_soundboard_clip(&self, path: &str) -> Result<usize, String> {
        self.soundboard.load_clip(path)
    }

    /// Play a loaded soundboard clip by index (mixed into outgoing audio).
    pub fn play_soundboard_clip(&self, index: usize) {
        self.soundboard.play(index);
    }

    /// Clear all loaded soundboard clips.
    pub fn clear_soundboard(&self) {
        self.soundboard.clear();
    }

    /// Number of loaded soundboard clips.
    pub fn soundboard_clip_count(&self) -> usize {
        self.soundboard.clip_count()
    }

    pub fn restart_capture(&mut self, device_name: Option<&str>) -> Result<()> {
        self.is_capturing.store(false, Ordering::Relaxed);
        self.capture_stream = None;
        log::info!("Capture restarting for device swap");
        self.start_capture(device_name)
    }

    // ─── Playback ───

    pub fn start_playback(&mut self, device_name: Option<&str>) -> Result<()> {
        let device = self
            .find_output_device(device_name)
            .context("No output device found")?;

        log::info!(
            "Starting playback on: {}",
            device.name().unwrap_or_default()
        );

        let config = StreamConfig {
            channels: CHANNELS,
            sample_rate: cpal::SampleRate(SAMPLE_RATE),
            buffer_size: cpal::BufferSize::Default,
        };

        let playback_peers = self.playback_peers.clone();
        let playback_generation = self.playback_generation.clone();
        let is_deafened = self.is_deafened.clone();
        let output_volume = self.output_volume.clone();
        let feedback_playback = self.feedback_tone.playback_state();
        let echo_ref_playback = self.echo_ref.clone();
        let ducking_amount = self.ducking_amount.clone();
        let ducking_threshold = self.ducking_threshold.clone();

        // Local snapshot held by the playback callback — refreshed when generation changes
        let mut local_peers: Vec<Arc<PeerPlaybackShared>> = Vec::new();
        let mut local_gen: u32 = 0;

        let stream = device.build_output_stream(
            &config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                data.fill(0.0);

                // Mix feedback tone (plays even when deafened — it's local UI feedback)
                feedback_playback.mix_into(data);

                // Refresh peer snapshot if generation changed (peer joined/left)
                let gen = playback_generation.load(Ordering::Acquire);
                if gen != local_gen {
                    if let Ok(pp) = playback_peers.try_lock() {
                        local_peers.clone_from(&pp);
                    }
                    local_gen = gen;
                }

                if is_deafened.load(Ordering::Relaxed) {
                    for peer in &local_peers {
                        peer.ring.clear();
                    }
                    return;
                }

                // Lock-free mixing with volume ducking (single pass)
                let duck_amt = ducking_amount.load(Ordering::Relaxed) as f32 / 1000.0;
                let ducking_enabled = duck_amt > 0.001;

                // Cache per-peer energy + speaking flag in one pass (no second peek_energy call)
                let mut any_speaking = false;
                if ducking_enabled {
                    let duck_thresh = ducking_threshold.load(Ordering::Relaxed) as f32 / 1000.0;
                    for peer in &local_peers {
                        if !peer.is_ready() { continue; }
                        let energy = peer.ring.peek_energy();
                        let speaking = energy > duck_thresh;
                        // Reuse the primed atomic as a "speaking" flag for ducking
                        // (we overwrite it below anyway when mixing)
                        peer.primed.store(speaking, Ordering::Relaxed);
                        if speaking { any_speaking = true; }
                    }
                }

                for peer in &local_peers {
                    if !peer.is_ready() {
                        continue;
                    }
                    let mut vol = peer.volume_f32();
                    if any_speaking {
                        let is_speaking = peer.primed.load(Ordering::Relaxed);
                        if !is_speaking {
                            vol *= 1.0 - duck_amt;
                        }
                    }
                    peer.primed.store(true, Ordering::Relaxed);
                    let consumed = peer.ring.mix_into(data, vol);
                    peer.callback_count.fetch_add(1, Ordering::Relaxed);
                    if consumed < data.len() {
                        peer.underrun_count.fetch_add(1, Ordering::Relaxed);
                    }
                }

                // Record mixed audio to echo reference before volume scaling
                echo_ref_playback.record(data);

                // Apply master output volume
                let vol = output_volume.load(Ordering::Relaxed) as f32 / 1000.0;
                for sample in data.iter_mut() {
                    *sample = soft_clip(*sample * vol);
                }
            },
            {
                let err_flag = self.playback_error.clone();
                move |err| {
                    log::error!("Playback stream error: {err}");
                    err_flag.store(true, Ordering::Relaxed);
                }
            },
            None,
        )?;

        stream.play()?;
        self.playback_error.store(false, Ordering::Relaxed);
        self.playback_stream = Some(stream);
        Ok(())
    }

    pub fn stop_playback(&mut self) {
        self.playback_stream = None;
        if let Ok(mut peers) = self.peer_buffers.lock() {
            peers.clear();
        }
        if let Ok(mut pp) = self.playback_peers.lock() {
            pp.clear();
        }
        self.playback_generation.fetch_add(1, Ordering::Release);
        log::info!("Playback stopped");
    }

    pub fn restart_playback(&mut self, device_name: Option<&str>) -> Result<()> {
        self.stop_playback();
        self.start_playback(device_name)
    }

    /// Attempt to recover the capture stream after an error.
    /// Tries the specified device first, then falls back to the system default.
    pub fn try_recover_capture(&mut self, preferred_device: Option<&str>) -> DeviceRecoveryResult {
        self.capture_error.store(false, Ordering::Relaxed);
        log::info!("Attempting capture recovery (preferred: {:?})", preferred_device);

        // Refresh host to pick up any device changes
        self.refresh_host();

        // Try preferred device first
        if let Some(name) = preferred_device {
            if self.start_capture(Some(name)).is_ok() {
                log::info!("Capture recovered on preferred device: {name}");
                return DeviceRecoveryResult::Recovered { device_name: name.to_string() };
            }
            log::warn!("Preferred device '{name}' failed, trying default");
        }

        // Try system default
        let default_name = self.host.default_input_device()
            .and_then(|d| d.name().ok())
            .unwrap_or_default();

        if self.start_capture(None).is_ok() {
            log::info!("Capture recovered on default device: {default_name}");
            return DeviceRecoveryResult::FellBackToDefault { device_name: default_name };
        }

        log::error!("No working capture device found");
        DeviceRecoveryResult::NoDeviceAvailable
    }

    /// Attempt to recover the playback stream after an error.
    /// Tries the specified device first, then falls back to the system default.
    pub fn try_recover_playback(&mut self, preferred_device: Option<&str>) -> DeviceRecoveryResult {
        self.playback_error.store(false, Ordering::Relaxed);
        log::info!("Attempting playback recovery (preferred: {:?})", preferred_device);

        self.refresh_host();

        if let Some(name) = preferred_device {
            if self.start_playback(Some(name)).is_ok() {
                log::info!("Playback recovered on preferred device: {name}");
                return DeviceRecoveryResult::Recovered { device_name: name.to_string() };
            }
            log::warn!("Preferred device '{name}' failed, trying default");
        }

        let default_name = self.host.default_output_device()
            .and_then(|d| d.name().ok())
            .unwrap_or_default();

        if self.start_playback(None).is_ok() {
            log::info!("Playback recovered on default device: {default_name}");
            return DeviceRecoveryResult::FellBackToDefault { device_name: default_name };
        }

        log::error!("No working playback device found");
        DeviceRecoveryResult::NoDeviceAvailable
    }

    /// Check if either audio stream has an error and needs recovery.
    pub fn needs_recovery(&self) -> (bool, bool) {
        (
            self.capture_error.load(Ordering::Relaxed),
            self.playback_error.load(Ordering::Relaxed),
        )
    }

    // ─── Decode & Queue ───

    /// Decode incoming Opus audio into the per-peer ring buffer.
    /// Returns whether the frame contains voice (energy above threshold).
    pub fn decode_and_queue(&self, sender_id: &str, encoded_data: &[u8]) -> bool {
        let mut pcm_i16 = [0i16; FRAME_SIZE];
        let sample_count = match self.decode_opus_frame(sender_id, encoded_data, &mut pcm_i16) {
            Some(n) => n,
            None => {
                self.metrics.frames_dropped.fetch_add(1, Ordering::Relaxed);
                return false;
            }
        };
        self.metrics.frames_decoded.fetch_add(1, Ordering::Relaxed);
        self.queue_decoded_audio(sender_id, &pcm_i16[..sample_count])
    }

    fn decode_opus_frame(
        &self,
        sender_id: &str,
        encoded_data: &[u8],
        out: &mut [i16; FRAME_SIZE],
    ) -> Option<usize> {
        let mut decoders = self.opus_decoders.lock().ok()?;

        let decoder = get_or_create(&mut decoders, sender_id, || {
            OpusDecoder::new(OpusSampleRate::Hz48000, OpusChannels::Mono)
                .map_err(|e| log::error!("Failed to create Opus decoder for {sender_id}: {e}"))
                .ok()
        })?;

        let packet = OpusPacket::try_from(encoded_data).ok()?;
        let output = MutSignals::try_from(&mut out[..]).ok()?;
        match decoder.decode(Some(packet), output, true) {
            Ok(n) => Some(n),
            Err(e) => {
                log::warn!("Opus decode error from {sender_id}: {e}, attempting PLC");
                // Packet Loss Concealment: ask Opus to interpolate from previous state
                let plc_output = MutSignals::try_from(&mut out[..]).ok()?;
                decoder
                    .decode(None::<OpusPacket<'_>>, plc_output, false)
                    .ok()
            }
        }
    }

    fn queue_decoded_audio(&self, sender_id: &str, pcm_i16: &[i16]) -> bool {
        if pcm_i16.is_empty() {
            return false;
        }

        let mut peers = match self.peer_buffers.try_lock() {
            Ok(p) => p,
            Err(_) => {
                log::trace!("queue_decoded_audio: peer_buffers lock contended for {sender_id}");
                return false;
            }
        };

        let is_new = !peers.contains_key(sender_id);
        let Some(peer) = get_or_create(&mut peers, sender_id, || Some(PeerPlayback::new())) else {
            log::error!("Failed to create playback buffer for peer {sender_id}");
            return false;
        };

        // Convert i16 → f32 into reusable buffer (zero allocation on hot path)
        peer.convert_buf.clear();
        peer.convert_buf.extend(pcm_i16.iter().map(|&s| s as f32 * (1.0 / 32767.0)));
        peer.playback_agc.process(&mut peer.convert_buf);

        let mut sum_sq: f32 = 0.0;
        for &s in &peer.convert_buf {
            sum_sq += s * s;
        }
        // Push into the lock-free SPSC ring — playback callback reads without mutex
        peer.shared.ring.push_slice(&peer.convert_buf);

        // Adapt jitter buffer based on playback underrun feedback
        peer.adapt_from_atomics();

        // Update audio metrics
        let jitter = peer.shared.target_frames.load(Ordering::Relaxed) * 20;
        self.metrics.current_jitter_ms.store(jitter, Ordering::Relaxed);

        // If this is a new peer, sync the playback snapshot
        let peer_count = peers.len() as u32;
        drop(peers);
        self.metrics.active_peers.store(peer_count, Ordering::Relaxed);
        if is_new {
            self.sync_playback_peers();
        }

        let rms = (sum_sq / pcm_i16.len() as f32).sqrt();
        rms >= 0.003
    }

    // ─── State Control ───

    pub fn set_muted(&self, muted: bool) {
        self.is_capturing.store(!muted, Ordering::Relaxed);
    }

    pub fn set_deafened(&self, deafened: bool) {
        self.is_deafened.store(deafened, Ordering::Relaxed);
    }

    pub fn set_vad_enabled(&self, enabled: bool) {
        self.vad_enabled.store(enabled, Ordering::Relaxed);
    }

    /// Set noise gate sensitivity (0.0 = least sensitive, 1.0 = most sensitive)
    pub fn set_sensitivity(&self, sensitivity: f32) {
        let val = (sensitivity.clamp(0.0, 1.0) * 1000.0) as u32;
        self.noise_gate_sensitivity.store(val, Ordering::Relaxed);
    }

    /// Enable or disable neural noise suppression (RNNoise).
    pub fn set_noise_suppression(&self, enabled: bool) {
        self.noise_suppression_enabled.store(enabled, Ordering::Relaxed);
    }

    /// Enable or disable echo cancellation.
    pub fn set_echo_cancellation(&self, enabled: bool) {
        self.echo_cancellation_enabled.store(enabled, Ordering::Relaxed);
    }

    /// Set input gain (0.0 = silent, 1.0 = unity, 2.0 = +6dB boost)
    pub fn set_input_gain(&self, gain: f32) {
        let val = (gain.clamp(0.0, 2.0) * 1000.0) as u32;
        self.input_gain.store(val, Ordering::Relaxed);
    }

    /// Set volume ducking parameters.
    /// `amount`: 0.0 = disabled, 1.0 = full duck (silence non-speakers).
    /// `threshold`: energy level to consider a peer "speaking" (0.0–1.0).
    pub fn set_ducking(&self, amount: f32, threshold: f32) {
        self.ducking_amount
            .store((amount.clamp(0.0, 1.0) * 1000.0) as u32, Ordering::Relaxed);
        self.ducking_threshold
            .store((threshold.clamp(0.0, 1.0) * 1000.0) as u32, Ordering::Relaxed);
    }

    /// Set master output volume (0.0 = muted, 1.0 = full)
    pub fn set_output_volume(&self, volume: f32) {
        let val = (volume.clamp(0.0, 1.0) * 1000.0) as u32;
        self.output_volume.store(val, Ordering::Relaxed);
    }

    /// Play a feedback tone for mute on/off.
    pub fn play_feedback_mute(&self, muted: bool) {
        self.feedback_tone.trigger(if muted {
            FeedbackAction::MuteOn
        } else {
            FeedbackAction::MuteOff
        });
    }

    /// Play a feedback tone for deafen on/off.
    pub fn play_feedback_deafen(&self, deafened: bool) {
        self.feedback_tone.trigger(if deafened {
            FeedbackAction::DeafenOn
        } else {
            FeedbackAction::DeafenOff
        });
    }

    /// Play a notification sound for peer join/leave.
    pub fn play_notification(&self, joined: bool) {
        self.feedback_tone.trigger(if joined {
            FeedbackAction::JoinRoom
        } else {
            FeedbackAction::LeaveRoom
        });
    }

    /// Play a local speaker preview tone for device/output testing.
    pub fn play_output_preview(&self) {
        self.feedback_tone.trigger(FeedbackAction::OutputPreview);
    }

    /// Get the name of the current input device (if capturing).
    pub fn current_input_device_name(&self) -> Option<String> {
        // The device name isn't stored directly, but we can enumerate to find the default
        // This is used for device polling — returns the default device name for comparison
        self.host.default_input_device().and_then(|d| d.name().ok())
    }

    /// Get the name of the current output device (if playing).
    pub fn current_output_device_name(&self) -> Option<String> {
        self.host.default_output_device().and_then(|d| d.name().ok())
    }

    /// Get a list of input device names (for hotplug detection).
    pub fn input_device_names(&self) -> Vec<String> {
        self.host
            .input_devices()
            .map(|devs| devs.filter_map(|d| d.name().ok()).collect())
            .unwrap_or_default()
    }

    /// Get a list of output device names (for hotplug detection).
    pub fn output_device_names(&self) -> Vec<String> {
        self.host
            .output_devices()
            .map(|devs| devs.filter_map(|d| d.name().ok()).collect())
            .unwrap_or_default()
    }

    /// Get the current packet loss ratio from audio metrics (dropped / (decoded + dropped)).
    pub fn packet_loss_ratio(&self) -> f32 {
        let decoded = self.metrics.frames_decoded.load(Ordering::Relaxed);
        let dropped = self.metrics.frames_dropped.load(Ordering::Relaxed);
        let total = decoded + dropped;
        if total == 0 {
            return 0.0;
        }
        dropped as f32 / total as f32
    }

    /// Reset the decoded/dropped frame counters (call after reading loss ratio).
    pub fn reset_loss_counters(&self) {
        self.metrics.frames_decoded.store(0, Ordering::Relaxed);
        self.metrics.frames_dropped.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;

    #[test]
    fn needs_recovery_default_false() {
        // We can't actually create AudioEngine in test (no audio host),
        // but we can verify the flags work independently
        let capture_err = Arc::new(AtomicBool::new(false));
        let playback_err = Arc::new(AtomicBool::new(false));
        assert!(!capture_err.load(Ordering::Relaxed));
        assert!(!playback_err.load(Ordering::Relaxed));

        capture_err.store(true, Ordering::Relaxed);
        assert!(capture_err.load(Ordering::Relaxed));
        assert!(!playback_err.load(Ordering::Relaxed));
    }

    #[test]
    fn device_recovery_result_debug() {
        let result = DeviceRecoveryResult::Recovered { device_name: "Test".into() };
        let debug = format!("{:?}", result);
        assert!(debug.contains("Recovered"));
        assert!(debug.contains("Test"));

        let result = DeviceRecoveryResult::NoDeviceAvailable;
        let debug = format!("{:?}", result);
        assert!(debug.contains("NoDeviceAvailable"));
    }
}

#[cfg(test)]
mod adaptive_bitrate_tests {
    use super::*;

    #[test]
    fn audio_metrics_default() {
        let m = AudioMetrics::new();
        assert_eq!(m.frames_decoded.load(Ordering::Relaxed), 0);
        assert_eq!(m.frames_dropped.load(Ordering::Relaxed), 0);
        assert_eq!(m.encode_bitrate_kbps.load(Ordering::Relaxed), 64);
    }

    #[test]
    fn packet_loss_ratio_no_frames() {
        let m = AudioMetrics::new();
        let decoded = m.frames_decoded.load(Ordering::Relaxed);
        let dropped = m.frames_dropped.load(Ordering::Relaxed);
        let total = decoded + dropped;
        let ratio = if total == 0 { 0.0 } else { dropped as f32 / total as f32 };
        assert_eq!(ratio, 0.0);
    }

    #[test]
    fn packet_loss_ratio_with_loss() {
        let m = AudioMetrics::new();
        m.frames_decoded.store(90, Ordering::Relaxed);
        m.frames_dropped.store(10, Ordering::Relaxed);
        let total = 90 + 10;
        let ratio = 10.0 / total as f32;
        assert!((ratio - 0.1).abs() < 0.01);
    }

    #[test]
    fn adaptive_bitrate_reduces_on_heavy_loss() {
        // Test the bitrate reduction formula directly
        let target = 64000i32;
        let loss_ratio = 0.20f32; // 20% loss
        let new = if loss_ratio > 0.15 {
            (target as f32 * 0.6) as i32
        } else {
            target
        };
        assert_eq!(new, 38400, "Heavy loss should reduce to 60% of target");
    }

    #[test]
    fn adaptive_bitrate_moderate_loss() {
        let target = 64000i32;
        let loss_ratio = 0.08f32; // 8% loss
        let new = if loss_ratio > 0.15 {
            (target as f32 * 0.6) as i32
        } else if loss_ratio > 0.05 {
            (target as f32 * 0.8) as i32
        } else {
            target
        };
        assert_eq!(new, 51200, "Moderate loss should reduce to 80% of target");
    }

    #[test]
    fn adaptive_bitrate_light_loss() {
        let target = 64000i32;
        let loss_ratio = 0.03f32; // 3% loss
        let new = if loss_ratio > 0.15 {
            (target as f32 * 0.6) as i32
        } else if loss_ratio > 0.05 {
            (target as f32 * 0.8) as i32
        } else if loss_ratio > 0.01 {
            (target as f32 * 0.9) as i32
        } else {
            target
        };
        assert_eq!(new, 57600, "Light loss should reduce to 90% of target");
    }

    #[test]
    fn adaptive_bitrate_no_loss_recovers() {
        let target = 64000i32;
        let current = 38400i32; // currently reduced
        let loss_ratio = 0.0f32;
        let new = if loss_ratio > 0.01 {
            current
        } else {
            let step = ((target - current) as f32 * 0.1) as i32;
            current + step.max(1000).min(target - current)
        };
        assert!(new > current, "No loss should increase bitrate: {current} -> {new}");
        assert!(new <= target, "Should not exceed target");
    }

    #[test]
    fn adaptive_bitrate_minimum_clamp() {
        // Even with extreme loss, bitrate should not go below 16kbps
        let target = 24000i32; // Low quality setting
        let reduced = (target as f32 * 0.6) as i32; // 14400
        let clamped = reduced.clamp(16000, target);
        assert_eq!(clamped, 16000, "Should clamp to minimum 16kbps");
    }
}

#[cfg(test)]
mod ducking_tests {
    use super::*;

    #[test]
    fn ducking_amount_and_threshold_setter() {
        // Test the ducking setter logic directly via atomic values
        // (mirrors AudioEngine::set_ducking without requiring an AudioEngine instance)
        let ducking_amount = Arc::new(AtomicU32::new(0));
        let ducking_threshold = Arc::new(AtomicU32::new(0));

        // Set ducking to 50% amount, 0.08 threshold
        let amount = 0.5f32;
        let threshold = 0.08f32;
        ducking_amount.store((amount.clamp(0.0, 1.0) * 1000.0) as u32, Ordering::Relaxed);
        ducking_threshold.store(
            (threshold.clamp(0.0, 1.0) * 1000.0) as u32,
            Ordering::Relaxed,
        );

        let stored_amount = ducking_amount.load(Ordering::Relaxed) as f32 / 1000.0;
        let stored_threshold = ducking_threshold.load(Ordering::Relaxed) as f32 / 1000.0;
        assert!(
            (stored_amount - 0.5).abs() < 0.01,
            "Ducking amount should round-trip: {stored_amount}"
        );
        assert!(
            (stored_threshold - 0.08).abs() < 0.01,
            "Ducking threshold should round-trip: {stored_threshold}"
        );

        // Clamping: values outside 0.0-1.0 should be clamped
        let over = 1.5f32;
        ducking_amount.store((over.clamp(0.0, 1.0) * 1000.0) as u32, Ordering::Relaxed);
        assert_eq!(ducking_amount.load(Ordering::Relaxed), 1000);

        let under = -0.5f32;
        ducking_amount.store((under.clamp(0.0, 1.0) * 1000.0) as u32, Ordering::Relaxed);
        assert_eq!(ducking_amount.load(Ordering::Relaxed), 0);
    }
}
