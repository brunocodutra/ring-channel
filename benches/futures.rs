use criterion::*;
use futures::{executor::*, future::*, prelude::*, stream::iter, stream::*, task::*};
use rayon::current_num_threads;
use ring_channel::*;
use std::{cmp::max, num::NonZeroUsize};

fn bench(m: usize, n: usize, msgs: usize) -> ParameterizedBenchmark<usize> {
    ParameterizedBenchmark::new(
        format!("{}x{}x{}", m, n, msgs),
        move |b, &cap| {
            let mut pool = ThreadPool::new().unwrap();

            b.iter_batched(
                || {
                    let (tx, rx) = ring_channel(NonZeroUsize::new(cap).unwrap());
                    (vec![tx; m], vec![rx; n])
                },
                move |(mut txs, mut rxs)| {
                    block_on(join(
                        join_all(
                            rxs.drain(..)
                                .map(StreamExt::collect::<Vec<_>>)
                                .map(|fut| pool.spawn_with_handle(fut))
                                .map(Result::unwrap),
                        ),
                        join_all(
                            txs.drain(..)
                                .enumerate()
                                .map(|(a, tx)| {
                                    iter(1..=msgs / m)
                                        .map(move |b| a * msgs / m + b)
                                        .map(NonZeroUsize::new)
                                        .map(Option::unwrap)
                                        .map(Ok)
                                        .forward(tx)
                                })
                                .map(|fut| pool.spawn_with_handle(fut))
                                .map(Result::unwrap),
                        ),
                    ));
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
    c.bench("futures/mpmc", bench(cardinality, cardinality, 500));
}

fn mpsc(c: &mut Criterion) {
    let cardinality = max(current_num_threads() - 1, 1);
    c.bench("futures/mpsc", bench(cardinality, 1, 500));
}

fn spmc(c: &mut Criterion) {
    let cardinality = max(current_num_threads() - 1, 1);
    c.bench("futures/spmc", bench(1, cardinality, 500));
}

fn spsc(c: &mut Criterion) {
    c.bench("futures/spsc", bench(1, 1, 500));
}

criterion_group!(benches, mpmc, mpsc, spmc, spsc);
criterion_main!(benches);
