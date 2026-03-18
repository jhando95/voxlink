mod buffers;
mod codec;
mod feedback;

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

use buffers::{CaptureRing, PeerPlayback};
use codec::{
    frame_energy, soft_clip, Agc, ComfortNoise, DeEsser, HighPassFilter, MuteRamp, NoiseGate,
    SendEncoder,
};
use feedback::{FeedbackAction, FeedbackTone};

const MAX_PEER_BUFFER_SAMPLES: usize = FRAME_SIZE * 10; // ~200ms per peer
const OPUS_MAX_PACKET: usize = 512; // headroom for complex frames at 32kbps
const OPUS_BITRATE: i32 = 64000; // 64 kbps — high quality voice, still very bandwidth efficient

// Adaptive jitter buffer constants
const JITTER_MIN_FRAMES: u16 = 1; // 20ms minimum playout delay
const JITTER_MAX_FRAMES: u16 = 5; // 100ms maximum playout delay
const JITTER_INITIAL: u16 = 2; // 40ms default — good for LAN + decent internet
const JITTER_STABLE_THRESHOLD: u16 = 500; // ~10s stable before reducing

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
    on_encoded_frame: Arc<Mutex<Option<EncodedFrameCallback>>>,
    opus_decoders: Mutex<HashMap<String, OpusDecoder>>,
    mic_level_raw: Arc<AtomicU32>,
    /// Noise gate sensitivity (0.0–1.0 stored as 0–1000). Shared with capture callback.
    noise_gate_sensitivity: Arc<AtomicU32>,
    /// Input gain (mic boost/cut, 0.0–2.0 stored as 0–2000). Shared with capture callback.
    input_gain: Arc<AtomicU32>,
    /// Output volume (master, 0.0–1.0 stored as 0–1000). Shared with playback callback.
    output_volume: Arc<AtomicU32>,
    /// Keybind feedback tone generator.
    feedback_tone: FeedbackTone,
    /// Runtime bitrate in bps. Encoder reads this each frame for dynamic quality switching.
    opus_bitrate: Arc<AtomicI32>,
    /// Target (base) bitrate from voice quality setting. Adaptive bitrate reduces from this.
    target_bitrate: Arc<AtomicI32>,
    /// Set to true when a stream error occurs — signals the app to attempt recovery (#1)
    pub capture_error: Arc<AtomicBool>,
    pub playback_error: Arc<AtomicBool>,
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
            on_encoded_frame: Arc::new(Mutex::new(None)),
            opus_decoders: Mutex::new(HashMap::new()),
            mic_level_raw: Arc::new(AtomicU32::new(0)),
            noise_gate_sensitivity: Arc::new(AtomicU32::new(500)), // 0.5 default
            input_gain: Arc::new(AtomicU32::new(1000)),            // 1.0 default (unity gain)
            output_volume: Arc::new(AtomicU32::new(1000)),         // 1.0 default
            feedback_tone: FeedbackTone::new(),
            opus_bitrate: Arc::new(AtomicI32::new(OPUS_BITRATE)),
            target_bitrate: Arc::new(AtomicI32::new(OPUS_BITRATE)),
            capture_error: Arc::new(AtomicBool::new(false)),
            playback_error: Arc::new(AtomicBool::new(false)),
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

    pub fn set_peer_volume(&self, peer_id: &str, volume: f32) {
        if let Ok(mut peers) = self.peer_buffers.lock() {
            if let Some(peer) = peers.get_mut(peer_id) {
                peer.volume = volume.clamp(0.0, 1.0);
            }
        }
    }

    pub fn remove_peer(&self, peer_id: &str) {
        if let Ok(mut peers) = self.peer_buffers.lock() {
            peers.remove(peer_id);
        }
        if let Ok(mut decs) = self.opus_decoders.lock() {
            decs.remove(peer_id);
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
        let input_gain = self.input_gain.clone();
        let opus_bitrate = self.opus_bitrate.clone();

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

                    // AGC: normalize volume (after gate so we don't amplify noise)
                    if gate_open {
                        agc.process(&mut frame_buf);
                    }

                    // Smooth mute/unmute fade (eliminates click on toggle)
                    let has_audio = mute_ramp.apply(&mut frame_buf);

                    // Skip encoding if fully silent (gate closed + ramp done, or muted + ramp done)
                    if noise_gate.is_silent() && !has_audio {
                        // Inject comfort noise to avoid jarring dead silence
                        comfort_noise.fill(&mut frame_buf);
                        // Still encode the comfort noise frame
                    } else {
                        // De-ess sibilant frequencies before encoding
                        deesser.process(&mut frame_buf);
                    }

                    for (out, &s) in pcm_i16.iter_mut().zip(frame_buf.iter()) {
                        *out = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                    }

                    // Apply runtime bitrate (changed when joining channels with different quality)
                    let target_bps = opus_bitrate.load(Ordering::Relaxed);
                    opus_encoder.set_bitrate(audiopus::Bitrate::BitsPerSecond(target_bps)).ok();

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

    /// Get the target (base) bitrate from the voice quality setting.
    pub fn target_bitrate(&self) -> i32 {
        self.target_bitrate.load(Ordering::Relaxed)
    }

    /// Get the current runtime bitrate.
    pub fn current_bitrate(&self) -> i32 {
        self.opus_bitrate.load(Ordering::Relaxed)
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

        let peer_buffers = self.peer_buffers.clone();
        let is_deafened = self.is_deafened.clone();
        let output_volume = self.output_volume.clone();
        let feedback_playback = self.feedback_tone.playback_state();

        let stream = device.build_output_stream(
            &config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                data.fill(0.0);

                // Mix feedback tone (plays even when deafened — it's local UI feedback)
                feedback_playback.mix_into(data);

                if let Ok(mut peers) = peer_buffers.try_lock() {
                    if is_deafened.load(Ordering::Relaxed) {
                        for peer in peers.values_mut() {
                            peer.buffer.clear();
                        }
                        return;
                    }

                    for peer in peers.values_mut() {
                        if !peer.is_ready() {
                            continue;
                        }
                        peer.primed = true;
                        let consumed = peer.buffer.mix_into(data, peer.volume);
                        peer.adapt(consumed, data.len());
                    }

                    // Apply master output volume
                    let vol = output_volume.load(Ordering::Relaxed) as f32 / 1000.0;
                    for sample in data.iter_mut() {
                        *sample = soft_clip(*sample * vol);
                    }
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
        log::info!("Playback stopped");
    }

    pub fn restart_playback(&mut self, device_name: Option<&str>) -> Result<()> {
        self.stop_playback();
        self.start_playback(device_name)
    }

    // ─── Decode & Queue ───

    /// Decode incoming Opus audio into the per-peer ring buffer.
    /// Returns whether the frame contains voice (energy above threshold).
    pub fn decode_and_queue(&self, sender_id: &str, encoded_data: &[u8]) -> bool {
        let mut pcm_i16 = [0i16; FRAME_SIZE];
        let sample_count = match self.decode_opus_frame(sender_id, encoded_data, &mut pcm_i16) {
            Some(n) => n,
            None => return false,
        };
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
        let mut peers = match self.peer_buffers.try_lock() {
            Ok(p) => p,
            Err(_) => return false,
        };

        let Some(peer) = get_or_create(&mut peers, sender_id, || Some(PeerPlayback::new())) else {
            log::error!("Failed to create playback buffer for peer {sender_id}");
            return false;
        };

        if pcm_i16.is_empty() {
            return false;
        }

        // Convert i16 → f32, apply per-peer playback AGC, then push to ring buffer
        let mut f32_buf: Vec<f32> = pcm_i16.iter().map(|&s| s as f32 * (1.0 / 32767.0)).collect();
        peer.playback_agc.process(&mut f32_buf);

        let mut sum_sq: f32 = 0.0;
        for &s in &f32_buf {
            sum_sq += s * s;
            peer.buffer.push(s);
        }

        let rms = (sum_sq / f32_buf.len() as f32).sqrt();
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

    /// Set input gain (0.0 = silent, 1.0 = unity, 2.0 = +6dB boost)
    pub fn set_input_gain(&self, gain: f32) {
        let val = (gain.clamp(0.0, 2.0) * 1000.0) as u32;
        self.input_gain.store(val, Ordering::Relaxed);
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
        // Use higher tone for join, lower for leave
        self.feedback_tone.trigger(if joined {
            FeedbackAction::MuteOff
        } else {
            FeedbackAction::DeafenOn
        });
    }

    /// Play a local speaker preview tone for device/output testing.
    pub fn play_output_preview(&self) {
        self.feedback_tone.trigger(FeedbackAction::OutputPreview);
    }
}
