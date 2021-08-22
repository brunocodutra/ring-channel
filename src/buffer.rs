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
    use alloc::{rc::Rc, string::String, sync::Arc};
    use core::{cmp::max, mem::discriminant, num::NonZeroUsize, ptr::NonNull};
    use proptest::{collection::vec, prelude::*};

    #[test]
    fn queue() {
        assert_eq!(
            discriminant(&RingBuffer::<String>::new(1)),
            discriminant(&RingBuffer::Queue(ArrayQueue::new(1)))
        );

        assert_eq!(
            discriminant(&RingBuffer::<NonZeroUsize>::new(2)),
            discriminant(&RingBuffer::Queue(ArrayQueue::new(2)))
        );

        assert_eq!(
            discriminant(&RingBuffer::<NonNull<String>>::new(2)),
            discriminant(&RingBuffer::Queue(ArrayQueue::new(2)))
        );

        assert_eq!(
            discriminant(&RingBuffer::<Rc<String>>::new(2)),
            discriminant(&RingBuffer::Queue(ArrayQueue::new(2)))
        );

        assert_eq!(
            discriminant(&RingBuffer::<Arc<String>>::new(2)),
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
            discriminant(&RingBuffer::<NonZeroUsize>::new(1)),
            discriminant(&RingBuffer::Cell(Default::default()))
        );

        assert_eq!(
            discriminant(&RingBuffer::<NonNull<String>>::new(1)),
            discriminant(&RingBuffer::Cell(Default::default()))
        );

        assert_eq!(
            discriminant(&RingBuffer::<Rc<String>>::new(1)),
            discriminant(&RingBuffer::Cell(Default::default()))
        );

        assert_eq!(
            discriminant(&RingBuffer::<Arc<String>>::new(1)),
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
        fn overflow(entries in vec(any::<usize>(), 1..=100), capacity in 1..=100usize) {
            let buffer = RingBuffer::new(capacity);

            for &entry in &entries {
                buffer.push(NonZeroUsize::new(entry).unwrap());
            }

            for &entry in entries.iter().skip(max(entries.len(), capacity) - capacity) {
                assert_eq!(buffer.pop(), Some(NonZeroUsize::new(entry).unwrap()));
            }

            for _ in entries.len()..max(entries.len(), capacity) {
                assert_eq!(buffer.pop(), None);
            }
        }
    }
}
