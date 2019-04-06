use crate::buffer::*;
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
}
