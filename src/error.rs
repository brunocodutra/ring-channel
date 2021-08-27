use core::fmt;
use derivative::Derivative;

#[cfg(test)]
use test_strategy::Arbitrary;

/// An error that may be returned by [`RingSender::send`].
///
/// [`RingSender::send`]: struct.RingSender.html#method.send
#[derive(Derivative)]
#[derivative(Debug)]
#[derive(Copy, Clone, Eq, PartialEq)]
#[cfg_attr(test, derive(Arbitrary))]
pub enum SendError<T> {
    /// The channel is disconnected.
    Disconnected(#[derivative(Debug = "ignore")] T),
}

impl<T> fmt::Display for SendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        "sending on a disconnected channel".fmt(f)
    }
}

/// Requires [feature] `"std"`.
///
/// [feature]: index.html#optional-features
#[cfg(feature = "std")]
impl<T: Send> std::error::Error for SendError<T> {}

/// An error that may be returned by [`RingReceiver::recv`].
///
/// [`RingReceiver::recv`]: struct.RingReceiver.html#method.recv
#[cfg(feature = "futures_api")]
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
#[cfg_attr(test, derive(Arbitrary))]
pub enum RecvError {
    /// No messages pending in the internal buffer and the channel is disconnected.
    Disconnected,
}

#[cfg(feature = "futures_api")]
impl fmt::Display for RecvError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use RecvError::*;
        match self {
            Disconnected => "receiving on an empty and disconnected channel".fmt(f),
        }
    }
}

/// Requires [feature] `"std"`.
///
/// [feature]: index.html#optional-features
#[cfg(all(feature = "std", feature = "futures_api"))]
impl std::error::Error for RecvError {}

/// An error that may be returned by [`RingReceiver::try_recv`].
///
/// [`RingReceiver::try_recv`]: struct.RingReceiver.html#method.try_recv
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
#[cfg_attr(test, derive(Arbitrary))]
pub enum TryRecvError {
    /// No messages pending in the internal buffer.
    Empty,

    /// No messages pending in the internal buffer and the channel is disconnected.
    Disconnected,
}

impl fmt::Display for TryRecvError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use TryRecvError::*;
        match self {
            Empty => "receiving on an empty channel".fmt(f),
            Disconnected => "receiving on an empty and disconnected channel".fmt(f),
        }
    }
}

/// Requires [feature] `"std"`.
///
/// [feature]: index.html#optional-features
#[cfg(feature = "std")]
impl std::error::Error for TryRecvError {}

#[cfg(all(feature = "std", test))]
mod tests {
    use super::*;
    use alloc::boxed::Box;
    use std::error::Error;
    use test_strategy::proptest;

    #[proptest]
    fn send_error_implements_error_trait(err: SendError<()>) {
        assert_eq!(
            format!("{}", err),
            format!("{}", Box::<dyn Error>::from(err))
        );
    }

    #[cfg(feature = "futures_api")]
    #[proptest]
    fn recv_error_implements_error_trait(err: RecvError) {
        assert_eq!(
            format!("{}", err),
            format!("{}", Box::<dyn Error>::from(err))
        );
    }

    #[proptest]
    fn try_recv_error_implements_error_trait(err: TryRecvError) {
        assert_eq!(
            format!("{}", err),
            format!("{}", Box::<dyn Error>::from(err))
        );
    }
}
