use alloc::boxed::Box;
use core::sync::atomic::{self, AtomicUsize, Ordering};
use core::{cell::UnsafeCell, mem::MaybeUninit};
use crossbeam_utils::{atomic::AtomicCell, Backoff, CachePadded};
use derivative::Derivative;

struct Slot<T> {
    // If the stamp equals the tail, this node will be next written to.
    // If it equals head + 1, this node will be next read from.
    stamp: AtomicUsize,
    value: UnsafeCell<MaybeUninit<T>>,
}

impl<T> Slot<T> {
    fn new(stamp: usize) -> Self {
        Slot {
            stamp: AtomicUsize::new(stamp),
            value: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }
}

pub struct CircularQueue<T> {
    head: CachePadded<AtomicUsize>,
    tail: CachePadded<AtomicUsize>,
    buffer: Box<[CachePadded<Slot<T>>]>,
    lap: usize,
}

unsafe impl<T: Send> Sync for CircularQueue<T> {}
unsafe impl<T: Send> Send for CircularQueue<T> {}

impl<T> CircularQueue<T> {
    fn new(capacity: usize) -> CircularQueue<T> {
        CircularQueue {
            buffer: (0..capacity).map(Slot::new).map(CachePadded::new).collect(),
            head: Default::default(),
            tail: Default::default(),
            lap: (capacity + 1).next_power_of_two(),
        }
    }

    #[inline]
    fn capacity(&self) -> usize {
        self.buffer.len()
    }

    #[inline]
    fn get(&self, cursor: usize) -> &Slot<T> {
        let index = cursor & (self.lap - 1);
        debug_assert!(index < self.capacity());
        unsafe { self.buffer.get_unchecked(index) }
    }

    fn advance(&self, cursor: usize) -> usize {
        let index = cursor & (self.lap - 1);
        let stamp = cursor & !(self.lap - 1);

        if index + 1 < self.capacity() {
            // Same lap, incremented index.
            // Set to `{ stamp: stamp, index: index + 1 }`.
            cursor + 1
        } else {
            // One lap forward, index wraps around to zero.
            // Set to `{ stamp: stamp.wrapping_add(1), index: 0 }`.
            stamp.wrapping_add(self.lap)
        }
    }

    fn push_or_swap(&self, value: T) -> Option<T> {
        let backoff = Backoff::new();
        let mut tail = self.tail.load(Ordering::Relaxed);

        loop {
            let new_tail = self.advance(tail);
            let slot = self.get(tail);
            let stamp = slot.stamp.load(Ordering::Acquire);

            // If the stamp matches the tail, we may attempt to push.
            if stamp == tail {
                // Try advancing the tail.
                match self.tail.compare_exchange_weak(
                    tail,
                    new_tail,
                    Ordering::SeqCst,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        // Write the value into the slot.
                        unsafe { slot.value.get().write(MaybeUninit::new(value)) };
                        slot.stamp.store(tail + 1, Ordering::Release);
                        return None;
                    }

                    Err(t) => {
                        tail = t;
                        backoff.spin();
                        continue;
                    }
                }
            // If the stamp lags one lap behind the tail, we may attempt to swap.
            } else if stamp.wrapping_add(self.lap) == tail + 1 {
                atomic::fence(Ordering::SeqCst);

                // Try advancing the head, if it lags one lap behind the tail as well.
                if self
                    .head
                    .compare_exchange_weak(
                        tail.wrapping_sub(self.lap),
                        new_tail.wrapping_sub(self.lap),
                        Ordering::SeqCst,
                        Ordering::Relaxed,
                    )
                    .is_ok()
                {
                    // Advance the tail.
                    debug_assert_eq!(self.tail.load(Ordering::SeqCst), tail);
                    self.tail.store(new_tail, Ordering::SeqCst);

                    // Replace the value in the slot.
                    let new = MaybeUninit::new(value);
                    let old = unsafe { slot.value.get().replace(new).assume_init() };
                    slot.stamp.store(tail + 1, Ordering::Release);
                    return Some(old);
                }
            }

            backoff.snooze();
            tail = self.tail.load(Ordering::Relaxed);
        }
    }

