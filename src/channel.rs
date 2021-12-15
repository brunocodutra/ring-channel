use crate::{control::*, error::*};
use core::sync::atomic::*;
use core::{mem::ManuallyDrop, num::NonZeroUsize};
use derivative::Derivative;

#[cfg(feature = "futures_api")]
use futures::{sink::Sink, stream::*, task::*};

#[cfg(feature = "futures_api")]
use core::pin::Pin;

/// The sending end of a [`ring_channel`].
#[derive(Derivative, Eq, PartialEq)]
#[derivative(Debug(bound = ""))]
pub struct RingSender<T> {
    #[derivative(Debug = "ignore")]
    handle: ManuallyDrop<ControlBlockRef<T>>,
}

unsafe impl<T: Send> Send for RingSender<T> {}
unsafe impl<T: Send> Sync for RingSender<T> {}

impl<T> RingSender<T> {
    fn new(handle: ManuallyDrop<ControlBlockRef<T>>) -> Self {
        Self { handle }
    }

    /// Sends a message through the channel without blocking.
    ///
    /// * If the channel is not disconnected, the message is pushed into the internal ring buffer.
    ///     * If the internal ring buffer is full, the oldest pending message is overwritten.
    /// * If the channel is disconnected, [`SendError::Disconnected`] is returned.
    pub fn send(&mut self, message: T) -> Result<(), SendError<T>> {
        if self.handle.receivers.load(Ordering::Relaxed) > 0 {
            self.handle.buffer.push(message);

            // A full memory barrier is necessary to prevent waitlist loads
            // from being reordered before storing the message.
            #[cfg(feature = "futures_api")]
            fence(Ordering::SeqCst);

            #[cfg(feature = "futures_api")]
            self.handle.waitlist.wake();

            Ok(())
        } else {
            Err(SendError::Disconnected(message))
        }
    }
}

impl<T> Clone for RingSender<T> {
    fn clone(&self) -> Self {
        self.handle.senders.fetch_add(1, Ordering::Relaxed);
        RingSender::new(self.handle.clone())
    }
}

