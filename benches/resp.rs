use bytes::BytesMut;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use ignix::*;

fn bench_resp_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("resp");
    group.bench_function("parse_many_1k", |b| {
        let mut buf = BytesMut::new();
        for i in 0..1000 {
            buf.extend_from_slice(
                format!(
                    "*3\r\n$3\r\nSET\r\n$3\r\nkey\r\n${}\r\nval{}\r\n",
                    3 + i.to_string().len(),
                    i
                )
                .as_bytes(),
            );
        }
        b.iter(|| {
            let mut tmp = buf.clone();
            let mut out = Vec::new();
            protocol::parse_many(&mut tmp, &mut out).unwrap();
            black_box(out.len());
        });
    });
    group.finish();
}

criterion_group!(benches, bench_resp_parse);
criterion_main!(benches);
