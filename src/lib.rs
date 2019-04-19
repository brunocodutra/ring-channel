//! Never blocking, bounded MPMC channel abstraction on top of a ring buffer.
//!
//! # Overview
//!
//! This crate provides a flavor of message passing that favors throughput over lossless
//! communication. Under the hood, [`ring_channel`] is just a thin abstraction layer on top of a
//! [MPMC lock-free ring-buffer][ring-buffer]. Neither sending nor receiving messages ever block,
//! however messages can be lost if the internal buffer overflows, as incoming messages gradually
//! overwrite older pending messages. This behavior is ideal for use-cases in which the consuming
//! threads only care about the most up-to-date message.
//!
//! * One example is a rendering GUI thread that runs at a fixed rate of frames per second and
//! which recives the current state of the application through the channel for display.
//!
//! * Another example is video streamer that sends frames across threads for display.
//! If the consuming thread experiences lag, the producing threads are at risk of overflowing the
//! communication buffer. Rather than panicking, it's often acceptable to skip frames without
//! noticeable impact to the user experience.
//!
//! [`ring_channel`]: fn.ring_channel.html
//! [ring-buffer]: https://github.com/brunocodutra/ring-channel/blob/master/src/buffer.rs
//!
//! # Hello, world!
//!
//! ```rust
//! use ring_channel::*;
//! use std::num::NonZeroUsize;
//!
//! // Open the channel.
//! let (tx, rx) = ring_channel(NonZeroUsize::new(1).unwrap());
//!
//! // Send a message through the inbound endpoint.
//! tx.send("Hello, world!").unwrap();
//!
//! // Receive the message through the outbound endpoint.
//! assert_eq!(rx.recv(), Ok("Hello, world!"));
//! ```
//!
//! # Communicating across threads
//!
//! Endpoints are just handles that may be cloned and sent to other threads.
//! They come in two flavors that allow sending and receiving messages through the channel,
//! respectively [`RingSender`] and [`RingReceiver`]. Cloning an endpoint produces a new
//! handle of the same kind associated with the same channel.
//! The channel lives as long as there is an endpoint associated with it.
//!
//! [`RingSender`]: struct.RingSender.html
//! [`RingReceiver`]: struct.RingReceiver.html
//!
//! ```rust
//! use ring_channel::*;
//! use std::{num::NonZeroUsize, thread};
//!
//! // Open the channel.
//! let (tx1, rx1) = ring_channel(NonZeroUsize::new(1).unwrap());
//!
//! let tx2 = tx1.clone();
//! let rx2 = rx1.clone();
//!
//! // Spawn a thread that echoes any message it receives.
//! thread::spawn(move || loop {
//!     if let Ok(msg) = rx2.recv() {
//!         tx2.send(msg).unwrap();
//!     }
//! });
//!
//! tx1.send("Hello, world!").unwrap();
//!
//! loop {
//!     if let Ok(msg) = rx1.recv() {
//!         // Depending on which thread goes first,
//!         // we might have received the direct message or the echo.
//!         break assert_eq!(msg, "Hello, world!");
//!     }
//! }
//! ```
//!
//! # Disconnection
//!
//! When all endpoints of one type get dropped, the channel becomes disconnected.
//! Attempting to send an message through a disconnected channel returns an error.
//! Receiving messages through a disconnected channel succeeds as long as there are
//! pending messages and an error is returned once all of them have been received.
//!
//! ```rust
//! use ring_channel::*;
//! use std::{num::NonZeroUsize, thread};
//!
//! let (tx1, rx) = ring_channel(NonZeroUsize::new(3).unwrap());
//!
//! let tx2 = tx1.clone();
//! let tx3 = tx2.clone();
//!
//! tx1.send(1).unwrap();
//! tx2.send(2).unwrap();
//! tx3.send(3).unwrap();
//!
//! // All senders are dropped, so the channel becomes disconnected.
//! drop((tx1, tx2, tx3));
//!
//! // Pending messages can still be received.
//! assert_eq!(rx.recv(), Ok(1));
//! assert_eq!(rx.recv(), Ok(2));
//! assert_eq!(rx.recv(), Ok(3));
//!
//! // Finally, the channel reports itself as disconnectd.
//! assert_eq!(rx.recv(), Err(RecvError::Disconnected));
//! ```

mod buffer;
mod channel;
mod error;
mod macros;
mod receiver;
mod sender;

pub use error::*;
pub use receiver::*;
pub use sender::*;

use std::num::NonZeroUsize;

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
/// use std::num::NonZeroUsize;
/// use std::thread;
/// use std::time::{Duration, Instant};
///
/// fn main() {
///     // Open a channel to transmit the time elapsed since the beginning of the countdown.
///     // We only need a buffer of size 1, since we're only interested in the current value.
///     let (tx, rx) = ring_channel(NonZeroUsize::new(1).unwrap());
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
pub fn ring_channel<T>(capacity: NonZeroUsize) -> (RingSender<T>, RingReceiver<T>) {
    let channel::RingChannel(left, right) = channel::RingChannel::new(capacity.get());
    (RingSender(left), RingReceiver(right))
}
