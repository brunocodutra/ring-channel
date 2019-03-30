use crossbeam_queue::{ArrayQueue, PushError};
use derivative::Derivative;

#[derive(Derivative)]
#[derivative(Debug)]
pub struct Buffer<T>(#[derivative(Debug = "ignore")] ArrayQueue<T>);

impl<T> Buffer<T> {
    pub fn new(capacity: usize) -> Self {
        Buffer(ArrayQueue::new(capacity))
    }

    #[cfg(test)]
    pub fn capacity(&self) -> usize {
        self.0.capacity()
    }

    pub fn push(&self, value: T) -> Option<T> {
        self.0.push(value).err().map(|PushError(value)| value)
    }

    pub fn pop(&self) -> Option<T> {
        self.0.pop().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::{collection::vec, prelude::*};
    use std::cmp::{max, min};

    proptest! {
        #[test]
        fn capacity(capacity in 1..=100usize) {
            let buffer = Buffer::<()>::new(capacity);
            assert_eq!(buffer.capacity(), capacity);
        }

        #[test]
        fn push(entries in vec(any::<u32>(), 1..=100), capacity in 1..=100usize) {
            let buffer = Buffer::new(capacity);

            for entry in entries.iter().take(min(entries.len(), capacity)) {
                assert_eq!(buffer.push(entry), None);
            }

            for entry in entries.iter().take(max(entries.len(), capacity)).skip(capacity) {
                assert_eq!(buffer.push(entry), Some(entry));
            }
        }

        #[test]
        fn pop(entries in vec(any::<u32>(), 1..=100), capacity in 1..=100usize) {
            let buffer = Buffer::new(capacity);

            for &entry in &entries {
                buffer.push(entry);
            }

            for &entry in entries.iter().take(min(entries.len(), capacity)) {
                assert_eq!(buffer.pop(), Some(entry));
            }

            for _ in capacity..max(entries.len(), capacity) {
                assert_eq!(buffer.pop(), None);
            }
        }
    }
}
