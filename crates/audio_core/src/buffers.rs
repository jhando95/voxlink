use shared_types::FRAME_SIZE;
use std::sync::atomic::{AtomicI32, AtomicU32, AtomicUsize, Ordering};

// ─── Capture ring buffer ───
// Zero-allocation input accumulator. Replaces Vec + drain() which is O(n).
// Uses a fixed ring with read/write cursors so consuming frames is O(1).

pub(crate) struct CaptureRing {
    data: Box<[f32]>,
    write: usize,
    read: usize,
    cap: usize,
}

impl CaptureRing {
    pub fn new(capacity: usize) -> Self {
        Self {
            data: vec![0.0; capacity].into_boxed_slice(),
            write: 0,
            read: 0,
            cap: capacity,
        }
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        (self.write + self.cap - self.read) % self.cap
    }

    /// Append samples. If the ring overflows, the oldest data is silently lost.
    #[inline]
    pub fn push_slice(&mut self, samples: &[f32]) {
        for &s in samples {
            self.data[self.write % self.cap] = s;
            self.write = (self.write + 1) % self.cap;
        }
    }

    /// Read exactly FRAME_SIZE samples into `dest` without allocation.
    /// Returns false if fewer than FRAME_SIZE samples are available.
    #[inline]
    pub fn read_frame(&mut self, dest: &mut [f32; FRAME_SIZE]) -> bool {
        if self.len() < FRAME_SIZE {
            return false;
        }
        let start = self.read;
        let cap = self.cap;
        // Contiguous fast path
        if start + FRAME_SIZE <= cap {
            dest.copy_from_slice(&self.data[start..start + FRAME_SIZE]);
        } else {
            let first = cap - start;
            dest[..first].copy_from_slice(&self.data[start..cap]);
            dest[first..FRAME_SIZE].copy_from_slice(&self.data[..FRAME_SIZE - first]);
        }
        self.read = (start + FRAME_SIZE) % cap;
        true
    }
}

// ─── SPSC Ring Buffer (lock-free) ───
// Single-producer single-consumer ring buffer using atomic cursors.
// Producer (decode thread) calls push(), consumer (playback callback) calls mix_into().
// Zero lock contention on the audio hot path.

pub(crate) struct SpscRingBuf {
    data: Box<[f32]>,
    cap: usize,
    /// Write cursor — only modified by producer, read by consumer
    write: AtomicUsize,
    /// Read cursor — only modified by consumer, read by producer
    read: AtomicUsize,
}

impl SpscRingBuf {
    pub fn new(capacity: usize) -> Self {
        Self {
            data: vec![0.0; capacity].into_boxed_slice(),
            cap: capacity,
            write: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
        }
    }

    /// Number of samples available for reading.
    #[inline(always)]
    pub fn len(&self) -> usize {
        let w = self.write.load(Ordering::Acquire);
        let r = self.read.load(Ordering::Acquire);
        (w + self.cap - r) % self.cap
    }

    /// Push a single sample. If the ring is full, the sample is dropped (bounded latency).
    /// Called from the decode/producer side only.
    #[inline]
    pub fn push(&self, sample: f32) {
        let w = self.write.load(Ordering::Relaxed);
        let r = self.read.load(Ordering::Acquire);
        let next_w = (w + 1) % self.cap;
        if next_w == r {
            // Ring full — drop sample to bound latency
            return;
        }
        // Safety: only the producer writes to data[w], and consumer never reads past read cursor.
        // We use UnsafeCell-free approach: the data slot at `w` is not being read by consumer
        // because consumer's read cursor hasn't reached it yet (next_w != r check above).
        let ptr = self.data.as_ptr() as *mut f32;
        unsafe {
            *ptr.add(w) = sample;
        }
        self.write.store(next_w, Ordering::Release);
    }

    /// Push a slice of samples. Drops any samples that don't fit.
    #[inline]
    pub fn push_slice(&self, samples: &[f32]) {
        for &s in samples {
            self.push(s);
        }
    }

