//! Framing. Spec §2.
//!
//! - MessagePack (default): a `u32` big-endian length, then the body.
//! - JSON (`COVALENCE_PROTOCOL_DEBUG=json`): one message per line.

use crate::Message;

/// Largest frame body accepted. Larger payloads must be chunked.
pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    MsgPack,
    Json,
}

impl Format {
    /// JSON if `COVALENCE_PROTOCOL_DEBUG=json`, otherwise MessagePack.
    pub fn from_env() -> Self {
        match std::env::var("COVALENCE_PROTOCOL_DEBUG").as_deref() {
            Ok("json") => Format::Json,
            _ => Format::MsgPack,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// The link can't recover from this; drop it.
    #[error("frame of {0} bytes exceeds the {MAX_FRAME_LEN}-byte limit")]
    TooLarge(usize),
    #[error(transparent)]
    Encode(#[from] rmp_serde::encode::Error),
    /// The frame was consumed, so the stream is still aligned.
    #[error(transparent)]
    Decode(#[from] rmp_serde::decode::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Encodes one message as a complete frame.
pub fn encode(message: &Message, format: Format) -> Result<Vec<u8>, FrameError> {
    match format {
        Format::MsgPack => {
            let body = rmp_serde::to_vec_named(message)?;
            let len = check_len(body.len())?;
            let mut frame = Vec::with_capacity(4 + body.len());
            frame.extend_from_slice(&len.to_be_bytes());
            frame.extend_from_slice(&body);
            Ok(frame)
        }
        Format::Json => {
            let mut line = serde_json::to_vec(message)?;
            check_len(line.len())?;
            line.push(b'\n');
            Ok(line)
        }
    }
}

fn check_len(len: usize) -> Result<u32, FrameError> {
    if len > MAX_FRAME_LEN {
        return Err(FrameError::TooLarge(len));
    }
    Ok(len as u32)
}

/// Splits a byte stream into messages.
#[derive(Debug)]
pub struct FrameDecoder {
    format: Format,
    buf: Vec<u8>,
}

impl FrameDecoder {
    pub fn new(format: Format) -> Self {
        FrameDecoder {
            format,
            buf: Vec::new(),
        }
    }

    /// Appends bytes read from the link.
    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Returns the next complete message, or `None` if more bytes are needed.
    pub fn next_message(&mut self) -> Result<Option<Message>, FrameError> {
        match self.format {
            Format::MsgPack => {
                let Some(header) = self.buf.first_chunk::<4>() else {
                    return Ok(None);
                };
                let len = u32::from_be_bytes(*header) as usize;
                if len > MAX_FRAME_LEN {
                    return Err(FrameError::TooLarge(len));
                }
                if self.buf.len() < 4 + len {
                    return Ok(None);
                }
                let frame: Vec<u8> = self.buf.drain(..4 + len).collect();
                Ok(Some(rmp_serde::from_slice(&frame[4..])?))
            }
            Format::Json => loop {
                let Some(end) = self.buf.iter().position(|&b| b == b'\n') else {
                    if self.buf.len() > MAX_FRAME_LEN {
                        return Err(FrameError::TooLarge(self.buf.len()));
                    }
                    return Ok(None);
                };
                let line: Vec<u8> = self.buf.drain(..=end).collect();
                let line = line.trim_ascii();
                if !line.is_empty() {
                    return Ok(Some(serde_json::from_slice(line)?));
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Notification, Value};

    fn ping() -> Message {
        Message::Notification(Notification {
            m: "log".into(),
            conn: None,
            p: Value::Nil,
        })
    }

    #[test]
    fn decodes_frames_split_across_reads() {
        for format in [Format::MsgPack, Format::Json] {
            let mut bytes = encode(&ping(), format).unwrap();
            bytes.extend(encode(&ping(), format).unwrap());
            let mut decoder = FrameDecoder::new(format);
            let mut out = Vec::new();
            for byte in bytes {
                decoder.push(&[byte]);
                while let Some(message) = decoder.next_message().unwrap() {
                    out.push(message);
                }
            }
            assert_eq!(out, vec![ping(), ping()], "{format:?}");
        }
    }

    #[test]
    fn binary_survives_msgpack() {
        let blob = Value::Map(vec![("blob".into(), Value::Binary(vec![0, 1, 255]))]);
        let message = Message::Notification(Notification {
            m: "log".into(),
            conn: None,
            p: blob,
        });
        let mut decoder = FrameDecoder::new(Format::MsgPack);
        decoder.push(&encode(&message, Format::MsgPack).unwrap());
        assert_eq!(decoder.next_message().unwrap(), Some(message));
    }

    #[test]
    fn rejects_oversized_frames() {
        let mut decoder = FrameDecoder::new(Format::MsgPack);
        decoder.push(&(MAX_FRAME_LEN as u32 + 1).to_be_bytes());
        assert!(matches!(
            decoder.next_message(),
            Err(FrameError::TooLarge(_))
        ));
    }

    #[test]
    fn stays_aligned_after_a_bad_frame() {
        let mut decoder = FrameDecoder::new(Format::MsgPack);
        decoder.push(&[0, 0, 0, 1, 0xc1]); // 0xc1 is never valid MessagePack
        decoder.push(&encode(&ping(), Format::MsgPack).unwrap());
        assert!(decoder.next_message().is_err());
        assert_eq!(decoder.next_message().unwrap(), Some(ping()));
    }
}
