//! Bounded message channel between the agent watcher and the viewer process.
//!
//! Messages travel over a Unix socket as `[u32 BE length][JSON payload]`
//! frames. The decoder is stateful and bounded exactly like the input decoder:
//! a complete frame is delivered one at a time, partial frames wait for more
//! bytes, an oversized header resyncs to the next plausible boundary, and the
//! pending buffer is capped so a slow or hostile peer cannot grow memory.

use std::io::Write as _;

use serde::{Deserialize, Serialize};

use crate::ipc::IPC_MAX_REQUEST_BYTES;

/// Wire-frame cap for one document message: the renderer's request byte cap
/// plus room for the JSON envelope.
pub const MAX_FRAME_BYTES: usize = IPC_MAX_REQUEST_BYTES + 4096;

/// Maximum bytes the decoder buffers before evicting the oldest prefix.
const MAX_PENDING_BYTES: usize = MAX_FRAME_BYTES + 64 * 1024;

/// A viewer-control message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Message {
    /// Render this new answer document (Markdown + math) in the viewer.
    Document { text: String },
    /// Close the viewer cleanly.
    Quit,
}

/// Why a message could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecError {
    /// The text payload exceeds the bounded frame size.
    TooLarge,
    /// The frame did not parse as a known message.
    Malformed,
}

/// Encodes a document message as one length-prefixed JSON frame.
pub fn encode_document(text: &str) -> Result<Vec<u8>, CodecError> {
    if text.len() > IPC_MAX_REQUEST_BYTES {
        return Err(CodecError::TooLarge);
    }
    encode(&Message::Document {
        text: text.to_string(),
    })
}

/// Encodes a quit message as one length-prefixed JSON frame.
pub fn encode_quit() -> Vec<u8> {
    encode(&Message::Quit).expect("quit message is small and valid")
}

fn encode(message: &Message) -> Result<Vec<u8>, CodecError> {
    let payload = serde_json::to_vec(message)
        .map_err(|_| CodecError::Malformed)
        .and_then(|bytes| {
            if bytes.len() > MAX_FRAME_BYTES {
                return Err(CodecError::TooLarge);
            }
            Ok(bytes)
        })?;
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.write_all(&payload).expect("in-memory write");
    Ok(frame)
}

/// Bounded incremental decoder over the raw socket byte stream.
#[derive(Debug, Default)]
pub struct Decoder {
    pending: Vec<u8>,
    total_dropped: u64,
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends raw bytes, evicting the oldest prefix when the cap is exceeded.
    pub fn push(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
        if self.pending.len() > MAX_PENDING_BYTES {
            let overflow = self.pending.len() - MAX_PENDING_BYTES;
            self.total_dropped += overflow as u64;
            self.pending.drain(..overflow);
        }
    }

    /// Delivers the next complete message, or `None` when more bytes are
    /// required. A malformed or oversized frame is consumed and reported.
    pub fn next_message(&mut self) -> Option<Result<Message, CodecError>> {
        loop {
            if self.pending.len() < 4 {
                return None;
            }
            let len = u32::from_be_bytes([self.pending[0], self.pending[1], self.pending[2], self.pending[3]]) as usize;
            if len == 0 || len > MAX_FRAME_BYTES {
                // Not a valid frame header: drop one byte and resync.
                self.pending.drain(..1);
                self.total_dropped += 1;
                continue;
            }
            if self.pending.len() < 4 + len {
                return None;
            }
            let payload = self.pending[4..4 + len].to_vec();
            self.pending.drain(..4 + len);
            let message = match serde_json::from_slice::<Message>(&payload) {
                Ok(message) => Ok(message),
                Err(_) => Err(CodecError::Malformed),
            };
            return Some(message);
        }
    }

    pub fn total_dropped(&self) -> u64 {
        self.total_dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::IPC_MAX_REQUEST_BYTES;

    #[test]
    fn document_message_round_trips() {
        let frame = encode_document("The relation is $E=mc^2$.").unwrap();
        let mut decoder = Decoder::new();
        for byte in frame.iter().copied() {
            decoder.push(&[byte]);
        }
        assert_eq!(
            decoder.next_message(),
            Some(Ok(Message::Document {
                text: "The relation is $E=mc^2$.".to_string()
            }))
        );
        assert_eq!(decoder.next_message(), None);
    }

    #[test]
    fn quit_message_is_tiny() {
        let frame = encode_quit();
        assert!(frame.len() < 64);
        let mut decoder = Decoder::new();
        decoder.push(&frame);
        assert_eq!(decoder.next_message(), Some(Ok(Message::Quit)));
    }

    #[test]
    fn multiple_frames_decoding_in_order() {
        let first = encode_document("A").unwrap();
        let quit = encode_quit();
        let mut joined = first.clone();
        joined.extend_from_slice(&quit);
        let mut decoder = Decoder::new();
        decoder.push(&joined);
        assert_eq!(decoder.next_message(), Some(Ok(Message::Document { text: "A".into() })));
        assert_eq!(decoder.next_message(), Some(Ok(Message::Quit)));
        assert_eq!(decoder.next_message(), None);
    }

    #[test]
    fn oversized_document_is_rejected_without_panicking() {
        let big = "x".repeat(IPC_MAX_REQUEST_BYTES + 1);
        assert_eq!(encode_document(&big), Err(CodecError::TooLarge));
    }

    #[test]
    fn an_oversized_frame_header_resyncs() {
        let mut decoder = Decoder::new();
        let good = encode_document("ok").unwrap();
        // A bogus huge-length header followed by the good frame must be
        // skipped so the good frame is still decoded.
        let mut stream = vec![0xff, 0xff, 0xff, 0xff];
        stream.extend_from_slice(&good);
        decoder.push(&stream);
        assert_eq!(decoder.next_message(), Some(Ok(Message::Document { text: "ok".into() })));
    }

    #[test]
    fn garbage_payload_is_reported_and_consumed() {
        let mut decoder = Decoder::new();
        let mut bad = (6u32).to_be_bytes().to_vec();
        bad.extend_from_slice(b"{{{{{{");
        decoder.push(&bad);
        assert_eq!(decoder.next_message(), Some(Err(CodecError::Malformed)));
        assert_eq!(decoder.next_message(), None);
    }

    #[test]
    fn pending_buffer_is_capped() {
        let mut decoder = Decoder::new();
        let chunk = vec![0u8; 16 * 1024];
        for _ in 0..100 {
            decoder.push(&chunk);
        }
        assert!(decoder.total_dropped > 0);
        // Under a cap the decoder never holds more than the bound.
        assert!(decoder.pending.capacity() <= MAX_PENDING_BYTES * 2);
    }
}
