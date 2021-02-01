use criterion::*;
use futures::{executor::*, future::*, prelude::*, sink::*, stream::*, task::*};
use rayon::current_num_threads;
use ring_channel::*;
use std::{cmp::max, num::NonZeroUsize};

fn bench(c: &mut Criterion, name: &str, m: usize, n: usize, msgs: usize) {
    let mut group = c.benchmark_group(name);

    for &cap in &[1, m + n, msgs] {
        group.throughput(Throughput::Elements(msgs as u64));
        group.bench_function(format!("{}x{}x{}/{}", m, n, msgs, cap), move |b| {
            let pool = ThreadPool::new().unwrap();

            b.iter_batched(
                || {
                    let (tx, rx) = ring_channel(NonZeroUsize::new(cap).unwrap());
                    (vec![tx; m], vec![rx; n])
                },
                |(txs, rxs)| {
                    let txs = txs
                        .into_iter()
                        .enumerate()
                        .map(|(a, tx)| {
                            iter(a * msgs / m + 1..=(a + 1) * msgs / m)
                                .map(NonZeroUsize::new)
                                .map(Option::unwrap)
                                .map(Ok)
                                .forward(tx)
                                .unwrap_or_else(|_| ())
                        })
                        .map(|f| pool.spawn_with_handle(f).unwrap());

                    let rxs = rxs
                        .into_iter()
                        .map(|rx| rx.map(Ok).forward(drain()).unwrap_or_else(|_| ()))
                        .map(|f| pool.spawn_with_handle(f).unwrap());

                    block_on(join_all(txs.chain(rxs)));
                },
                BatchSize::SmallInput,
            );
        });
    }
}

fn mpmc(c: &mut Criterion) {
    let cardinality = max(current_num_threads() / 2, 1);
    bench(c, "futures/mpmc", cardinality, cardinality, 1000);
}

fn mpsc(c: &mut Criterion) {
    let cardinality = max(current_num_threads() - 1, 1);
    bench(c, "futures/mpsc", cardinality, 1, 1000);
}

fn spmc(c: &mut Criterion) {
    let cardinality = max(current_num_threads() - 1, 1);
    bench(c, "futures/spmc", 1, cardinality, 1000);
}

fn spsc(c: &mut Criterion) {
    bench(c, "futures/spsc", 1, 1, 1000);
}

criterion_group!(benches, mpmc, mpsc, spmc, spsc);
criterion_main!(benches);
