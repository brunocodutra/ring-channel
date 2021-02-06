use criterion::*;
use futures::{future::join_all, prelude::*, sink::drain, stream::iter};
use rayon::current_num_threads;
use ring_channel::ring_channel;
use smol::{block_on, spawn};
use std::{env::set_var, num::NonZeroUsize};

fn bench(c: &mut Criterion, name: &str, m: usize, n: usize, msgs: usize) {
    set_var("SMOL_THREADS", current_num_threads().to_string());

    let mut group = c.benchmark_group(name);

    for &cap in &[1, msgs] {
        group.throughput(Throughput::Elements(msgs as u64));
        group.bench_function(format!("{}x{}x{}/{}", m, n, msgs, cap), move |b| {
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
                        .map(spawn);

                    let rxs = rxs
                        .into_iter()
                        .map(|rx| rx.map(Ok).forward(drain()).unwrap_or_else(|_| ()))
                        .map(spawn);

                    block_on(join_all(txs.chain(rxs)));
                },
                BatchSize::SmallInput,
            );
        });
    }
}

fn mpmc(c: &mut Criterion) {
    bench(c, "futures/mpmc", 32, 32, 1000);
}

fn mpsc(c: &mut Criterion) {
    bench(c, "futures/mpsc", 63, 1, 1000);
}

fn spmc(c: &mut Criterion) {
    bench(c, "futures/spmc", 1, 63, 1000);
}

fn spsc(c: &mut Criterion) {
    bench(c, "futures/spsc", 1, 1, 1000);
}

criterion_group!(benches, mpmc, mpsc, spmc, spsc);
criterion_main!(benches);
