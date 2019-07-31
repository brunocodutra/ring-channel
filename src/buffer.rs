use crossbeam_queue::{ArrayQueue, PushError};
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
                while let Err(PushError(v)) = q.push(value) {
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
            Queue(q) => q.pop().ok(),
            Cell(c) => c.swap(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RingReceiver, RingSender};
    use proptest::{collection::vec, prelude::*};
    use std::{cmp::max, mem::discriminant};

    #[test]
    fn queue() {
        assert_eq!(
            discriminant(&RingBuffer::<String>::new(1)),
            discriminant(&RingBuffer::Queue(ArrayQueue::new(1)))
        );

        assert_eq!(
            discriminant(&RingBuffer::<std::num::NonZeroUsize>::new(2)),
            discriminant(&RingBuffer::Queue(ArrayQueue::new(2)))
        );

        assert_eq!(
            discriminant(&RingBuffer::<std::ptr::NonNull<String>>::new(2)),
            discriminant(&RingBuffer::Queue(ArrayQueue::new(2)))
        );

        assert_eq!(
            discriminant(&RingBuffer::<std::rc::Rc<String>>::new(2)),
            discriminant(&RingBuffer::Queue(ArrayQueue::new(2)))
        );

        assert_eq!(
            discriminant(&RingBuffer::<std::sync::Arc<String>>::new(2)),
            discriminant(&RingBuffer::Queue(ArrayQueue::new(2)))
        );

        assert_eq!(
            discriminant(&RingBuffer::<RingReceiver<String>>::new(2)),
            discriminant(&RingBuffer::Queue(ArrayQueue::new(2)))
        );

        assert_eq!(
            discriminant(&RingBuffer::<RingSender<String>>::new(2)),
            discriminant(&RingBuffer::Queue(ArrayQueue::new(2)))
        );
    }

    #[test]
    fn cell() {
        assert_eq!(
            discriminant(&RingBuffer::<std::num::NonZeroUsize>::new(1)),
            discriminant(&RingBuffer::Cell(Default::default()))
        );

        assert_eq!(
            discriminant(&RingBuffer::<std::ptr::NonNull<String>>::new(1)),
            discriminant(&RingBuffer::Cell(Default::default()))
        );

        assert_eq!(
            discriminant(&RingBuffer::<std::rc::Rc<String>>::new(1)),
            discriminant(&RingBuffer::Cell(Default::default()))
        );

        assert_eq!(
            discriminant(&RingBuffer::<std::sync::Arc<String>>::new(1)),
            discriminant(&RingBuffer::Cell(Default::default()))
        );

        assert_eq!(
            discriminant(&RingBuffer::<RingReceiver<String>>::new(1)),
            discriminant(&RingBuffer::Cell(Default::default()))
        );

        assert_eq!(
            discriminant(&RingBuffer::<RingSender<String>>::new(1)),
            discriminant(&RingBuffer::Cell(Default::default()))
        );
    }

    proptest! {
        #[test]
        fn capacity(capacity in 1..=100usize) {
            let buffer = RingBuffer::<()>::new(capacity);
            assert_eq!(buffer.capacity(), capacity);
        }

        #[test]
        fn overflow(entries in vec(any::<u32>(), 1..=100), capacity in 1..=100usize) {
            let buffer = RingBuffer::new(capacity);

            for &entry in &entries {
                buffer.push(entry);
            }

            for &entry in entries.iter().skip(max(entries.len(), capacity) - capacity) {
                assert_eq!(buffer.pop(), Some(entry));
            }

            for _ in entries.len()..max(entries.len(), capacity) {
                assert_eq!(buffer.pop(), None);
            }
        }
    }
}
