use crate::{buffer::*, error::*};
use crossbeam_utils::CachePadded;
use derivative::Derivative;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::{num::NonZeroUsize, ops::Deref};

#[derive(Derivative)]
#[derivative(Debug(bound = ""))]
struct ControlBlock<T> {
    left: CachePadded<AtomicUsize>,
    right: CachePadded<AtomicUsize>,
    buffer: RingBuffer<T>,
    connected: AtomicBool,
}

impl<T> ControlBlock<T> {
    fn new(capacity: usize) -> Self {
        Self {
            left: CachePadded::new(AtomicUsize::new(1)),
            right: CachePadded::new(AtomicUsize::new(1)),
            buffer: RingBuffer::new(capacity),
            connected: AtomicBool::new(true),
        }
    }

    unsafe fn delete(&self) {
        debug_assert!(!self.connected.load(Ordering::Relaxed));
        debug_assert_eq!(self.left.load(Ordering::Relaxed), 0);
        debug_assert_eq!(self.right.load(Ordering::Relaxed), 0);

        Box::from_raw(self as *const Self as *mut Self);
    }

    fn send(&self, value: T) -> Result<(), SendError<T>> {
        if self.connected.load(Ordering::Relaxed) {
            self.buffer.push(value);
            Ok(())
        } else {
            Err(SendError::Disconnected(value))
        }
    }

    fn recv(&self) -> Result<T, RecvError> {
        self.buffer.pop().ok_or_else(|| {
            if !self.connected.load(Ordering::Relaxed) {
                RecvError::Disconnected
            } else {
                RecvError::Empty
            }
        })
    }
}

impl<T> Eq for ControlBlock<T> {}

impl<T> PartialEq for ControlBlock<T> {
    fn eq(&self, other: &Self) -> bool {
        self as *const _ == other as *const _
    }
}

#[derive(Derivative, Eq, PartialEq)]
#[derivative(Debug(bound = ""))]
enum Endpoint<T> {
    Left(*const ControlBlock<T>),
    Right(*const ControlBlock<T>),
}

unsafe impl<T: Send> Send for Endpoint<T> {}
unsafe impl<T: Send> Sync for Endpoint<T> {}

impl<T> Deref for Endpoint<T> {
    type Target = ControlBlock<T>;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        use Endpoint::*;
        let ptr = match *self {
            Left(ptr) => ptr,
            Right(ptr) => ptr,
        };

        unsafe { &*ptr }
    }
}

impl<T> Clone for Endpoint<T> {
    fn clone(&self) -> Self {
        use Endpoint::*;
        match *self {
            Left(ptr) => {
                self.left.fetch_add(1, Ordering::Relaxed);
                Left(ptr)
            }

            Right(ptr) => {
                self.right.fetch_add(1, Ordering::Relaxed);
                Right(ptr)
            }
        }
    }
}

impl<T> Drop for Endpoint<T> {
    fn drop(&mut self) {
        use Endpoint::*;
        if match *self {
            // Synchronizes with other left endpoints.
            Left(_) => self.left.fetch_sub(1, Ordering::AcqRel) == 1,

            // Synchronizes with other right endpoints.
            Right(_) => self.right.fetch_sub(1, Ordering::AcqRel) == 1,
        } {
            // Synchronizes the last left and right endpoints with each other.
            if !self.connected.swap(false, Ordering::AcqRel) {
                unsafe { self.delete() }
            }
        }
    }
}

/// The sending end of a [`ring_channel`].
///
/// [`ring_channel`]: fn.ring_channel.html
#[derive(Derivative)]
#[derivative(Debug(bound = ""), Clone(bound = ""))]
pub struct RingSender<T>(#[derivative(Debug = "ignore")] Endpoint<T>);

impl<T> RingSender<T> {
    /// Sends a message through the channel without blocking.
    ///
    /// * If the channel is not disconnected, the message is pushed into the internal ring buffer.
    ///     * If the internal ring buffer is full, the oldest pending message is overwritten.
    /// * If the channel is disconnected, [`SendError::Disconnected`] is returned.
    ///
    /// [`SendError::Disconnected`]: enum.SendError.html#variant.Disconnected
    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        self.0.send(value)
    }
}

