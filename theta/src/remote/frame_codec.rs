use bytes::BytesMut;
use serde::{Deserialize, Serialize};
use std::io;
use std::marker::PhantomData;
use tokio_util::codec::{Decoder, Encoder};

/// A minimal codec using postcard serialization with COBS encoding.
///
/// COBS encoding eliminates zero bytes from data and uses a trailing zero as delimiter,
/// making it ideal for self-delimiting message streams.
#[derive(Debug, Clone)]
pub struct PostcardCobsCodec<T>(PhantomData<T>);

impl<T> Default for PostcardCobsCodec<T> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<T: Serialize> Encoder<T> for PostcardCobsCodec<T> {
    type Error = io::Error;

    fn encode(&mut self, item: T, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let encoded = postcard::to_allocvec_cobs(&item)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        dst.reserve(encoded.len());
        dst.extend_from_slice(&encoded);
        Ok(())
    }
}

impl<T: for<'de> Deserialize<'de>> Decoder for PostcardCobsCodec<T> {
    type Item = T;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if let Some(zero_pos) = src.iter().position(|&b| b == 0) {
            // Extract frame and remove delimiter in-place
            let mut frame = src.split_to(zero_pos + 1);
            frame.truncate(zero_pos);

            // Decode COBS frame
            postcard::from_bytes_cobs(&mut frame)
                .map(Some)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
    struct TestMessage {
        id: u32,
        data: Vec<u8>,
        text: String,
    }

    #[tokio::test]
    async fn test_roundtrip() {
        let mut codec = PostcardCobsCodec::<TestMessage>::default();

        let message = TestMessage {
            id: 42,
            data: vec![0, 1, 0, 2, 0, 3], // Test COBS with zeros
            text: "Hello\0World".to_string(),
        };

        let mut buffer = BytesMut::new();
        codec.encode(message.clone(), &mut buffer).unwrap();

        // Verify COBS: only one zero at the end
        assert_eq!(buffer.iter().filter(|&&b| b == 0).count(), 1);
        assert_eq!(buffer[buffer.len() - 1], 0);

        let decoded = codec.decode(&mut buffer).unwrap().unwrap();
        assert_eq!(message, decoded);
        assert!(buffer.is_empty());
    }

    #[tokio::test]
    async fn test_multiple_messages() {
        let mut codec = PostcardCobsCodec::<TestMessage>::default();

        let messages = vec![
            TestMessage {
                id: 1,
                data: vec![0, 1],
                text: "First".to_string(),
            },
            TestMessage {
                id: 2,
                data: vec![2, 0],
                text: "Second".to_string(),
            },
            TestMessage {
                id: 3,
                data: vec![0, 0],
                text: "Third".to_string(),
            },
        ];

        let mut buffer = BytesMut::new();
        for msg in &messages {
            codec.encode(msg.clone(), &mut buffer).unwrap();
        }

        let mut decoded = Vec::new();
        while let Some(msg) = codec.decode(&mut buffer).unwrap() {
            decoded.push(msg);
        }

        assert_eq!(messages, decoded);
    }

    #[tokio::test]
    async fn test_partial_frame() {
        let mut codec = PostcardCobsCodec::<TestMessage>::default();

        let message = TestMessage {
            id: 99,
            data: vec![1, 2, 3],
            text: "Test".to_string(),
        };

        // Encode full message
        let mut full_buffer = BytesMut::new();
        codec.encode(message.clone(), &mut full_buffer).unwrap();

        // Test partial decode (without delimiter)
        let mut partial = BytesMut::new();
        partial.extend_from_slice(&full_buffer[..full_buffer.len() - 1]);

        assert!(codec.decode(&mut partial).unwrap().is_none());

        // Add delimiter
        partial.extend_from_slice(&[0]);
        let decoded = codec.decode(&mut partial).unwrap().unwrap();
        assert_eq!(message, decoded);
    }
}
