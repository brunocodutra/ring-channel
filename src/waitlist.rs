use crossbeam_utils::CachePadded;
use derivative::Derivative;
use smallvec::SmallVec;
use spin::Mutex;
use std::{sync::atomic::*, task::Waker};

#[cfg_attr(test, mockall::automock)]
pub(super) trait Wake {
    fn wake(self);
}

impl Wake for Waker {
    fn wake(self) {
        self.wake()
    }
}

#[derive(Derivative)]
#[derivative(Debug, Default(bound = "", new = "true"))]
pub(super) struct Waitlist<W> {
    #[derivative(Default(value = "AtomicBool::new(true)"))]
    empty: AtomicBool,
    wakers: CachePadded<Mutex<SmallVec<[W; 6]>>>,
}

impl<W> Waitlist<W> {
    pub(super) fn wait(&self, waker: W) {
        {
            self.wakers.lock().push(waker);
        }

        self.empty.store(false, Ordering::Release);
    }
}

impl<W: Wake> Waitlist<W> {
    pub(super) fn wake(&self) {
        if !self.empty.swap(true, Ordering::Acquire) {
            // Drain all wakers in case any has become stale.
            for waker in self.wakers.lock().drain() {
                waker.wake();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use rayon::scope;
    use std::{mem::size_of, task::Waker};

    #[test]
    fn waitlist_of_wakers_does_not_take_extra_space() {
        assert_eq!(size_of::<Waitlist::<()>>(), size_of::<Waitlist::<Waker>>());
    }

    #[test]
    fn waitlist_starts_empty() {
        let waitlist = Waitlist::<MockWake>::new();
        assert_eq!(waitlist.empty.load(Ordering::Relaxed), true);
        assert_eq!(waitlist.wakers.lock().len(), 0);
    }

    proptest! {
        #[test]
        fn waitlist_wakes_all_wakers_exactly_once(m in 1..=100usize, n in 1..=100usize) {
            let waitlist = Waitlist::new();

            for _ in 0..m {
                let mut waker = MockWake::new();
                waker.expect_wake().once().return_const(());
                waitlist.wait(waker);
            }

            assert_eq!(waitlist.empty.load(Ordering::Relaxed), false);
            assert_eq!(waitlist.wakers.lock().len(), m);

            for _ in 0..n {
                waitlist.wake();
            }
        }

        #[test]
        fn waitlist_is_safe_to_share_across_threads(m in 1..=100usize, n in 1..=100usize) {
            let waitlist = Waitlist::new();

            scope(|s| {
                for _ in 0..m {
                    s.spawn(|_| {
                        let mut waker = MockWake::new();
                        waker.expect_wake().times(0..=1).return_const(());
                        waitlist.wait(waker);
                    });
                }

                for _ in 0..n {
                    s.spawn(|_| {
                        waitlist.wake();
                    });
                }
            });
        }
    }
}
