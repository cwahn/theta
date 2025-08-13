// use std::fmt::Display;

// use thiserror::Error;

// use crate::remote::base::Tag;

// pub trait NetworkError: std::error::Error + Clone + Send + Sync {}

// pub enum ExitCode {
//     Dropped,
//     Terminated,
// }

// #[derive(Debug, Error)]
// pub enum SendError<T> {
//     #[error("Channel closed")]
//     TxClosed(T),
// }

// #[derive(Debug, Error)]
// pub enum BytesSendError {
//     #[error("Failed to deserialize: {0}")]
//     DeserializationFailed(#[from] postcard::Error),
//     #[error("Channel closed")]
//     TxClosed((Tag, Vec<u8>)),
// }

// #[derive(Debug, Error)]
// pub enum RequestError<T> {
//     #[error("Channel closed")]
//     TxClosed(T),
//     #[error("Channel closed")]
//     RxClosed,
//     #[error("Failed to downcast")]
//     DowncastError,
//     #[error("Failed to deserialize: {0}")]
//     DeserializationFailed(#[from] postcard::Error),
//     #[error("Request timed out")]
//     Timeout,
// }

