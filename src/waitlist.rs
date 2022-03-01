use crate::atomic::AtomicOption;
use crossbeam_utils::CachePadded;
use derivative::Derivative;
use slotmap::{DefaultKey, HopSlotMap};
use spin::RwLock;

#[derive(Debug, Default, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(super) struct Slot(DefaultKey);

#[derive(Derivative, Debug)]
#[derivative(Default(bound = "", new = "true"))]
pub(super) struct Waitlist<T> {
    items: CachePadded<RwLock<HopSlotMap<DefaultKey, AtomicOption<T>>>>,
}

impl<T> Waitlist<T> {
    #[cfg(test)]
    #[inline]
    pub(super) fn len(&self) -> usize {
        self.items.read().len()
    }

    pub(super) fn register(&self) -> Slot {
        Slot(self.items.write().insert(Default::default()))
    }

    pub(super) fn deregister(&self, Slot(k): Slot) -> Option<T> {
        self.items.write().remove(k).unwrap().into_inner()
    }

    pub(super) fn insert(&self, Slot(k): Slot, item: T) -> Option<T> {
        self.items.read()[k].swap(item)
    }

    pub(super) fn remove(&self, Slot(k): Slot) -> Option<T> {
        self.items.read()[k].take()
    }

    pub(super) fn pop(&self) -> Option<T> {
        self.items.read().values().find_map(AtomicOption::take)
    }
}

#[cfg(test)]
impl<T: Clone> Waitlist<T> {
    #[inline]
    pub(super) fn get(&self, Slot(k): Slot) -> Option<T> {
        self.items.write()[k].get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Void;
    use alloc::collections::{BTreeSet, BinaryHeap};
    use alloc::{sync::Arc, vec::Vec};
    use core::iter;
    use futures::future::try_join_all;
    use proptest::collection::size_range;
    use test_strategy::proptest;
    use tokio::{runtime, task::spawn_blocking};

    #[proptest]
    fn waitlist_starts_empty() {
        let waitlist = Waitlist::<Void>::new();
        assert_eq!(waitlist.items.read().len(), 0);
    }

    #[proptest]
    fn register_allocates_slot(#[strategy(1..=10usize)] n: usize) {
        let waitlist = Waitlist::<Void>::new();

        let slots = iter::repeat_with(|| waitlist.register())
            .take(n)
            .collect::<Vec<_>>();

        assert_eq!(
            Vec::from_iter(BTreeSet::from_iter(slots.clone())),
            BinaryHeap::from(slots).into_sorted_vec()
        );

        assert_eq!(waitlist.items.read().len(), n);
    }

    #[proptest]
    fn deregister_deallocates_slot(#[strategy(1..=10usize)] n: usize) {
        let waitlist = Waitlist::<()>::new();

        for _ in 0..n {
            let slot = waitlist.register();
            assert_eq!(waitlist.deregister(slot), None);
        }

        assert_eq!(waitlist.items.read().len(), 0);
    }

    #[proptest]
    fn deregister_returns_current_item(#[any(size_range(1..=10).lift())] items: Vec<u8>) {
        let waitlist = Waitlist::new();

        for item in items {
            let slot = waitlist.register();
            assert_eq!(waitlist.insert(slot, item), None);
            assert_eq!(waitlist.deregister(slot), Some(item));
        }

        assert_eq!(waitlist.items.read().len(), 0);
    }

    #[proptest]
    fn inserts_replaces_current_item(#[any(size_range(1..=10).lift())] items: Vec<u8>) {
        let waitlist = Waitlist::new();

        let slot = waitlist.register();
        assert_eq!(waitlist.insert(slot, items[0]), None);

        for (&prev, &item) in items.iter().zip(&items[1..]) {
            assert_eq!(waitlist.insert(slot, item), Some(prev));
        }

        assert_eq!(waitlist.items.read().len(), 1);
    }

    #[proptest]
    fn remove_takes_item(item: u8) {
        let waitlist = Waitlist::new();
        let slot = waitlist.register();
        waitlist.insert(slot, item);
        assert_eq!(waitlist.remove(slot), Some(item));
        assert_eq!(waitlist.remove(slot), None);
        assert_eq!(waitlist.items.read().len(), 1);
    }

    #[proptest]
    fn get_clones_item(item: u8) {
        let waitlist = Waitlist::new();
        let slot = waitlist.register();
        waitlist.insert(slot, item);
        assert_eq!(waitlist.get(slot), Some(item));
        assert_eq!(waitlist.get(slot), Some(item));
        assert_eq!(waitlist.items.read().len(), 1);
    }

    #[proptest]
    fn pop_removes_one_item_from_any_slot(#[any(size_range(1..=10).lift())] items: Vec<u8>) {
        let waitlist = Waitlist::new();

        for &item in &items {
            let slot = waitlist.register();
            waitlist.insert(slot, item);
        }

        assert_eq!(
            iter::from_fn(|| waitlist.pop())
                .collect::<BinaryHeap<_>>()
                .into_sorted_vec(),
            BinaryHeap::from(items).into_sorted_vec()
        );
    }

    #[proptest]
    fn waitlist_is_linearizable(
        #[strategy(1..=10usize)] m: usize,
        #[strategy(1..=10usize)] n: usize,
    ) {
        let rt = runtime::Builder::new_multi_thread().build()?;
        let waitlist = Arc::new(Waitlist::new());

        let items = rt.block_on(async {
            try_join_all(iter::repeat(waitlist).enumerate().take(m).map(|(i, w)| {
                spawn_blocking(move || {
                    let slot = w.register();
                    (i * n..(i + 1) * n)
                        .flat_map(|j| match w.insert(slot, j) {
                            None => w.pop(),
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
