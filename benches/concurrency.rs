use criterion::*;
use rayon::{current_num_threads, scope};
use ring_channel::*;
use std::{cmp::max, num::NonZeroUsize};

fn concurrency(c: &mut Criterion) {
    let cardinality = 10000;
    let concurrency = current_num_threads();

    c.bench(
        "concurrency",
        Benchmark::new(cardinality.to_string(), move |b| {
            b.iter_batched_ref(
                || ring_channel::<usize>(NonZeroUsize::new(1).unwrap()),
                |(tx, rx)| {
                    scope(|s| {
                        for _ in 0..max(concurrency / 2, 1) {
                            s.spawn(|_| {
                                for _ in 0..cardinality / concurrency {
                                    drop(tx.clone());
                                }
                            });

                            s.spawn(|_| {
                                for _ in 0..cardinality / concurrency {
                                    drop(rx.clone());
                                }
                            });
                        }
                    })
                },
                BatchSize::SmallInput,
            );
        })
        .throughput(Throughput::Elements(cardinality as u64)),
    );
}

criterion_group!(benches, concurrency);
criterion_main!(benches);
