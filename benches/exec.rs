//! Criterion benchmarks for `Shard::exec`.
//!
//! Inputs are built outside the timed code and one pre-sized output buffer is
//! reused. Only `Cmd` variants with a stable shape (Set, Get, Incr, MGet) are
//! used, so saved baselines stay comparable across changes.

use bytes::{Bytes, BytesMut};
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use ignix::{Cmd, Shard};
use std::hint::black_box;

// Match the allocator used by the server binary.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

const N: usize = 10_000;
const MGET_KEYS: usize = 16;

fn keys() -> Vec<Bytes> {
    (0..N).map(|i| Bytes::from(format!("key:{i:06}"))).collect()
}

fn str_values() -> Vec<Bytes> {
    (0..N)
        .map(|i| Bytes::from(format!("value-{i:010}")))
        .collect()
}

fn int_values() -> Vec<Bytes> {
    (0..N)
        .map(|i| Bytes::from((i as i64 * 7_919 - 1_000_000).to_string()))
        .collect()
}

fn filled(keys: &[Bytes], values: &[Bytes]) -> Shard {
    let shard = Shard::new(0, None);
    let mut out = BytesMut::new();
    for (k, v) in keys.iter().zip(values) {
        shard.exec(Cmd::Set(k.clone(), v.clone()), &mut out);
    }
    shard
}

fn bench_exec(c: &mut Criterion) {
    let keys = keys();
    let str_values = str_values();
    let int_values = int_values();
    let mut out = BytesMut::with_capacity(1 << 20);

    let mut group = c.benchmark_group("exec");
    group.throughput(Throughput::Elements(N as u64));

    let shard = Shard::new(0, None);
    group.bench_function("set_str_10k", |b| {
        b.iter(|| {
            out.clear();
            for (k, v) in keys.iter().zip(&str_values) {
                shard.exec(Cmd::Set(k.clone(), v.clone()), &mut out);
            }
            black_box(out.len())
        })
    });

    let shard = Shard::new(0, None);
    group.bench_function("set_int_10k", |b| {
        b.iter(|| {
            out.clear();
            for (k, v) in keys.iter().zip(&int_values) {
                shard.exec(Cmd::Set(k.clone(), v.clone()), &mut out);
            }
            black_box(out.len())
        })
    });

    let shard = filled(&keys, &str_values);
    group.bench_function("get_str_10k", |b| {
        b.iter(|| {
            out.clear();
            for k in &keys {
                shard.exec(Cmd::Get(k.clone()), &mut out);
            }
            black_box(out.len())
        })
    });

    let shard = filled(&keys, &int_values);
    group.bench_function("get_int_10k", |b| {
        b.iter(|| {
            out.clear();
            for k in &keys {
                shard.exec(Cmd::Get(k.clone()), &mut out);
            }
            black_box(out.len())
        })
    });

    let shard = filled(&keys, &int_values);
    group.bench_function("incr_10k", |b| {
        b.iter(|| {
            out.clear();
            for k in &keys {
                shard.exec(Cmd::Incr(k.clone()), &mut out);
            }
            black_box(out.len())
        })
    });

    let shard = filled(&keys, &str_values);
    let mget_rounds = N / 10;
    group.throughput(Throughput::Elements((mget_rounds * MGET_KEYS) as u64));
    group.bench_function("mget_16x1k", |b| {
        b.iter(|| {
            out.clear();
            for round in 0..mget_rounds {
                let start = (round * MGET_KEYS) % (N - MGET_KEYS);
                let batch = keys[start..start + MGET_KEYS].to_vec();
                shard.exec(Cmd::MGet(batch), &mut out);
            }
            black_box(out.len())
        })
    });

    group.finish();
}

criterion_group!(benches, bench_exec);
criterion_main!(benches);
