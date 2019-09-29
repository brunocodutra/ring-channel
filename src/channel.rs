use crate::{control::*, error::*};
use derivative::Derivative;
use std::sync::atomic::*;
use std::{mem::ManuallyDrop, num::NonZeroUsize};

#[cfg(feature = "futures_api")]
use futures::{sink::Sink, stream::Stream, task::*};

#[cfg(feature = "futures_api")]
use std::pin::Pin;

/// The sending end of a [`ring_channel`].
///
/// [`ring_channel`]: fn.ring_channel.html
#[derive(Derivative, Eq, PartialEq)]
#[derivative(Debug(bound = ""))]
pub struct RingSender<T> {
    #[derivative(Debug = "ignore")]
    handle: ManuallyDrop<ControlBlockRef<T>>,
}

unsafe impl<T: Send> Send for RingSender<T> {}

impl<T> RingSender<T> {
    /// Sends a message through the channel without blocking.
    ///
    /// * If the channel is not disconnected, the message is pushed into the internal ring buffer.
    ///     * If the internal ring buffer is full, the oldest pending message is overwritten.
    /// * If the channel is disconnected, [`SendError::Disconnected`] is returned.
    ///
    /// [`SendError::Disconnected`]: enum.SendError.html#variant.Disconnected
    pub fn send(&mut self, value: T) -> Result<(), SendError<T>> {
        if self.handle.receivers.load(Ordering::Relaxed) > 0 {
            self.handle.buffer.push(value);

            // A full memory barrier is necessary to prevent waitlist loads
            // from being reordered before the buffer stores.
            #[cfg(feature = "futures_api")]
            fence(Ordering::SeqCst);

            #[cfg(feature = "futures_api")]
            self.handle.waitlist.wake();

            Ok(())
        } else {
            Err(SendError::Disconnected(value))
        }
    }
}

impl<T> Clone for RingSender<T> {
    fn clone(&self) -> Self {
        let handle = self.handle.clone();
        handle.senders.fetch_add(1, Ordering::Relaxed);
        Self { handle }
    }
}

impl<T> Drop for RingSender<T> {
    fn drop(&mut self) {
        // Synchronizes with other senders.
        if self.handle.senders.fetch_sub(1, Ordering::AcqRel) == 1 {
            // A full memory barrier is necessary to prevent waitlist loads
            // from being reordered before senders stores.
            #[cfg(feature = "futures_api")]
            fence(Ordering::SeqCst);

            #[cfg(feature = "futures_api")]
            self.handle.waitlist.wake();

            // Synchronizes the last sender and receiver with each other.
            if !self.handle.connected.swap(false, Ordering::AcqRel) {
                unsafe { ManuallyDrop::drop(&mut self.handle) }
            }
        }
    }
}

#[cfg(feature = "futures_api")]
impl<T> Sink<T> for RingSender<T> {
    type Error = SendError<T>;

    fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(mut self: Pin<&mut Self>, item: T) -> Result<(), Self::Error> {
        self.send(item)
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}

/// The receiving end of a [`ring_channel`].
///
/// [`ring_channel`]: fn.ring_channel.html
#[derive(Derivative, Eq, PartialEq)]
#[derivative(Debug(bound = ""))]
pub struct RingReceiver<T> {
    #[derivative(Debug = "ignore")]
    handle: ManuallyDrop<ControlBlockRef<T>>,
}

unsafe impl<T: Send> Send for RingReceiver<T> {}

impl<T> RingReceiver<T> {
    /// Receives a message through the channel without blocking.
    ///
    /// * If the internal ring buffer isn't empty, the oldest pending message is returned.
    /// * If the internal ring buffer is empty, [`TryRecvError::Empty`] is returned.
    /// * If the channel is disconnected and the internal ring buffer is empty,
    /// [`TryRecvError::Disconnected`] is returned.
    ///
    /// [`TryRecvError::Empty`]: enum.TryRecvError.html#variant.Empty
    /// [`TryRecvError::Disconnected`]: enum.TryRecvError.html#variant.Disconnected
    pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
        self.handle.buffer.pop().ok_or_else(|| {
            if self.handle.senders.load(Ordering::Relaxed) > 0 {
                TryRecvError::Empty
            } else {
                TryRecvError::Disconnected
            }
        })
    }
}

