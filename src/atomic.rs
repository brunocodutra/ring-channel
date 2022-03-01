use alloc::boxed::Box;
use crossbeam_utils::{atomic::AtomicCell, CachePadded};
use derivative::Derivative;

#[derive(Derivative)]
#[derivative(Debug(bound = ""))]
pub(super) enum AtomicOption<T> {
    Small(#[derivative(Debug = "ignore")] CachePadded<AtomicCell<Option<T>>>),
    Large(#[derivative(Debug = "ignore")] CachePadded<AtomicCell<Option<Box<T>>>>),
}

impl<T> AtomicOption<T> {
    pub(super) fn new() -> Self {
        if AtomicCell::<Option<T>>::is_lock_free() {
            AtomicOption::Small(Default::default())
        } else {
            debug_assert!(AtomicCell::<Option<Box<T>>>::is_lock_free());
            AtomicOption::Large(Default::default())
        }
    }

    #[inline]
    pub(super) fn take(&self) -> Option<T> {
        match self {
            AtomicOption::Small(c) => c.take(),
            AtomicOption::Large(c) => Some(*c.take()?),
        }
    }

    #[cfg(test)]
    #[inline]
    pub(super) fn store(&self, item: T) {
        match self {
            AtomicOption::Small(c) => c.store(Some(item)),
            AtomicOption::Large(c) => c.store(Some(Box::new(item))),
        }
    }

    #[inline]
    pub(super) fn swap(&self, item: T) -> Option<T> {
        match self {
            AtomicOption::Small(c) => c.swap(Some(item)),
            AtomicOption::Large(c) => Some(*c.swap(Some(Box::new(item)))?),
        }
    }

    #[cfg(any(test, feature = "futures_api"))]
    #[inline]
    pub(super) fn into_inner(self) -> Option<T> {
        match self {
            AtomicOption::Small(c) => c.into_inner().into_inner(),
            AtomicOption::Large(c) => Some(*c.into_inner().into_inner()?),
        }
    }
}

#[cfg(test)]
impl<T: Clone> AtomicOption<T> {
    pub(super) fn get(&mut self) -> Option<T> {
        let item = self.take()?;
        self.store(item.clone());
        Some(item)
    }
}

impl<T> Default for AtomicOption<T> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{collections::BinaryHeap, sync::Arc, vec::Vec};
    use core::{iter, mem::discriminant};
    use futures::future::try_join_all;
    use proptest::collection::size_range;
    use test_strategy::proptest;
    use tokio::{runtime, task::spawn_blocking};

    #[proptest]
    fn falls_back_to_boxing_for_large_types() {
        assert_eq!(
            discriminant(&AtomicOption::<[char; 1]>::new()),
            discriminant(&AtomicOption::Small(Default::default()))
        );

        assert_eq!(
            discriminant(&AtomicOption::<[char; 8]>::new()),
            discriminant(&AtomicOption::Large(Default::default()))
        );
    }

    #[proptest]
    fn store_inserts_item(a: u8, b: u8) {
        let small = AtomicOption::new();
        let large = AtomicOption::new();

        small.store([a; 1]);
        large.store([a; 8]);

        small.store([b; 1]);
        large.store([b; 8]);

        assert_eq!(small.into_inner(), Some([b; 1]));
        assert_eq!(large.into_inner(), Some([b; 8]));
    }

    #[proptest]
    fn take_removes_item(item: u8) {
        let small = AtomicOption::new();
        let large = AtomicOption::new();

        small.store([item; 1]);
        large.store([item; 8]);

        assert_eq!(small.take(), Some([item; 1]));
        assert_eq!(large.take(), Some([item; 8]));

        assert_eq!(small.take(), None);
        assert_eq!(large.take(), None);
    }

    #[proptest]
    fn swap_replaces_item(#[any(size_range(1..=10).lift())] items: Vec<u8>) {
        let small = AtomicOption::new();
        let large = AtomicOption::new();

        assert_eq!(small.swap([items[0]; 1]), None);
        assert_eq!(large.swap([items[0]; 8]), None);

        for (&prev, &item) in items.iter().zip(&items[1..]) {
            assert_eq!(small.swap([item; 1]), Some([prev; 1]));
            assert_eq!(large.swap([item; 8]), Some([prev; 8]));
        }
    }

    #[proptest]
    fn get_clones_the_item(item: u8) {
        let mut small = AtomicOption::new();
        let mut large = AtomicOption::new();

        small.store([item; 1]);
        large.store([item; 8]);

        assert_eq!(small.get(), Some([item; 1]));
        assert_eq!(large.get(), Some([item; 8]));

        assert_eq!(small.get(), Some([item; 1]));
        assert_eq!(large.get(), Some([item; 8]));
    }

    #[proptest]
    fn atomic_option_is_linearizable(
        #[strategy(1..=10usize)] m: usize,
        #[strategy(1..=10usize)] n: usize,
    ) {
        let rt = runtime::Builder::new_multi_thread().build()?;
        let buffer = Arc::new(AtomicOption::new());

        let items = rt.block_on(async {
            try_join_all(iter::repeat(buffer).enumerate().take(m).map(|(i, o)| {
                spawn_blocking(move || {
                    (i * n..(i + 1) * n)
                        .flat_map(|j| match o.swap(j) {
                            None => o.take(),
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
