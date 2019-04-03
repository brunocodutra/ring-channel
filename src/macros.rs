#[macro_export]
#[doc(hidden)]
macro_rules! same {
    ($l:expr, $r:expr) => {{
        debug_assert_eq!($l, $r);
        $l
    }};
}

#[cfg(test)]
mod tests {
    #[test]
    fn same() {
        let x = same!(1 + 1, 4 / 2);
        assert_eq!(x, 2);
    }

    #[test]
    #[should_panic]
    #[cfg(debug_assertions)]
    fn different() {
        same!(0, 1);
    }
}
