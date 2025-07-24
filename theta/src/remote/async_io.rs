use postcard::{Error, Result};
use serde::Deserialize;
use serde::de::{self, DeserializeSeed, Visitor};
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, ReadBuf};

/// SlidingBuffer implementation (copied from postcard internals)
struct SlidingBuffer<'de> {
    cursor: *mut u8,
    end: *const u8,
    _pl: PhantomData<&'de [u8]>,
}

impl<'de> SlidingBuffer<'de> {
    pub fn new(sli: &'de mut [u8]) -> Self {
        let range = sli.as_mut_ptr_range();
        Self {
            cursor: range.start,
            end: range.end,
            _pl: PhantomData,
        }
    }

    #[inline]
    fn take_n(&mut self, ct: usize) -> Result<&'de mut [u8]> {
        let remain = (self.end as usize) - (self.cursor as usize);
        let buff = if remain < ct {
            return Err(Error::DeserializeUnexpectedEnd);
        } else {
            unsafe {
                let sli = core::slice::from_raw_parts_mut(self.cursor, ct);
                self.cursor = self.cursor.add(ct);
                sli
            }
        };
        Ok(buff)
    }

    #[inline]
    fn take_n_temp(&mut self, ct: usize) -> Result<&mut [u8]> {
        let remain = (self.end as usize) - (self.cursor as usize);
        let buff = if remain < ct {
            return Err(Error::DeserializeUnexpectedEnd);
        } else {
            unsafe {
                let sli = core::slice::from_raw_parts_mut(self.cursor, ct);
                sli
            }
        };
        Ok(buff)
    }

    fn complete(self) -> Result<&'de mut [u8]> {
        let remain = (self.end as usize) - (self.cursor as usize);
        unsafe { Ok(core::slice::from_raw_parts_mut(self.cursor, remain)) }
    }
}

/// Async version of postcard's Flavor trait
pub trait AsyncFlavor<'de> {
    type Remainder;
    type Source;

    async fn pop(&mut self) -> Result<u8>;
    fn size_hint(&self) -> Option<usize>;
    async fn try_take_n(&mut self, ct: usize) -> Result<&'de [u8]>;
    async fn try_take_n_temp<'a>(&'a mut self, ct: usize) -> Result<&'a [u8]>
    where
        'de: 'a;
    fn finalize(self) -> Result<Self::Remainder>;
}

/// AsyncIO Reader flavor for postcard deserialization
pub struct AsyncIOReader<'de, T>
where
    T: AsyncRead + Unpin,
{
    reader: T,
    buff: SlidingBuffer<'de>,
}

impl<'de, T> AsyncIOReader<'de, T>
where
    T: AsyncRead + Unpin,
{
    /// Create a new [`AsyncIOReader`] from an async reader and a buffer.
    pub fn new(reader: T, buff: &'de mut [u8]) -> Self {
        Self {
            reader,
            buff: SlidingBuffer::new(buff),
        }
    }

    /// Read exactly `buf.len()` bytes into `buf`
    async fn read_exact_async(&mut self, mut buf: &mut [u8]) -> Result<()> {
        while !buf.is_empty() {
            let mut read_buf = ReadBuf::new(buf);
            let filled_before = read_buf.filled().len();

            std::future::poll_fn(|cx| {
                let mut pinned = Pin::new(&mut self.reader);
                pinned.as_mut().poll_read(cx, &mut read_buf)
            })
            .await
            .map_err(|_| Error::DeserializeUnexpectedEnd)?;

            let filled_after = read_buf.filled().len();
            if filled_after == filled_before {
                return Err(Error::DeserializeUnexpectedEnd);
            }

            let bytes_read = filled_after - filled_before;
            buf = &mut buf[bytes_read..];
        }
        Ok(())
    }
}

impl<'de, T> AsyncFlavor<'de> for AsyncIOReader<'de, T>
where
    T: AsyncRead + Unpin + 'de,
{
    type Remainder = (T, &'de mut [u8]);
    type Source = &'de [u8];

    async fn pop(&mut self) -> Result<u8> {
        let mut val = [0; 1];
        self.read_exact_async(&mut val).await?;
        Ok(val[0])
    }

    fn size_hint(&self) -> Option<usize> {
        None
    }

    async fn try_take_n(&mut self, ct: usize) -> Result<&'de [u8]> {
        let buff = self.buff.take_n(ct)?;
        self.read_exact_async(buff).await?;
        Ok(buff)
    }

    async fn try_take_n_temp<'a>(&'a mut self, ct: usize) -> Result<&'a [u8]>
    where
        'de: 'a,
    {
        let buff = self.buff.take_n_temp(ct)?;
        self.read_exact_async(buff).await?;
        Ok(buff)
    }

    fn finalize(self) -> Result<(T, &'de mut [u8])> {
        let buf = self.buff.complete()?;
        Ok((self.reader, buf))
    }
}

