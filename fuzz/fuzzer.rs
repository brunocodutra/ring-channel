#![cfg(not(tarpaulin))]
#![no_main]

use futures::{future::*, prelude::*, stream::*};
use libfuzzer_sys::arbitrary::{Arbitrary, Error as LibfuzzerError, Unstructured};
use libfuzzer_sys::fuzz_target;
use ring_channel::*;
use std::{collections::HashSet, error::Error, iter::repeat, num::NonZeroUsize};
use tokio::spawn;

#[tokio::main]
async fn fuzzer(
    mut data: Box<[u16]>,
    senders: usize,
    receivers: usize,
    capacity: NonZeroUsize,
) -> Result<(), Box<dyn Error>> {
    let chunk_size = if !data.is_empty() && senders > 0 {
        (data.len() + senders - 1) / senders
    } else {
        1
    };

    let (tx, rx) = ring_channel(capacity);
    let (txs, rxs) = (vec![tx; senders], vec![rx; receivers]);

    let sender = data
        .chunks(chunk_size)
        .chain(repeat([].as_ref()))
        .zip(txs)
        .map(|(data, tx)| spawn(iter(data.to_owned()).map(Ok).forward(tx)));

    let receiver = rxs.into_iter().map(|rx| spawn(rx.collect::<Vec<_>>()));
    let (received, results) = try_join(try_join_all(receiver), try_join_all(sender)).await?;
    let mut received = received.into_iter().flatten().collect::<Box<[_]>>();

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

    Ok(())
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct Input {
    data: Box<[u16]>,
    senders: usize,
    receivers: usize,
    capacity: NonZeroUsize,
}

impl<'a> Arbitrary<'a> for Input {
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self, LibfuzzerError> {
        Ok(Self {
            data: u.arbitrary()?,
            senders: u.arbitrary::<u8>()?.into(),
            receivers: u.arbitrary::<u8>()?.into(),
            capacity: NonZeroUsize::new(u.arbitrary::<u8>()?.into())
                .ok_or(LibfuzzerError::IncorrectFormat)?,
        })
    }
}

fuzz_target!(|input: Input| {
    fuzzer(input.data, input.senders, input.receivers, input.capacity).unwrap()
});
