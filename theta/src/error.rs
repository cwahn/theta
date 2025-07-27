pub trait NetworkError: std::error::Error + Clone + Send + Sync {}

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

impl<T> std::fmt::Display for SendError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::ClosedTx(_) => write!(f, "Sender closed"),
        }
    }
}

impl<T> std::fmt::Display for RequestError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RequestError::ClosedTx(_) => write!(f, "Sender closed"),
            RequestError::ClosedRx => write!(f, "Receiver closed"),
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
