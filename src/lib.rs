//! Bounded MPMC channel abstraction on top of a ring buffer.
//!
//! # Overview
//!
//! This crate provides a flavor of message passing that favors throughput over lossless
//! communication. Under the hood, [`ring_channel`] is just a thin abstraction layer on top of a
//! multi-producer multi-consumer lock-free ring-buffer. Sending nor receiving messages never
//! blocks, however messages can be lost if the internal buffer overflows, as incoming messages
//! gradually overwrite older pending messages. This behavior is ideal for use-cases in which the
//! consuming threads only care about the most up-to-date message and applying back pressure to
//! producer threads is not desirable.
//!
//! * One example is a rendering GUI thread that runs at a fixed rate of frames per second and
//! which receives the current state of the application through the channel for display.
//!
//! * Another example is video streamer that sends frames across threads for display.
//! If the consuming thread experiences lag, the producing threads are at risk of overflowing the
//! communication buffer. Rather than panicking, it's often acceptable to skip frames without
//! noticeable impact to the user experience.
//!
//! [`ring_channel`]: fn.ring_channel.html
//!
//! # Hello, world!
//!
//! ```rust
//! use ring_channel::*;
//! use std::num::NonZeroUsize;
//!
//! // Open the channel.
//! let (mut tx, mut rx) = ring_channel(NonZeroUsize::new(1).unwrap());
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
//! let (mut tx1, mut rx1) = ring_channel(NonZeroUsize::new(1).unwrap());
//!
//! let mut tx2 = tx1.clone();
//! let mut rx2 = rx1.clone();
//!
//! // Spawn a thread that echoes any message it receives.
//! thread::spawn(move || {
//!     if let Ok(msg) = rx2.recv() {
//!         tx2.send(msg).unwrap();
//!     }
//! });
//!
//! tx1.send("Hello, world!").unwrap();
//!
//! if let Ok(msg) = rx1.recv() {
//!     // Depending on which thread goes first,
//!     // we might have received the direct message or the echo.
//!     assert_eq!(msg, "Hello, world!");
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
//! let (mut tx1, mut rx) = ring_channel(NonZeroUsize::new(3).unwrap());
//!
//! let mut tx2 = tx1.clone();
//! let mut tx3 = tx2.clone();
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
//! // Finally, the channel reports itself as disconnected.
//! assert_eq!(rx.recv(), Err(RecvError::Disconnected));
//! ```
//!
//! # Experimental Features
//!
//! The following cargo feature flags are available:
//! * `futures_api` (depends on nightly Rust)
//!
//!     Provides experimental implementations of Sink for [`RingSender`] and
//!     Stream for [`RingReceiver`] based on [futures-rs].
//!
//!     ```rust
//!     # {
//!     #![cfg(feature = "futures_api")]
//!
//!     use ring_channel::*;
//!     use futures::{executor::*, prelude::*, stream};
//!     use std::{num::NonZeroUsize, thread};
//!
//!     // Open the channel.
//!     let (mut tx, mut rx) = ring_channel(NonZeroUsize::new(13).unwrap());
//!
//!     let message = &['H', 'e', 'l', 'l', 'o', ',', ' ', 'w', 'o', 'r', 'l', 'd', '!'];
//!
//!     thread::spawn(move || {
//!         // Send the stream of characters through the Sink.
//!         block_on(tx.send_all(&mut stream::iter(message).map(|msg| Ok(msg)))).unwrap();
//!     });
//!
//!     // Receive the stream of characters through the outbound endpoint.
//!     assert_eq!(&block_on_stream(rx).collect::<String>(), "Hello, world!");
//!     # }
//!     ```
//!
//! [`RingSender`]: struct.RingSender.html
//! [`RingReceiver`]: struct.RingReceiver.html
//! [futures-rs]: https://crates.io/crates/futures-preview

mod buffer;
mod channel;
mod control;
mod error;

#[cfg(feature = "futures_api")]
mod waitlist;

pub use channel::*;
pub use error::*;
