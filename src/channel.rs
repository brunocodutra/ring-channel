use crate::{buffer::*, error::*};
use crossbeam_utils::CachePadded;
use derivative::Derivative;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::{mem::ManuallyDrop, num::NonZeroUsize, ops::Deref, ptr::NonNull};

#[derive(Derivative)]
#[derivative(Debug(bound = ""))]
struct ControlBlock<T> {
    senders: CachePadded<AtomicUsize>,
    receivers: CachePadded<AtomicUsize>,
    connected: AtomicBool,
    buffer: RingBuffer<T>,
}

impl<T> ControlBlock<T> {
    fn new(capacity: usize) -> Self {
        Self {
            senders: CachePadded::new(AtomicUsize::new(1)),
            receivers: CachePadded::new(AtomicUsize::new(1)),
            connected: AtomicBool::new(true),
            buffer: RingBuffer::new(capacity),
        }
    }
}

#[derive(Derivative, Eq, PartialEq)]
#[derivative(Debug(bound = ""), Clone(bound = ""))]
struct ControlBlockRef<T>(NonNull<ControlBlock<T>>);

impl<T> ControlBlockRef<T> {
    fn new(capacity: usize) -> Self {
        ControlBlockRef(unsafe {
            NonNull::new_unchecked(Box::into_raw(Box::new(ControlBlock::new(capacity))))
        })
    }
}

impl<T> Deref for ControlBlockRef<T> {
    type Target = ControlBlock<T>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        unsafe { self.0.as_ref() }
    }
}

impl<T> Drop for ControlBlockRef<T> {
    fn drop(&mut self) {
        unsafe {
            debug_assert!(!self.connected.load(Ordering::Relaxed));
            debug_assert_eq!(self.senders.load(Ordering::Relaxed), 0);
            debug_assert_eq!(self.receivers.load(Ordering::Relaxed), 0);

            Box::from_raw(&**self as *const ControlBlock<T> as *mut ControlBlock<T>);
        }
    }
}

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
unsafe impl<T: Send> Sync for RingSender<T> {}

