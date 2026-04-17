use criterion::{black_box, criterion_group, criterion_main, Criterion};
use shared_types::{decode_screen_chunk_metadata, SignalMessage};
use signaling_server::histogram::Histogram;

/// Benchmark a single histogram observation: bucket scan + three atomic fetch_adds.
fn bench_histogram_observe(c: &mut Criterion) {
    let h = Histogram::new("bench", "bench help");
    c.bench_function("histogram_observe", |b| {
        b.iter(|| h.observe(black_box(0.003)))
    });
}

/// Deserialize a simple unit-variant SignalMessage.
fn bench_signal_from_slice_simple(c: &mut Criterion) {
    let data = br#""LeaveRoom""#;
    c.bench_function("signal_message_from_slice_simple", |b| {
        b.iter(|| {
            let _: SignalMessage =
                serde_json::from_slice(black_box(data)).expect("parse");
        })
    });
}

/// Deserialize a realistic struct-variant SignalMessage.
fn bench_signal_from_slice_complex(c: &mut Criterion) {
    let data = br#"{"CreateRoom":{"user_name":"alice","password":null}}"#;
    c.bench_function("signal_message_from_slice_complex", |b| {
        b.iter(|| {
            let _: SignalMessage =
                serde_json::from_slice(black_box(data)).expect("parse");
        })
    });
}

/// Serialize a realistic SignalMessage to JSON.
fn bench_signal_to_string(c: &mut Criterion) {
    let msg = SignalMessage::CreateRoom {
        user_name: "alice".into(),
        password: None,
    };
    c.bench_function("signal_message_to_string", |b| {
        b.iter(|| serde_json::to_string(black_box(&msg)).expect("serialize"))
    });
}

/// Decode the fixed-size UDP screen-chunk metadata header.
fn bench_decode_screen_chunk_metadata(c: &mut Criterion) {
    // 8-byte metadata header: sequence(u32)=0, chunk_index(u16)=0, chunk_count(u16)=1
    // chunk_count must be > 0 and chunk_index < chunk_count for a valid parse.
    // Followed by 8 bytes of payload.
    let mut data = [0u8; 16];
    // chunk_count = 1 at bytes [6..8] in big-endian
    data[7] = 1;
    c.bench_function("decode_screen_chunk_metadata", |b| {
        b.iter(|| decode_screen_chunk_metadata(black_box(&data)))
    });
}

criterion_group!(
    benches,
    bench_histogram_observe,
    bench_signal_from_slice_simple,
    bench_signal_from_slice_complex,
    bench_signal_to_string,
    bench_decode_screen_chunk_metadata,
);
criterion_main!(benches);
