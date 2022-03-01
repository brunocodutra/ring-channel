//! Bounded MPMC channel abstraction on top of a ring buffer.
//!
//! # Overview
//!
//! This crate provides a flavor of message passing that favors throughput over lossless
//! communication. Under the hood, [`ring_channel`] is just a thin abstraction layer on top of a
//! multi-producer multi-consumer lock-free ring-buffer. Sending messages never blocks, however
//! messages can be lost if the internal buffer overflows, as incoming messages gradually overwrite
//! older pending messages. This behavior is ideal for use-cases in which the consuming threads
//! only care about the most recent messages and applying back pressure to producer threads is
//! not desirable.
//!
//! * A classic example are video streamers that send frames across threads for display.
//! If the consuming thread experiences lag, the producing threads are at risk of overflowing the
//! communication buffer. Rather than panicking, it's often acceptable to skip frames with little
//! to no impact to the user experience.
//!
//! * Another example is a rendering GUI thread that runs at a fixed rate of frames per second and
//! receives the current state of the application through a channel for display. Only the most
//! up-to-date version of the application state matters at the point in time the GUI is refreshed,
//! so there's no use in keeping a backlog of all intermediary state transitions.
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
//! # #[cfg(all(feature = "std", feature = "futures_api"))]
//! assert_eq!(rx.recv(), Ok("Hello, world!"));
//! ```
//!
//! # Overflowing the buffer
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
//! // Since the buffer can hold at most one message in this case,
//! // sending a second message overwrites the former.
//! tx.send("Hello, universe!").unwrap();
//!
//! // Receive the message through the outbound endpoint.
//! # #[cfg(all(feature = "std", feature = "futures_api"))]
//! assert_eq!(rx.recv(), Ok("Hello, universe!"));
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
//! ```rust
//! # #[cfg(all(feature = "std", feature = "futures_api"))]
//! # fn doctest() -> Result<(), Box<dyn std::error::Error>> {
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
//!     while let Ok(msg) = rx2.recv() {
//!         if let Err(SendError::Disconnected(_)) = tx2.send(msg) {
//!             break;
//!         }
//!     }
//! });
//!
//! tx1.send("Hello, world!")?;
//!
//! if let Ok(msg) = rx1.recv() {
//!     // Depending on which thread goes first,
//!     // we might have received the direct message or the echo.
//!     assert_eq!(msg, "Hello, world!");
//! }
//! # Ok(())
//! # }
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
//! use std::num::NonZeroUsize;
//!
//! // Open the channel.
//! let (mut tx1, mut rx) = ring_channel(NonZeroUsize::new(3).unwrap());
//!
//! let mut tx2 = tx1.clone();
//! let mut tx3 = tx2.clone();
//!
//! tx1.send(1).unwrap();
//! tx2.send(2).unwrap();
//! tx3.send(3).unwrap();
//!
//! // All senders are dropped and the channel becomes disconnected.
//! drop((tx1, tx2, tx3));
//!
//! // Pending messages can still be received.
//! assert_eq!(rx.try_recv(), Ok(1));
//! assert_eq!(rx.try_recv(), Ok(2));
//! assert_eq!(rx.try_recv(), Ok(3));
//!
//! // Finally, the channel reports itself as disconnected.
//! assert_eq!(rx.try_recv(), Err(TryRecvError::Disconnected));
//! ```
//!
//! # Futures API
//!
//! By default, [`RingSender`] implements [`futures::sink::Sink`] and
//! [`RingReceiver`] implements [`futures::stream::Stream`].
//!
//! The cargo feature `futures_api` can be disabled to opt out of the dependency on [futures-rs].
//!
//! ```rust
//! # #[cfg(all(feature = "std", feature = "futures_api"))]
//! # #[tokio::main]
//! # async fn doctest() -> Result<(), Box<dyn std::error::Error>> {
//! use ring_channel::*;
//! use futures::{prelude::*, stream};
//! use std::num::NonZeroUsize;
//!
//! // Open the channel.
//! let (tx, rx) = ring_channel(NonZeroUsize::try_from(13)?);
//!
//! let message = &['H', 'e', 'l', 'l', 'o', ',', ' ', 'w', 'o', 'r', 'l', 'd', '!'];
//!
//! // Send the stream of characters through the Sink.
//! stream::iter(message).map(Ok).forward(tx).await?;
//!
//! // Collect the Stream into a String.
//! assert_eq!(&rx.collect::<String>().await, "Hello, world!");
//! # Ok(())
//! # }
//! ```
//!
//! # Optional Features
//!
//! * `std` (enabled by default)
//!
//!     Controls whether [crate `std`] is linked.
//!
//! * `futures_api` (enabled by default)
//!
//!     Enables integration with [futures-rs](https://crates.io/crates/futures),
//!     see [§ Futures API](index.html#futures-api).
//!
//! [crate `std`]: https://doc.rust-lang.org/std/
//! [futures-rs]: https://crates.io/crates/futures

#![no_std]

extern crate alloc;

#[cfg(any(feature = "std", test))]
#[cfg_attr(test, macro_use)]
extern crate std;

mod atomic;
mod buffer;
mod channel;
mod control;
mod error;

#[cfg(feature = "futures_api")]
mod waitlist;

pub use channel::*;
pub use error::*;

#[cfg(test)]
enum Void {}
