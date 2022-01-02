use criterion::*;
use futures::{future::join, prelude::*, stream::iter};
use ring_channel::{ring_channel, TryRecvError};
use smol::{block_on, unblock};
use std::{env::set_var, num::NonZeroUsize, thread};

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
                        iter(rxs).for_each_concurrent(None, |mut rx| {
                            unblock(move || loop {
                                match rx.try_recv() {
                                    Ok(_) => continue,
                                    Err(TryRecvError::Disconnected) => break,
                                    Err(TryRecvError::Empty) => thread::yield_now(),
                                }
                            })
                        }),
                        iter(txs)
                            .enumerate()
                            .for_each_concurrent(None, |(a, mut tx)| {
                                unblock(move || {
                                    (a * msgs / m + 1..=(a + 1) * msgs / m)
                                        .map(NonZeroUsize::new)
                                        .map(Option::unwrap)
                                        .for_each(|msg| tx.send(msg).unwrap());
                                })
                            }),
                    ));
                },
                BatchSize::SmallInput,
            );
        });
    }
}

fn mpmc(c: &mut Criterion) {
    bench(c, "throughput/mpmc", THREADS / 2, THREADS / 2, 1000);
}

fn mpsc(c: &mut Criterion) {
    bench(c, "throughput/mpsc", THREADS - 1, 1, 1000);
}

fn spmc(c: &mut Criterion) {
    bench(c, "throughput/spmc", 1, THREADS - 1, 1000);
}

fn spsc(c: &mut Criterion) {
    bench(c, "throughput/spsc", 1, 1, 1000);
}

criterion_group!(benches, mpmc, mpsc, spmc, spsc);
criterion_main!(benches);
