use core::sync::atomic::*;
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
            registry: self,
            count: self.len.swap(0, Ordering::AcqRel),
        }
    }
}

struct Drain<'a, T> {
    registry: &'a Waitlist<T>,
    count: usize,
}

impl<'a, T> Iterator for Drain<'a, T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.count == 0 {
            return None;
        }

        loop {
            if let item @ Some(_) = self.registry.queue.pop() {
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
    use alloc::{sync::Arc, vec::Vec};
    use core::sync::atomic::Ordering;
    use futures::{future::try_join, prelude::*, stream::repeat};
    use proptest::collection::size_range;
    use test_strategy::proptest;
    use tokio::{runtime, task::spawn_blocking};

    #[proptest]
    fn waitlist_starts_empty() {
        let waitlist = Waitlist::<()>::new();
        assert_eq!(waitlist.len.load(Ordering::Relaxed), 0);
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

        assert_eq!(waitlist.len.load(Ordering::Relaxed), items.len());
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
        assert_eq!(waitlist.len.load(Ordering::Relaxed), 0);
        assert_eq!(waitlist.queue.len(), 0);
    }

    #[cfg(not(miri))] // https://github.com/rust-lang/miri/issues/1388
    #[proptest]
    fn waitlist_is_thread_safe(
        #[strategy(1..=10usize)] m: usize,
        #[strategy(1..=10usize)] n: usize,
    ) {
        let rt = runtime::Builder::new_multi_thread().build()?;
        let waitlist = Arc::new(Waitlist::new());

        rt.block_on(try_join(
            repeat(waitlist.clone())
                .enumerate()
                .take(m)
                .map(Ok)
                .try_for_each_concurrent(None, |(item, w)| spawn_blocking(move || w.push(item))),
            repeat(waitlist)
                .take(n)
                .map(Ok)
                .try_for_each_concurrent(None, |w| {
                    spawn_blocking(move || w.drain().for_each(drop))
                }),
        ))?;
    }
}
