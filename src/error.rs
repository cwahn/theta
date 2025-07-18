pub trait NetworkError: std::error::Error + Clone + Send + Sync {}

// pub enum MsgError<T> {
//     LocalClosed(T),   // Local mpsc channel is closed
//     RemoteClosed(T),  // Failed to put message on network
//     RemoteAckTimeout, // Failed to get ack from remote side
//     NotApplicable,    // Message can't be interpreted by target (Usually type mismatch)

//                       // From target's error it is not sender's business
// }

pub enum SendError<T> {
    ClosedTx(T), // Sender closed
}

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
