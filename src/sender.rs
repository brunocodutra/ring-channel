use crate::{channel::*, error::*};
use derivative::Derivative;

/// The sending end of a [`ring_channel`].
///
/// [`ring_channel`]: fn.ring_channel.html
#[derive(Derivative)]
#[derivative(Debug(bound = ""), Clone(bound = ""))]
pub struct RingSender<T>(#[derivative(Debug = "ignore")] pub Endpoint<T>);

impl<T> RingSender<T> {
    /// Sends a message through the channel without blocking.
    ///
    /// * If the channel is not disconnected, the message is pushed into the internal ring buffer.
    ///     * If the internal ring buffer is full, the oldest pending message is overwritten.
    /// * If the channel is disconnected, [`SendError::Disconnected`] is returned.
    ///
    /// [`SendError::Disconnected`]: enum.SendError.html#variant.Disconnected
    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        self.0.send(value)
    }
}
