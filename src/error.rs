pub trait NetworkError: std::error::Error + Clone + Send + Sync {}

pub enum MsgError<T> {
    LocalClosed(T),   // Local mpsc channel is closed
    RemoteClosed(T),  // Failed to put message on network
    RemoteAckTimeout, // Failed to get ack from remote side
    NotApplicable,    // Message can't be interpreted by target (Usually type mismatch)

                      // From target's error it is not sender's business
}
