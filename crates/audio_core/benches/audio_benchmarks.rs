use criterion::{black_box, criterion_group, criterion_main, Criterion};

/// Benchmark the frame energy calculation (hot path in audio pipeline).
fn bench_frame_energy(c: &mut Criterion) {
    // Simulate a 960-sample frame (20ms at 48kHz)
    let frame: Vec<f32> = (0..960)
        .map(|i| (i as f32 / 960.0 * std::f32::consts::TAU).sin() * 0.5)
        .collect();

    c.bench_function("frame_energy_960", |b| {
        b.iter(|| {
            let mut sum_sq: f32 = 0.0;
            for &s in black_box(&frame) {
                sum_sq += s * s;
            }
            black_box((sum_sq / frame.len() as f32).sqrt())
        })
    });
}

/// Benchmark soft clipping (applied to every output sample).
fn bench_soft_clip(c: &mut Criterion) {
    let samples: Vec<f32> = (0..960)
        .map(|i| (i as f32 / 480.0) - 1.0) // range -1.0 to 1.0
        .collect();

    c.bench_function("soft_clip_960", |b| {
        b.iter(|| {
            let mut output = samples.clone();
            for s in black_box(&mut output) {
                // Inline soft clip logic
                if *s > 1.0 {
                    *s = 1.0 - (-(*s - 1.0)).exp() * 0.1;
                } else if *s < -1.0 {
                    *s = -1.0 + (-(-*s - 1.0)).exp() * 0.1;
                }
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
                .map(|i| ((i + p * 100) as f32 / frame_size as f32 * std::f32::consts::TAU).sin() * 0.3)
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

criterion_group!(
    benches,
    bench_frame_energy,
    bench_soft_clip,
    bench_i16_to_f32_conversion,
    bench_peer_mixing,
);
criterion_main!(benches);