    fn pop(&self) -> Option<T> {
        let backoff = Backoff::new();
        let mut head = self.head.load(Ordering::Relaxed);

        loop {
            let slot = self.get(head);
            let stamp = slot.stamp.load(Ordering::Acquire);

            // If the the stamp is ahead of the head by 1, we may attempt to pop.
            if stamp == head + 1 {
                // Try advancing the head.
                match self.head.compare_exchange_weak(
                    head,
                    self.advance(head),
                    Ordering::SeqCst,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        // Read the value from the slot.
                        let msg = unsafe { slot.value.get().read().assume_init() };
                        slot.stamp
                            .store(head.wrapping_add(self.lap), Ordering::Release);
                        return Some(msg);
                    }

                    Err(h) => {
                        head = h;
                        backoff.spin();
                        continue;
                    }
                }
            // If the stamp matches the head, the queue may be empty.
            } else if stamp == head {
                atomic::fence(Ordering::SeqCst);

                // If the tail matches the head as well, the queue is empty.
                if self.tail.load(Ordering::Relaxed) == head {
                    return None;
                }
            }

            backoff.snooze();
            head = self.head.load(Ordering::Relaxed);
        }
    }
}

impl<T> Drop for CircularQueue<T> {
    fn drop(&mut self) {
        let mut cursor = self.head.load(Ordering::Relaxed);
        let end = self.tail.load(Ordering::Relaxed);

        // Loop over all slots that hold a message and drop them.
        while cursor != end {
            let slot = self.get(cursor);
            unsafe { (&mut *slot.value.get()).as_mut_ptr().drop_in_place() };
            cursor = self.advance(cursor);
        }
    }
}

#[derive(Derivative)]
#[derivative(Debug)]
#[allow(clippy::large_enum_variant)]
pub(super) enum RingBuffer<T> {
    Atomic(#[derivative(Debug = "ignore")] AtomicCell<Option<T>>),
    Boxed(#[derivative(Debug = "ignore")] AtomicCell<Option<Box<T>>>),
    Queue(#[derivative(Debug = "ignore")] CircularQueue<T>),
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
            RingBuffer::Queue(CircularQueue::new(capacity))
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

    pub(super) fn push(&self, value: T) -> Option<T> {
        match self {
            RingBuffer::Atomic(c) => c.swap(Some(value)),
            RingBuffer::Boxed(b) => Some(*b.swap(Some(Box::new(value)))?),
            RingBuffer::Queue(q) => q.push_or_swap(value),
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
    use alloc::{collections::BinaryHeap, sync::Arc, vec::Vec};
    use core::{iter, mem::discriminant};
    use futures::future::try_join_all;
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
            discriminant(&RingBuffer::Queue(CircularQueue::new(2)))
        );

        assert_eq!(
            discriminant(&RingBuffer::<[char; 4]>::new(1)),
            discriminant(&RingBuffer::Boxed(Default::default()))
        );

        assert_eq!(
            discriminant(&RingBuffer::<[char; 4]>::new(2)),
            discriminant(&RingBuffer::Queue(CircularQueue::new(2)))
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
        #[strategy(1..=10usize)] capacity: usize,
        #[any(size_range(#capacity..=10).lift())] items: Vec<char>,
    ) {
        let buffer = RingBuffer::new(capacity);

        for &item in &items[..capacity] {
            assert_eq!(buffer.push(item), None);
        }

        for (i, &item) in (0..(items.len() - capacity)).zip(&items[capacity..]) {
            assert_eq!(buffer.push(item), Some(items[i]));
        }

        assert_eq!(
            iter::from_fn(|| buffer.pop()).collect::<Vec<_>>(),
            items[(items.len() - capacity)..]
        );
    }

    #[proptest]
    fn buffer_is_linearizable(
        #[strategy(1..=10usize)] n: usize,
        #[strategy(1..=10usize)] capacity: usize,
    ) {
        let rt = runtime::Builder::new_multi_thread().build()?;
        let buffer = Arc::new(RingBuffer::new(capacity));

        let items = rt.block_on(async {
            try_join_all(iter::repeat(buffer).enumerate().take(n).map(|(i, b)| {
                spawn_blocking(move || match b.push(i) {
                    None => b.pop(),
                    item => item,
                })
            }))
            .await
        })?;

        let sorted = items
            .into_iter()
            .flatten()
            .collect::<BinaryHeap<_>>()
            .into_sorted_vec();

        assert_eq!(sorted, (0..n).collect::<Vec<_>>());
    }
}
