// use bytes::BytesMut;
// use serde::{Deserialize, Serialize};
// use std::io;
// use std::marker::PhantomData;
// use tokio_util::codec::{Decoder, Encoder};

// /// A codec that uses postcard serialization with COBS (Consistent Overhead Byte Stuffing) encoding.
// ///
// /// COBS encoding ensures that the serialized data contains no zero bytes, and uses a trailing
// /// zero byte as a frame delimiter. This makes it perfect for protocols where zero bytes have
// /// special meaning or for self-delimiting message streams.
// #[derive(Debug, Clone)]
// pub struct PostcardCobsCodec<T> {
//     max_frame_size: usize,
//     _phantom: PhantomData<T>,
// }

// impl<T> PostcardCobsCodec<T> {
//     /// Create a new codec with default maximum frame size (1MB)
//     pub fn new() -> Self {
//         Self {
//             max_frame_size: 1024 * 1024, // 1MB default
//             _phantom: PhantomData,
//         }
//     }

//     /// Create a new codec with a custom maximum frame size
//     pub fn with_max_frame_size(max_size: usize) -> Self {
//         Self {
//             max_frame_size: max_size,
//             _phantom: PhantomData,
//         }
//     }
// }

// impl<T> Default for PostcardCobsCodec<T> {
//     fn default() -> Self {
//         Self::new()
//     }
// }

// impl<T> Encoder<T> for PostcardCobsCodec<T>
// where
//     T: Serialize,
// {
//     type Error = io::Error;

//     fn encode(&mut self, item: T, dst: &mut BytesMut) -> Result<(), Self::Error> {
//         // Serialize and COBS encode in one stepds the zero delimiter
//         let encoded = postcard::to_allocvec_cobs(&item).map_err(|e| {
//             io::Error::new(
//                 io::ErrorKind::InvalidData,
//                 format!("Postcard encoding error: {}", e),
//             )
//         })?;

//         // Check if the encoded message exceeds our maximum frame size
//         if encoded.len() > self.max_frame_size {
//             return Err(io::Error::new(
//                 io::ErrorKind::InvalidData,
//                 format!(
//                     "Encoded frame size {} exceeds maximum {}",
//                     encoded.len(),
//                     self.max_frame_size
//                 ),
//             ));
//         }

//         // Reserve space and write the encoded data (includes trailing zero)
//         dst.reserve(encoded.len());
//         dst.extend_from_slice(&encoded);

//         Ok(())
//     }
// }

// impl<T> Decoder for PostcardCobsCodec<T>
// where
//     T: for<'de> Deserialize<'de>,
// {
//     type Item = T;
//     type Error = io::Error;

//     fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
//         // Look for COBS delimiter (0x00 byte)
//         if let Some(zero_pos) = src.iter().position(|&b| b == 0) {
//             // Check frame size before processing
//             if zero_pos > self.max_frame_size {
//                 return Err(io::Error::new(
//                     io::ErrorKind::InvalidData,
//                     format!(
//                         "Frame size {} exceeds maximum {}",
//                         zero_pos, self.max_frame_size
//                     ),
//                 ));
//             }

//             // Found a complete COBS frame, extract it including the zero delimiter
//             let frame_with_delimiter = src.split_to(zero_pos + 1);

//             // Decode the COBS frame (excluding the trailing zero)
//             // postcard::from_bytes_cobs requires a mutable slice
//             let mut frame_data = frame_with_delimiter[..zero_pos].to_vec();
//             let item = postcard::from_bytes_cobs(&mut frame_data).map_err(|e| {
//                 io::Error::new(
//                     io::ErrorKind::InvalidData,
//                     format!("Postcard decoding error: {}", e),
//                 )
//             })?;

//             Ok(Some(item))
//         } else {
//             // No complete frame yet, check if we're getting too much data without a delimiter
//             if src.len() > self.max_frame_size {
//                 return Err(io::Error::new(
//                     io::ErrorKind::InvalidData,
//                     format!(
//                         "Buffer size {} exceeds maximum frame size {} without finding delimiter",
//                         src.len(),
//                         self.max_frame_size
//                     ),
//                 ));
//             }
//             Ok(None)
//         }
//     }
// }

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use futures::{SinkExt, StreamExt};
//     use serde::{Deserialize, Serialize};
//     use std::pin::Pin;
//     use std::task::{Context, Poll};
//     use tokio::io::{AsyncRead, AsyncWrite};
//     use tokio_util::codec::{FramedRead, FramedWrite};

//     // Test data structures
//     #[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
//     struct SimpleMessage {
//         id: u32,
//         content: String,
//     }

