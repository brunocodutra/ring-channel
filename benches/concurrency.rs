use criterion::*;
use rayon::iter::*;
use ring_channel::*;
use std::num::NonZeroUsize;

fn concurrency(c: &mut Criterion) {
    const CARDINALITY: usize = 10000;

    c.benchmark_group("concurrency")
        .throughput(Throughput::Elements(CARDINALITY as u64))
        .bench_function(CARDINALITY.to_string(), move |b| {
            b.iter_batched_ref(
                || ring_channel::<()>(NonZeroUsize::new(1).unwrap()),
                |(tx, rx)| repeatn((tx.clone(), rx.clone()), CARDINALITY).for_each(drop),
                BatchSize::SmallInput,
            );
        });
}

criterion_group!(benches, concurrency);
criterion_main!(benches);
