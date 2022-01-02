use criterion::*;
use futures::{future::join, prelude::*, sink::drain, stream::iter};
use ring_channel::ring_channel;
use smol::{block_on, spawn};
use std::{env::set_var, num::NonZeroUsize};

const THREADS: usize = 16;

fn bench(c: &mut Criterion, name: &str, m: usize, n: usize, msgs: usize) {
    set_var("SMOL_THREADS", THREADS.to_string());

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
                    block_on(join(
                        iter(rxs).for_each_concurrent(None, |rx| {
                            spawn(rx.map(Ok).forward(drain()).map(Result::unwrap))
                        }),
                        iter(txs).enumerate().for_each_concurrent(None, |(a, tx)| {
                            spawn(
                                iter(a * msgs / m + 1..=(a + 1) * msgs / m)
                                    .map(NonZeroUsize::new)
                                    .map(Option::unwrap)
                                    .map(Ok)
                                    .forward(tx)
                                    .map(Result::unwrap),
                            )
                        }),
                    ));
                },
                BatchSize::SmallInput,
            );
        });
    }
}

fn mpmc(c: &mut Criterion) {
    bench(c, "futures/mpmc", THREADS / 2, THREADS / 2, 1000);
}

fn mpsc(c: &mut Criterion) {
    bench(c, "futures/mpsc", THREADS - 1, 1, 1000);
}

fn spmc(c: &mut Criterion) {
    bench(c, "futures/spmc", 1, THREADS - 1, 1000);
}

fn spsc(c: &mut Criterion) {
    bench(c, "futures/spsc", 1, 1, 1000);
}

criterion_group!(benches, mpmc, mpsc, spmc, spsc);
criterion_main!(benches);