    /// Mix available samples into `dest` with volume scaling. Zero-allocation.
    /// Called from the playback/consumer side only.
    /// Returns how many samples were consumed.
    #[inline]
    #[allow(dead_code)] // Used in tests; kept for API completeness alongside drain_into
    #[allow(clippy::needless_range_loop)] // Indexed access is clearer for ring buffer arithmetic
    pub fn mix_into(&self, dest: &mut [f32], volume: f32) -> usize {
        let w = self.write.load(Ordering::Acquire);
        let r = self.read.load(Ordering::Relaxed);
        let available = (w + self.cap - r) % self.cap;
        let count = dest.len().min(available);
        if count == 0 {
            return 0;
        }

        let cap = self.cap;
        let first = cap - r;

        if count <= first {
            for i in 0..count {
                dest[i] += self.data[(r + i) % cap] * volume;
            }
        } else {
            for i in 0..first {
                dest[i] += self.data[r + i] * volume;
            }
            let remaining = count - first;
            for i in 0..remaining {
                dest[first + i] += self.data[i] * volume;
            }
        }

        self.read.store((r + count) % cap, Ordering::Release);
        count
    }

    /// Drain available samples into `dest` (overwrite, not additive). Zero-allocation.
    /// Called from the playback/consumer side only.
    /// Returns how many samples were consumed.
    #[inline]
    #[allow(clippy::needless_range_loop)]
    pub fn drain_into(&self, dest: &mut [f32]) -> usize {
        let w = self.write.load(Ordering::Acquire);
        let r = self.read.load(Ordering::Relaxed);
        let available = (w + self.cap - r) % self.cap;
        let count = dest.len().min(available);
        if count == 0 {
            return 0;
        }

        let cap = self.cap;
        let first = cap - r;

        if count <= first {
            for i in 0..count {
                dest[i] = self.data[(r + i) % cap];
            }
        } else {
            for i in 0..first {
                dest[i] = self.data[r + i];
            }
            let remaining = count - first;
            for i in 0..remaining {
                dest[first + i] = self.data[i];
            }
        }

        self.read.store((r + count) % cap, Ordering::Release);
        count
    }

    /// Peek at the RMS energy of buffered samples without consuming them.
    /// Returns 0.0 if the buffer is empty. Used for ducking decisions.
    /// Uses contiguous fast path to avoid modulo per sample.
    #[inline]
    pub fn peek_energy(&self) -> f32 {
        let w = self.write.load(Ordering::Acquire);
        let r = self.read.load(Ordering::Relaxed);
        let available = (w + self.cap - r) % self.cap;
        if available == 0 {
            return 0.0;
        }
        let count = available.min(480); // ~10ms at 48kHz
        let cap = self.cap;
        let first = cap - r; // contiguous samples before wrap
        let mut sum = 0.0f32;
        if count <= first {
            for i in 0..count {
                let s = self.data[r + i];
                sum += s * s;
            }
        } else {
            for i in 0..first {
                let s = self.data[r + i];
                sum += s * s;
            }
            let remaining = count - first;
            for i in 0..remaining {
                let s = self.data[i];
                sum += s * s;
            }
        }
        (sum / count as f32).sqrt()
    }

    /// Discard all buffered samples.
    pub fn clear(&self) {
        let w = self.write.load(Ordering::Relaxed);
        self.read.store(w, Ordering::Release);
    }
}

// Safety: SpscRingBuf is designed for cross-thread use (producer + consumer).
// The atomic cursors ensure proper synchronization.
unsafe impl Send for SpscRingBuf {}
unsafe impl Sync for SpscRingBuf {}

// ─── Playback ring buffer (legacy, used in tests) ───

#[cfg(test)]
pub(crate) struct RingBuf {
    data: Vec<f32>,
    read: usize,
    write: usize,
    pub len: usize,
}