impl<T> RingSender<T> {
    /// Sends a message through the channel without blocking.
    ///
    /// * If the channel is not disconnected, the message is pushed into the internal ring buffer.
    ///     * If the internal ring buffer is full, the oldest pending message is overwritten.
    /// * If the channel is disconnected, [`SendError::Disconnected`] is returned.
    ///
    /// [`SendError::Disconnected`]: enum.SendError.html#variant.Disconnected
    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        if self.handle.connected.load(Ordering::Relaxed) {
            self.handle.buffer.push(value);
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
            // Synchronizes the last sender and receiver with each other.
            if !self.handle.connected.swap(false, Ordering::AcqRel) {
                unsafe { ManuallyDrop::drop(&mut self.handle) }
            }
        }
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
unsafe impl<T: Send> Sync for RingReceiver<T> {}

impl<T> RingReceiver<T> {
    /// Receives a message through the channel without blocking.
    ///
    /// * If the internal ring buffer isn't empty, the oldest pending message is returned.
    /// * If the internal ring buffer is empty, [`RecvError::Empty`] is returned.
    /// * If the channel is disconnected and the internal ring buffer is empty,
    /// [`RecvError::Disconnected`] is returned.
    ///
    /// [`RecvError::Empty`]: enum.RecvError.html#variant.Empty
    /// [`RecvError::Disconnected`]: enum.RecvError.html#variant.Disconnected
    pub fn recv(&self) -> Result<T, RecvError> {
        self.handle.buffer.pop().ok_or_else(|| {
            if !self.handle.connected.load(Ordering::Relaxed) {
                RecvError::Disconnected
            } else {
                RecvError::Empty
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
///     let (tx, rx) = ring_channel(NonZeroUsize::new(1).unwrap());
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
///         match rx.recv() {
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
///             Err(RecvError::Empty) => thread::yield_now(),
///             Err(RecvError::Disconnected) => break,
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
    use proptest::{collection::vec, prelude::*};
    use rayon::{iter::repeatn, prelude::*};
    use std::{cmp::min, iter};

    #[test]
    fn control_block_starts_connected() {
        let ctrl = ControlBlock::<()>::new(1);
        assert_eq!(ctrl.connected.load(Ordering::Relaxed), true);
    }

    #[test]
    fn control_block_starts_with_reference_counters_equal_to_one() {
        let ctrl = ControlBlock::<()>::new(1);
        assert_eq!(ctrl.senders.load(Ordering::Relaxed), 1);
        assert_eq!(ctrl.receivers.load(Ordering::Relaxed), 1);
    }

    proptest! {
        #[test]
        fn control_block_allocates_buffer_given_capacity(cap in 1..=100usize) {
            let ctrl = ControlBlock::<()>::new(cap);
            assert_eq!(ctrl.buffer.capacity(), cap);
        }
    }

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

    #[derive(Clone)]
    enum Endpoint<T> {
        Sender(RingSender<T>),
        Receiver(RingReceiver<T>),
    }

    proptest! {
        #[test]
        fn endpoints_are_safe_to_send_across_threads(m in 1..=100usize, n in 1..=100usize) {
            let (s, r) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());
            let ls = repeatn(Endpoint::Sender(s), m);
            let rs = repeatn(Endpoint::Receiver(r), n);
            ls.chain(rs).for_each(drop);
        }

        #[test]
        fn endpoints_are_safe_to_share_across_threads(m in 1..=100usize, n in 1..=100usize) {
            let (s, r) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());
            let s = Endpoint::Sender(s);
            let r = Endpoint::Receiver(r);
            let ls = repeatn((), m).map(|_| s.clone());
            let rs = repeatn((), n).map(|_| r.clone());
            ls.chain(rs).for_each(drop);
        }
    }

    proptest! {
        #[test]
        fn send_succeeds_on_connected_channel(cap in 1..=100usize, msgs in vec("[a-z]", 1..=100)) {
            let (s, _r) = ring_channel(NonZeroUsize::new(cap).unwrap());
            repeatn(s, msgs.len()).zip(msgs.par_iter().cloned()).for_each(|(c, msg)| {
                assert_eq!(c.send(msg), Ok(()));
            });
        }

        #[test]
        fn send_fails_on_disconnected_channel(cap in 1..=100usize, msgs in vec("[a-z]", 1..=100)) {
            let (s, _) = ring_channel(NonZeroUsize::new(cap).unwrap());
            repeatn(s, msgs.len()).zip(msgs.par_iter().cloned()).for_each(|(c, msg)| {
                assert_eq!(c.send(msg.clone()), Err(SendError::Disconnected(msg)));
            });
        }

        #[test]
        fn send_overwrites_old_messages(cap in 1..=100usize, mut msgs in vec("[a-z]", 1..=100)) {
            let (s, r) = ring_channel(NonZeroUsize::new(cap).unwrap());
            let overwritten = msgs.len() - min(msgs.len(), cap);

            for msg in msgs.iter().cloned() {
                assert_eq!(s.send(msg), Ok(()));
            }

            assert_eq!(
                iter::from_fn(move || r.handle.buffer.pop()).collect::<Vec<_>>(),
                msgs.drain(..).skip(overwritten).collect::<Vec<_>>()
            );
        }
    }

    proptest! {
        #[test]
        fn recv_succeeds_on_non_empty_connected_channel(msgs in vec("[a-z]", 1..=100)) {
            let (s, r) = ring_channel(NonZeroUsize::new(msgs.len()).unwrap());

            for msg in msgs.iter().cloned().enumerate() {
                s.handle.buffer.push(msg);
            }

            let mut received = vec![(0usize, Default::default()); msgs.len()];

            repeatn(r, msgs.len()).zip(received.par_iter_mut()).for_each(|(c, slot)| {
                match c.recv() {
                    Ok(msg) => *slot = msg,
                    Err(e) => panic!(e),
                };
            });

            received.sort_by_key(|(k, _)| *k);
            assert_eq!(received.drain(..).map(|(_, v)| v).collect::<Vec<_>>(), msgs);
        }

        #[test]
        fn recv_succeeds_on_non_empty_disconnected_channel(msgs in vec("[a-z]", 1..=100)) {
            let (_, r) = ring_channel(NonZeroUsize::new(msgs.len()).unwrap());

            for msg in msgs.iter().cloned().enumerate() {
                r.handle.buffer.push(msg);
            }

            let mut received = vec![(0usize, Default::default()); msgs.len()];

            repeatn(r, msgs.len()).zip(received.par_iter_mut()).for_each(|(c, slot)| {
                match c.recv() {
                    Ok(msg) => *slot = msg,
                    Err(e) => panic!(e),
                };
            });

            received.sort_by_key(|(k, _)| *k);
            assert_eq!(received.drain(..).map(|(_, v)| v).collect::<Vec<_>>(), msgs);
        }

        #[test]
        fn recv_fails_on_empty_connected_channel(cap in 1..=100usize, n in 1..=100usize) {
            let (_s, r) = ring_channel::<()>(NonZeroUsize::new(cap).unwrap());
            repeatn((), n).for_each(move |_| {
                assert_eq!(r.recv(), Err(RecvError::Empty));
            });
        }

        #[test]
        fn recv_fails_on_empty_disconnected_channel(cap in 1..=100usize, n in 1..=100usize) {
            let (_, r) = ring_channel::<()>(NonZeroUsize::new(cap).unwrap());
            repeatn((), n).for_each(move |_| {
                assert_eq!(r.recv(), Err(RecvError::Disconnected));
            });
        }
    }
}
