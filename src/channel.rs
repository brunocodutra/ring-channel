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
}

impl<T> ControlBlock<T> {
    fn new(capacity: usize) -> Self {
        Self {
            left: AtomicUsize::new(1),
            right: AtomicUsize::new(1),
            connected: AtomicBool::new(true),
            buffer: Buffer::new(capacity),
        }
    }

    unsafe fn delete(&self) {
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
}
