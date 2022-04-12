use crate::atomic::AtomicOption;
use crossbeam_queue::ArrayQueue;
use derivative::Derivative;

#[derive(Derivative)]
#[derivative(Debug(bound = ""))]
#[allow(clippy::large_enum_variant)]
pub(super) enum RingBuffer<T> {
    Atomic(AtomicOption<T>),
    Queue(ArrayQueue<T>),
}

impl<T> RingBuffer<T> {
    pub(super) fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "capacity must be non-zero");

        if capacity == 1 {
            RingBuffer::Atomic(Default::default())
        } else {
            RingBuffer::Queue(ArrayQueue::new(capacity))
        }
    }

    #[cfg(test)]
    #[inline]
    pub(super) fn capacity(&self) -> usize {
        match self {
            RingBuffer::Atomic(_) => 1,
            RingBuffer::Queue(q) => q.capacity(),
        }
    }

    #[inline]
    pub(super) fn push(&self, value: T) -> Option<T> {
        match self {
            RingBuffer::Atomic(c) => c.swap(value),
            RingBuffer::Queue(q) => q.force_push(value),
        }
    }

    #[inline]
    pub(super) fn pop(&self) -> Option<T> {
        match self {
            RingBuffer::Atomic(c) => c.take(),
            RingBuffer::Queue(q) => q.pop(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Void;
    use alloc::{collections::BinaryHeap, sync::Arc, vec::Vec};
    use core::{iter, mem::discriminant};
    use futures::future::try_join_all;
    use proptest::collection::size_range;
    use test_strategy::proptest;
    use tokio::{runtime, task::spawn_blocking};

    #[should_panic]
    #[proptest]
    fn new_panics_if_capacity_is_zero() {
        RingBuffer::<Void>::new(0);
    }

    #[proptest]
    fn atomic_option_is_used_when_capacity_is_one() {
        assert_eq!(
            discriminant(&RingBuffer::<Void>::new(1)),
            discriminant(&RingBuffer::Atomic(Default::default()))
        );

        assert_eq!(
            discriminant(&RingBuffer::<Void>::new(2)),
            discriminant(&RingBuffer::Queue(ArrayQueue::new(2)))
        );
    }

    #[proptest]
    fn capacity_returns_the_maximum_buffer_size(#[strategy(1..=10usize)] cap: usize) {
        assert_eq!(RingBuffer::<Void>::new(cap).capacity(), cap);
    }

    #[proptest]
    fn oldest_items_are_overwritten_on_overflow(
        #[strategy(1..=10usize)] cap: usize,
        #[any(size_range(#cap..=10).lift())] items: Vec<u8>,
    ) {
        let buffer = RingBuffer::new(cap);

        for &item in &items[..cap] {
            assert_eq!(buffer.push(item), None);
        }

        for (&prev, &item) in items.iter().zip(&items[cap..]) {
            assert_eq!(buffer.push(item), Some(prev));
        }

        assert_eq!(
            iter::from_fn(|| buffer.pop()).collect::<Vec<_>>(),
            items[(items.len() - cap)..]
        );
    }

    #[proptest]
    fn buffer_is_linearizable(
        #[strategy(1..=10usize)] n: usize,
        #[strategy(1..=10usize)] m: usize,
        #[strategy(1..=10usize)] cap: usize,
    ) {
        let rt = runtime::Builder::new_multi_thread().build()?;
        let buffer = Arc::new(RingBuffer::new(cap));

        let items = rt.block_on(async {
            try_join_all(iter::repeat(buffer).enumerate().take(m).map(|(i, b)| {
                spawn_blocking(move || {
                    (i * n..(i + 1) * n)
                        .flat_map(|j| match b.push(j) {
                            None => b.pop(),
                            item => item,
                        })
                        .collect::<Vec<_>>()
                })
            }))
            .await
        })?;

        let sorted = items
            .into_iter()
            .flatten()
            .collect::<BinaryHeap<_>>()
            .into_sorted_vec();

        assert_eq!(sorted, (0..m * n).collect::<Vec<_>>());
    }
}