//     #[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
//     struct ComplexMessage {
//         timestamp: u64,
//         payload: Vec<u8>,
//         metadata: std::collections::HashMap<String, String>,
//         flags: [bool; 8],
//     }

//     #[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
//     struct MessageWithZeros {
//         data: Vec<u8>, // This will contain zero bytes to test COBS
//         text: String,
//     }

//     // Mock bidirectional pipe for testing
//     struct MockPipe {
//         read_buf: BytesMut,
//         write_buf: BytesMut,
//         read_pos: usize,
//     }

//     impl MockPipe {
//         fn new() -> (Self, Self) {
//             let pipe1 = MockPipe {
//                 read_buf: BytesMut::new(),
//                 write_buf: BytesMut::new(),
//                 read_pos: 0,
//             };
//             let pipe2 = MockPipe {
//                 read_buf: BytesMut::new(),
//                 write_buf: BytesMut::new(),
//                 read_pos: 0,
//             };
//             (pipe1, pipe2)
//         }

//         fn connect(&mut self, other: &mut MockPipe) {
//             // Transfer written data to the other pipe's read buffer
//             if !self.write_buf.is_empty() {
//                 other.read_buf.extend_from_slice(&self.write_buf);
//                 self.write_buf.clear();
//             }
//             if !other.write_buf.is_empty() {
//                 self.read_buf.extend_from_slice(&other.write_buf);
//                 other.write_buf.clear();
//             }
//         }
//     }

//     impl AsyncRead for MockPipe {
//         fn poll_read(
//             mut self: Pin<&mut Self>,
//             _cx: &mut Context<'_>,
//             buf: &mut tokio::io::ReadBuf<'_>,
//         ) -> Poll<io::Result<()>> {
//             let available = self.read_buf.len() - self.read_pos;
//             if available == 0 {
//                 return Poll::Ready(Ok(())); // EOF
//             }

//             let to_read = std::cmp::min(buf.remaining(), available);
//             let data = &self.read_buf[self.read_pos..self.read_pos + to_read];
//             buf.put_slice(data);
//             self.read_pos += to_read;

//             Poll::Ready(Ok(()))
//         }
//     }

//     impl AsyncWrite for MockPipe {
//         fn poll_write(
//             mut self: Pin<&mut Self>,
//             _cx: &mut Context<'_>,
//             buf: &[u8],
//         ) -> Poll<Result<usize, io::Error>> {
//             self.write_buf.extend_from_slice(buf);
//             Poll::Ready(Ok(buf.len()))
//         }

//         fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
//             Poll::Ready(Ok(()))
//         }

//         fn poll_shutdown(
//             self: Pin<&mut Self>,
//             _cx: &mut Context<'_>,
//         ) -> Poll<Result<(), io::Error>> {
//             Poll::Ready(Ok(()))
//         }
//     }

//     #[tokio::test]
//     async fn test_simple_roundtrip() {
//         let mut codec = PostcardCobsCodec::<SimpleMessage>::new();

//         let message = SimpleMessage {
//             id: 42,
//             content: "Hello, World!".to_string(),
//         };

//         // Encode
//         let mut buffer = BytesMut::new();
//         codec.encode(message.clone(), &mut buffer).unwrap();

//         // Decode
//         let decoded = codec.decode(&mut buffer).unwrap().unwrap();

//         assert_eq!(message, decoded);
//         assert!(buffer.is_empty()); // Buffer should be fully consumed
//     }

//     #[tokio::test]
//     async fn test_complex_message_roundtrip() {
//         let mut codec = PostcardCobsCodec::<ComplexMessage>::new();

//         let mut metadata = std::collections::HashMap::new();
//         metadata.insert("key1".to_string(), "value1".to_string());
//         metadata.insert("key2".to_string(), "value2".to_string());

//         let message = ComplexMessage {
//             timestamp: 1234567890,
//             payload: vec![1, 2, 3, 4, 5, 255, 128, 0, 1], // Mix of values including zero
//             metadata,
//             flags: [true, false, true, false, true, false, true, false],
//         };

//         let mut buffer = BytesMut::new();
//         codec.encode(message.clone(), &mut buffer).unwrap();

//         let decoded = codec.decode(&mut buffer).unwrap().unwrap();

//         assert_eq!(message, decoded);
//     }

//     #[tokio::test]
//     async fn test_message_with_zeros() {
//         let mut codec = PostcardCobsCodec::<MessageWithZeros>::new();

//         let message = MessageWithZeros {
//             data: vec![0, 1, 0, 2, 0, 3, 0], // Lots of zeros - perfect test for COBS
//             text: "Text with\0embedded\0zeros".to_string(),
//         };

