use crate::{channel::*, error::*};
use futures::task::{Context, Poll};
use futures::{sink::Sink, stream::Stream};
use std::pin::Pin;

impl<T> Sink<T> for RingSender<T> {
    type Error = SendError<T>;

    fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: T) -> Result<(), Self::Error> {
        self.send(item)
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}

impl<T> Stream for RingReceiver<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.recv() {
            Ok(msg) => Poll::Ready(Some(msg)),
            Err(RecvError::Disconnected) => Poll::Ready(None),
            Err(RecvError::Empty) => {
                // Keep polling thread awake.
                ctx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::ring_channel;
    use futures::{executor::*, prelude::*};
    use proptest::{collection::vec_deque, prelude::*};
    use rayon::scope;
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

    proptest! {
        #[test]
        fn stream(cap in 1..=100usize, mut msgs in vec_deque("[a-z]", 1..=100)) {
            let (tx, rx) = ring_channel(NonZeroUsize::new(cap).unwrap());
            let overwritten = msgs.len() - min(msgs.len(), cap);

            for msg in msgs.iter().cloned() {
                assert_eq!(tx.send(msg), Ok(()));
            }

            drop(tx); // hang-up

            assert_eq!(
                block_on_stream(rx).collect::<Vec<_>>(),
                msgs.drain(..).skip(overwritten).collect::<Vec<_>>()
            );
        }
    }

    proptest! {
        #[test]
        fn wake_on_disconnect(n in 1..=100usize) {
            let (tx, rx) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());

            scope(move |s| {
                for _ in 0..n {
                    let rx = rx.clone();
                    s.spawn(move |_| assert_eq!(block_on_stream(rx).collect::<Vec<_>>(), vec![]));
                }

                drop(tx);
            });
        }

        #[test]
        fn wake_on_send(n in 1..=100usize) {
            let (tx, rx) = ring_channel(NonZeroUsize::new(n).unwrap());

            scope(move |s| {
                for _ in 0..n {
                    let tx = tx.clone();
                    let mut rx = rx.clone();
                    s.spawn(move |_| {
                        assert_eq!(block_on(rx.next()), Some(42));
                        drop(tx); // Avoids waking on disconnect
                    });
                }

                for _ in 0..n {
                    assert_eq!(tx.send(42), Ok(()));
                }
            });
        }
    }
}