/// Custom error to signal when we need to yield in async context
#[derive(Debug)]
pub struct WouldBlockError;

impl From<WouldBlockError> for Error {
    fn from(_: WouldBlockError) -> Self {
        // We'll use DeserializeUnexpectedEnd as a proxy, but in practice
        // this should never surface to the user since we handle it in the future
        Error::DeserializeUnexpectedEnd
    }
}

/// Result type that can represent both success and "would block"
type PollResult<T> = std::result::Result<T, WouldBlockError>;

/// Async Deserializer that implements serde::de::Deserializer
pub struct AsyncDeserializer<'de, F: AsyncFlavor<'de>> {
    flavor: F,
    _plt: PhantomData<&'de ()>,
}

impl<'de, F> AsyncDeserializer<'de, F>
where
    F: AsyncFlavor<'de> + 'de,
{
    /// Create a new AsyncDeserializer from an AsyncFlavor
    pub fn from_flavor(flavor: F) -> Self {
        Self {
            flavor,
            _plt: PhantomData,
        }
    }

    /// Deserialize a type T asynchronously using regular serde::Deserialize
    pub async fn deserialize<T>(&mut self) -> Result<T>
    where
        T: Deserialize<'de>,
    {
        // We'll read all the data we need first, then deserialize from a buffer
        // This is simpler and avoids the complex async-sync bridge
        let mut temp_buffer = Vec::new();

        // For now, we'll use a simple approach: read data in chunks until we can deserialize
        // This isn't optimal but it works with regular serde
        let mut chunk_size = 64; // Start with small chunks

        loop {
            // Try to read more data
            let mut chunk = vec![0u8; chunk_size];
            match self.read_chunk(&mut chunk).await {
                Ok(bytes_read) => {
                    temp_buffer.extend_from_slice(&chunk[..bytes_read]);

                    // Try to deserialize what we have
                    match postcard::from_bytes::<T>(&temp_buffer) {
                        Ok(result) => return Ok(result),
                        Err(Error::DeserializeUnexpectedEnd) => {
                            // Need more data, increase chunk size and continue
                            chunk_size = std::cmp::min(chunk_size * 2, 4096);
                            continue;
                        }
                        Err(e) => return Err(e),
                    }
                }
                Err(_) => {
                    // Try to deserialize with what we have
                    return postcard::from_bytes::<T>(&temp_buffer);
                }
            }
        }
    }

    async fn read_chunk(&mut self, buf: &mut [u8]) -> Result<usize> {
        let mut bytes_read = 0;
        for i in 0..buf.len() {
            match self.flavor.pop().await {
                Ok(byte) => {
                    buf[i] = byte;
                    bytes_read += 1;
                }
                Err(_) => break,
            }
        }
        if bytes_read == 0 {
            Err(Error::DeserializeUnexpectedEnd)
        } else {
            Ok(bytes_read)
        }
    }

    /// Return the remaining (unused) bytes in the Deserializer
    pub fn finalize(self) -> Result<F::Remainder> {
        self.flavor.finalize()
    }
}

/// A much simpler approach: buffer all data first, then deserialize synchronously
pub struct BufferedAsyncDeserializer<'de, F: AsyncFlavor<'de>> {
    flavor: F,
    _plt: PhantomData<&'de ()>,
}

impl<'de, F> BufferedAsyncDeserializer<'de, F>
where
    F: AsyncFlavor<'de> + 'de,
{
    pub fn from_flavor(flavor: F) -> Self {
        Self {
            flavor,
            _plt: PhantomData,
        }
    }

    /// Read all available data into a buffer, then deserialize synchronously
    pub async fn deserialize<T>(&mut self) -> Result<T>
    where
        T: Deserialize<'de>,
    {
        // Read data until we have enough or hit EOF
        let mut buffer = Vec::new();
        let mut temp_buf = [0u8; 1024];

        // Read initial chunk to get started
        loop {
            match self.try_read_chunk(&mut temp_buf).await {
                Ok(n) if n > 0 => {
                    buffer.extend_from_slice(&temp_buf[..n]);

                    // Try deserializing what we have so far
                    match postcard::from_bytes::<T>(&buffer) {
                        Ok(result) => return Ok(result),
                        Err(Error::DeserializeUnexpectedEnd) => {
                            // Need more data, continue reading
                            continue;
                        }
                        Err(e) => return Err(e),
                    }
                }
                _ => {
                    // No more data available, try final deserialization
                    return postcard::from_bytes::<T>(&buffer);
                }
            }
        }
    }

    async fn try_read_chunk(&mut self, buf: &mut [u8]) -> Result<usize> {
        for i in 0..buf.len() {
            match self.flavor.pop().await {
                Ok(byte) => buf[i] = byte,
                Err(_) => return Ok(i), // Return how many bytes we read
            }
        }
        Ok(buf.len())
    }

    pub fn finalize(self) -> Result<F::Remainder> {
        self.flavor.finalize()
    }
}

