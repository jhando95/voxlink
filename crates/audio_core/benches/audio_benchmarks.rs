use criterion::{black_box, criterion_group, criterion_main, Criterion};

/// Benchmark the frame energy calculation (hot path in audio pipeline).
/// Uses the real `frame_energy` function from audio_core.
fn bench_frame_energy(c: &mut Criterion) {
    // Simulate a 960-sample frame (20ms at 48kHz)
    let frame: Vec<f32> = (0..960)
        .map(|i| (i as f32 / 960.0 * std::f32::consts::TAU).sin() * 0.5)
        .collect();

    c.bench_function("frame_energy_960", |b| {
        b.iter(|| audio_core::frame_energy(black_box(&frame)))
    });
}

/// Benchmark soft clipping (applied to every output sample).
/// Uses the real `soft_clip` function from audio_core.
fn bench_soft_clip(c: &mut Criterion) {
    let samples: Vec<f32> = (0..960)
        .map(|i| (i as f32 / 480.0) - 1.0) // range -1.0 to 1.0
        .collect();

    c.bench_function("soft_clip_960", |b| {
        b.iter(|| {
            let mut output = samples.clone();
            for s in output.iter_mut() {
                *s = audio_core::soft_clip(black_box(*s));
            }
            black_box(output)
        })
    });
}

/// Benchmark i16 to f32 conversion (done on every decoded frame).
fn bench_i16_to_f32_conversion(c: &mut Criterion) {
    let pcm: Vec<i16> = (0..960).map(|i| (i * 33) as i16).collect();

    c.bench_function("i16_to_f32_960", |b| {
        b.iter(|| {
            let converted: Vec<f32> = black_box(&pcm)
                .iter()
                .map(|&s| s as f32 * (1.0 / 32767.0))
                .collect();
            black_box(converted)
        })
    });
}

/// Benchmark mixing N peer buffers into output (playback hot path).
fn bench_peer_mixing(c: &mut Criterion) {
    let num_peers = 4;
    let frame_size = 960;

    let peer_buffers: Vec<Vec<f32>> = (0..num_peers)
        .map(|p| {
            (0..frame_size)
                .map(|i| {
                    ((i + p * 100) as f32 / frame_size as f32 * std::f32::consts::TAU).sin() * 0.3
                })
                .collect()
        })
        .collect();

    c.bench_function("mix_4_peers_960", |b| {
        b.iter(|| {
            let mut output = vec![0.0f32; frame_size];
            for peer in black_box(&peer_buffers) {
                for (out, &sample) in output.iter_mut().zip(peer.iter()) {
                    *out += sample;
                }
            }
            black_box(output)
        })
    });
}

/// Benchmark frame energy on silence (DTX optimization path).
fn bench_frame_energy_silence(c: &mut Criterion) {
    let silence = vec![0.0f32; 960];

    c.bench_function("frame_energy_silence", |b| {
        b.iter(|| audio_core::frame_energy(black_box(&silence)))
    });
}

/// Benchmark soft clip on values that don't clip (common case).
fn bench_soft_clip_passthrough(c: &mut Criterion) {
    let samples: Vec<f32> = (0..960)
        .map(|i| (i as f32 / 960.0) * 0.8 - 0.4) // range -0.4 to 0.4 (no clipping)
        .collect();

    c.bench_function("soft_clip_passthrough_960", |b| {
        b.iter(|| {
            let mut output = samples.clone();
            for s in output.iter_mut() {
                *s = audio_core::soft_clip(black_box(*s));
            }
            black_box(output)
        })
    });
}

criterion_group!(
    benches,
    bench_frame_energy,
    bench_frame_energy_silence,
    bench_soft_clip,
    bench_soft_clip_passthrough,
    bench_i16_to_f32_conversion,
    bench_peer_mixing,
);
criterion_main!(benches);
