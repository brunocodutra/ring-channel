use criterion::*;
use futures::{executor::*, prelude::*, stream::iter};
use rayon::{current_num_threads, scope};
use ring_channel::*;
use std::{cmp::max, num::NonZeroUsize};

fn throughput(m: usize, n: usize, msgs: usize) -> ParameterizedBenchmark<usize> {
    ParameterizedBenchmark::new(
        format!("{}x{}x{}", m, n, msgs),
        move |b, &cap| {
            b.iter_batched(
                || {
                    let (tx, rx) = ring_channel(NonZeroUsize::new(cap).unwrap());
                    (vec![tx; m], vec![rx; n])
                },
                |(txs, rxs)| {
                    scope(move |s| {
                        for rx in rxs {
                            s.spawn(move |_| for _ in block_on_stream(rx) {});
                        }

                        for mut tx in txs {
                            s.spawn(move |_| {
                                let mut data = iter(1..=msgs / m)
                                    .map(NonZeroUsize::new)
                                    .map(Option::unwrap);

                                block_on(tx.send_all(&mut data)).unwrap();
                            });
                        }
                    })
                },
                BatchSize::SmallInput,
            );
        },
        vec![1, m + n, msgs],
    )
    .throughput(move |_| Throughput::Elements(msgs as u64))
}

fn mpmc(c: &mut Criterion) {
    let cardinality = max(current_num_threads() / 2, 1);
    c.bench("futures/mpmc", throughput(cardinality, cardinality, 1000));
}

fn mpsc(c: &mut Criterion) {
    let cardinality = max(current_num_threads() - 1, 1);
    c.bench("futures/mpsc", throughput(cardinality, 1, 1000));
}

fn spmc(c: &mut Criterion) {
    let cardinality = max(current_num_threads() - 1, 1);
    c.bench("futures/spmc", throughput(1, cardinality, 1000));
}

fn spsc(c: &mut Criterion) {
    c.bench("futures/spsc", throughput(1, 1, 1000));
}

criterion_group!(benches, mpmc, mpsc, spmc, spsc);
criterion_main!(benches);
