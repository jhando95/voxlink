use criterion::{black_box, criterion_group, criterion_main, Criterion};
use shared_types::SignalMessage;

/// Benchmark the jump-table match that indexes a struct variant.
fn bench_variant_index_struct(c: &mut Criterion) {
    let msg = SignalMessage::CreateRoom {
        user_name: "alice".into(),
        password: None,
    };
    c.bench_function("signal_message_variant_index_struct", |b| {
        b.iter(|| black_box(&msg).variant_index())
    });
}

/// Benchmark the same function on a unit variant (different match arm shape).
fn bench_variant_index_unit(c: &mut Criterion) {
    let msg = SignalMessage::LeaveRoom;
    c.bench_function("signal_message_variant_index_unit", |b| {
        b.iter(|| black_box(&msg).variant_index())
    });
}

criterion_group!(benches, bench_variant_index_struct, bench_variant_index_unit);
criterion_main!(benches);