#[cfg(test)]
impl RingBuf {
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            data: vec![0.0; cap],
            read: 0,
            write: 0,
            len: 0,
        }
    }

    #[inline(always)]
    pub fn push(&mut self, sample: f32) {
        if self.len == self.data.len() {
            // Overwrite oldest sample (bounded latency)
            self.read = (self.read + 1) % self.data.len();
        } else {
            self.len += 1;
        }
        self.data[self.write] = sample;
        self.write = (self.write + 1) % self.data.len();
    }

    /// Mix up to `dest.len()` samples into `dest` with volume scaling.
    /// Zero-allocation. Returns how many samples were consumed.
    #[inline]
    pub fn mix_into(&mut self, dest: &mut [f32], volume: f32) -> usize {
        let count = dest.len().min(self.len);
        let cap = self.data.len();
        let first = cap - self.read;

        if count <= first {
            let src = &self.data[self.read..self.read + count];
            for (d, &s) in dest[..count].iter_mut().zip(src) {
                *d += s * volume;
            }
        } else {
            let src1 = &self.data[self.read..];
            for (d, &s) in dest[..first].iter_mut().zip(src1) {
                *d += s * volume;
            }
            let remaining = count - first;
            let src2 = &self.data[..remaining];
            for (d, &s) in dest[first..count].iter_mut().zip(src2) {
                *d += s * volume;
            }
        }

        self.read = (self.read + count) % cap;
        self.len -= count;
        count
    }
}

// ─── Per-peer playback state (lock-free) ───
//
// Wraps an SPSC ring buffer with per-peer volume, AGC, and jitter adaptation.
// The playback callback holds `Arc<PeerPlaybackShared>` directly — no mutex needed.

use super::codec::PlaybackAgc;
use super::{
    JITTER_INITIAL, JITTER_MAX_FRAMES, JITTER_MIN_FRAMES, JITTER_STABLE_THRESHOLD,
    MAX_PEER_BUFFER_SAMPLES,
};
use std::sync::Arc;

pub(crate) struct PeerPlaybackShared {
    pub ring: SpscRingBuf,
    /// Per-peer volume (0.0–1.0 stored as 0–1000). Atomic for lock-free access.
    pub volume: AtomicU32,
    /// Whether this peer has been primed (enough data buffered to start playout)
    pub primed: std::sync::atomic::AtomicBool,
    /// Jitter target frames — atomic so playback callback can read
    pub target_frames: AtomicU32,
    /// Underrun counter — incremented by playback callback, read by decode side for adaptation
    pub underrun_count: AtomicU32,
    /// Callback count — incremented by playback callback to track activity
    pub callback_count: AtomicU32,
    /// RMS audio level (0–1000 fixed-point, i.e. level * 1000). Updated by playback callback.
    pub rms_level: AtomicU32,
    /// Per-peer 3-band EQ gains in millibels (-600 to +600, i.e. -6dB to +6dB).
    /// Low shelf at 300Hz.
    pub eq_bass: AtomicI32,
    /// Peaking EQ at 1kHz.
    pub eq_mid: AtomicI32,
    /// High shelf at 3kHz.
    pub eq_treble: AtomicI32,
    /// Stereo pan position (-100 = full left, 0 = center, +100 = full right).
    pub pan: AtomicI32,
}

impl PeerPlaybackShared {
    pub fn new() -> Self {
        Self {
            ring: SpscRingBuf::new(MAX_PEER_BUFFER_SAMPLES),
            volume: AtomicU32::new(1000), // 1.0
            primed: std::sync::atomic::AtomicBool::new(false),
            target_frames: AtomicU32::new(JITTER_INITIAL as u32),
            underrun_count: AtomicU32::new(0),
            callback_count: AtomicU32::new(0),
            rms_level: AtomicU32::new(0),
            eq_bass: AtomicI32::new(0),
            eq_mid: AtomicI32::new(0),
            eq_treble: AtomicI32::new(0),
            pan: AtomicI32::new(0),
        }
    }

    /// Check if peer has enough buffered data to start playout.
    #[inline]
    pub fn is_ready(&self) -> bool {
        self.primed.load(Ordering::Relaxed)
            || self.ring.len() >= (self.target_frames.load(Ordering::Relaxed) as usize * FRAME_SIZE)
    }

    /// Get volume as f32
    #[inline]
    pub fn volume_f32(&self) -> f32 {
        self.volume.load(Ordering::Relaxed) as f32 / 1000.0
    }
}

