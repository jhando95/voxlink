use shared_types::FRAME_SIZE;

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

// ─── Playback ring buffer ───
// Fixed-size ring for per-peer output mixing. Zero-allocation mix_into().

pub(crate) struct RingBuf {
    data: Vec<f32>,
    read: usize,
    write: usize,
    pub len: usize,
}

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

    pub fn clear(&mut self) {
        self.read = 0;
        self.write = 0;
        self.len = 0;
    }
}

// ─── Per-peer mixing state with adaptive jitter buffer ───
//
// Self-tuning playout delay per peer.
// Instead of a fixed buffer depth, each peer's playback adapts:
//   - Starts with 40ms buffer (2 frames)
//   - On underrun: increases target depth (+20ms), re-primes
//   - After 10s stable: decreases target depth (-20ms)

use super::{MAX_PEER_BUFFER_SAMPLES, JITTER_INITIAL, JITTER_MIN_FRAMES, JITTER_MAX_FRAMES, JITTER_STABLE_THRESHOLD};

pub(crate) struct PeerPlayback {
    pub buffer: RingBuf,
    pub volume: f32,
    pub primed: bool,
    target_frames: u16,
    underrun_ticks: u16,
    stable_ticks: u16,
}

impl PeerPlayback {
    pub fn new() -> Self {
        Self {
            buffer: RingBuf::with_capacity(MAX_PEER_BUFFER_SAMPLES),
            volume: 1.0,
            primed: false,
            target_frames: JITTER_INITIAL,
            underrun_ticks: 0,
            stable_ticks: 0,
        }
    }

    /// Called after each output callback to adapt playout depth.
    #[inline]
    pub fn adapt(&mut self, consumed: usize, requested: usize) {
        if consumed < requested {
            self.underrun_ticks += 1;
            self.stable_ticks = 0;
            if self.underrun_ticks >= 3 && self.target_frames < JITTER_MAX_FRAMES {
                self.target_frames += 1;
                self.primed = false;
                self.underrun_ticks = 0;
                log::debug!("Jitter buffer expanded to {}ms", self.target_frames as u32 * 20);
            }
        } else {
            self.underrun_ticks = 0;
            self.stable_ticks += 1;
            if self.stable_ticks > JITTER_STABLE_THRESHOLD
                && self.target_frames > JITTER_MIN_FRAMES
            {
                self.target_frames -= 1;
                self.stable_ticks = 0;
                log::debug!("Jitter buffer reduced to {}ms", self.target_frames as u32 * 20);
            }
        }
    }

    /// Returns true if this peer has enough buffered data to start playout.
    #[inline]
    pub fn is_ready(&self) -> bool {
        self.primed || self.buffer.len >= (self.target_frames as usize * FRAME_SIZE)
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
        assert_eq!(len, FRAME_SIZE * 3, "len() wrong after wraparound: got {len}, expected {}",
                   FRAME_SIZE * 3);

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
        assert_eq!(peer.target_frames, JITTER_INITIAL);

        // Simulate 3 consecutive underruns → should expand
        peer.adapt(0, FRAME_SIZE);
        peer.adapt(0, FRAME_SIZE);
        peer.adapt(0, FRAME_SIZE);
        assert_eq!(peer.target_frames, JITTER_INITIAL + 1);

        // Simulate stable operation for JITTER_STABLE_THRESHOLD + 1 ticks → should contract
        for _ in 0..=JITTER_STABLE_THRESHOLD {
            peer.adapt(FRAME_SIZE, FRAME_SIZE);
        }
        assert_eq!(peer.target_frames, JITTER_INITIAL);
    }
}
