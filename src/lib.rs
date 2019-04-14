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
/// ```rust,no_run
/// use ring_channel::*;
/// use std::thread;
/// use std::time::{Duration, Instant};
///
/// fn main() {
///     // Open a channel to transmit the time elapsed since the beginning of the countdown.
///     // We only need a buffer of size 1, since we're only interested in the current value.
///     let (tx, rx) = ring_channel(1);
///
///     thread::spawn(move || {
///         let countdown = Instant::now() + Duration::from_secs(10);
///
///         // Update the channel with the time elapsed so far.
///         while let Ok(_) = tx.send(countdown - Instant::now()) {
///
///             // We only need millisecond precision.
///             thread::sleep(Duration::from_millis(1));
///
///             if Instant::now() > countdown {
///                 break;
///             }
///         }
///     });
///
///     loop {
///         match rx.recv() {
///             // Print the current time elapsed.
///             Ok(timer) => {
///                 print!("\r{:02}.{:03}", timer.as_secs(), timer.as_millis() % 1000);
///
///                 if timer <= Duration::from_millis(6600) {
///                     print!(" - Main engine start                           ");
///                 } else {
///                     print!(" - Activate main engine hydrogen burnoff system");
///                 }
///             }
///             Err(RecvError::Empty) => thread::yield_now(),
///             Err(RecvError::Disconnected) => break,
///         }
///     }
///
///     println!("\r00.0000 - Solid rocket booster ignition and liftoff!")
/// }
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
