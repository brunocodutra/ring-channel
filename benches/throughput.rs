#![cfg(not(tarpaulin))]

use criterion::measurement::{Measurement, WallTime};
use criterion::{criterion_group, criterion_main, BenchmarkGroup, Criterion, Throughput};
use futures::future::{try_join, try_join_all};
use futures::prelude::*;
use ring_channel::{ring_channel, RingReceiver, RingSender, TryRecvError};
use std::{collections::BTreeSet, fmt::Debug, hint::spin_loop, iter, mem::size_of, time::Duration};
use tokio::runtime::Runtime;
use tokio::task::{self, JoinHandle};

#[cfg(feature = "futures_api")]
use futures::{stream::unfold, SinkExt};

trait Routine<T> {
    fn produce(tx: RingSender<T>, limit: usize) -> JoinHandle<usize>;
    fn consume(rx: RingReceiver<T>, limit: usize) -> JoinHandle<usize>;
}

async fn run<R, const N: usize>(msgs: usize, cap: usize, p: usize, c: usize) -> Duration
where
    R: Routine<[char; N]>,
{
    let (tx, rx) = ring_channel(cap.try_into().unwrap());
    let producer = try_join_all(iter::repeat(tx).take(p).map(|tx| R::produce(tx, msgs / p)));
    let consumer = try_join_all(iter::repeat(rx).take(c).map(|rx| R::consume(rx, msgs / c)));

    let checkpoint = WallTime.start();
    let (produced, consumed) = try_join(producer, consumer).await.unwrap();
    let elapsed = WallTime.end(checkpoint);

    elapsed.div_f64(consumed.iter().sum::<usize>() as f64 / produced.iter().sum::<usize>() as f64)
}

fn bench<R, const N: usize>(group: &mut BenchmarkGroup<WallTime>, p: usize, c: usize)
where
    R: Debug + Default + Routine<[char; N]>,
{
    let prefix = format!("{}x{}/{:?}/{}B", p, c, R::default(), size_of::<[char; N]>());

    let messages_per_iter = 128 * p;
    group.throughput(Throughput::Elements(messages_per_iter as u64));

    for cap in BTreeSet::from([1, 2, p + c]) {
        group.bench_function(format!("{}/{}", prefix, cap), |b| {
            b.to_async(Runtime::new().unwrap())
                .iter_custom(|iters| run::<R, N>(iters as usize * messages_per_iter, cap, p, c));
        });
    }
}

#[cfg(feature = "futures_api")]
#[derive(Debug, Default)]
struct Async;

#[cfg(feature = "futures_api")]
impl<T: 'static + Send + Default> Routine<T> for Async {
    fn produce(tx: RingSender<T>, limit: usize) -> JoinHandle<usize> {
        let producer = unfold(tx, |mut tx| async move {
            <_ as SinkExt<_>>::send(&mut tx, T::default()).await.ok()?;
            Some(((), tx))
        });

        task::spawn(producer.take(limit).count())
    }

    fn consume(rx: RingReceiver<T>, limit: usize) -> JoinHandle<usize> {
        task::spawn(rx.take(limit).count())
    }
}

#[derive(Debug, Default)]
struct Block;

impl<T: 'static + Send + Default> Routine<T> for Block {
    fn produce(mut tx: RingSender<T>, limit: usize) -> JoinHandle<usize> {
        let producer = iter::from_fn(move || tx.send(T::default()).ok());
        task::spawn_blocking(move || producer.take(limit).count())
    }

    fn consume(mut rx: RingReceiver<T>, limit: usize) -> JoinHandle<usize> {
        let consumer = iter::from_fn(move || loop {
            match rx.try_recv() {
                Ok(m) => return Some(m),
                Err(TryRecvError::Disconnected) => return None,
                Err(TryRecvError::Empty) => spin_loop(),
            }
        });

        task::spawn_blocking(move || consumer.take(limit).count())
    }
}

fn mpmc(c: &mut Criterion) {
    let mut group = c.benchmark_group("mpmc");
    let concurrency = (num_cpus::get() / 2).max(2);

    #[cfg(feature = "futures_api")]
    {
        bench::<Async, 1>(&mut group, concurrency, concurrency);
        bench::<Async, 8>(&mut group, concurrency, concurrency);
    }

    {
        bench::<Block, 1>(&mut group, concurrency, concurrency);
        bench::<Block, 8>(&mut group, concurrency, concurrency);
    }
}

fn mpsc(c: &mut Criterion) {
    let mut group = c.benchmark_group("mpsc");
    let concurrency = (num_cpus::get() - 1).max(2);

    #[cfg(feature = "futures_api")]
    {
        bench::<Async, 1>(&mut group, concurrency, 1);
        bench::<Async, 8>(&mut group, concurrency, 1);
    }

    {
        bench::<Block, 1>(&mut group, concurrency, 1);
        bench::<Block, 8>(&mut group, concurrency, 1);
    }
}

fn spmc(c: &mut Criterion) {
    let mut group = c.benchmark_group("spmc");
    let concurrency = (num_cpus::get() - 1).max(2);

    #[cfg(feature = "futures_api")]
    {
        bench::<Async, 1>(&mut group, 1, concurrency);
        bench::<Async, 8>(&mut group, 1, concurrency);
    }

    {
        bench::<Block, 1>(&mut group, 1, concurrency);
        bench::<Block, 8>(&mut group, 1, concurrency);
    }
}

fn spsc(c: &mut Criterion) {
    let mut group = c.benchmark_group("spsc");

    #[cfg(feature = "futures_api")]
    {
        bench::<Async, 1>(&mut group, 1, 1);
        bench::<Async, 8>(&mut group, 1, 1);
    }

    {
        bench::<Block, 1>(&mut group, 1, 1);
        bench::<Block, 8>(&mut group, 1, 1);
    }
}

criterion_group!(benches, mpmc, mpsc, spmc, spsc);
criterion_main!(benches);
