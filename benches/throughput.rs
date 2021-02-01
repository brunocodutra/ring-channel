use criterion::*;
use rayon::{current_num_threads, scope};
use ring_channel::*;
use std::{cmp::max, num::NonZeroUsize, thread};

fn bench(c: &mut Criterion, name: &str, m: usize, n: usize, msgs: usize) {
    let mut group = c.benchmark_group(name);

    for &cap in &[1, m + n, msgs] {
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
    let cardinality = max(current_num_threads() / 2, 1);
    bench(c, "throughput/mpmc", cardinality, cardinality, 1000);
}

fn mpsc(c: &mut Criterion) {
    let cardinality = max(current_num_threads() - 1, 1);
    bench(c, "throughput/mpsc", cardinality, 1, 1000);
}

fn spmc(c: &mut Criterion) {
    let cardinality = max(current_num_threads() - 1, 1);
    bench(c, "throughput/spmc", 1, cardinality, 1000);
}

fn spsc(c: &mut Criterion) {
    bench(c, "throughput/spsc", 1, 1, 1000);
}

criterion_group!(benches, mpmc, mpsc, spmc, spsc);
criterion_main!(benches);
