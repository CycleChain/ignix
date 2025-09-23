use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};
use ignix::*;

fn bench_exec_set_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("exec");
    group.bench_function("set_get", |b| {
        b.iter_batched(
            || Shard::new(0, None),
            |mut shard| {
                for i in 0..1000u32 {
                    let k = format!("k{}", i).into_bytes();
                    let v = format!("v{}", i).into_bytes();
                    let _ = shard.exec(Cmd::Set(k.clone(), v));
                    let _ = shard.exec(Cmd::Get(k));
                }
                black_box(shard)
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group!(benches, bench_exec_set_get);
criterion_main!(benches);
