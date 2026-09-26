//! Criterion benchmarks for the RESP parser and the reply writers.
//!
//! Buffers are built outside the timed code; `parse_*` benches clone the input
//! in the untimed batch setup so only parsing is measured.

use bytes::BytesMut;
use criterion::{criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use ignix::protocol::{parse_many, write_array_len, write_bulk, write_integer};
use std::hint::black_box;

// Match the allocator used by the server binary.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn push_command(buf: &mut BytesMut, args: &[&[u8]]) {
    buf.extend_from_slice(format!("*{}\r\n", args.len()).as_bytes());
    for arg in args {
        buf.extend_from_slice(format!("${}\r\n", arg.len()).as_bytes());
        buf.extend_from_slice(arg);
        buf.extend_from_slice(b"\r\n");
    }
}

fn set_commands(n: usize) -> BytesMut {
    let mut buf = BytesMut::new();
    for i in 0..n {
        let value = format!("val{i}");
        push_command(&mut buf, &[b"SET", b"key", value.as_bytes()]);
    }
    buf
}

fn mixed_commands(n: usize) -> BytesMut {
    let mut buf = BytesMut::new();
    for i in 0..n {
        let key = format!("user:{i:05}");
        let k = key.as_bytes();
        match i % 8 {
            0 => push_command(&mut buf, &[b"PING"]),
            1 => push_command(&mut buf, &[b"GET", k]),
            2 => push_command(&mut buf, &[b"SET", k, b"some-session-payload"]),
            3 => push_command(&mut buf, &[b"INCR", k]),
            4 => push_command(&mut buf, &[b"MGET", k, b"a", b"b", b"c"]),
            5 => push_command(&mut buf, &[b"MSET", k, b"1", b"other", b"2"]),
            6 => push_command(&mut buf, &[b"DEL", k]),
            _ => push_command(&mut buf, &[b"EXISTS", k]),
        }
    }
    buf
}

fn large_bulk_commands(n: usize, size: usize) -> BytesMut {
    let value = vec![b'v'; size];
    let mut buf = BytesMut::new();
    for i in 0..n {
        let key = format!("blob:{i}");
        push_command(&mut buf, &[b"SET", key.as_bytes(), &value]);
    }
    buf
}

fn bench_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("resp");

    let inputs = [
        ("parse_many_1k", set_commands(1_000), 1_000u64),
        ("parse_many_mixed", mixed_commands(1_000), 1_000u64),
        (
            "parse_large_bulk",
            large_bulk_commands(16, 64 * 1024),
            16u64,
        ),
    ];

    let mut out = Vec::with_capacity(1_000);
    for (name, buf, commands) in inputs {
        group.throughput(Throughput::Elements(commands));
        group.bench_function(name, |b| {
            b.iter_batched_ref(
                || buf.clone(),
                |input| {
                    out.clear();
                    parse_many(input, &mut out).unwrap();
                    black_box(out.len())
                },
                BatchSize::SmallInput,
            )
        });
    }

    group.finish();
}

fn bench_reply(c: &mut Criterion) {
    const N: usize = 10_000;
    let payload = [b'x'; 16];
    let mut out = BytesMut::with_capacity(1 << 20);

    let mut group = c.benchmark_group("reply");
    group.throughput(Throughput::Elements(N as u64));

    group.bench_function("write_bulk_10k", |b| {
        b.iter(|| {
            out.clear();
            for _ in 0..N {
                write_bulk(black_box(&payload), &mut out);
            }
            black_box(out.len())
        })
    });

    group.bench_function("write_integer_10k", |b| {
        b.iter(|| {
            out.clear();
            for i in 0..N as i64 {
                write_integer(black_box(i * 7_919 - 1_000_000), &mut out);
            }
            black_box(out.len())
        })
    });

    group.bench_function("write_array_len_10k", |b| {
        b.iter(|| {
            out.clear();
            for i in 0..N {
                write_array_len(black_box(i), &mut out);
            }
            black_box(out.len())
        })
    });

    group.finish();
}

criterion_group!(benches, bench_parse, bench_reply);
criterion_main!(benches);
