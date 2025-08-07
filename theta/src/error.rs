use core::{error::Error, fmt::Display};

pub trait NetworkError: Error + Clone + Send + Sync {}

pub enum ExitCode {
    Dropped,
    Terminated,
}

#[derive(Debug)]
pub enum SendError<T> {
    ClosedTx(T), // Sender closed
}

#[derive(Debug)]
pub enum RequestError<T> {
    ClosedTx(T), // Sender closed
    ClosedRx,    // Receiver closed
    DowncastFail,
    DeserializeFail,
    Timeout,
}

impl<T> Display for SendError<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SendError::ClosedTx(_) => write!(f, "Sender closed"),
        }
    }
}

impl<T> Display for RequestError<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RequestError::ClosedTx(_) => write!(f, "Sender closed"),
            RequestError::ClosedRx => write!(f, "Receiver closed"),
            RequestError::DowncastFail => write!(f, "Failed to downcast"),
            RequestError::DeserializeFail => write!(f, "Failed to deserialize"),
            RequestError::Timeout => write!(f, "Request timed out"),
        }
    }
}

impl<T> From<SendError<T>> for RequestError<T> {
    fn from(send_error: SendError<T>) -> Self {
        match send_error {
            SendError::ClosedTx(value) => RequestError::ClosedTx(value),
        }
    }
}
