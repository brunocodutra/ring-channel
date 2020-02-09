#![no_main]
use futures::{executor::*, future::*, prelude::*, stream::iter, stream::*, task::*};
use lazy_static::lazy_static;
use libfuzzer_sys::fuzz_target;
use ring_channel::*;
use std::{collections::HashSet, iter::from_fn, num::NonZeroUsize};

lazy_static! {
    static ref POOL: ThreadPool = ThreadPool::new().unwrap();
}

fuzz_target!(|input: (Vec<u8>, u8, u8, u16)| {
    let (mut data, senders, receivers, capacity) = input;
    let (senders, receivers, capacity) = (senders as usize, receivers as usize, capacity as usize);

    if let Some(capacity) = NonZeroUsize::new(capacity) {
        let (tx, rx) = ring_channel::<u8>(capacity);
        let (mut txs, mut rxs) = (vec![tx; senders], vec![rx; receivers]);

        let chunks: Box<dyn Iterator<Item = &[u8]>> = if !data.is_empty() && senders > 0 {
            Box::new(data.chunks((data.len() + senders - 1) / senders))
        } else {
            Box::new(from_fn(|| Some(&[] as &[u8])))
        };

        let (results, mut received) = block_on(join(
            join_all(
                txs.drain(..)
                    .zip(chunks)
                    .map(|(tx, data)| iter(data.to_owned()).map(Ok).forward(tx))
                    .map(|fut| POOL.spawn_with_handle(fut))
                    .map(Result::unwrap),
            ),
            join_all(
                rxs.drain(..)
                    .map(StreamExt::collect::<Vec<_>>)
                    .map(|fut| POOL.spawn_with_handle(fut))
                    .map(Result::unwrap),
            )
            .map(IntoIterator::into_iter)
            .map(Iterator::flatten)
            .map(Iterator::collect::<Vec<_>>),
        ));

        if senders > 0 && receivers > 0 {
            for result in results {
                assert_eq!(result, Ok(()));
            }

            if capacity.get() >= data.len() {
                data.sort();
                received.sort();
                assert_eq!(received, data);
            } else {
                let data = data.drain(..).collect::<HashSet<_>>();
                let received = received.drain(..).collect::<HashSet<_>>();
                assert_eq!(received, received.intersection(&data).cloned().collect());
            }
        }
    }
});