/// The receiving end of a [`ring_channel`].
///
/// [`ring_channel`]: fn.ring_channel.html
#[derive(Derivative)]
#[derivative(Debug(bound = ""), Clone(bound = ""))]
pub struct RingReceiver<T>(#[derivative(Debug = "ignore")] Endpoint<T>);

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
        self.0.recv()
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
    let ctrl = Box::into_raw(Box::new(ControlBlock::new(capacity.get())));

    (
        RingSender(Endpoint::Left(ctrl)),
        RingReceiver(Endpoint::Right(ctrl)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::{collection::vec, prelude::*};
    use rayon::{iter::repeatn, prelude::*};
    use std::{cmp::min, iter};

    #[test]
    fn control_block_has_object_identity() {
        let ctrl = ControlBlock::<()>::new(1);
        assert_eq!(ctrl, ctrl);
        assert_ne!(ctrl, ControlBlock::<()>::new(1));
    }

    #[test]
    fn control_block_starts_connected() {
        let ctrl = ControlBlock::<()>::new(1);
        assert_eq!(ctrl.connected.load(Ordering::Relaxed), true);
    }

    #[test]
    fn control_block_starts_with_reference_counters_equal_to_one() {
        let ctrl = ControlBlock::<()>::new(1);
        assert_eq!(ctrl.left.load(Ordering::Relaxed), 1);
        assert_eq!(ctrl.right.load(Ordering::Relaxed), 1);
    }

    proptest! {
        #[test]
        fn control_block_allocates_buffer_given_capacity(cap in 1..=100usize) {
            let ctrl = ControlBlock::<()>::new(cap);
            assert_eq!(ctrl.buffer.capacity(), cap);
        }
    }

    #[test]
    fn ring_channel_holds_endpoints_of_the_same_control_block() {
        let (l, r) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());
        assert_eq!(&l.0 as &ControlBlock<_>, &r.0 as &ControlBlock<_>);
        assert_ne!(l.0, r.0);
    }

    #[test]
    fn cloning_left_endpoint_increments_left_counter() {
        let (l, _r) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());
        let x = l.clone();
        assert_eq!(x.0.left.load(Ordering::Relaxed), 2);
        assert_eq!(x.0.right.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn cloning_right_endpoint_increments_right_counter() {
        let (_l, r) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());
        let x = r.clone();
        assert_eq!(x.0.left.load(Ordering::Relaxed), 1);
        assert_eq!(x.0.right.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn dropping_left_endpoint_decrements_left_counter() {
        let (_, r) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());
        assert_eq!(r.0.left.load(Ordering::Relaxed), 0);
        assert_eq!(r.0.right.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn dropping_right_endpoint_decrements_right_counter() {
        let (l, _) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());
        assert_eq!(l.0.left.load(Ordering::Relaxed), 1);
        assert_eq!(l.0.right.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn channel_is_disconnected_if_there_are_no_left_endpoints() {
        let (_, r) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());
        assert_eq!(r.0.left.load(Ordering::Relaxed), 0);
        assert_eq!(r.0.connected.load(Ordering::Relaxed), false);
    }

    #[test]
    fn channel_is_disconnected_if_there_are_no_right_endpoints() {
        let (l, _) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());
        assert_eq!(l.0.right.load(Ordering::Relaxed), 0);
        assert_eq!(l.0.connected.load(Ordering::Relaxed), false);
    }

    proptest! {
        #[test]
        fn endpoints_are_safe_to_send_across_threads(m in 1..=100usize, n in 1..=100usize) {
            let (l, r) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());
            let ls = repeatn(l.0, m);
            let rs = repeatn(r.0, n);
            ls.chain(rs).for_each(drop);
        }

        #[test]
        fn endpoints_are_safe_to_share_across_threads(m in 1..=100usize, n in 1..=100usize) {
            let (l, r) = ring_channel::<()>(NonZeroUsize::new(1).unwrap());
            let ls = repeatn((), m).map(|_| l.0.clone());
            let rs = repeatn((), n).map(|_| r.0.clone());
            ls.chain(rs).for_each(drop);
        }
    }

    proptest! {
        #[test]
        fn send_succeeds_on_connected_channel(cap in 1..=100usize, msgs in vec("[a-z]", 1..=100)) {
            let (l, _r) = ring_channel(NonZeroUsize::new(cap).unwrap());
            repeatn(l, msgs.len()).zip(msgs.par_iter().cloned()).for_each(|(c, msg)| {
                assert_eq!(c.send(msg), Ok(()));
            });
        }

        #[test]
        fn send_fails_on_disconnected_channel(cap in 1..=100usize, msgs in vec("[a-z]", 1..=100)) {
            let (l, _) = ring_channel(NonZeroUsize::new(cap).unwrap());
            repeatn(l, msgs.len()).zip(msgs.par_iter().cloned()).for_each(|(c, msg)| {
                assert_eq!(c.send(msg.clone()), Err(SendError::Disconnected(msg)));
            });
        }

        #[test]
        fn send_overwrites_old_messages(cap in 1..=100usize, mut msgs in vec("[a-z]", 1..=100)) {
            let (l, r) = ring_channel(NonZeroUsize::new(cap).unwrap());
            let overwritten = msgs.len() - min(msgs.len(), cap);

            for msg in msgs.iter().cloned() {
                assert_eq!(l.send(msg), Ok(()));
            }

            assert_eq!(
                iter::from_fn(move || r.0.buffer.pop()).collect::<Vec<_>>(),
                msgs.drain(..).skip(overwritten).collect::<Vec<_>>()
            );
        }
    }

    proptest! {
        #[test]
        fn recv_succeeds_on_non_empty_connected_channel(msgs in vec("[a-z]", 1..=100)) {
            let (l, r) = ring_channel(NonZeroUsize::new(msgs.len()).unwrap());
            for msg in msgs.iter().cloned().enumerate() {
                l.0.buffer.push(msg);
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
                r.0.buffer.push(msg);
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
            let (_l, r) = ring_channel::<()>(NonZeroUsize::new(cap).unwrap());
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
