use criterion::*;
use rayon::{current_num_threads, scope};
use ring_channel::*;
use std::{cmp::max, num::NonZeroUsize, thread};

fn bench(m: usize, n: usize, msgs: usize) -> ParameterizedBenchmark<usize> {
    ParameterizedBenchmark::new(
        format!("{}x{}x{}", m, n, msgs),
        move |b, &cap| {
            b.iter_batched(
                || {
                    let (tx, rx) = ring_channel(NonZeroUsize::new(cap).unwrap());
                    (vec![tx; m], vec![rx; n])
                },
                |(mut txs, mut rxs)| {
                    scope(move |s| {
                        rxs.drain(..).for_each(|mut rx| {
                            s.spawn(move |_| loop {
                                match rx.try_recv() {
                                    Ok(_) => continue,
                                    Err(TryRecvError::Disconnected) => break,
                                    Err(TryRecvError::Empty) => thread::yield_now(),
                                }
                            });
                        });

                        txs.drain(..).enumerate().for_each(|(a, mut tx)| {
                            s.spawn(move |_| {
                                (1..=msgs / m)
                                    .map(|b| a * msgs / m + b)
                                    .map(NonZeroUsize::new)
                                    .map(Option::unwrap)
                                    .for_each(|msg| tx.send(msg).unwrap());
                            });
                        });
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
    c.bench("throughput/mpmc", bench(cardinality, cardinality, 1000));
}

fn mpsc(c: &mut Criterion) {
    let cardinality = max(current_num_threads() - 1, 1);
    c.bench("throughput/mpsc", bench(cardinality, 1, 1000));
}

fn spmc(c: &mut Criterion) {
    let cardinality = max(current_num_threads() - 1, 1);
    c.bench("throughput/spmc", bench(1, cardinality, 1000));
}

fn spsc(c: &mut Criterion) {
    c.bench("throughput/spsc", bench(1, 1, 1000));
}

criterion_group!(benches, mpmc, mpsc, spmc, spsc);
criterion_main!(benches);
