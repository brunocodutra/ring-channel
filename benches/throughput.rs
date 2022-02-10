use async_trait::async_trait;
use criterion::measurement::{Measurement, WallTime};
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use futures::{future::join, prelude::*, stream::repeat};
use ring_channel::{ring_channel, RingReceiver, RingSender, TryRecvError};
use smol::{block_on, spawn, unblock};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{env::set_var, hint::spin_loop, iter, mem::size_of, num::NonZeroUsize};

#[cfg(feature = "futures_api")]
use futures::{stream::unfold, SinkExt};

const MESSAGES: usize = 100;

#[async_trait]
trait Routine<T> {
    async fn produce(tx: RingSender<T>, limit: usize) -> usize;
    async fn consume(rx: RingReceiver<T>, limit: usize) -> usize;
}

fn bench<R: Routine<[char; N]>, const N: usize>(c: &mut Criterion, name: &str, m: usize, n: usize) {
    set_var("SMOL_THREADS", num_cpus::get().to_string());

    let mut group = c.benchmark_group(name);
    group.throughput(Throughput::Elements(MESSAGES as u64));

    for &cap in &[1, m + n] {
        group.bench_function(
            format!("{}B/{}x{}/{}", size_of::<[char; N]>(), m, n, cap),
            |b| {
                b.iter_custom(|iters| {
                    let produced = AtomicUsize::new(0);
                    let consumed = AtomicUsize::new(0);
                    let checkpoint = WallTime.start();

                    let (tx, rx) = ring_channel(NonZeroUsize::new(cap).unwrap());

                    block_on(join(
                        repeat(tx).take(m).for_each_concurrent(None, |tx| async {
                            produced.fetch_add(
                                spawn(R::produce(tx, iters as usize * MESSAGES / m)).await,
                                Ordering::Relaxed,
                            );
                        }),
                        repeat(rx).take(n).for_each_concurrent(None, |rx| async {
                            consumed.fetch_add(
                                spawn(R::consume(rx, iters as usize * MESSAGES / n)).await,
                                Ordering::Relaxed,
                            );
                        }),
                    ));

                    let elapsed = WallTime.end(checkpoint);
                    elapsed.div_f64(consumed.into_inner() as f64 / produced.into_inner() as f64)
                });
            },
        );
    }
}

#[cfg(feature = "futures_api")]
struct Async;

#[cfg(feature = "futures_api")]
#[async_trait]
impl<T: 'static + Send + Default> Routine<T> for Async {
    async fn produce(tx: RingSender<T>, limit: usize) -> usize {
        let producer = unfold(tx, |mut tx| async move {
            <_ as SinkExt<_>>::send(&mut tx, T::default()).await.ok()?;
            Some(((), tx))
        });

        producer.take(limit).count().await
    }

    async fn consume(rx: RingReceiver<T>, limit: usize) -> usize {
        rx.take(limit).count().await
    }
}
struct Block;

#[async_trait]
impl<T: 'static + Send + Default> Routine<T> for Block {
    async fn produce(mut tx: RingSender<T>, limit: usize) -> usize {
        let producer = iter::from_fn(move || tx.send(T::default()).ok());
        unblock(move || producer.take(limit).count()).await
    }

    async fn consume(mut rx: RingReceiver<T>, limit: usize) -> usize {
        let consumer = iter::from_fn(move || loop {
            match rx.try_recv() {
                Ok(m) => return Some(m),
                Err(TryRecvError::Disconnected) => return None,
                Err(TryRecvError::Empty) => spin_loop(),
            }
        });

        unblock(move || consumer.take(limit).count()).await
    }
}

fn mpmc(c: &mut Criterion) {
    let concurrency = (num_cpus::get() / 2).max(2);

    #[cfg(feature = "futures_api")]
    bench::<Async, 1>(c, "async/mpmc", concurrency, concurrency);
    bench::<Block, 1>(c, "block/mpmc", concurrency, concurrency);

    #[cfg(feature = "futures_api")]
    bench::<Async, 8>(c, "async/mpmc", concurrency, concurrency);
    bench::<Block, 8>(c, "block/mpmc", concurrency, concurrency);
}

fn mpsc(c: &mut Criterion) {
    let concurrency = (num_cpus::get() - 1).max(2);

    #[cfg(feature = "futures_api")]
    bench::<Async, 1>(c, "async/mpsc", concurrency, 1);
    bench::<Block, 1>(c, "block/mpsc", concurrency, 1);

    #[cfg(feature = "futures_api")]
    bench::<Async, 8>(c, "async/mpsc", concurrency, 1);
    bench::<Block, 8>(c, "block/mpsc", concurrency, 1);
}

fn spmc(c: &mut Criterion) {
    let concurrency = (num_cpus::get() - 1).max(2);

    #[cfg(feature = "futures_api")]
    bench::<Async, 1>(c, "async/spmc", 1, concurrency);
    bench::<Block, 1>(c, "block/spmc", 1, concurrency);

    #[cfg(feature = "futures_api")]
    bench::<Async, 8>(c, "async/spmc", 1, concurrency);
    bench::<Block, 8>(c, "block/spmc", 1, concurrency);
}

fn spsc(c: &mut Criterion) {
    #[cfg(feature = "futures_api")]
    bench::<Async, 1>(c, "async/spsc", 1, 1);
    bench::<Block, 1>(c, "block/spsc", 1, 1);

    #[cfg(feature = "futures_api")]
    bench::<Async, 8>(c, "async/spsc", 1, 1);
    bench::<Block, 8>(c, "block/spsc", 1, 1);
}

criterion_group!(benches, mpmc, mpsc, spmc, spsc);
criterion_main!(benches);