impl<T> Clone for RingReceiver<T> {
    fn clone(&self) -> Self {
        let handle = self.handle.clone();
        handle.receivers.fetch_add(1, Ordering::Relaxed);
        Self { handle }
    }
}

impl<T> Drop for RingReceiver<T> {
    fn drop(&mut self) {
        // Synchronizes with other receivers.
        if self.handle.receivers.fetch_sub(1, Ordering::AcqRel) == 1 {
            // Synchronizes the last sender and receiver with each other.
            if !self.handle.connected.swap(false, Ordering::AcqRel) {
                unsafe { ManuallyDrop::drop(&mut self.handle) }
            }
        }
    }
}

#[cfg(feature = "futures_api")]
impl<T> Stream for RingReceiver<T> {
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.try_recv() {
            Ok(msg) => Poll::Ready(Some(msg)),
            Err(TryRecvError::Disconnected) => Poll::Ready(None),
            Err(TryRecvError::Empty) => {
                self.handle.waitlist.wait(ctx.waker().clone());

                // A full memory barrier is necessary to prevent the following loads
                // from being reordered before waitlist stores.
                fence(Ordering::SeqCst);

                // Look at the buffer again in case a new message has been sent in the meantime.
                match self.try_recv() {
                    Ok(msg) => Poll::Ready(Some(msg)),
                    Err(TryRecvError::Disconnected) => Poll::Ready(None),
                    Err(TryRecvError::Empty) => Poll::Pending,
                }
            }
        }
    }
}

