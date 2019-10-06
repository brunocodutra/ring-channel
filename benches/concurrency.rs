use criterion::*;
use rayon::iter::*;
use ring_channel::*;
use std::num::NonZeroUsize;

fn concurrency(c: &mut Criterion) {
    let cardinality = 10000;

    c.bench(
        "concurrency",
        Benchmark::new(cardinality.to_string(), move |b| {
            b.iter_batched_ref(
                || ring_channel::<()>(NonZeroUsize::new(1).unwrap()),
                |(tx, rx)| repeatn((tx.clone(), rx.clone()), cardinality).for_each(drop),
                BatchSize::SmallInput,
            );
        })
        .throughput(Throughput::Elements(cardinality as u64)),
    );
}

criterion_group!(benches, concurrency);
criterion_main!(benches);
