use crossbeam_queue::{ArrayQueue, PushError};
use derivative::Derivative;

#[derive(Derivative)]
#[derivative(Debug)]
pub(super) struct RingBuffer<T>(#[derivative(Debug = "ignore")] ArrayQueue<T>);

impl<T> RingBuffer<T> {
    pub(super) fn new(capacity: usize) -> Self {
        RingBuffer(ArrayQueue::new(capacity))
    }

    #[cfg(test)]
    pub(super) fn capacity(&self) -> usize {
        self.0.capacity()
    }

    pub(super) fn push(&self, mut value: T) {
        while let Err(PushError(v)) = self.0.push(value) {
            self.pop();
            value = v;
        }
    }

    pub(super) fn pop(&self) -> Option<T> {
        self.0.pop().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::{collection::vec, prelude::*};
    use std::cmp::max;

    proptest! {
        #[test]
        fn capacity(capacity in 1..=100usize) {
            let buffer = RingBuffer::<()>::new(capacity);
            assert_eq!(buffer.capacity(), capacity);
        }

        #[test]
        fn push_pop(entries in vec(any::<u32>(), 1..=100), capacity in 1..=100usize) {
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
