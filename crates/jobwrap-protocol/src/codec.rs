//! Length-prefixed frame codec.
//!
//! Wire format: 4-byte big-endian length followed by a JSON payload. A frame
//! may be at most [`MAX_FRAME_BYTES`]; anything larger is rejected so an
//! untrusted peer cannot exhaust memory.

use std::io::{self, Read, Write};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::error::ProtocolError;

/// Bytes used for the length prefix.
pub const PROTOCOL_HEADER_BYTES: usize = 4;
/// Maximum accepted frame size (1 MiB).
pub const MAX_FRAME_BYTES: usize = 1 << 20;

/// Incremental decoder that accepts arbitrary byte chunks and yields frames of
/// message type `T` (a `DeserializeOwned` protocol enum).
#[derive(Debug, Default)]
pub struct FrameDecoder<T = super::DaemonToClient> {
    buffer: Vec<u8>,
    _marker: std::marker::PhantomData<fn() -> T>,
}

impl<T> FrameDecoder<T> {
    pub fn new() -> Self {
        Self {
            buffer: Vec::with_capacity(16 * 1024),
            _marker: std::marker::PhantomData,
        }
    }

    /// Feed bytes and return any complete frames in order.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<T>, ProtocolError>
    where
        T: serde::de::DeserializeOwned,
    {
        self.buffer.extend_from_slice(bytes);
        if self.buffer.len() > MAX_FRAME_BYTES + PROTOCOL_HEADER_BYTES {
            return Err(ProtocolError::FrameTooLarge {
                size: self.buffer.len(),
            });
        }
        let mut messages = Vec::new();
        loop {
            if self.buffer.len() < PROTOCOL_HEADER_BYTES {
                break;
            }
            let len = u32::from_be_bytes([
                self.buffer[0],
                self.buffer[1],
                self.buffer[2],
                self.buffer[3],
            ]) as usize;
            if len > MAX_FRAME_BYTES {
                return Err(ProtocolError::FrameTooLarge { size: len });
            }
            if self.buffer.len() < PROTOCOL_HEADER_BYTES + len {
                break;
            }
            let body: Vec<u8> = self
                .buffer
                .drain(PROTOCOL_HEADER_BYTES..PROTOCOL_HEADER_BYTES + len)
                .collect();
            self.buffer.drain(0..PROTOCOL_HEADER_BYTES);
            messages.push(decode_frame::<T>(&body)?);
        }
        Ok(messages)
    }
}

/// Serialize a message into a single frame (bytes ready to write).
pub fn encode_frame<T: serde::Serialize>(msg: &T) -> Result<Vec<u8>, ProtocolError> {
    let body = serde_json::to_vec(msg)?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge { size: body.len() });
    }
    let mut frame = Vec::with_capacity(PROTOCOL_HEADER_BYTES + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(&body);
    Ok(frame)
}

/// Decode a complete frame body into a message.
pub fn decode_frame<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, ProtocolError> {
    Ok(serde_json::from_slice(body)?)
}

/// Write a frame to a blocking writer.
pub fn write_frame<W: Write>(w: &mut W, msg: &super::DaemonToClient) -> io::Result<()> {
    let frame = encode_frame(msg).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    w.write_all(&frame)
}

/// Read one frame from a blocking reader.
pub fn read_frame<R: Read>(r: &mut R) -> io::Result<super::DaemonToClient> {
    let mut header = [0u8; PROTOCOL_HEADER_BYTES];
    r.read_exact(&mut header)?;
    let len = u32::from_be_bytes(header) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    decode_frame(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Write a frame to an async writer.
pub async fn write_frame_async<W: AsyncWrite + Unpin>(
    w: &mut W,
    msg: &super::DaemonToClient,
) -> io::Result<()> {
    let frame = encode_frame(msg).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    w.write_all(&frame).await
}

/// Read one frame from an async reader.
pub async fn read_frame_async<R: AsyncRead + Unpin>(
    r: &mut R,
) -> io::Result<super::DaemonToClient> {
    let mut header = [0u8; PROTOCOL_HEADER_BYTES];
    r.read_exact(&mut header).await?;
    let len = u32::from_be_bytes(header) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    decode_frame(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{ClientToDaemon, Hello, HelloRole};

    #[test]
    fn round_trips_across_chunk_boundaries() {
        let mut decoder = FrameDecoder::<ClientToDaemon>::new();
        let msg = ClientToDaemon::ListJobs;
        let frame = encode_frame(&msg).expect("encode");
        // Feed one byte at a time.
        let mut found = Vec::new();
        for b in frame {
            found.extend(decoder.push(&[b]).expect("push"));
        }
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn rejects_oversized_frames() {
        let mut decoder = FrameDecoder::<ClientToDaemon>::new();
        let mut frame = Vec::new();
        frame.extend_from_slice(&(MAX_FRAME_BYTES as u32 + 1).to_be_bytes());
        assert!(decoder.push(&frame).is_err());
    }

    #[test]
    fn hello_serializes() {
        let msg = ClientToDaemon::Hello(Hello {
            protocol_version: 1,
            role: HelloRole::Cli,
            pid: 123,
            uid: 1000,
        });
        let frame = encode_frame(&msg).expect("encode");
        let body = &frame[PROTOCOL_HEADER_BYTES..];
        let text = std::str::from_utf8(body).expect("utf8");
        assert!(text.contains("hello"));
    }
}
