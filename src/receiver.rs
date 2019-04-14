use crate::{channel::*, error::*};
use derivative::Derivative;

/// The receiving end of a [`ring_channel`].
///
/// [`ring_channel`]: fn.ring_channel.html
#[derive(Derivative)]
#[derivative(Debug(bound = ""), Clone(bound = ""))]
pub struct RingReceiver<T>(#[derivative(Debug = "ignore")] pub Endpoint<T>);

impl<T> RingReceiver<T> {
    /// Receives a message through the channel without blocking.
    ///
    /// * If the internal ring buffer isn't empty, the oldest pending message is returned.
    /// * If the internal ring buffer is empty, [`RecvError::Empty`] is returned.
    /// * If the channel is disconnected and the internal ring buffer is empty,
    /// [`RecvError::Disconnected`] is returned.
    ///
    /// [`RecvError::Empty`]: enum.RecvError.html#variant.Empty
    /// [`RecvError::Disconnected`]: enum.RecvError.html#variant.Disconnected
    pub fn recv(&self) -> Result<T, RecvError> {
        self.0.recv()
    }
}
