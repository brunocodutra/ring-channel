use crossbeam_queue::ArrayQueue;
use derivative::Derivative;

type AtomicOption<T> = crossbeam_utils::atomic::AtomicCell<Option<T>>;

#[derive(Derivative)]
#[derivative(Debug)]
#[allow(clippy::large_enum_variant)]
pub(super) enum RingBuffer<T> {
    Queue(#[derivative(Debug = "ignore")] ArrayQueue<T>),
    Cell(#[derivative(Debug = "ignore")] AtomicOption<T>),
}

impl<T> RingBuffer<T> {
    pub(super) fn new(capacity: usize) -> Self {
        if capacity > 1 || !AtomicOption::<T>::is_lock_free() {
            RingBuffer::Queue(ArrayQueue::new(capacity))
        } else {
            RingBuffer::Cell(AtomicOption::new(None))
        }
    }

    #[cfg(test)]
    pub(super) fn capacity(&self) -> usize {
        use RingBuffer::*;
        match self {
            Queue(q) => q.capacity(),
            Cell(_) => 1,
        }
    }

    pub(super) fn push(&self, mut value: T) {
        use RingBuffer::*;
        match self {
            Queue(q) => {
                while let Err(v) = q.push(value) {
                    self.pop();
                    value = v;
                }
            }

            Cell(c) => {
                c.swap(Some(value));
            }
        }
    }

    pub(super) fn pop(&self) -> Option<T> {
        use RingBuffer::*;
        match self {
            Queue(q) => q.pop(),
            Cell(c) => c.swap(None),
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

    #[proptest]
    fn new_uses_atomic_cell_when_possible() {
        assert_eq!(
            discriminant(&RingBuffer::<[char; 1]>::new(1)),
            discriminant(&RingBuffer::Cell(Default::default()))
        );

        assert_eq!(
            discriminant(&RingBuffer::<[char; 1]>::new(2)),
            discriminant(&RingBuffer::Queue(ArrayQueue::new(2)))
        );

        assert_eq!(
            discriminant(&RingBuffer::<[char; 4]>::new(1)),
            discriminant(&RingBuffer::Queue(ArrayQueue::new(1)))
        );

        assert_eq!(
            discriminant(&RingBuffer::<RingSender<()>>::new(1)),
            discriminant(&RingBuffer::Cell(Default::default()))
        );

        assert_eq!(
            discriminant(&RingBuffer::<RingReceiver<()>>::new(1)),
            discriminant(&RingBuffer::Cell(Default::default()))
        );
    }

    #[proptest]
    fn capacity_returns_the_maximum_buffer_size(#[strategy(1..=100usize)] capacity: usize) {
        let buffer = RingBuffer::<()>::new(capacity);
        assert_eq!(buffer.capacity(), capacity);
    }

    #[proptest]
    fn oldest_items_are_overwritten_on_overflow(
        #[any(size_range(1..=100).lift())] items: Vec<char>,
        #[strategy(1..=100usize)] capacity: usize,
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

    #[proptest]
    fn buffer_is_thread_safe(
        #[strategy(1..=100usize)] m: usize,
        #[strategy(1..=100usize)] n: usize,
        #[strategy(1..=100usize)] capacity: usize,
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