//         let mut buffer = BytesMut::new();
//         codec.encode(message.clone(), &mut buffer).unwrap();

//         // Verify that the encoded data contains no zeros except the delimiter
//         let frame_data = &buffer[..buffer.len() - 1]; // Exclude delimiter
//         assert!(
//             !frame_data.contains(&0),
//             "COBS encoding should remove all zeros from frame data"
//         );

//         let decoded = codec.decode(&mut buffer).unwrap().unwrap();

//         assert_eq!(message, decoded);
//     }

//     #[tokio::test]
//     async fn test_multiple_messages_in_buffer() {
//         let mut codec = PostcardCobsCodec::<SimpleMessage>::new();

//         let messages = vec![
//             SimpleMessage {
//                 id: 1,
//                 content: "First message".to_string(),
//             },
//             SimpleMessage {
//                 id: 2,
//                 content: "Second message".to_string(),
//             },
//             SimpleMessage {
//                 id: 3,
//                 content: "Third message".to_string(),
//             },
//         ];

//         // Encode all messages into the same buffer
//         let mut buffer = BytesMut::new();
//         for msg in &messages {
//             codec.encode(msg.clone(), &mut buffer).unwrap();
//         }

//         // Decode all messages
//         let mut decoded_messages = Vec::new();

//         while let Some(msg) = codec.decode(&mut buffer).unwrap() {
//             decoded_messages.push(msg);
//         }

//         assert_eq!(messages, decoded_messages);
//         assert!(buffer.is_empty());
//     }

//     #[tokio::test]
//     async fn test_partial_frame_handling() {
//         let mut codec = PostcardCobsCodec::<SimpleMessage>::new();

//         let message = SimpleMessage {
//             id: 99,
//             content: "Partial test".to_string(),
//         };

//         // Encode the full message
//         let mut full_buffer = BytesMut::new();
//         codec.encode(message.clone(), &mut full_buffer).unwrap();

//         // Test partial decoding
//         let full_data = full_buffer.freeze();

//         // Try decoding with only part of the data (no delimiter yet)
//         let mut partial_buffer = BytesMut::new();
//         partial_buffer.extend_from_slice(&full_data[..full_data.len() - 1]); // Exclude delimiter

//         let result = codec.decode(&mut partial_buffer).unwrap();
//         assert!(result.is_none(), "Should return None for incomplete frame");

//         // Add the delimiter
//         partial_buffer.extend_from_slice(&full_data[full_data.len() - 1..]);

//         let decoded = codec.decode(&mut partial_buffer).unwrap().unwrap();
//         assert_eq!(message, decoded);
//     }

//     #[tokio::test]
//     async fn test_framed_stream_roundtrip() {
//         let (mut pipe1, mut pipe2) = MockPipe::new();

//         let messages = vec![
//             SimpleMessage {
//                 id: 1,
//                 content: "Message 1".to_string(),
//             },
//             SimpleMessage {
//                 id: 2,
//                 content: "Message 2".to_string(),
//             },
//             SimpleMessage {
//                 id: 3,
//                 content: "Message 3".to_string(),
//             },
//         ];

//         // Send all messages first
//         {
//             let mut writer =
//                 FramedWrite::new(&mut pipe1, PostcardCobsCodec::<SimpleMessage>::new());
//             for msg in &messages {
//                 writer.send(msg.clone()).await.unwrap();
//             }
//         } // writer is dropped here, releasing the borrow on pipe1

//         // Connect the pipes to transfer data
//         pipe1.connect(&mut pipe2);

//         // Create reader and read all messages
//         let mut reader = FramedRead::new(&mut pipe2, PostcardCobsCodec::<SimpleMessage>::new());
//         let mut received_messages = Vec::new();
//         for _ in 0..messages.len() {
//             if let Some(msg) = reader.next().await {
//                 received_messages.push(msg.unwrap());
//             }
//         }

//         assert_eq!(messages, received_messages);
//     }

//     #[tokio::test]
//     async fn test_large_message() {
//         let mut codec = PostcardCobsCodec::<MessageWithZeros>::new();

//         // Create a large message
//         let large_data = vec![123u8; 100_000]; // 100KB of data
//         let message = MessageWithZeros {
//             data: large_data.clone(),
//             text: "Large message test".to_string(),
//         };

//         let mut buffer = BytesMut::new();
//         codec.encode(message.clone(), &mut buffer).unwrap();

//         let decoded = codec.decode(&mut buffer).unwrap().unwrap();

//         assert_eq!(message, decoded);
//     }

