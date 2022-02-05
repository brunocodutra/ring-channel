use async_trait::async_trait;
use criterion::*;
use futures::{future::join, prelude::*, stream::repeat};
use ring_channel::{ring_channel, RingReceiver, RingSender, TryRecvError};
use smol::{block_on, unblock};
use std::{env::set_var, hint::spin_loop, mem::size_of, num::NonZeroUsize};

#[cfg(feature = "futures_api")]
use futures::sink::drain;

#[cfg(feature = "futures_api")]
use smol::spawn;

const MESSAGES_PER_PRODUCER: usize = 128;

#[async_trait]
trait Routine<T> {
    async fn produce(tx: RingSender<T>);
    async fn consume(rx: RingReceiver<T>);
}

fn bench<R: Routine<[char; N]>, const N: usize>(c: &mut Criterion, name: &str, m: usize, n: usize) {
    set_var("SMOL_THREADS", num_cpus::get().to_string());

    let msgs = MESSAGES_PER_PRODUCER * m;
    let mut group = c.benchmark_group(name);

    for &cap in &[1, msgs] {
        group.throughput(Throughput::Elements(msgs as u64));
        group.bench_function(
            format!("{}x{}x{}/{}/{}B", m, n, msgs, cap, size_of::<[char; N]>()),
            |b| {
                b.iter(|| {
                    let (tx, rx) = ring_channel(NonZeroUsize::new(cap).unwrap());
                    block_on(join(
                        repeat(tx).take(m).for_each_concurrent(None, R::produce),
                        repeat(rx).take(n).for_each_concurrent(None, R::consume),
                    ));
                });
            },
        );
    }
}

#[cfg(feature = "futures_api")]
struct Async;

#[cfg(feature = "futures_api")]
#[async_trait]
impl<T: 'static + Send + Default + Clone> Routine<T> for Async {
    async fn produce(tx: RingSender<T>) {
        let msgs = repeat(T::default()).take(MESSAGES_PER_PRODUCER);
        spawn(msgs.map(Ok).forward(tx).map(Result::unwrap)).await
    }

    async fn consume(rx: RingReceiver<T>) {
        spawn(rx.map(Ok).forward(drain()).map(Result::unwrap)).await
    }
}
struct Block;

#[async_trait]
impl<T: 'static + Send + Default> Routine<T> for Block {
    async fn produce(mut tx: RingSender<T>) {
        unblock(move || {
            for _ in 0..MESSAGES_PER_PRODUCER {
                tx.send(T::default()).unwrap();
            }
        })
        .await
    }

    async fn consume(mut rx: RingReceiver<T>) {
        unblock(move || loop {
            match rx.try_recv() {
                Ok(_) => continue,
                Err(TryRecvError::Disconnected) => break,
                Err(TryRecvError::Empty) => spin_loop(),
            }
        })
        .await
    }
}

fn mpmc(c: &mut Criterion) {
    let concurrency = (num_cpus::get() / 2).max(2);

    #[cfg(feature = "futures_api")]
    bench::<Async, 1>(c, "async/mpmc", concurrency, concurrency);
    #[cfg(feature = "futures_api")]
    bench::<Async, 8>(c, "async/mpmc", concurrency, concurrency);

    bench::<Block, 1>(c, "block/mpmc", concurrency, concurrency);
    bench::<Block, 8>(c, "block/mpmc", concurrency, concurrency);
}

fn mpsc(c: &mut Criterion) {
    let concurrency = (num_cpus::get() - 1).max(2);

    #[cfg(feature = "futures_api")]
    bench::<Async, 1>(c, "async/mpsc", concurrency, 1);
    #[cfg(feature = "futures_api")]
    bench::<Async, 8>(c, "async/mpsc", concurrency, 1);

    bench::<Block, 1>(c, "block/mpsc", concurrency, 1);
    bench::<Block, 8>(c, "block/mpsc", concurrency, 1);
}

fn spmc(c: &mut Criterion) {
    let concurrency = (num_cpus::get() - 1).max(2);

    #[cfg(feature = "futures_api")]
    bench::<Async, 1>(c, "async/spmc", 1, concurrency);
    #[cfg(feature = "futures_api")]
    bench::<Async, 8>(c, "async/spmc", 1, concurrency);

    bench::<Block, 1>(c, "block/spmc", 1, concurrency);
    bench::<Block, 8>(c, "block/spmc", 1, concurrency);
}

fn spsc(c: &mut Criterion) {
    #[cfg(feature = "futures_api")]
    bench::<Async, 1>(c, "async/spsc", 1, 1);
    #[cfg(feature = "futures_api")]
    bench::<Async, 8>(c, "async/spsc", 1, 1);

    bench::<Block, 1>(c, "block/spsc", 1, 1);
    bench::<Block, 8>(c, "block/spsc", 1, 1);
}

criterion_group!(benches, mpmc, mpsc, spmc, spsc);
criterion_main!(benches);
