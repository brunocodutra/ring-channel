use crate::{buffer::*, error::*, same};
use derivative::Derivative;
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Derivative)]
#[derivative(Debug(bound = ""))]
pub(super) struct ControlBlock<T> {
    left: AtomicUsize,
    right: AtomicUsize,
    connected: AtomicBool,
    buffer: RingBuffer<T>,
}

impl<T> ControlBlock<T> {
    fn new(capacity: usize) -> Self {
        Self {
            left: AtomicUsize::new(1),
            right: AtomicUsize::new(1),
            connected: AtomicBool::new(true),
            buffer: RingBuffer::new(capacity),
        }
    }

    unsafe fn delete(&self) {
        debug_assert!(self.left.load(Ordering::Relaxed) == 0);
        debug_assert!(self.right.load(Ordering::Relaxed) == 0);
        debug_assert!(!self.connected.load(Ordering::Relaxed));

        Box::from_raw(self as *const Self as *mut Self);
    }

    pub(super) fn send(&self, value: T) -> Result<(), SendError<T>> {
        if self.connected.load(Ordering::Relaxed) {
            self.buffer.push(value);
            Ok(())
        } else {
            Err(SendError::Disconnected(value))
        }
    }

    pub(super) fn recv(&self) -> Result<T, RecvError> {
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
pub(super) enum Endpoint<T> {
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
        let disconnect = match *self {
            // synchronizes with other left endpoints
            Left(_) => self.left.fetch_sub(1, Ordering::AcqRel) == 1,

            // synchronizes with other right endpoints
            Right(_) => self.right.fetch_sub(1, Ordering::AcqRel) == 1,
        };

        // synchronizes the last left and right endpoints with each other
        if disconnect && !self.connected.swap(false, Ordering::AcqRel) {
            unsafe {
                self.delete();
            }
        }
    }
}

pub(super) struct RingChannel<T>(pub(super) Endpoint<T>, pub(super) Endpoint<T>);

impl<T> RingChannel<T> {
    pub(super) fn new(capacity: usize) -> Self {
        let ctrl = Box::into_raw(Box::new(ControlBlock::new(capacity)));
        RingChannel(Endpoint::Left(ctrl), Endpoint::Right(ctrl))
    }
}

impl<T> Deref for RingChannel<T> {
    type Target = ControlBlock<T>;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        let RingChannel(l, r) = self;
        same!(l as &Self::Target, r as &Self::Target)
    }
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
        let RingChannel::<()>(l, r) = RingChannel::new(1);
        assert_eq!(&l as &ControlBlock<_>, &r as &ControlBlock<_>);
        assert_ne!(l, r);
    }

    #[test]
    fn cloning_left_endpoint_increments_left_counter() {
        let RingChannel::<()>(l, _r) = RingChannel::new(1);
        let x = l.clone();
        assert_eq!(x.left.load(Ordering::Relaxed), 2);
        assert_eq!(x.right.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn cloning_right_endpoint_increments_right_counter() {
        let RingChannel::<()>(_l, r) = RingChannel::new(1);
        let x = r.clone();
        assert_eq!(x.left.load(Ordering::Relaxed), 1);
        assert_eq!(x.right.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn dropping_left_endpoint_decrements_left_counter() {
        let RingChannel::<()>(_, r) = RingChannel::new(1);
        assert_eq!(r.left.load(Ordering::Relaxed), 0);
        assert_eq!(r.right.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn dropping_right_endpoint_decrements_right_counter() {
        let RingChannel::<()>(l, _) = RingChannel::new(1);
        assert_eq!(l.left.load(Ordering::Relaxed), 1);
        assert_eq!(l.right.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn channel_is_disconnected_if_there_are_no_left_endpoints() {
        let RingChannel::<()>(_, r) = RingChannel::new(1);
        assert_eq!(r.left.load(Ordering::Relaxed), 0);
        assert_eq!(r.connected.load(Ordering::Relaxed), false);
    }

    #[test]
    fn channel_is_disconnected_if_there_are_no_right_endpoints() {
        let RingChannel::<()>(l, _) = RingChannel::new(1);
        assert_eq!(l.right.load(Ordering::Relaxed), 0);
        assert_eq!(l.connected.load(Ordering::Relaxed), false);
    }

    proptest! {
        #[test]
        fn endpoints_are_safe_to_send_across_threads(m in 1..=100usize, n in 1..=100usize) {
            let RingChannel::<()>(l, r) = RingChannel::new(1);
            let ls = repeatn(l, m);
            let rs = repeatn(r, n);
            ls.chain(rs).for_each(drop);
        }

        #[test]
        fn endpoints_are_safe_to_share_across_threads(m in 1..=100usize, n in 1..=100usize) {
            let RingChannel::<()>(l, r) = RingChannel::new(1);
            let ls = repeatn((), m).map(|_| l.clone());
            let rs = repeatn((), n).map(|_| r.clone());
            ls.chain(rs).for_each(drop);
        }
    }

    proptest! {
        #[test]
        fn send_succeeds_on_connected_channel(cap in 1..=100usize, msgs in vec("[a-z]", 1..=100)) {
            let RingChannel(l, _r) = RingChannel::new(cap);
            repeatn(l, msgs.len()).zip(msgs.par_iter().cloned()).for_each(|(c, msg)| {
                assert_eq!(c.send(msg), Ok(()));
            });
        }

        #[test]
        fn send_fails_on_disconnected_channel(cap in 1..=100usize, msgs in vec("[a-z]", 1..=100)) {
            let RingChannel(l, _) = RingChannel::new(cap);
            repeatn(l, msgs.len()).zip(msgs.par_iter().cloned()).for_each(|(c, msg)| {
                assert_eq!(c.send(msg.clone()), Err(SendError::Disconnected(msg)));
            });
        }

        #[test]
        fn send_overwrites_old_messages(cap in 1..=100usize, mut msgs in vec("[a-z]", 1..=100)) {
            let RingChannel(l, r) = RingChannel::new(cap);
            let overwritten = msgs.len() - min(msgs.len(), cap);

            for msg in msgs.iter().cloned() {
                assert_eq!(l.send(msg), Ok(()));
            }

            assert_eq!(
                iter::from_fn(move || r.buffer.pop()).collect::<Vec<_>>(),
                msgs.drain(..).skip(overwritten).collect::<Vec<_>>()
            );
        }
    }

    proptest! {
        #[test]
        fn recv_succeeds_on_non_empty_connected_channel(msgs in vec("[a-z]", 1..=100)) {
            let RingChannel(l, r) = RingChannel::new(msgs.len());
            for msg in msgs.iter().cloned().enumerate() {
                l.buffer.push(msg);
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
            let RingChannel(l, _) = RingChannel::new(msgs.len());

            for msg in msgs.iter().cloned().enumerate() {
                l.buffer.push(msg);
            }

            let mut received = vec![(0usize, Default::default()); msgs.len()];

            repeatn(l, msgs.len()).zip(received.par_iter_mut()).for_each(|(c, slot)| {
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
            let channel = RingChannel::<()>::new(cap);
            repeatn((), n).for_each(move |_| {
                assert_eq!(channel.recv(), Err(RecvError::Empty));
            });
        }

        #[test]
        fn recv_fails_on_empty_disconnected_channel(cap in 1..=100usize, n in 1..=100usize) {
            let RingChannel::<()>(l, _) = RingChannel::new(cap);
            repeatn((), n).for_each(move |_| {
                assert_eq!(l.recv(), Err(RecvError::Disconnected));
            });
        }
    }
}