/// Opens a multi-producer multi-consumer channel backed by a ring buffer.
///
/// The associated ring buffer can contain up to `capacity` pending messages.
///
/// Sending and receiving messages through this channel _never blocks_, however
/// pending messages may be overwritten if the internal ring buffer overflows.
///
/// # Panics
///
/// Panics if the `capacity` is `0`.
///
/// # Examples
///
/// ```rust,no_run
/// use ring_channel::*;
/// use std::num::NonZeroUsize;
/// use std::thread;
/// use std::time::{Duration, Instant};
///
/// fn main() {
///     // Open a channel to transmit the time elapsed since the beginning of the countdown.
///     // We only need a buffer of size 1, since we're only interested in the current value.
///     let (mut tx, mut rx) = ring_channel(NonZeroUsize::new(1).unwrap());
///
///     thread::spawn(move || {
///         let countdown = Instant::now() + Duration::from_secs(10);
///
///         // Update the channel with the time elapsed so far.
///         while let Ok(_) = tx.send(countdown - Instant::now()) {
///
///             // We only need millisecond precision.
///             thread::sleep(Duration::from_millis(1));
///
///             if Instant::now() > countdown {
///                 break;
///             }
///         }
///     });
///
///     loop {
///         match rx.try_recv() {
///             // Print the current time elapsed.
///             Ok(timer) => {
///                 print!("\r{:02}.{:03}", timer.as_secs(), timer.as_millis() % 1000);
///
///                 if timer <= Duration::from_millis(6600) {
///                     print!(" - Main engine start                           ");
///                 } else {
///                     print!(" - Activate main engine hydrogen burnoff system");
///                 }
///             }
///             Err(TryRecvError::Empty) => thread::yield_now(),
///             Err(TryRecvError::Disconnected) => break,
///         }
///     }
///
///     println!("\r00.0000 - Solid rocket booster ignition and liftoff!")
/// }
/// ```
pub fn ring_channel<T>(capacity: NonZeroUsize) -> (RingSender<T>, RingReceiver<T>) {
    let l = ManuallyDrop::new(ControlBlockRef::new(capacity.get()));
    let r = l.clone();
    (RingSender { handle: l }, RingReceiver { handle: r })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::{collection::*, prelude::*};
    use rayon::{iter::repeatn, prelude::*};
    use std::{cmp::min, iter};

    #[cfg(feature = "futures_api")]
    use futures::{executor::*, prelude::*, stream};

    #[test]
    fn ring_channel_is_associated_with_a_single_control_block() {
        let (s, r) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());
        assert_eq!(s.handle, r.handle);
    }

    #[test]
    fn senders_are_equal_if_they_are_associated_with_the_same_ring_channel() {
        let (s1, _) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());
        let (s2, _) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());

        assert_eq!(s1, s1.clone());
        assert_eq!(s2, s2.clone());
        assert_ne!(s1, s2);
    }

    #[test]
    fn receivers_are_equal_if_they_are_associated_with_the_same_ring_channel() {
        let (_, r1) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());
        let (_, r2) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());

        assert_eq!(r1, r1.clone());
        assert_eq!(r2, r2.clone());
        assert_ne!(r1, r2);
    }

    #[test]
    fn cloning_sender_increments_senders() {
        let (s, _r) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());
        let x = s.clone();
        assert_eq!(x.handle.senders.load(Ordering::Relaxed), 2);
        assert_eq!(x.handle.receivers.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn cloning_receiver_increments_receivers_counter() {
        let (_s, r) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());
        let x = r.clone();
        assert_eq!(x.handle.senders.load(Ordering::Relaxed), 1);
        assert_eq!(x.handle.receivers.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn dropping_sender_decrements_senders_counter() {
        let (_, r) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());
        assert_eq!(r.handle.senders.load(Ordering::Relaxed), 0);
        assert_eq!(r.handle.receivers.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn dropping_receiver_decrements_receivers_counter() {
        let (s, _) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());
        assert_eq!(s.handle.senders.load(Ordering::Relaxed), 1);
        assert_eq!(s.handle.receivers.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn channel_is_disconnected_if_there_are_no_senders() {
        let (_, r) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());
        assert_eq!(r.handle.senders.load(Ordering::Relaxed), 0);
        assert_eq!(r.handle.connected.load(Ordering::Relaxed), false);
    }

    #[test]
    fn channel_is_disconnected_if_there_are_no_receivers() {
        let (s, _) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());
        assert_eq!(s.handle.receivers.load(Ordering::Relaxed), 0);
        assert_eq!(s.handle.connected.load(Ordering::Relaxed), false);
    }

    proptest! {
        #[test]
        fn endpoints_are_safe_to_send_across_threads(m in 1..=100usize, n in 1..=100usize) {
            #[derive(Clone)]
            enum Endpoint<T> {
                Sender(RingSender<T>),
                Receiver(RingReceiver<T>),
            }

            let (s, r) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());
            let ls = repeatn(Endpoint::Sender(s), m);
            let rs = repeatn(Endpoint::Receiver(r), n);
            ls.chain(rs).for_each(drop);
        }

        #[test]
        fn send_succeeds_on_connected_channel(cap in 1..=100usize, msgs in vec("[a-z]", 1..=100)) {
            let (s, _r) = ring_channel(NonZeroUsize::new(cap).unwrap());
            repeatn(s, msgs.len()).zip(msgs.par_iter().cloned()).for_each(|(mut c, msg)| {
                assert_eq!(c.send(msg), Ok(()));
            });
        }

        #[test]
        fn send_fails_on_disconnected_channel(cap in 1..=100usize, msgs in vec("[a-z]", 1..=100)) {
            let (s, _) = ring_channel(NonZeroUsize::new(cap).unwrap());
            repeatn(s, msgs.len()).zip(msgs.par_iter().cloned()).for_each(|(mut c, msg)| {
                assert_eq!(c.send(msg.clone()), Err(SendError::Disconnected(msg)));
            });
        }

        #[test]
        fn send_overwrites_old_messages(cap in 1..=100usize, mut msgs in vec("[a-z]", 1..=100)) {
            let (mut s, r) = ring_channel(NonZeroUsize::new(cap).unwrap());
            let overwritten = msgs.len() - min(msgs.len(), cap);

            for msg in msgs.iter().cloned() {
                assert_eq!(s.send(msg), Ok(()));
            }

            assert_eq!(
                iter::from_fn(move || r.handle.buffer.pop()).collect::<Vec<_>>(),
                msgs.drain(..).skip(overwritten).collect::<Vec<_>>()
            );
        }

        #[test]
        fn try_recv_succeeds_on_non_empty_connected_channel(msgs in vec("[a-z]", 1..=100)) {
            let (s, r) = ring_channel(NonZeroUsize::new(msgs.len()).unwrap());

            for msg in msgs.iter().cloned().enumerate() {
                s.handle.buffer.push(msg);
            }

            let mut received = vec![(0usize, Default::default()); msgs.len()];

            repeatn(r, msgs.len()).zip(received.par_iter_mut()).for_each(|(mut c, slot)| {
                match c.try_recv() {
                    Ok(msg) => *slot = msg,
                    Err(e) => panic!(e),
                };
            });

            received.sort_by_key(|(k, _)| *k);
            assert_eq!(received.drain(..).map(|(_, v)| v).collect::<Vec<_>>(), msgs);
        }

        #[test]
        fn try_recv_succeeds_on_non_empty_disconnected_channel(msgs in vec("[a-z]", 1..=100)) {
            let (_, r) = ring_channel(NonZeroUsize::new(msgs.len()).unwrap());

            for msg in msgs.iter().cloned().enumerate() {
                r.handle.buffer.push(msg);
            }

            let mut received = vec![(0usize, Default::default()); msgs.len()];

            repeatn(r, msgs.len()).zip(received.par_iter_mut()).for_each(|(mut c, slot)| {
                match c.try_recv() {
                    Ok(msg) => *slot = msg,
                    Err(e) => panic!(e),
                };
            });

            received.sort_by_key(|(k, _)| *k);
            assert_eq!(received.drain(..).map(|(_, v)| v).collect::<Vec<_>>(), msgs);
        }

        #[test]
        fn try_recv_fails_on_empty_connected_channel(cap in 1..=100usize, n in 1..=100usize) {
            let (_s, r) = ring_channel::<()>(NonZeroUsize::new(cap).unwrap());
            repeatn(r, n).for_each(|mut r| {
                assert_eq!(r.try_recv(), Err(TryRecvError::Empty));
            });
        }

        #[test]
        fn try_recv_fails_on_empty_disconnected_channel(cap in 1..=100usize, n in 1..=100usize) {
            let (_, r) = ring_channel::<()>(NonZeroUsize::new(cap).unwrap());
            repeatn(r, n).for_each(move |mut r| {
                assert_eq!(r.try_recv(), Err(TryRecvError::Disconnected));
            });
        }
    }

    #[cfg(feature = "futures_api")]
    proptest! {
        #[test]
        fn sink(cap in 1..=100usize, mut msgs in vec_deque("[a-z]", 1..=100)) {
            let (mut tx, mut rx) = ring_channel(NonZeroUsize::new(cap).unwrap());
            let overwritten = msgs.len() - min(msgs.len(), cap);

            assert_eq!(block_on(tx.send_all(&mut msgs.clone())), Ok(()));

            drop(tx); // hang-up

            assert_eq!(
                iter::from_fn(move || rx.try_recv().ok()).collect::<Vec<_>>(),
                msgs.drain(..).skip(overwritten).collect::<Vec<_>>()
            );
        }

        #[test]
        fn stream(cap in 1..=100usize, mut msgs in vec_deque("[a-z]", 1..=100)) {
            let (mut tx, rx) = ring_channel(NonZeroUsize::new(cap).unwrap());
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

        #[test]
        fn stream_wakes_on_disconnect(n in 1..=100usize) {
            let (tx, rx) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());

            rayon::scope(move |s| {
                for _ in 0..n {
                    let rx = rx.clone();
                    s.spawn(move |_| assert_eq!(block_on_stream(rx).collect::<Vec<_>>(), vec![]));
                }

                s.spawn(move |_| drop(tx));
            });
        }

        #[test]
        fn stream_wakes_on_send(n in 1..=100usize) {
            let (tx, rx) = ring_channel(NonZeroUsize::new(n).unwrap());

            rayon::scope(move |s| {
                for _ in 0..n {
                    let tx = tx.clone();
                    let mut rx = rx.clone();
                    s.spawn(move |_| {
                        assert_eq!(block_on(rx.next()), Some(42));
                        drop(tx); // Avoids waking on disconnect
                    });
                }

                for _ in 0..n {
                    let mut tx = tx.clone();
                    s.spawn(move |_| assert_eq!(tx.send(42), Ok(())));
                }
            });
        }

        #[test]
        fn stream_wakes_on_send_all(n in 1..=100usize) {
            let (mut tx, rx) = ring_channel(NonZeroUsize::new(n).unwrap());

            rayon::scope(move |s| {
                for _ in 0..n {
                    let tx = tx.clone();
                    let mut rx = rx.clone();
                    s.spawn(move |_| {
                        assert_eq!(block_on(rx.next()), Some(42));
                        drop(tx); // Avoids waking on disconnect
                    });
                }

                let mut msgs = stream::iter(vec![42; n]);
                s.spawn(move |_| assert_eq!(block_on(tx.send_all(&mut msgs)), Ok(())));
            });
        }
    }
}
