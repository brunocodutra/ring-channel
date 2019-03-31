use derivative::Derivative;
use std::{error, fmt};

/// An error that may be returned from [`RingSender::send`].
///
/// [`RingSender::send`]: struct.RingSender.html#method.send
#[derive(Derivative)]
#[derivative(Debug)]
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum SendError<T> {
    /// The channel is disconnected.
    Disconnected(#[derivative(Debug = "ignore")] T),
}

impl<T> fmt::Display for SendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        "sending on a disconnected channel".fmt(f)
    }
}

impl<T: Send> error::Error for SendError<T> {}

/// An error that may be returned from [`RingReceiver::recv`].
///
/// [`RingReceiver::recv`]: struct.RingReceiver.html#method.recv
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum RecvError {
    /// No messages pending in the internal buffer.
    Empty,

    /// No messages pending in the internal buffer and the channel is disconnected.
    Disconnected,
}

impl fmt::Display for RecvError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use RecvError::*;
        match self {
            Empty => "receiving on an empty channel".fmt(f),
            Disconnected => "receiving on an empty and disconnected channel".fmt(f),
        }
    }
}

impl error::Error for RecvError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_error_implements_error_trait() {
        let err: Box<dyn error::Error> = SendError::Disconnected("").into();
        assert_eq!(
            format!("{}", err),
            format!("{}", SendError::Disconnected(""))
        );
    }

    #[test]
    fn recv_error_implements_error_trait() {
        let err: Box<dyn error::Error> = RecvError::Disconnected.into();
        assert_eq!(format!("{}", err), format!("{}", RecvError::Disconnected));
    }
}
