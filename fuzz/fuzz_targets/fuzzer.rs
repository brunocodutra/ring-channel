#![no_main]
use futures::{executor::*, future::*, prelude::*, stream::*, task::*};
use lazy_static::lazy_static;
use libfuzzer_sys::{arbitrary, fuzz_target};
use ring_channel::*;
use std::{collections::HashSet, iter::repeat, num::NonZeroUsize};

#[derive(Debug, arbitrary::Arbitrary)]
struct Input {
    data: Vec<u32>,
    capacity: u16,
    senders: u8,
    receivers: u8,
}

lazy_static! {
    static ref POOL: ThreadPool = ThreadPool::new().unwrap();
}

fuzz_target!(|input: Input| {
    let mut data = input.data;
    let capacity = input.capacity as usize;
    let senders = input.senders as usize;
    let receivers = input.receivers as usize;

    if let Some(capacity) = NonZeroUsize::new(capacity) {
        let (tx, rx) = ring_channel(capacity);
        let (mut txs, mut rxs) = (vec![tx; senders], vec![rx; receivers]);

        let chunk_size = if !data.is_empty() && senders > 0 {
            (data.len() + senders - 1) / senders
        } else {
            1
        };

        let (mut received, results) = block_on(join(
            join_all(
                rxs.drain(..)
                    .map(StreamExt::collect::<Vec<_>>)
                    .map(|fut| POOL.spawn_with_handle(fut))
                    .map(Result::unwrap),
            )
            .map(IntoIterator::into_iter)
            .map(Iterator::flatten)
            .map(Iterator::collect::<Vec<_>>),
            join_all(
                txs.drain(..)
                    .zip(data.chunks(chunk_size).chain(repeat([].as_ref())))
                    .map(|(tx, data)| iter(data.to_owned()).map(Ok).forward(tx))
                    .map(|fut| POOL.spawn_with_handle(fut))
                    .map(Result::unwrap),
            ),
        ));

        if receivers == 0 {
            assert_eq!(
                results,
                data.chunks(chunk_size)
                    .map(<[_]>::first)
                    .map(Option::unwrap)
                    .cloned()
                    .map(SendError::Disconnected)
                    .map(Err)
                    .chain(repeat(Ok(())))
                    .take(results.len())
                    .collect::<Vec<_>>(),
                "Sending fails with disconnection error"
            );
        } else if senders > 0 {
            assert_eq!(results, vec![Ok(()); results.len()], "Sending succeeds");

            if capacity.get() >= data.len() {
                data.sort();
                received.sort();
                assert_eq!(received, data, "Data received equals data sent");
            } else {
                let data: HashSet<_> = data.drain(..).collect();
                let received: HashSet<_> = received.drain(..).collect();
                assert_eq!(
                    received,
                    received.intersection(&data).cloned().collect(),
                    "Data received is a subset of data sent"
                );
            }
        }
    }
});
