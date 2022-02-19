use core::sync::atomic::{AtomicUsize, Ordering};
use crossbeam_queue::SegQueue;
use crossbeam_utils::CachePadded;
use derivative::Derivative;

#[derive(Derivative, Debug)]
#[derivative(Default(bound = "", new = "true"))]
pub(super) struct Waitlist<T> {
    len: CachePadded<AtomicUsize>,
    queue: SegQueue<T>,
}

impl<T> Waitlist<T> {
    pub(super) fn push(&self, item: T) {
        self.len.fetch_add(1, Ordering::AcqRel);
        self.queue.push(item);
    }

    // The queue is cleared, even if the iterator is not fully consumed.
    pub(super) fn drain(&self) -> impl Iterator<Item = T> + '_ {
        Drain {
            waitlist: self,
            count: self.len.swap(0, Ordering::AcqRel),
        }
    }
}

struct Drain<'a, T> {
    waitlist: &'a Waitlist<T>,
    count: usize,
}

impl<'a, T> Drop for Drain<'a, T> {
    fn drop(&mut self) {
        self.for_each(drop);
    }
}

impl<'a, T> Iterator for Drain<'a, T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.count == 0 {
            return None;
        }

        loop {
            if let item @ Some(_) = self.waitlist.queue.pop() {
                self.count -= 1;
                return item;
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.count, self.count.into())
    }
}

impl<'a, T> ExactSizeIterator for Drain<'a, T> {}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{collections::BinaryHeap, sync::Arc, vec::Vec};
    use core::{iter, sync::atomic::Ordering};
    use futures::future::try_join_all;
    use proptest::collection::size_range;
    use test_strategy::proptest;
    use tokio::{runtime, task::spawn_blocking};

    #[proptest]
    fn waitlist_starts_empty() {
        let waitlist = Waitlist::<()>::new();
        assert_eq!(waitlist.len.load(Ordering::SeqCst), 0);
        assert_eq!(waitlist.queue.len(), 0);
    }

    #[proptest]
    fn push_inserts_item_at_the_back_of_the_queue(
        #[any(size_range(1..=10).lift())] items: Vec<char>,
    ) {
        let waitlist = Waitlist::new();

        for &item in &items {
            waitlist.push(item);
        }

        assert_eq!(waitlist.len.load(Ordering::SeqCst), items.len());
        assert_eq!(waitlist.queue.len(), items.len());
    }

    #[proptest]
    fn drain_removes_items_from_the_queue_in_fifo_order(
        #[any(size_range(1..=10).lift())] items: Vec<char>,
    ) {
        let waitlist = Waitlist::new();

        for &item in &items {
            waitlist.push(item);
        }

        assert_eq!(waitlist.drain().collect::<Vec<_>>(), items);
        assert_eq!(waitlist.len.load(Ordering::SeqCst), 0);
        assert_eq!(waitlist.queue.len(), 0);
    }

    #[proptest]
    fn items_are_popped_if_drain_iterator_is_dropped(
        #[any(size_range(1..=10).lift())] items: Vec<char>,
    ) {
        let waitlist = Waitlist::new();

        for &item in &items {
            waitlist.push(item);
        }

        drop(waitlist.drain());

        assert_eq!(waitlist.len.load(Ordering::SeqCst), 0);
        assert_eq!(waitlist.queue.len(), 0);
    }

    #[cfg(not(miri))] // https://github.com/rust-lang/miri/issues/1388
    #[proptest]
    fn waitlist_is_linearizable(#[strategy(1..=10usize)] n: usize) {
        let rt = runtime::Builder::new_multi_thread().build()?;
        let waitlist = Arc::new(Waitlist::new());

        let items = rt.block_on(async {
            try_join_all(iter::repeat(waitlist).enumerate().take(n).map(|(i, w)| {
                spawn_blocking(move || {
                    w.push(i);
                    w.drain().collect::<Vec<_>>()
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
