mod buffer;
mod channel;
mod error;
mod macros;
mod receiver;
mod sender;

pub use error::*;
pub use receiver::*;
pub use sender::*;

/// Opens a multi-producer multi-consumer channel backed by a ring buffer.
///
/// The associated ring buffer can contain up to `capacity` pending messages.
///
/// Sending and receiving messages through this channel _never blocks_, however
/// pending messages may be overwritten if the internal ring buffer overflows.
///
/// # Panics
///
/// Panics if the `capacity` is `0`.
///
/// # Examples
///
/// ```rust
/// // TODO
/// ```
pub fn ring_channel<T>(capacity: usize) -> (RingSender<T>, RingReceiver<T>) {
    assert!(capacity > 0, "capacity must be greater than zero");
    let channel::RingChannel(left, right) = channel::RingChannel::new(capacity);
    (RingSender(left), RingReceiver(right))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic]
    fn ring_channel_panics_on_zero_capacity() {
        ring_channel::<()>(0);
    }
}
