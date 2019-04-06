use crate::{buffer::*, same};
use derivative::Derivative;
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Derivative)]
#[derivative(Debug(bound = ""))]
pub struct ControlBlock<T> {
    left: AtomicUsize,
    right: AtomicUsize,
    connected: AtomicBool,
    buffer: Buffer<T>,

    #[cfg(test)]
    dropped: AtomicBool,
}

impl<T> ControlBlock<T> {
    fn new(capacity: usize) -> Self {
        Self {
            left: AtomicUsize::new(1),
            right: AtomicUsize::new(1),
            connected: AtomicBool::new(true),
            buffer: Buffer::new(capacity),

            #[cfg(test)]
            dropped: AtomicBool::new(false),
        }
    }

    unsafe fn delete(&self) {
        debug_assert!(self.left.load(Ordering::Relaxed) == 0);
        debug_assert!(self.right.load(Ordering::Relaxed) == 0);
        debug_assert!(!self.connected.load(Ordering::Relaxed));

        #[cfg(test)]
        assert!(self.dropped.load(Ordering::Relaxed));

        Box::from_raw(self as *const Self as *mut Self);
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
pub enum Endpoint<T> {
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

        #[cfg(test)]
        assert!(!unsafe { &*ptr }.dropped.load(Ordering::Relaxed));

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
            #[cfg(test)]
            self.dropped.store(true, Ordering::Release);

            #[cfg(not(test))]
            unsafe {
                self.delete();
            }
        }
    }
}

pub struct RingChannel<T>(pub Endpoint<T>, pub Endpoint<T>);

impl<T> RingChannel<T> {
    pub fn new(capacity: usize) -> Self {
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
    use proptest::prelude::*;
    use rayon::{iter::repeatn, prelude::*};

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

    fn given_ring_channel<T, F: FnOnce(RingChannel<T>)>(capacity: usize, then: F) {
        let channel = RingChannel::<T>::new(capacity);
        let ctrl: *const ControlBlock<T> = &*channel;
        then(channel);
        unsafe { (*ctrl).delete() };
    }

    #[test]
    fn ring_channel_holds_endpoints_of_the_same_control_block() {
        given_ring_channel(1, |RingChannel::<()>(l, r)| {
            assert_eq!(&l as &ControlBlock<_>, &r as &ControlBlock<_>);
            assert_ne!(l, r);
        });
    }

    #[test]
    fn cloning_left_endpoint_increments_left_counter() {
        given_ring_channel(1, |RingChannel::<()>(l, _r)| {
            let x = l.clone();
            assert_eq!(x.left.load(Ordering::Relaxed), 2);
            assert_eq!(x.right.load(Ordering::Relaxed), 1);
        });
    }

    #[test]
    fn cloning_right_endpoint_increments_right_counter() {
        given_ring_channel(1, |RingChannel::<()>(_l, r)| {
            let x = r.clone();
            assert_eq!(x.left.load(Ordering::Relaxed), 1);
            assert_eq!(x.right.load(Ordering::Relaxed), 2);
        });
    }

    #[test]
    fn dropping_left_endpoint_decrements_left_counter() {
        given_ring_channel(1, |RingChannel::<()>(l, r)| {
            drop(l);
            assert_eq!(r.left.load(Ordering::Relaxed), 0);
            assert_eq!(r.right.load(Ordering::Relaxed), 1);
        });
    }

    #[test]
    fn dropping_right_endpoint_decrements_right_counter() {
        given_ring_channel(1, |RingChannel::<()>(l, r)| {
            drop(r);
            assert_eq!(l.left.load(Ordering::Relaxed), 1);
            assert_eq!(l.right.load(Ordering::Relaxed), 0);
        });
    }

    #[test]
    fn channel_is_disconnected_if_there_are_no_left_endpoints() {
        given_ring_channel(1, |RingChannel::<()>(l, r)| {
            drop(l);
            assert_eq!(r.left.load(Ordering::Relaxed), 0);
            assert_eq!(r.connected.load(Ordering::Relaxed), false);
        });
    }

    #[test]
    fn channel_is_disconnected_if_there_are_no_right_endpoints() {
        given_ring_channel(1, |RingChannel::<()>(l, r)| {
            drop(r);
            assert_eq!(l.right.load(Ordering::Relaxed), 0);
            assert_eq!(l.connected.load(Ordering::Relaxed), false);
        });
    }

    #[test]
    fn control_block_is_not_dropped_if_there_are_left_endpoints() {
        given_ring_channel(1, |RingChannel::<()>(l, r)| {
            drop(r);
            assert_eq!(l.dropped.load(Ordering::Relaxed), false);
        });
    }

    #[test]
    fn control_block_is_not_dropped_if_there_are_right_endpoints() {
        given_ring_channel(1, |RingChannel::<()>(l, r)| {
            drop(l);
            assert_eq!(r.dropped.load(Ordering::Relaxed), false);
        });
    }

    proptest! {
        #[test]
        fn endpoints_are_safe_to_send_across_threads(m in 1..=100usize, n in 1..=100usize) {
            given_ring_channel(1, |RingChannel::<()>(l, r)| {
                let ls = repeatn(l, m);
                let rs = repeatn(r, n);
                ls.chain(rs).for_each(drop);
            });
        }

        #[test]
        fn endpoints_are_safe_to_share_across_threads(m in 1..=100usize, n in 1..=100usize) {
            given_ring_channel(1, |RingChannel::<()>(l, r)| {
                let ls = repeatn((), m).map(|_| l.clone());
                let rs = repeatn((), n).map(|_| r.clone());
                ls.chain(rs).for_each(drop);
            });
        }
    }
}