impl<T> Drop for RingSender<T> {
    fn drop(&mut self) {
        // Synchronizes with other senders.
        if self.handle.senders.fetch_sub(1, Ordering::AcqRel) == 1 {
            // A full memory barrier is necessary to prevent waitlist loads
            // from being reordered before updating senders.
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

/// Requires [feature] `"futures_api"`
///
/// [feature]: index.html#optional-features
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
#[derive(Derivative, Eq, PartialEq)]
#[derivative(Debug(bound = ""))]
pub struct RingReceiver<T> {
    #[derivative(Debug = "ignore")]
    handle: ManuallyDrop<ControlBlockRef<T>>,
}

unsafe impl<T: Send> Send for RingReceiver<T> {}
unsafe impl<T: Send> Sync for RingReceiver<T> {}

impl<T> RingReceiver<T> {
    fn new(handle: ManuallyDrop<ControlBlockRef<T>>) -> Self {
        Self { handle }
    }

    /// Receives a message through the channel (requires [features] `"futures_api"` and `"std"`).
    ///
    /// * If the internal ring buffer isn't empty, the oldest pending message is returned.
    /// * If the internal ring buffer is empty, the call blocks until a message is sent
    /// or the channel disconnects.
    /// * If the channel is disconnected and the internal ring buffer is empty,
    /// [`RecvError::Disconnected`] is returned.
    ///
    /// [features]: index.html#optional-features
    #[cfg(all(feature = "std", feature = "futures_api"))]
    pub fn recv(&mut self) -> Result<T, RecvError> {
        futures::executor::block_on(self.next()).ok_or(RecvError::Disconnected)
    }

    /// Receives a message through the channel without blocking.
    ///
    /// * If the internal ring buffer isn't empty, the oldest pending message is returned.
    /// * If the internal ring buffer is empty, [`TryRecvError::Empty`] is returned.
    /// * If the channel is disconnected and the internal ring buffer is empty,
    /// [`TryRecvError::Disconnected`] is returned.
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
        self.handle.receivers.fetch_add(1, Ordering::Relaxed);
        RingReceiver::new(self.handle.clone())
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

/// Requires [feature] `"futures_api"`.
///
/// [feature]: index.html#optional-features
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
                // from being reordered before storing the waker.
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
/// use std::{num::NonZeroUsize, thread};
/// use std::time::{Duration, Instant};
///
/// // Open a channel to transmit the time elapsed since the beginning of the countdown.
/// // We only need a buffer of size 1, since we're only interested in the current value.
/// let (mut tx, mut rx) = ring_channel(NonZeroUsize::new(1).unwrap());
///
/// thread::spawn(move || {
///     let countdown = Instant::now() + Duration::from_secs(10);
///
///     // Update the channel with the time elapsed so far.
///     while let Ok(_) = tx.send(countdown - Instant::now()) {
///
///         // We only need millisecond precision.
///         thread::sleep(Duration::from_millis(1));
///
///         if Instant::now() > countdown {
///             break;
///         }
///     }
/// });
///
/// loop {
///     match rx.try_recv() {
///         // Print the current time elapsed.
///         Ok(timer) => {
///             print!("\r{:02}.{:03}", timer.as_secs(), timer.as_millis() % 1000);
///
///             if timer <= Duration::from_millis(6600) {
///                 print!(" - Main engine start                           ");
///             } else {
///                 print!(" - Activate main engine hydrogen burnoff system");
///             }
///         }
///         Err(TryRecvError::Disconnected) => break,
///         Err(TryRecvError::Empty) => thread::yield_now(),
///     }
/// }
///
/// println!("\r00.0000 - Solid rocket booster ignition and liftoff!")
/// ```
pub fn ring_channel<T>(capacity: NonZeroUsize) -> (RingSender<T>, RingReceiver<T>) {
    let handle = ManuallyDrop::new(ControlBlockRef::new(capacity.get()));
    (RingSender::new(handle.clone()), RingReceiver::new(handle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{string::String, vec::Vec};
    use core::{cmp::min, iter};
    use rayon::{iter::repeatn, prelude::*};
    use test_strategy::proptest;

    #[cfg(feature = "futures_api")]
    use smol::block_on;

    #[cfg(feature = "futures_api")]
    use futures::{prelude::*, stream};

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
        #[allow(clippy::redundant_clone)]
        let x = s.clone();
        assert_eq!(x.handle.senders.load(Ordering::Relaxed), 2);
        assert_eq!(x.handle.receivers.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn cloning_receiver_increments_receivers_counter() {
        let (_s, r) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());
        #[allow(clippy::redundant_clone)]
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
        assert!(!r.handle.connected.load(Ordering::Relaxed));
    }

    #[test]
    fn channel_is_disconnected_if_there_are_no_receivers() {
        let (s, _) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());
        assert_eq!(s.handle.receivers.load(Ordering::Relaxed), 0);
        assert!(!s.handle.connected.load(Ordering::Relaxed));
    }

    #[derive(Clone)]
    enum Endpoint<T> {
        Sender(RingSender<T>),
        Receiver(RingReceiver<T>),
    }

    #[proptest]
    fn endpoints_are_safe_to_send_across_threads(
        #[strategy(1..=100usize)] m: usize,
        #[strategy(1..=100usize)] n: usize,
    ) {
        let (s, r) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());
        let ls = repeatn(s, m).map(Endpoint::Sender);
        let rs = repeatn(r, n).map(Endpoint::Receiver);
        ls.chain(rs).for_each(drop);
    }

    #[proptest]
    fn endpoints_are_safe_to_share_across_threads(
        #[strategy(1..=100usize)] m: usize,
        #[strategy(1..=100usize)] n: usize,
    ) {
        let (s, r) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());
        let ls = repeatn(&s, m).cloned().map(Endpoint::Sender);
        let rs = repeatn(&r, n).cloned().map(Endpoint::Receiver);
        ls.chain(rs).for_each(drop);
    }

    #[proptest]
    fn send_succeeds_on_connected_channel(
        #[strategy(1..=100usize)] capacity: usize,
        #[any(((1..=100).into(), "[[:ascii:]]".into()))] msgs: Vec<String>,
    ) {
        let (s, _r) = ring_channel(NonZeroUsize::new(capacity).unwrap());
        repeatn(s, msgs.len())
            .zip(msgs.par_iter().cloned())
            .for_each(|(mut c, msg)| {
                assert_eq!(c.send(msg), Ok(()));
            });
    }

    #[proptest]
    fn send_fails_on_disconnected_channel(
        #[strategy(1..=100usize)] capacity: usize,
        #[any(((1..=100).into(), "[[:ascii:]]".into()))] msgs: Vec<String>,
    ) {
        let (s, _) = ring_channel(NonZeroUsize::new(capacity).unwrap());
        repeatn(s, msgs.len())
            .zip(msgs.par_iter().cloned())
            .for_each(|(mut c, msg)| {
                assert_eq!(c.send(msg.clone()), Err(SendError::Disconnected(msg)));
            });
    }

    #[proptest]
    fn send_overwrites_old_messages(
        #[strategy(1..=100usize)] capacity: usize,
        #[any(((1..=100).into(), "[[:ascii:]]".into()))] msgs: Vec<String>,
    ) {
        let (mut s, r) = ring_channel(NonZeroUsize::new(capacity).unwrap());
        let overwritten = msgs.len() - min(msgs.len(), capacity);

        for msg in msgs.iter().cloned() {
            assert_eq!(s.send(msg), Ok(()));
        }

        assert_eq!(
            iter::from_fn(|| r.handle.buffer.pop()).collect::<Vec<_>>(),
            msgs.into_iter().skip(overwritten).collect::<Vec<_>>()
        );
    }

    #[cfg(all(feature = "std", feature = "futures_api"))]
    #[proptest]
    fn recv_succeeds_on_non_empty_connected_channel(
        #[any(((1..=100).into(), "[[:ascii:]]".into()))] msgs: Vec<String>,
    ) {
        let (s, r) = ring_channel(NonZeroUsize::new(msgs.len()).unwrap());

        for msg in msgs.iter().cloned().enumerate() {
            s.handle.buffer.push(msg);
        }

        let mut received = vec![(0usize, Default::default()); msgs.len()];
        repeatn(r, msgs.len())
            .zip(received.par_iter_mut())
            .for_each(|(mut c, slot)| {
                *slot = c.recv().unwrap();
            });

        received.sort_by_key(|(k, _)| *k);
        assert_eq!(
            received.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
            msgs
        );
    }

    #[cfg(all(feature = "std", feature = "futures_api"))]
    #[proptest]
    fn recv_succeeds_on_non_empty_disconnected_channel(
        #[any(((1..=100).into(), "[[:ascii:]]".into()))] msgs: Vec<String>,
    ) {
        let (_, r) = ring_channel(NonZeroUsize::new(msgs.len()).unwrap());

        for msg in msgs.iter().cloned().enumerate() {
            r.handle.buffer.push(msg);
        }

        let mut received = vec![(0usize, Default::default()); msgs.len()];
        repeatn(r, msgs.len())
            .zip(received.par_iter_mut())
            .for_each(|(mut c, slot)| {
                *slot = c.recv().unwrap();
            });

        received.sort_by_key(|(k, _)| *k);
        assert_eq!(
            received.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
            msgs
        );
    }

    #[cfg(all(feature = "std", feature = "futures_api"))]
    #[proptest]
    fn recv_fails_on_empty_disconnected_channel(
        #[strategy(1..=100usize)] capacity: usize,
        #[strategy(1..=100usize)] n: usize,
    ) {
        let (_, r) = ring_channel::<()>(NonZeroUsize::new(capacity).unwrap());
        repeatn(r, n).for_each(|mut r| {
            assert_eq!(r.recv(), Err(RecvError::Disconnected));
        });
    }

    #[proptest]
    fn try_recv_succeeds_on_non_empty_connected_channel(
        #[any(((1..=100).into(), "[[:ascii:]]".into()))] msgs: Vec<String>,
    ) {
        let (s, r) = ring_channel(NonZeroUsize::new(msgs.len()).unwrap());

        for msg in msgs.iter().cloned().enumerate() {
            s.handle.buffer.push(msg);
        }

        let mut received = vec![(0usize, Default::default()); msgs.len()];
        repeatn(r, msgs.len())
            .zip(received.par_iter_mut())
            .for_each(|(mut c, slot)| {
                *slot = c.try_recv().unwrap();
            });

        received.sort_by_key(|(k, _)| *k);
        assert_eq!(
            received.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
            msgs
        );
    }

    #[proptest]
    fn try_recv_succeeds_on_non_empty_disconnected_channel(
        #[any(((1..=100).into(), "[[:ascii:]]".into()))] msgs: Vec<String>,
    ) {
        let (_, r) = ring_channel(NonZeroUsize::new(msgs.len()).unwrap());

        for msg in msgs.iter().cloned().enumerate() {
            r.handle.buffer.push(msg);
        }

        let mut received = vec![(0usize, Default::default()); msgs.len()];

        repeatn(r, msgs.len())
            .zip(received.par_iter_mut())
            .for_each(|(mut c, slot)| {
                *slot = c.try_recv().unwrap();
            });

        received.sort_by_key(|(k, _)| *k);
        assert_eq!(
            received.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
            msgs
        );
    }

    #[proptest]
    fn try_recv_fails_on_empty_connected_channel(
        #[strategy(1..=100usize)] capacity: usize,
        #[strategy(1..=100usize)] n: usize,
    ) {
        let (_s, r) = ring_channel::<()>(NonZeroUsize::new(capacity).unwrap());
        repeatn(r, n).for_each(|mut r| {
            assert_eq!(r.try_recv(), Err(TryRecvError::Empty));
        });
    }

    #[proptest]
    fn try_recv_fails_on_empty_disconnected_channel(
        #[strategy(1..=100usize)] capacity: usize,
        #[strategy(1..=100usize)] n: usize,
    ) {
        let (_, r) = ring_channel::<()>(NonZeroUsize::new(capacity).unwrap());
        repeatn(r, n).for_each(|mut r| {
            assert_eq!(r.try_recv(), Err(TryRecvError::Disconnected));
        });
    }

    #[cfg(feature = "futures_api")]
    #[proptest]
    fn sink(
        #[strategy(1..=100usize)] capacity: usize,
        #[any(((1..=100).into(), "[[:ascii:]]".into()))] msgs: Vec<String>,
    ) {
        let (mut tx, mut rx) = ring_channel(NonZeroUsize::new(capacity).unwrap());
        let overwritten = msgs.len() - min(msgs.len(), capacity);

        assert_eq!(
            block_on(tx.send_all(&mut iter(msgs.clone()).map(Ok))),
            Ok(())
        );

        drop(tx); // hang-up

        assert_eq!(
            iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>(),
            msgs.into_iter().skip(overwritten).collect::<Vec<_>>()
        );
    }

    #[cfg(feature = "futures_api")]
    #[proptest]
    fn stream(
        #[strategy(1..=100usize)] capacity: usize,
        #[any(((1..=100).into(), "[[:ascii:]]".into()))] msgs: Vec<String>,
    ) {
        let (mut tx, rx) = ring_channel(NonZeroUsize::new(capacity).unwrap());
        let overwritten = msgs.len() - min(msgs.len(), capacity);

        for msg in msgs.iter().cloned() {
            assert_eq!(tx.send(msg), Ok(()));
        }

        drop(tx); // hang-up

        assert_eq!(
            block_on(rx.collect::<Vec<_>>()),
            msgs.into_iter().skip(overwritten).collect::<Vec<_>>()
        );
    }

    #[cfg(feature = "futures_api")]
    #[proptest]
    fn stream_wakes_on_disconnect(#[strategy(1..=100usize)] n: usize) {
        let (tx, rx) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());

        rayon::scope(|s| {
            for _ in 0..n {
                let rx = rx.clone();
                s.spawn(|_| assert_eq!(block_on(rx.collect::<Vec<_>>()), vec![]));
            }

            s.spawn(|_| drop(tx));
        });
    }

    #[cfg(feature = "futures_api")]
    #[proptest]
    fn stream_wakes_on_send(#[strategy(1..=100usize)] n: usize) {
        let (tx, rx) = ring_channel(NonZeroUsize::new(n).unwrap());

        rayon::scope(|s| {
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

    #[cfg(feature = "futures_api")]
    #[proptest]
    fn stream_wakes_on_send_all(#[strategy(1..=100usize)] n: usize) {
        let (mut tx, rx) = ring_channel(NonZeroUsize::new(n).unwrap());

        rayon::scope(|s| {
            for _ in 0..n {
                let tx = tx.clone();
                let mut rx = rx.clone();
                s.spawn(move |_| {
                    assert_eq!(block_on(rx.next()), Some(42));
                    drop(tx); // Avoids waking on disconnect
                });
            }

            let mut msgs = stream::iter(vec![Ok(42); n]);
            s.spawn(move |_| assert_eq!(block_on(tx.send_all(&mut msgs)), Ok(())));
        });
    }

    #[cfg(all(feature = "futures_api", feature = "std"))]
    #[proptest]
    fn recv_wakes_on_disconnect(#[strategy(1..=100usize)] n: usize) {
        let (tx, rx) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());

        rayon::scope(|s| {
            for _ in 0..n {
                let mut rx = rx.clone();
                s.spawn(move |_| assert_eq!(rx.recv(), Err(RecvError::Disconnected)));
            }

            s.spawn(|_| drop(tx));
        });
    }

    #[cfg(all(feature = "futures_api", feature = "std"))]
    #[proptest]
    fn recv_wakes_on_send(#[strategy(1..=100usize)] n: usize) {
        let (tx, rx) = ring_channel(NonZeroUsize::new(n).unwrap());

        rayon::scope(|s| {
            for _ in 0..n {
                let tx = tx.clone();
                let mut rx = rx.clone();
                s.spawn(move |_| {
                    assert_eq!(rx.recv(), Ok(42));
                    drop(tx); // Avoids waking on disconnect
                });
            }

            for _ in 0..n {
                let mut tx = tx.clone();
                s.spawn(move |_| assert_eq!(tx.send(42), Ok(())));
            }
        });
    }

    #[cfg(all(feature = "futures_api", feature = "std"))]
    #[proptest]
    fn recv_wakes_on_send_all(#[strategy(1..=100usize)] n: usize) {
        let (mut tx, rx) = ring_channel(NonZeroUsize::new(n).unwrap());

        rayon::scope(|s| {
            for _ in 0..n {
                let tx = tx.clone();
                let mut rx = rx.clone();
                s.spawn(move |_| {
                    assert_eq!(rx.recv(), Ok(42));
                    drop(tx); // Avoids waking on disconnect
                });
            }

            let mut msgs = stream::iter(vec![Ok(42); n]);
            s.spawn(move |_| assert_eq!(block_on(tx.send_all(&mut msgs)), Ok(())));
        });
    }
}
