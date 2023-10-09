use crate::{control::ControlBlockRef, error::*};
use core::{mem::ManuallyDrop, num::NonZeroUsize, sync::atomic::*};
use derivative::Derivative;

#[cfg(feature = "futures_api")]
use crate::waitlist::Slot;

#[cfg(feature = "futures_api")]
use futures::{task::*, Sink, Stream};

#[cfg(feature = "futures_api")]
use core::{mem, pin::Pin};

/// The sending end of a [`ring_channel`].
#[derive(Derivative)]
#[derivative(Debug(bound = ""), Eq(bound = ""), PartialEq(bound = ""))]
pub struct RingSender<T> {
    handle: ManuallyDrop<ControlBlockRef<T>>,

    #[cfg(feature = "futures_api")]
    #[derivative(PartialEq = "ignore")]
    backoff: bool,
}

unsafe impl<T: Send> Send for RingSender<T> {}
unsafe impl<T: Send> Sync for RingSender<T> {}

impl<T> RingSender<T> {
    fn new(handle: ManuallyDrop<ControlBlockRef<T>>) -> Self {
        Self {
            handle,

            #[cfg(feature = "futures_api")]
            backoff: false,
        }
    }

    /// Sends a message through the channel without blocking.
    ///
    /// * If the channel is not disconnected, the message is pushed into the internal ring buffer.
    ///     * If the internal ring buffer is full, the oldest pending message is overwritten
    ///       and returned as `Ok(Some(_))`, otherwise `Ok(None)` is returned.
    /// * If the channel is disconnected, [`SendError::Disconnected`] is returned.
    pub fn send(&self, message: T) -> Result<Option<T>, SendError<T>> {
        if self.handle.receivers.load(Ordering::Acquire) > 0 {
            let overwritten = self.handle.buffer.push(message);

            #[cfg(feature = "futures_api")]
            if overwritten.is_none() {
                // A full memory barrier is necessary to ensure that storing the message
                // happens before waking receivers.
                fence(Ordering::SeqCst);

                if let Some(waker) = self.handle.waitlist.pop() {
                    waker.wake();
                }
            }

            Ok(overwritten)
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
            #[cfg(feature = "futures_api")]
            {
                // A full memory barrier is necessary to ensure that updating senders
                // happens before waking receivers.
                fence(Ordering::SeqCst);

                while let Some(waker) = self.handle.waitlist.pop() {
                    waker.wake();
                }
            }

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

    #[inline]
    fn poll_ready(self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.poll_flush(ctx)
    }

    #[inline]
    fn start_send(mut self: Pin<&mut Self>, item: T) -> Result<(), Self::Error> {
        self.backoff = self.send(item)?.is_some();
        Ok(())
    }

    #[inline]
    fn poll_flush(
        mut self: Pin<&mut Self>,
        ctx: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        if mem::take(&mut self.backoff) {
            ctx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(Ok(()))
        }
    }

    #[inline]
    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}

/// The receiving end of a [`ring_channel`].
#[derive(Derivative)]
#[derivative(Debug(bound = ""), Eq(bound = ""), PartialEq(bound = ""))]
pub struct RingReceiver<T> {
    handle: ManuallyDrop<ControlBlockRef<T>>,

    #[cfg(feature = "futures_api")]
    #[derivative(PartialEq = "ignore")]
    slot: Slot,
}

unsafe impl<T: Send> Send for RingReceiver<T> {}
unsafe impl<T: Send> Sync for RingReceiver<T> {}

impl<T> RingReceiver<T> {
    fn new(handle: ManuallyDrop<ControlBlockRef<T>>) -> Self {
        Self {
            #[cfg(feature = "futures_api")]
            slot: handle.waitlist.register(),

            handle,
        }
    }

    /// Receives a message through the channel without blocking.
    ///
    /// * If the internal ring buffer isn't empty, the oldest pending message is returned.
    /// * If the internal ring buffer is empty, [`TryRecvError::Empty`] is returned.
    /// * If the channel is disconnected and the internal ring buffer is empty,
    /// [`TryRecvError::Disconnected`] is returned.
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        // We must check whether the channel is connected using acquire ordering before we look at
        // the buffer, in order to ensure that the loads associated with popping from the buffer
        // happen after the stores associated with a push into the buffer that may have happened
        // immediately before the channel was disconnected.
        if self.handle.senders.load(Ordering::Acquire) > 0 {
            self.handle.buffer.pop().ok_or(TryRecvError::Empty)
        } else {
            self.handle.buffer.pop().ok_or(TryRecvError::Disconnected)
        }
    }

    /// Receives a message through the channel (requires [feature] `"futures_api"`).
    ///
    /// * If the internal ring buffer isn't empty, the oldest pending message is returned.
    /// * If the internal ring buffer is empty, the call blocks until a message is sent
    /// or the channel disconnects.
    /// * If the channel is disconnected and the internal ring buffer is empty,
    /// [`RecvError::Disconnected`] is returned.
    ///
    /// [feature]: index.html#optional-features
    #[cfg(feature = "futures_api")]
    pub fn recv(&self) -> Result<T, RecvError> {
        futures::executor::block_on(futures::future::poll_fn(|ctx| self.poll(ctx)))
            .ok_or(RecvError::Disconnected)
    }

    #[cfg(feature = "futures_api")]
    fn poll(&self, ctx: &mut Context<'_>) -> Poll<Option<T>> {
        match self.try_recv() {
            result @ Ok(_) | result @ Err(TryRecvError::Disconnected) => {
                self.handle.waitlist.remove(self.slot);
                Poll::Ready(result.ok())
            }

            Err(TryRecvError::Empty) => {
                self.handle.waitlist.insert(self.slot, ctx.waker().clone());

                // A full memory barrier is necessary to ensure that storing the waker
                // happens before attempting to retrieve a message from the buffer.
                fence(Ordering::SeqCst);

                // Look at the buffer again in case a new message has been sent in the meantime.
                match self.try_recv() {
                    result @ Ok(_) | result @ Err(TryRecvError::Disconnected) => {
                        self.handle.waitlist.remove(self.slot);
                        Poll::Ready(result.ok())
                    }

                    Err(TryRecvError::Empty) => Poll::Pending,
                }
            }
        }
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
        #[cfg(feature = "futures_api")]
        if self.handle.waitlist.deregister(self.slot).is_none() {
            // Wake some other receiver in case we have been woken in the meantime.
            if let Some(waker) = self.handle.waitlist.pop() {
                waker.wake();
            }
        }

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

    fn poll_next(self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.poll(ctx)
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
/// ```no_run
/// # fn doctest() -> Result<(), Box<dyn std::error::Error>> {
/// use ring_channel::*;
/// use std::{num::NonZeroUsize, thread};
/// use std::time::{Duration, Instant};
///
/// // Open a channel to transmit the time elapsed since the beginning of the countdown.
/// // We only need a buffer of size 1, since we're only interested in the current value.
/// let (tx, rx) = ring_channel(NonZeroUsize::try_from(1)?);
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
/// println!("\r00.0000 - Solid rocket booster ignition and liftoff!");
/// # Ok(())
/// # }
/// ```
pub fn ring_channel<T>(capacity: NonZeroUsize) -> (RingSender<T>, RingReceiver<T>) {
    let handle = ManuallyDrop::new(ControlBlockRef::new(capacity.get()));
    (RingSender::new(handle.clone()), RingReceiver::new(handle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Void;
    use alloc::vec::Vec;
    use core::{cmp::min, iter};
    use futures::stream::{iter, repeat};
    use futures::{future::try_join_all, prelude::*};
    use proptest::collection::size_range;
    use test_strategy::proptest;
    use tokio::runtime;
    use tokio::task::spawn_blocking;

    #[cfg(feature = "futures_api")]
    use core::time::Duration;

    #[cfg(feature = "futures_api")]
    use alloc::{sync::Arc, task::Wake};

    #[cfg(feature = "futures_api")]
    use futures::future::try_join;

    #[cfg(feature = "futures_api")]
    use tokio::{task::spawn, time::timeout};

    #[cfg(feature = "futures_api")]
    #[derive(Debug, Copy, Clone, Eq, PartialEq)]
    struct MockWaker;

    #[cfg(feature = "futures_api")]
    impl Wake for MockWaker {
        fn wake(self: Arc<Self>) {}
    }

    #[proptest]
    fn ring_channel_is_associated_with_a_single_control_block() {
        let (tx, rx) = ring_channel::<Void>(NonZeroUsize::try_from(1)?);
        assert_eq!(tx.handle, rx.handle);
    }

    #[proptest]
    fn senders_are_equal_if_they_are_associated_with_the_same_ring_channel() {
        let (s1, _) = ring_channel::<Void>(NonZeroUsize::try_from(1)?);
        let (s2, _) = ring_channel::<Void>(NonZeroUsize::try_from(1)?);

        assert_eq!(s1, s1.clone());
        assert_eq!(s2, s2.clone());
        assert_ne!(s1, s2);
    }

    #[cfg(feature = "futures_api")]
    #[proptest]
    fn senders_are_equal_even_if_backoff_is_different() {
        let (mut tx, _) = ring_channel::<Void>(NonZeroUsize::try_from(1)?);
        tx.backoff = true;
        assert_eq!(tx, tx.clone());
    }

    #[proptest]
    fn receivers_are_equal_if_they_are_associated_with_the_same_ring_channel() {
        let (_, r1) = ring_channel::<Void>(NonZeroUsize::try_from(1)?);
        let (_, r2) = ring_channel::<Void>(NonZeroUsize::try_from(1)?);

        assert_eq!(r1, r1.clone());
        assert_eq!(r2, r2.clone());
        assert_ne!(r1, r2);
    }

    #[proptest]
    fn cloning_sender_increments_senders() {
        let (tx, rx) = ring_channel::<Void>(NonZeroUsize::try_from(1)?);
        #[allow(clippy::redundant_clone)]
        let tx = tx.clone();
        assert_eq!(tx.handle.senders.load(Ordering::SeqCst), 2);
        assert_eq!(rx.handle.receivers.load(Ordering::SeqCst), 1);
    }

    #[cfg(feature = "futures_api")]
    #[proptest]
    fn cloning_sender_resets_backoff_flag() {
        let (mut tx, _) = ring_channel::<Void>(NonZeroUsize::try_from(1)?);
        tx.backoff = true;
        assert_ne!(tx.clone().backoff, tx.backoff);
    }

    #[proptest]
    fn cloning_receiver_increments_receivers_counter() {
        let (tx, rx) = ring_channel::<Void>(NonZeroUsize::try_from(1)?);
        #[allow(clippy::redundant_clone)]
        let rx = rx.clone();
        assert_eq!(tx.handle.senders.load(Ordering::SeqCst), 1);
        assert_eq!(rx.handle.receivers.load(Ordering::SeqCst), 2);
    }

    #[cfg(feature = "futures_api")]
    #[proptest]
    fn cloning_receiver_registers_waitlist_slot() {
        let (_, rx) = ring_channel::<Void>(NonZeroUsize::try_from(1)?);
        assert_ne!(rx.clone().slot, rx.slot);
    }

    #[proptest]
    fn dropping_sender_decrements_senders_counter() {
        let (_, rx) = ring_channel::<Void>(NonZeroUsize::try_from(1)?);
        assert_eq!(rx.handle.senders.load(Ordering::SeqCst), 0);
        assert_eq!(rx.handle.receivers.load(Ordering::SeqCst), 1);
    }

    #[proptest]
    fn dropping_receiver_decrements_receivers_counter() {
        let (tx, _) = ring_channel::<Void>(NonZeroUsize::try_from(1)?);
        assert_eq!(tx.handle.senders.load(Ordering::SeqCst), 1);
        assert_eq!(tx.handle.receivers.load(Ordering::SeqCst), 0);
    }

    #[cfg(feature = "futures_api")]
    #[proptest]
    fn dropping_receiver_deregisters_waitlist_slot() {
        let (tx, rx) = ring_channel::<Void>(NonZeroUsize::try_from(1)?);
        assert_eq!(tx.handle.waitlist.len(), 1);
        drop(rx);
        assert_eq!(tx.handle.waitlist.len(), 0);
    }

    #[proptest]
    fn channel_is_disconnected_if_there_are_no_senders() {
        let (_, rx) = ring_channel::<Void>(NonZeroUsize::try_from(1)?);
        assert_eq!(rx.handle.senders.load(Ordering::SeqCst), 0);
        assert!(!rx.handle.connected.load(Ordering::SeqCst));
    }

    #[proptest]
    fn channel_is_disconnected_if_there_are_no_receivers() {
        let (tx, _) = ring_channel::<Void>(NonZeroUsize::try_from(1)?);
        assert_eq!(tx.handle.receivers.load(Ordering::SeqCst), 0);
        assert!(!tx.handle.connected.load(Ordering::SeqCst));
    }

    #[proptest]
    fn endpoints_are_safe_to_send_across_threads() {
        fn must_be_send(_: impl Send) {}
        must_be_send(ring_channel::<Void>(NonZeroUsize::try_from(1)?));
    }

    #[proptest]
    fn endpoints_are_safe_to_share_across_threads() {
        fn must_be_sync(_: impl Sync) {}
        must_be_sync(ring_channel::<Void>(NonZeroUsize::try_from(1)?));
    }

    #[proptest]
    fn send_succeeds_on_connected_channel(
        #[strategy(1..=10usize)] cap: usize,
        #[any(size_range(1..=10).lift())] msgs: Vec<u8>,
    ) {
        let rt = runtime::Builder::new_multi_thread().build()?;
        let (tx, _rx) = ring_channel(NonZeroUsize::try_from(cap)?);

        rt.block_on(iter(msgs).map(Ok).try_for_each_concurrent(None, |msg| {
            let tx = tx.clone();
            spawn_blocking(move || assert!(tx.send(msg).is_ok()))
        }))?;
    }

    #[proptest]
    fn send_fails_on_disconnected_channel(
        #[strategy(1..=10usize)] cap: usize,
        #[any(size_range(1..=10).lift())] msgs: Vec<u8>,
    ) {
        let rt = runtime::Builder::new_multi_thread().build()?;
        let (tx, _) = ring_channel(NonZeroUsize::try_from(cap)?);

        rt.block_on(iter(msgs).map(Ok).try_for_each_concurrent(None, |msg| {
            let tx = tx.clone();
            spawn_blocking(move || assert_eq!(tx.send(msg), Err(SendError::Disconnected(msg))))
        }))?;
    }

    #[proptest]
    fn send_overwrites_old_messages(
        #[strategy(1..=10usize)] cap: usize,
        #[any(size_range(#cap..=10).lift())] msgs: Vec<u8>,
    ) {
        let (tx, rx) = ring_channel(NonZeroUsize::try_from(cap)?);
        let overwritten = msgs.len() - min(msgs.len(), cap);

        for &msg in &msgs[..cap] {
            assert_eq!(tx.send(msg), Ok(None));
        }

        for (&prev, &msg) in msgs.iter().zip(&msgs[cap..]) {
            assert_eq!(tx.send(msg), Ok(Some(prev)));
        }

        assert_eq!(
            iter::from_fn(|| rx.handle.buffer.pop()).collect::<Vec<_>>(),
            &msgs[overwritten..]
        );
    }

    #[proptest]
    fn try_recv_succeeds_on_non_empty_connected_channel(
        #[any(size_range(1..=10).lift())] msgs: Vec<u8>,
    ) {
        let rt = runtime::Builder::new_multi_thread().build()?;
        let (tx, rx) = ring_channel(NonZeroUsize::try_from(msgs.len())?);

        for msg in msgs.iter().cloned().enumerate() {
            tx.handle.buffer.push(msg);
        }

        let mut received = rt.block_on(async {
            try_join_all(
                iter::repeat(rx)
                    .take(msgs.len())
                    .map(|rx| spawn_blocking(move || rx.try_recv().unwrap())),
            )
            .await
        })?;

        received.sort_by_key(|(k, _)| *k);
        assert_eq!(received, msgs.into_iter().enumerate().collect::<Vec<_>>());
    }

    #[proptest]
    fn try_recv_succeeds_on_non_empty_disconnected_channel(
        #[any(size_range(1..=10).lift())] msgs: Vec<u8>,
    ) {
        let rt = runtime::Builder::new_multi_thread().build()?;
        let (_, rx) = ring_channel(NonZeroUsize::try_from(msgs.len())?);

        for msg in msgs.iter().cloned().enumerate() {
            rx.handle.buffer.push(msg);
        }

        let mut received = rt.block_on(async {
            try_join_all(
                iter::repeat(rx)
                    .take(msgs.len())
                    .map(|rx| spawn_blocking(move || rx.try_recv().unwrap())),
            )
            .await
        })?;

        received.sort_by_key(|(k, _)| *k);
        assert_eq!(received, msgs.into_iter().enumerate().collect::<Vec<_>>());
    }

    #[proptest]
    fn try_recv_fails_on_empty_connected_channel(
        #[strategy(1..=10usize)] cap: usize,
        #[strategy(1..=10usize)] n: usize,
    ) {
        let rt = runtime::Builder::new_multi_thread().build()?;
        let (_tx, rx) = ring_channel::<()>(NonZeroUsize::try_from(cap)?);

        rt.block_on(
            repeat(rx)
                .take(n)
                .map(Ok)
                .try_for_each_concurrent(None, |rx| {
                    spawn_blocking(move || assert_eq!(rx.try_recv(), Err(TryRecvError::Empty)))
                }),
        )?;
    }

    #[proptest]
    fn try_recv_fails_on_empty_disconnected_channel(
        #[strategy(1..=10usize)] cap: usize,
        #[strategy(1..=10usize)] n: usize,
    ) {
        let rt = runtime::Builder::new_multi_thread().build()?;
        let (_, rx) = ring_channel::<()>(NonZeroUsize::try_from(cap)?);

        rt.block_on(
            repeat(rx)
                .take(n)
                .map(Ok)
                .try_for_each_concurrent(None, |rx| {
                    spawn_blocking(move || {
                        assert_eq!(rx.try_recv(), Err(TryRecvError::Disconnected))
                    })
                }),
        )?;
    }

    #[cfg(feature = "futures_api")]
    #[proptest]
    fn recv_succeeds_on_non_empty_connected_channel(
        #[any(size_range(1..=10).lift())] msgs: Vec<u8>,
    ) {
        let rt = runtime::Builder::new_multi_thread().build()?;
        let (tx, rx) = ring_channel(NonZeroUsize::try_from(msgs.len())?);

        for msg in msgs.iter().cloned().enumerate() {
            tx.handle.buffer.push(msg);
        }

        let mut received = rt.block_on(async {
            try_join_all(
                iter::repeat(rx)
                    .take(msgs.len())
                    .map(|rx| spawn_blocking(move || rx.recv().unwrap())),
            )
            .await
        })?;

        received.sort_by_key(|(k, _)| *k);
        assert_eq!(received, msgs.into_iter().enumerate().collect::<Vec<_>>());
    }

    #[cfg(feature = "futures_api")]
    #[proptest]
    fn recv_succeeds_on_non_empty_disconnected_channel(
        #[any(size_range(1..=10).lift())] msgs: Vec<u8>,
    ) {
        let rt = runtime::Builder::new_multi_thread().build()?;
        let (_, rx) = ring_channel(NonZeroUsize::try_from(msgs.len())?);

        for msg in msgs.iter().cloned().enumerate() {
            rx.handle.buffer.push(msg);
        }

        let mut received = rt.block_on(async {
            try_join_all(
                iter::repeat(rx)
                    .take(msgs.len())
                    .map(|rx| spawn_blocking(move || rx.recv().unwrap())),
            )
            .await
        })?;

        received.sort_by_key(|(k, _)| *k);
        assert_eq!(received, msgs.into_iter().enumerate().collect::<Vec<_>>());
    }

    #[cfg(feature = "futures_api")]
    #[proptest]
    fn recv_fails_on_empty_disconnected_channel(
        #[strategy(1..=10usize)] cap: usize,
        #[strategy(1..=10usize)] n: usize,
    ) {
        let rt = runtime::Builder::new_multi_thread().build()?;
        let (_, rx) = ring_channel::<()>(NonZeroUsize::try_from(cap)?);

        rt.block_on(
            repeat(rx)
                .take(n)
                .map(Ok)
                .try_for_each_concurrent(None, |rx| {
                    spawn_blocking(move || assert_eq!(rx.recv(), Err(RecvError::Disconnected)))
                }),
        )?;
    }

    #[cfg(feature = "futures_api")]
    #[proptest]
    fn recv_wakes_on_disconnect(
        #[strategy(1..=10usize)] m: usize,
        #[strategy(1..=10usize)] n: usize,
    ) {
        let rt = runtime::Builder::new_multi_thread().enable_time().build()?;
        let (tx, rx) = ring_channel::<()>(NonZeroUsize::try_from(1)?);

        let producer = repeat(tx)
            .take(m)
            .map(Ok)
            .try_for_each_concurrent(None, |tx| spawn_blocking(move || drop(tx)));

        let consumer = repeat(rx)
            .take(n)
            .map(Ok)
            .try_for_each_concurrent(None, |rx| {
                spawn_blocking(move || assert_eq!(rx.recv(), Err(RecvError::Disconnected)))
            });

        rt.block_on(async move {
            timeout(Duration::from_secs(60), try_join(consumer, producer)).await
        })??;
    }

    #[cfg(feature = "futures_api")]
    #[proptest]
    fn recv_wakes_on_send(#[strategy(1..=10usize)] n: usize) {
        let rt = runtime::Builder::new_multi_thread().enable_time().build()?;
        let (tx, rx) = ring_channel(NonZeroUsize::try_from(n)?);
        let _prevent_disconnection = tx.clone();

        let producer = repeat(tx)
            .take(n)
            .map(Ok)
            .try_for_each_concurrent(None, |tx| {
                spawn_blocking(move || assert!(tx.send(()).is_ok()))
            });

        let consumer = repeat(rx)
            .take(n)
            .map(Ok)
            .try_for_each_concurrent(None, |rx| {
                spawn_blocking(move || assert_eq!(rx.recv(), Ok(())))
            });

        rt.block_on(async move {
            timeout(Duration::from_secs(60), try_join(consumer, producer)).await
        })??;
    }

    #[cfg(feature = "futures_api")]
    #[proptest]
    fn sender_implements_sink(
        #[strategy(1..=10usize)] cap: usize,
        #[any(size_range(1..=10).lift())] msgs: Vec<u8>,
    ) {
        let rt = runtime::Builder::new_multi_thread().build()?;
        let (mut tx, rx) = ring_channel(NonZeroUsize::try_from(cap)?);
        let overwritten = msgs.len() - min(msgs.len(), cap);

        assert_eq!(rt.block_on(iter(&msgs).map(Ok).forward(&mut tx)), Ok(()));

        drop(tx); // hang-up

        assert_eq!(
            iter::from_fn(|| rx.try_recv().ok().copied()).collect::<Vec<_>>(),
            &msgs[overwritten..]
        );
    }

    #[cfg(feature = "futures_api")]
    #[proptest]
    fn sender_sets_backoff_to_true_if_sink_overwrites_on_send() {
        let (mut tx, _rx) = ring_channel(NonZeroUsize::try_from(1)?);

        assert!(!tx.backoff);
        assert_eq!(Pin::new(&mut tx).start_send(()), Ok(()));
        assert!(!tx.backoff);
        assert_eq!(Pin::new(&mut tx).start_send(()), Ok(()));
        assert!(tx.backoff);
    }

    #[cfg(feature = "futures_api")]
    #[proptest]
    fn sender_yields_once_on_poll_ready_if_backoff_is_true(#[strategy(1..=10usize)] cap: usize) {
        let (mut tx, _) = ring_channel::<()>(NonZeroUsize::try_from(cap)?);
        tx.backoff = true;

        let waker = Arc::new(MockWaker).into();
        let mut ctx = Context::from_waker(&waker);

        assert_eq!(Pin::new(&mut tx).poll_ready(&mut ctx), Poll::Pending);
        assert!(!tx.backoff);
        assert_eq!(Pin::new(&mut tx).poll_ready(&mut ctx), Poll::Ready(Ok(())));
    }

    #[cfg(feature = "futures_api")]
    #[proptest]
    fn receiver_implements_stream(
        #[strategy(1..=10usize)] cap: usize,
        #[any(size_range(1..=10).lift())] msgs: Vec<u8>,
    ) {
        let rt = runtime::Builder::new_multi_thread().build()?;
        let (tx, rx) = ring_channel(NonZeroUsize::try_from(cap)?);
        let overwritten = msgs.len() - min(msgs.len(), cap);

        for &msg in &msgs {
            assert!(tx.send(msg).is_ok());
        }

        drop(tx); // hang-up

        assert_eq!(rt.block_on(rx.collect::<Vec<_>>()), &msgs[overwritten..]);
    }

    #[cfg(feature = "futures_api")]
    #[cfg(not(miri))] // will_wake sometimes returns false under miri
    #[proptest]
    fn receiver_stores_most_recent_waker_if_channel_is_empty(#[strategy(1..=10usize)] cap: usize) {
        let (_tx, mut rx) = ring_channel::<()>(NonZeroUsize::try_from(cap)?);

        let a = Arc::new(MockWaker).into();
        let mut ctx = Context::from_waker(&a);

        assert_eq!(Pin::new(&mut rx).poll_next(&mut ctx), Poll::Pending);
        assert!(rx.handle.waitlist.get(rx.slot).unwrap().will_wake(&a));

        let b = Arc::new(MockWaker).into();
        let mut ctx = Context::from_waker(&b);

        assert_eq!(Pin::new(&mut rx).poll_next(&mut ctx), Poll::Pending);
        assert!(!rx.handle.waitlist.get(rx.slot).unwrap().will_wake(&a));
        assert!(rx.handle.waitlist.get(rx.slot).unwrap().will_wake(&b));
    }

    #[cfg(feature = "futures_api")]
    #[cfg(not(miri))] // will_wake sometimes returns false under miri
    #[proptest]
    fn receiver_withdraws_waker_if_channel_not_empty(#[strategy(1..=10usize)] cap: usize, msg: u8) {
        let (tx, mut rx) = ring_channel(NonZeroUsize::try_from(cap)?);

        let waker = Arc::new(MockWaker).into();
        let mut ctx = Context::from_waker(&waker);

        assert_eq!(Pin::new(&mut rx).poll_next(&mut ctx), Poll::Pending);
        assert!(rx.handle.waitlist.get(rx.slot).unwrap().will_wake(&waker));

        assert_eq!(tx.send(&msg), Ok(None));

        assert_eq!(
            Pin::new(&mut rx).poll_next(&mut ctx),
            Poll::Ready(Some(&msg))
        );

        assert!(rx.handle.waitlist.get(rx.slot).is_none());
    }

    #[cfg(feature = "futures_api")]
    #[proptest]
    fn stream_wakes_on_disconnect(
        #[strategy(1..=10usize)] m: usize,
        #[strategy(1..=10usize)] n: usize,
    ) {
        let rt = runtime::Builder::new_multi_thread().enable_time().build()?;
        let (tx, rx) = ring_channel::<()>(NonZeroUsize::try_from(1)?);

        let producer = repeat(tx)
            .take(m)
            .map(Ok)
            .try_for_each_concurrent(None, |mut tx| {
                spawn(async move { assert_eq!(tx.close().await, Ok(())) })
            });

        let consumer = repeat(rx)
            .take(n)
            .map(Ok)
            .try_for_each_concurrent(None, |mut rx| {
                spawn(async move { assert_eq!(rx.next().await, None) })
            });

        rt.block_on(async move {
            timeout(Duration::from_secs(60), try_join(consumer, producer)).await
        })??;
    }

    #[cfg(feature = "futures_api")]
    #[proptest]
    fn stream_wakes_on_sink(#[strategy(1..=10usize)] n: usize) {
        let rt = runtime::Builder::new_multi_thread().enable_time().build()?;
        let (tx, rx) = ring_channel(NonZeroUsize::try_from(n)?);
        let _prevent_disconnection = tx.clone();

        let producer = repeat(tx)
            .take(n)
            .map(Ok)
            .try_for_each_concurrent(None, |tx| {
                spawn(async move { assert_eq!(iter(Some(Ok(()))).forward(tx).await, Ok(())) })
            });

        let consumer = repeat(rx)
            .take(n)
            .map(Ok)
            .try_for_each_concurrent(None, |mut rx| {
                spawn(async move { assert_eq!(rx.next().await, Some(())) })
            });

        rt.block_on(async move {
            timeout(Duration::from_secs(60), try_join(consumer, producer)).await
        })??;
    }
}