/// Main async deserialization function that works with regular serde::Deserialize
/// This version buffers data before deserializing to avoid the complex async-sync bridge
pub async fn from_async_io<'a, T, R>(val: (R, &'a mut [u8])) -> Result<(T, (R, &'a mut [u8]))>
where
    T: for<'de> Deserialize<'de>,
    R: AsyncRead + Unpin + 'a,
{
    let flavor = AsyncIOReader::new(val.0, val.1);
    let mut deserializer = BufferedAsyncDeserializer::from_flavor(flavor);
    let t = deserializer.deserialize().await?;
    Ok((t, deserializer.finalize()?))
}

/// Alternative version that pre-reads into a Vec for simpler usage
pub async fn from_async_io_buffered<T, R>(mut reader: R) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
    R: AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;

    let mut buffer = Vec::new();
    reader
        .read_to_end(&mut buffer)
        .await
        .map_err(|_| Error::DeserializeUnexpectedEnd)?;

    postcard::from_bytes(&buffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use std::io::Cursor;

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct TestStruct {
        a: u32,
        b: String,
        c: Vec<u8>,
    }

    #[tokio::test]
    async fn test_buffered_approach() {
        let test_data = TestStruct {
            a: 42,
            b: "hello".to_string(),
            c: vec![1, 2, 3, 4, 5],
        };

        // Serialize with regular postcard
        let serialized = postcard::to_allocvec(&test_data).unwrap();
        let cursor = Cursor::new(serialized);

        // Deserialize async with regular serde::Deserialize
        let result: TestStruct = from_async_io_buffered(cursor).await.unwrap();
        assert_eq!(result, test_data);
    }

    #[tokio::test]
    async fn test_streaming_approach() {
        let test_data = TestStruct {
            a: 12345,
            b: "test string".to_string(),
            c: vec![10, 20, 30],
        };

        let serialized = postcard::to_allocvec(&test_data).unwrap();
        let cursor = Cursor::new(serialized);
        let mut buffer = [0u8; 1024];

        let (result, _): (TestStruct, _) = from_async_io((cursor, &mut buffer)).await.unwrap();
        assert_eq!(result, test_data);
    }

    #[tokio::test]
    async fn test_basic_types() {
        // Test various basic types
        let test_u32: u32 = 42;
        let serialized = postcard::to_allocvec(&test_u32).unwrap();
        let cursor = Cursor::new(serialized);
        let result: u32 = from_async_io_buffered(cursor).await.unwrap();
        assert_eq!(result, test_u32);

        let test_string = "hello world".to_string();
        let serialized = postcard::to_allocvec(&test_string).unwrap();
        let cursor = Cursor::new(serialized);
        let result: String = from_async_io_buffered(cursor).await.unwrap();
        assert_eq!(result, test_string);

        let test_vec = vec![1u8, 2, 3, 4, 5];
        let serialized = postcard::to_allocvec(&test_vec).unwrap();
        let cursor = Cursor::new(serialized);
        let result: Vec<u8> = from_async_io_buffered(cursor).await.unwrap();
        assert_eq!(result, test_vec);
    }

    #[tokio::test]
    async fn test_option_types() {
        let test_some: Option<u32> = Some(42);
        let serialized = postcard::to_allocvec(&test_some).unwrap();
        let cursor = Cursor::new(serialized);
        let result: Option<u32> = from_async_io_buffered(cursor).await.unwrap();
        assert_eq!(result, test_some);

        let test_none: Option<u32> = None;
        let serialized = postcard::to_allocvec(&test_none).unwrap();
        let cursor = Cursor::new(serialized);
        let result: Option<u32> = from_async_io_buffered(cursor).await.unwrap();
        assert_eq!(result, test_none);
    }
}
