use alloc::boxed::Box;
use crossbeam_queue::ArrayQueue;
use crossbeam_utils::atomic::AtomicCell;
use derivative::Derivative;

#[derive(Derivative)]
#[derivative(Debug)]
#[allow(clippy::large_enum_variant)]
pub(super) enum RingBuffer<T> {
    Atomic(#[derivative(Debug = "ignore")] AtomicCell<Option<T>>),
    Boxed(#[derivative(Debug = "ignore")] AtomicCell<Option<Box<T>>>),
    Queue(#[derivative(Debug = "ignore")] ArrayQueue<T>),
}

impl<T> RingBuffer<T> {
    pub(super) fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "capacity must be non-zero");

        if capacity == 1 && AtomicCell::<Option<T>>::is_lock_free() {
            RingBuffer::Atomic(AtomicCell::new(None))
        } else if capacity == 1 {
            debug_assert!(AtomicCell::<Option<Box<T>>>::is_lock_free());
            RingBuffer::Boxed(AtomicCell::new(None))
        } else {
            RingBuffer::Queue(ArrayQueue::new(capacity))
        }
    }

    #[cfg(test)]
    pub(super) fn capacity(&self) -> usize {
        match self {
            RingBuffer::Atomic(_) => 1,
            RingBuffer::Boxed(_) => 1,
            RingBuffer::Queue(q) => q.capacity(),
        }
    }

    pub(super) fn push(&self, mut value: T) {
        match self {
            RingBuffer::Atomic(c) => {
                c.store(Some(value));
            }

            RingBuffer::Boxed(b) => {
                b.store(Some(Box::new(value)));
            }

            RingBuffer::Queue(q) => {
                while let Err(v) = q.push(value) {
                    self.pop();
                    value = v;
                }
            }
        }
    }

    pub(super) fn pop(&self) -> Option<T> {
        match self {
            RingBuffer::Atomic(c) => c.take(),
            RingBuffer::Boxed(b) => Some(*b.take()?),
            RingBuffer::Queue(q) => q.pop(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RingReceiver, RingSender};
    use alloc::{sync::Arc, vec::Vec};
    use core::{cmp::max, mem::discriminant};
    use futures::{future::try_join, prelude::*, stream::repeat};
    use proptest::collection::size_range;
    use test_strategy::proptest;
    use tokio::{runtime, task::spawn_blocking};

    #[should_panic]
    #[proptest]
    fn new_panics_if_capacity_is_zero() {
        RingBuffer::<()>::new(0);
    }

    #[proptest]
    fn new_uses_atomic_cell_when_capacity_is_one() {
        assert_eq!(
            discriminant(&RingBuffer::<[char; 1]>::new(1)),
            discriminant(&RingBuffer::Atomic(Default::default()))
        );

        assert_eq!(
            discriminant(&RingBuffer::<[char; 1]>::new(2)),
            discriminant(&RingBuffer::Queue(ArrayQueue::new(2)))
        );

        assert_eq!(
            discriminant(&RingBuffer::<[char; 4]>::new(1)),
            discriminant(&RingBuffer::Boxed(Default::default()))
        );

        assert_eq!(
            discriminant(&RingBuffer::<[char; 4]>::new(2)),
            discriminant(&RingBuffer::Queue(ArrayQueue::new(2)))
        );

        assert_eq!(
            discriminant(&RingBuffer::<RingSender<()>>::new(1)),
            discriminant(&RingBuffer::Atomic(Default::default()))
        );

        assert_eq!(
            discriminant(&RingBuffer::<RingReceiver<()>>::new(1)),
            discriminant(&RingBuffer::Atomic(Default::default()))
        );
    }

    #[proptest]
    fn capacity_returns_the_maximum_buffer_size(#[strategy(1..=10usize)] capacity: usize) {
        assert_eq!(RingBuffer::<[char; 1]>::new(capacity).capacity(), capacity);
        assert_eq!(RingBuffer::<[char; 4]>::new(capacity).capacity(), capacity);
    }

    #[proptest]
    fn oldest_items_are_overwritten_on_overflow(
        #[any(size_range(1..=10).lift())] items: Vec<char>,
        #[strategy(1..=10usize)] capacity: usize,
    ) {
        let buffer = RingBuffer::new(capacity);

        for &item in &items {
            buffer.push(item);
        }

        for &item in items.iter().skip(max(items.len(), capacity) - capacity) {
            assert_eq!(buffer.pop(), Some(item));
        }

        for _ in items.len()..max(items.len(), capacity) {
            assert_eq!(buffer.pop(), None);
        }
    }

    #[cfg(not(miri))] // https://github.com/rust-lang/miri/issues/1388
    #[proptest]
    fn buffer_is_thread_safe(
        #[strategy(1..=10usize)] m: usize,
        #[strategy(1..=10usize)] n: usize,
        #[strategy(1..=10usize)] capacity: usize,
    ) {
        let rt = runtime::Builder::new_multi_thread().build()?;
        let buffer = Arc::new(RingBuffer::new(capacity));

        rt.block_on(try_join(
            repeat(buffer.clone())
                .enumerate()
                .take(m)
                .map(Ok)
                .try_for_each_concurrent(None, |(item, b)| spawn_blocking(move || b.push(item))),
            repeat(buffer)
                .take(n)
                .map(Ok)
                .try_for_each_concurrent(None, |b| {
                    spawn_blocking(move || {
                        b.pop();
                    })
                }),
        ))?;
    }
}