/// Per-peer decode-side state (held under the peer_buffers Mutex, NOT on the playback path).
pub(crate) struct PeerPlayback {
    pub shared: Arc<PeerPlaybackShared>,
    pub playback_agc: PlaybackAgc,
    /// Reusable buffer for i16→f32 conversion on decode path (avoids allocation per frame)
    pub convert_buf: Vec<f32>,
    // Jitter adaptation state (only accessed from decode side)
    last_underrun_count: u32,
    last_callback_count: u32,
    consecutive_underrun_checks: u16,
    stable_checks: u16,
    /// Exponential expansion threshold: starts at 3, doubles each expansion
    expansion_threshold: u16,
}

impl PeerPlayback {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(PeerPlaybackShared::new()),
            playback_agc: PlaybackAgc::new(),
            convert_buf: Vec::with_capacity(shared_types::FRAME_SIZE),
            last_underrun_count: 0,
            last_callback_count: 0,
            consecutive_underrun_checks: 0,
            stable_checks: 0,
            expansion_threshold: 3, // First expansion at 3 underruns, then 6, 12, ...
        }
    }

    /// Check underrun/stable counters from the playback callback and adapt jitter depth.
    /// Called periodically from the decode side (e.g. on each decoded frame).
    pub fn adapt_from_atomics(&mut self) {
        let underruns = self.shared.underrun_count.load(Ordering::Relaxed);
        let callbacks = self.shared.callback_count.load(Ordering::Relaxed);
        let new_underruns = underruns.wrapping_sub(self.last_underrun_count);
        let new_callbacks = callbacks.wrapping_sub(self.last_callback_count);
        self.last_underrun_count = underruns;
        self.last_callback_count = callbacks;

        if new_callbacks == 0 {
            return; // No activity since last check
        }

        let target = self.shared.target_frames.load(Ordering::Relaxed) as u16;

        if new_underruns > 0 {
            self.consecutive_underrun_checks += 1;
            self.stable_checks = 0;
            // Exponential expansion trigger: first at 3 underruns, then 6, 12, ...
            // Prevents rapid expansion from transient network bursts.
            if self.consecutive_underrun_checks >= self.expansion_threshold
                && target < JITTER_MAX_FRAMES
            {
                let new_target = target + 1;
                self.shared
                    .target_frames
                    .store(new_target as u32, Ordering::Relaxed);
                self.shared.primed.store(false, Ordering::Relaxed);
                self.consecutive_underrun_checks = 0;
                self.expansion_threshold = (self.expansion_threshold * 2).min(24); // cap at 24
                log::debug!(
                    "Jitter buffer expanded to {}ms (next threshold: {})",
                    new_target as u32 * 20,
                    self.expansion_threshold
                );
            }
        } else {
            self.consecutive_underrun_checks = 0;
            self.stable_checks += 1;
            if self.stable_checks > JITTER_STABLE_THRESHOLD && target > JITTER_MIN_FRAMES {
                let new_target = target - 1;
                self.shared
                    .target_frames
                    .store(new_target as u32, Ordering::Relaxed);
                self.stable_checks = 0;
                log::debug!("Jitter buffer reduced to {}ms", new_target as u32 * 20);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_ring_basic_read_write() {
        let mut ring = CaptureRing::new(FRAME_SIZE * 4);
        assert_eq!(ring.len(), 0);

        // Push one frame worth of samples
        let data = vec![0.5f32; FRAME_SIZE];
        ring.push_slice(&data);
        assert_eq!(ring.len(), FRAME_SIZE);

        // Read it back
        let mut out = [0.0f32; FRAME_SIZE];
        assert!(ring.read_frame(&mut out));
        assert_eq!(ring.len(), 0);
        assert_eq!(out[0], 0.5);
    }

    #[test]
    fn capture_ring_wraparound_len() {
        // Verify len() works correctly when write wraps around past read.
        // The old wrapping_sub formula gave wrong results when write < read.
        // Use production-sized buffer (4x FRAME_SIZE) to avoid overfill.
        let cap = FRAME_SIZE * 4;
        let mut ring = CaptureRing::new(cap);
        let data = vec![1.0f32; FRAME_SIZE];
        let mut out = [0.0f32; FRAME_SIZE];

        // Push and read several times to advance write past the end of the buffer.
        // After 4 push+read cycles, write wraps to position 0 while read is at 3840%cap=0.
        // Do one more push to get write > read again.
        for _ in 0..4 {
            ring.push_slice(&data);
            assert_eq!(ring.len(), FRAME_SIZE);
            assert!(ring.read_frame(&mut out));
            assert_eq!(ring.len(), 0);
        }

        // Now both write and read are at 0 (4*960 % 3840 = 0).
        // Push two frames — write=1920, read=0 → len=1920
        ring.push_slice(&data);
        ring.push_slice(&data);
        assert_eq!(ring.len(), FRAME_SIZE * 2);

        // Read one frame — write=1920, read=960 → len=960
        assert!(ring.read_frame(&mut out));
        assert_eq!(ring.len(), FRAME_SIZE);

        // Push two more frames — write=3840%3840=0, read=960 → write < read!
        // This is the wraparound case: (0 + 3840 - 960) % 3840 = 2880
        ring.push_slice(&data);
        ring.push_slice(&data);
        let len = ring.len();
        assert_eq!(
            len,
            FRAME_SIZE * 3,
            "len() wrong after wraparound: got {len}, expected {}",
            FRAME_SIZE * 3
        );

        // Should be able to read all 3 frames
        assert!(ring.read_frame(&mut out));
        assert!(ring.read_frame(&mut out));
        assert!(ring.read_frame(&mut out));
        assert!(!ring.read_frame(&mut out)); // empty now
    }

    #[test]
    fn capture_ring_not_enough_data() {
        let mut ring = CaptureRing::new(FRAME_SIZE * 4);
        let data = vec![0.5f32; FRAME_SIZE / 2];
        ring.push_slice(&data);
        let mut out = [0.0f32; FRAME_SIZE];
        assert!(!ring.read_frame(&mut out)); // not enough data
    }

    #[test]
    fn spsc_ring_push_and_mix() {
        let ring = SpscRingBuf::new(1024);
        for i in 0..100 {
            ring.push(i as f32);
        }
        assert_eq!(ring.len(), 100);

        let mut dest = [0.0f32; 50];
        let consumed = ring.mix_into(&mut dest, 1.0);
        assert_eq!(consumed, 50);
        assert_eq!(ring.len(), 50);
        assert_eq!(dest[0], 0.0);
        assert_eq!(dest[49], 49.0);
    }

    #[test]
    fn spsc_ring_full_drops_samples() {
        let ring = SpscRingBuf::new(11); // capacity 11, usable 10 (SPSC needs 1 slot gap)
        for i in 0..20 {
            ring.push(i as f32);
        }
        // Should hold 10 samples (capacity - 1 for SPSC gap)
        assert_eq!(ring.len(), 10);

        let mut dest = [0.0f32; 10];
        let consumed = ring.mix_into(&mut dest, 1.0);
        assert_eq!(consumed, 10);
        // First 10 samples should be 0..10, extras were dropped
        assert_eq!(dest[0], 0.0);
        assert_eq!(dest[9], 9.0);
    }

    #[test]
    fn spsc_ring_clear() {
        let ring = SpscRingBuf::new(1024);
        for i in 0..100 {
            ring.push(i as f32);
        }
        assert_eq!(ring.len(), 100);
        ring.clear();
        assert_eq!(ring.len(), 0);
    }

    #[test]
    fn spsc_ring_concurrent_push_read() {
        use std::sync::Arc;

        let ring = Arc::new(SpscRingBuf::new(4096));
        let ring_producer = ring.clone();
        let ring_consumer = ring.clone();

        // Producer: push 10000 samples
        let producer = std::thread::spawn(move || {
            for i in 0..10000u32 {
                ring_producer.push(i as f32);
                // Occasional yield to interleave with consumer
                if i % 100 == 0 {
                    std::thread::yield_now();
                }
            }
        });

        // Consumer: read all samples
        let consumer = std::thread::spawn(move || {
            let mut total_consumed = 0usize;
            let mut buf = [0.0f32; 64];
            loop {
                let n = ring_consumer.mix_into(&mut buf, 1.0);
                total_consumed += n;
                if total_consumed >= 10000 {
                    break;
                }
                if n == 0 {
                    std::thread::yield_now();
                }
            }
            total_consumed
        });

        producer.join().unwrap();
        let total = consumer.join().unwrap();
        assert_eq!(total, 10000);
    }

    #[test]
    fn ring_buf_mix_into() {
        let mut ring = RingBuf::with_capacity(1024);
        for i in 0..100 {
            ring.push(i as f32);
        }
        assert_eq!(ring.len, 100);

        let mut dest = [0.0f32; 50];
        let consumed = ring.mix_into(&mut dest, 1.0);
        assert_eq!(consumed, 50);
        assert_eq!(ring.len, 50);
        assert_eq!(dest[0], 0.0);
        assert_eq!(dest[49], 49.0);
    }

    #[test]
    fn ring_buf_overflow_drops_oldest() {
        let mut ring = RingBuf::with_capacity(10);
        for i in 0..15 {
            ring.push(i as f32);
        }
        // Buffer holds 10 samples, oldest 5 were dropped
        assert_eq!(ring.len, 10);

        let mut dest = [0.0f32; 10];
        ring.mix_into(&mut dest, 1.0);
        // Should contain samples 5..15
        assert_eq!(dest[0], 5.0);
        assert_eq!(dest[9], 14.0);
    }

    #[test]
    fn peer_playback_jitter_adaptation() {
        let mut peer = PeerPlayback::new();
        assert_eq!(
            peer.shared.target_frames.load(Ordering::Relaxed),
            JITTER_INITIAL as u32
        );

        // Simulate 3 consecutive underrun checks via atomic counters
        // Each adapt_from_atomics() reads the delta from last check
        for i in 0..3u32 {
            // Simulate playback callback reporting underruns
            peer.shared.underrun_count.store(i + 1, Ordering::Relaxed);
            peer.shared.callback_count.store(i + 1, Ordering::Relaxed);
            peer.adapt_from_atomics();
        }
        assert_eq!(
            peer.shared.target_frames.load(Ordering::Relaxed),
            JITTER_INITIAL as u32 + 1
        );

        // Simulate stable operation for JITTER_STABLE_THRESHOLD + 1 checks
        let base_cb = peer.shared.callback_count.load(Ordering::Relaxed);
        let base_ur = peer.shared.underrun_count.load(Ordering::Relaxed);
        for i in 0..=JITTER_STABLE_THRESHOLD {
            // Callbacks increment but underruns stay the same (stable)
            peer.shared
                .callback_count
                .store(base_cb + i as u32 + 1, Ordering::Relaxed);
            peer.shared.underrun_count.store(base_ur, Ordering::Relaxed);
            peer.adapt_from_atomics();
        }
        assert_eq!(
            peer.shared.target_frames.load(Ordering::Relaxed),
            JITTER_INITIAL as u32
        );
    }

    #[test]
    fn peer_playback_shared_volume() {
        let shared = PeerPlaybackShared::new();
        assert!((shared.volume_f32() - 1.0).abs() < 0.01);
        shared.volume.store(500, Ordering::Relaxed);
        assert!((shared.volume_f32() - 0.5).abs() < 0.01);
    }

    #[test]
    fn spsc_ring_peek_energy_empty() {
        let ring = SpscRingBuf::new(1024);
        assert_eq!(ring.peek_energy(), 0.0);
    }

    #[test]
    fn spsc_ring_peek_energy_nonzero_after_push() {
        let ring = SpscRingBuf::new(1024);
        // Push a sine-like signal with known energy
        for i in 0..480 {
            let t = i as f32 / 48000.0;
            ring.push((std::f32::consts::TAU * 440.0 * t).sin() * 0.5);
        }
        let energy = ring.peek_energy();
        assert!(
            energy > 0.0,
            "peek_energy should be non-zero after pushing samples, got {energy}"
        );
        // RMS of 0.5 * sin should be ~0.35
        assert!(
            energy > 0.1 && energy < 0.6,
            "Energy {energy} outside expected range"
        );
        // peek_energy should not consume samples
        assert_eq!(ring.len(), 480);
    }
}
