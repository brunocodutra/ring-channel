use crate::{error::*, sender::*};
use futures::task::{Context, Poll};
use futures::sink::Sink;
use std::{marker::Unpin, pin::Pin};

impl<T> Unpin for RingSender<T> {}

impl<T> Sink<T> for RingSender<T> {
    type SinkError = SendError<T>;

    fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::SinkError>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: T) -> Result<(), Self::SinkError> {
        self.send(item)
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::SinkError>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::SinkError>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use crate::ring_channel;
    use futures::{executor::*, prelude::*};
    use proptest::{collection::vec_deque, prelude::*};
    use std::{cmp::min, iter, num::NonZeroUsize};

    proptest! {
        #[test]
        fn sink(cap in 1..=100usize, mut msgs in vec_deque("[a-z]", 1..=100)) {
            let (mut tx, rx) = ring_channel(NonZeroUsize::new(cap).unwrap());
            let overwritten = msgs.len() - min(msgs.len(), cap);

            assert_eq!(block_on(tx.send_all(&mut msgs.clone())), Ok(()));

            drop(tx); // hang-up

            assert_eq!(
                iter::from_fn(move || rx.recv().ok()).collect::<Vec<_>>(),
                msgs.drain(..).skip(overwritten).collect::<Vec<_>>()
            );
        }
    }
}
