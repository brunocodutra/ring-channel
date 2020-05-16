#![no_main]

use futures::{executor::*, future::*, prelude::*, stream::*, task::*};
use lazy_static::lazy_static;
use libfuzzer_sys::fuzz_target;
use ring_channel::*;
use std::{collections::HashSet, iter::repeat, num::NonZeroUsize};

lazy_static! {
    static ref POOL: ThreadPool = ThreadPool::new().unwrap();
}

fuzz_target!(|input: (Box<[u32]>, u16, u8, u8)| {
    let mut data = input.0;
    let capacity = input.1 as usize;
    let senders = input.2 as usize;
    let receivers = input.3 as usize;

    if let Some(capacity) = NonZeroUsize::new(capacity) {
        let (tx, rx) = ring_channel(capacity);
        let (txs, rxs) = (vec![tx; senders], vec![rx; receivers]);

        let chunk_size = if !data.is_empty() && senders > 0 {
            (data.len() + senders - 1) / senders
        } else {
            1
        };

        let (mut received, results) = block_on(join(
            join_all(
                rxs.into_iter()
                    .map(StreamExt::collect::<Vec<_>>)
                    .map(|fut| POOL.spawn_with_handle(fut))
                    .map(Result::unwrap),
            )
            .map(IntoIterator::into_iter)
            .map(Iterator::flatten)
            .map(Iterator::collect::<Box<[_]>>),
            join_all(
                txs.into_iter()
                    .zip(data.chunks(chunk_size).chain(repeat([].as_ref())))
                    .map(|(tx, data)| iter(data.to_owned()).map(Ok).forward(tx))
                    .map(|fut| POOL.spawn_with_handle(fut))
                    .map(Result::unwrap),
            ),
        ));

        if senders == 0 {
            assert!(
                received.is_empty(),
                "no data is received if there are no senders"
            );
        } else if receivers == 0 {
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
                "sending fails with disconnection error if there are no receivers"
            );
        } else {
            assert_eq!(
                results,
                vec![Ok(()); results.len()],
                "sending succeeds if there are both senders and receivers"
            );

            if capacity.get() >= data.len() {
                data.sort();
                received.sort();
                assert_eq!(
                    received, data,
                    "data received equals data sent if the buffer capacity is sufficient"
                );
            } else {
                let data: HashSet<_> = data.into_iter().collect();
                let received: HashSet<_> = received.into_iter().collect();
                assert_eq!(
                    received,
                    received.intersection(&data).cloned().collect(),
                    "data received is a subset of data sent if the buffer capacity is insufficient"
                );
            }
        }
    }
});
