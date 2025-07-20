pub trait NetworkError: std::error::Error + Clone + Send + Sync {}

#[derive(Debug)]
pub enum SendError<T> {
    ClosedTx(T), // Sender closed
}

#[derive(Debug)]
pub enum RequestError<T> {
    ClosedTx(T), // Sender closed
    ClosedRx,    // Receiver closed
    Timeout,
}

impl<T> From<SendError<T>> for RequestError<T> {
    fn from(send_error: SendError<T>) -> Self {
        match send_error {
            SendError::ClosedTx(value) => RequestError::ClosedTx(value),
        }
    }
}