//     #[tokio::test]
//     async fn test_max_frame_size_limit() {
//         let mut codec = PostcardCobsCodec::<MessageWithZeros>::with_max_frame_size(1000); // 1KB limit

//         // Create a message that will exceed the limit when encoded
//         let large_data = vec![42u8; 2000]; // 2KB of data
//         let message = MessageWithZeros {
//             data: large_data,
//             text: "Too large".to_string(),
//         };

//         let mut buffer = BytesMut::new();

//         // Should fail to encode due to size limit
//         let result = codec.encode(message, &mut buffer);
//         assert!(result.is_err());

//         let error = result.unwrap_err();
//         assert_eq!(error.kind(), io::ErrorKind::InvalidData);
//         assert!(error.to_string().contains("exceeds maximum"));
//     }

//     #[tokio::test]
//     async fn test_decode_buffer_size_limit() {
//         let mut codec = PostcardCobsCodec::<SimpleMessage>::with_max_frame_size(100);

//         // Create a buffer with too much data and no delimiter
//         let mut buffer = BytesMut::new();
//         buffer.extend_from_slice(&vec![1u8; 200]); // 200 bytes, no zero delimiter

//         let result = codec.decode(&mut buffer);

//         assert!(result.is_err());
//         let error = result.unwrap_err();
//         assert_eq!(error.kind(), io::ErrorKind::InvalidData);
//         assert!(error.to_string().contains("exceeds maximum frame size"));
//     }

//     #[tokio::test]
//     async fn test_empty_message() {
//         let mut codec = PostcardCobsCodec::<EmptyMessage>::new();

//         #[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
//         struct EmptyMessage;

//         let message = EmptyMessage;

//         let mut buffer = BytesMut::new();
//         codec.encode(message.clone(), &mut buffer).unwrap();

//         let decoded = codec.decode(&mut buffer).unwrap().unwrap();

//         assert_eq!(message, decoded);
//     }

//     #[tokio::test]
//     async fn test_series_of_different_message_types() {
//         // Test encoding/decoding different message types in sequence
//         // This demonstrates how you'd typically use different codecs for different types

//         let simple_msg = SimpleMessage {
//             id: 1,
//             content: "Simple".to_string(),
//         };

//         let zero_msg = MessageWithZeros {
//             data: vec![0, 1, 0, 2, 0],
//             text: "Zeros".to_string(),
//         };

//         // Encode both message types with their respective codecs
//         let mut simple_codec = PostcardCobsCodec::<SimpleMessage>::new();
//         let mut zero_codec = PostcardCobsCodec::<MessageWithZeros>::new();

//         let mut buffer1 = BytesMut::new();
//         let mut buffer2 = BytesMut::new();

//         simple_codec
//             .encode(simple_msg.clone(), &mut buffer1)
//             .unwrap();
//         zero_codec.encode(zero_msg.clone(), &mut buffer2).unwrap();

//         // Decode with their respective codecs
//         let decoded_simple = simple_codec.decode(&mut buffer1).unwrap().unwrap();
//         let decoded_zeros = zero_codec.decode(&mut buffer2).unwrap().unwrap();

//         assert_eq!(simple_msg, decoded_simple);
//         assert_eq!(zero_msg, decoded_zeros);
//     }

//     #[tokio::test]
//     async fn test_cobs_encoding_verification() {
//         let mut codec = PostcardCobsCodec::<MessageWithZeros>::new();

//         // Create a message with many zero bytes
//         let message = MessageWithZeros {
//             data: vec![0, 0, 0, 1, 0, 0, 2, 0, 0, 0], // Many zeros
//             text: "Zero\0test\0string".to_string(),   // Zeros in string too
//         };

//         let mut buffer = BytesMut::new();
//         codec.encode(message.clone(), &mut buffer).unwrap();

//         // The encoded buffer should have exactly one zero byte (the delimiter) at the end
//         let zero_count = buffer.iter().filter(|&&b| b == 0).count();
//         assert_eq!(
//             zero_count, 1,
//             "COBS encoded data should have exactly one zero byte (delimiter)"
//         );

//         // The zero should be at the end
//         assert_eq!(
//             buffer[buffer.len() - 1],
//             0,
//             "Zero delimiter should be at the end"
//         );

//         // Everything before the delimiter should be non-zero
//         for &byte in &buffer[..buffer.len() - 1] {
//             assert_ne!(byte, 0, "No zero bytes should exist before the delimiter");
//         }

//         // Verify roundtrip still works
//         let decoded = codec.decode(&mut buffer).unwrap().unwrap();
//         assert_eq!(message, decoded);
//     }
// }
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
