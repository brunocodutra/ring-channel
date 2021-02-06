use criterion::*;
use rayon::scope;
use ring_channel::{ring_channel, TryRecvError};
use std::{num::NonZeroUsize, thread};

fn bench(c: &mut Criterion, name: &str, m: usize, n: usize, msgs: usize) {
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
                    scope(move |s| {
                        rxs.into_iter().for_each(|mut rx| {
                            s.spawn(move |_| loop {
                                match rx.try_recv() {
                                    Ok(_) => continue,
                                    Err(TryRecvError::Disconnected) => break,
                                    Err(TryRecvError::Empty) => thread::yield_now(),
                                }
                            });
                        });

                        txs.into_iter().enumerate().for_each(|(a, mut tx)| {
                            s.spawn(move |_| {
                                (a * msgs / m + 1..=(a + 1) * msgs / m)
                                    .map(NonZeroUsize::new)
                                    .map(Option::unwrap)
                                    .for_each(|msg| tx.send(msg).unwrap());
                            });
                        });
                    })
                },
                BatchSize::SmallInput,
            );
        });
    }
}

fn mpmc(c: &mut Criterion) {
    bench(c, "throughput/mpmc", 32, 32, 1000);
}

fn mpsc(c: &mut Criterion) {
    bench(c, "throughput/mpsc", 63, 1, 1000);
}

fn spmc(c: &mut Criterion) {
    bench(c, "throughput/spmc", 1, 63, 1000);
}

fn spsc(c: &mut Criterion) {
    bench(c, "throughput/spsc", 1, 1, 1000);
}

criterion_group!(benches, mpmc, mpsc, spmc, spsc);
criterion_main!(benches);
