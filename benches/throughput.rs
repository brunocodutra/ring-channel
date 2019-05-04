use criterion::*;
use rayon::{current_num_threads, scope};
use ring_channel::*;
use std::{num::NonZeroUsize, thread};

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
                            s.spawn(move |_| loop {
                                match rx.recv() {
                                    Ok(_) => continue,
                                    Err(RecvError::Disconnected) => break,
                                    Err(RecvError::Empty) => thread::yield_now(),
                                }
                            });
                        }

                        for tx in txs {
                            s.spawn(move |_| {
                                for msg in 1..=msgs / m {
                                    tx.send(NonZeroUsize::new(msg).unwrap()).unwrap();
                                }
                            });
                        }
                    })
                },
                BatchSize::SmallInput,
            );
        },
        vec![1, m + n, msgs],
    )
    .throughput(move |_| Throughput::Elements(msgs as u32))
}

fn mpmc(c: &mut Criterion) {
    let cardinality = current_num_threads() / 2;
    c.bench("mpmc", throughput(cardinality, cardinality, 1000));
}

fn mpsc(c: &mut Criterion) {
    c.bench("mpsc", throughput(current_num_threads() - 1, 1, 1000));
}

fn spmc(c: &mut Criterion) {
    c.bench("spmc", throughput(1, current_num_threads() - 1, 1000));
}

fn spsc(c: &mut Criterion) {
    c.bench("spsc", throughput(1, 1, 1000));
}

criterion_group!(benches, mpmc, mpsc, spmc, spsc);
criterion_main!(benches);
