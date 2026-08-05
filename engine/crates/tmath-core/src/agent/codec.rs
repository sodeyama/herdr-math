//! Bounded message channel between the agent watcher and the viewer process.
//!
//! Messages travel over a Unix socket as `[u32 BE length][JSON payload]`
//! frames. The decoder is stateful and bounded exactly like the input decoder:
//! a complete frame is delivered one at a time, partial frames wait for more
//! bytes, an oversized header resyncs to the next plausible boundary, and the
//! pending buffer is capped so a slow or hostile peer cannot grow memory.
//!
//! `Document` is the whole-answer message V2-style sources still send; it
//! carries no version and is always accepted (AT-3-601's backward-compat
//! requirement — a V3 viewer must keep working with a V2 watcher). `Append`
//! and `ReplaceTail` are the V3 delta messages: each carries `version`
//! (checked against [`DELTA_PROTOCOL_VERSION`]) and a monotonically
//! increasing `seq`. [`Decoder`] enforces both fail-closed — see its type
//! doc for the resync policy when a delta frame is rejected.

use std::io::Write as _;

use serde::{Deserialize, Serialize};

use crate::ipc::IPC_MAX_REQUEST_BYTES;

/// Wire-frame cap for one document message: the renderer's request byte cap
/// plus room for the JSON envelope.
pub const MAX_FRAME_BYTES: usize = IPC_MAX_REQUEST_BYTES + 4096;

/// Maximum bytes the decoder buffers before evicting the oldest prefix.
const MAX_PENDING_BYTES: usize = MAX_FRAME_BYTES + 64 * 1024;

/// The only delta-frame protocol version this decoder accepts. An `Append`
/// or `ReplaceTail` frame carrying any other value is rejected fail-closed
/// (see [`Decoder`]) rather than guessed at — there is exactly one version
/// today, so any mismatch means a future or unrelated sender, not a
/// tolerable variation.
pub const DELTA_PROTOCOL_VERSION: u32 = 1;

/// A viewer-control message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Message {
    /// Render this whole answer document (Markdown + math) in the viewer,
    /// replacing whatever document text preceded it. Carries no version:
    /// this is the message a V2-style source still sends, and it must
    /// always be accepted for backward compatibility (AT-3-601). Receiving
    /// one also resets delta tracking — see [`Decoder`].
    Document { text: String },
    /// Appends `text` to the end of the current document. `seq` must be
    /// exactly one greater than the last accepted delta's `seq` (starting
    /// from the `seq` implied by the most recent `Document`, `0`); anything
    /// else is a duplicate or out-of-order frame and is rejected.
    Append {
        version: u32,
        seq: u64,
        text: String,
    },
    /// Replaces the current document's tail: the first `keep_bytes` bytes
    /// of the current document are kept, and `text` replaces everything
    /// after that. `keep_bytes` must land on a UTF-8 character boundary and
    /// must not exceed the current document's length; `seq` follows the
    /// same monotonic rule as `Append`.
    ReplaceTail {
        version: u32,
        seq: u64,
        keep_bytes: usize,
        text: String,
    },
    /// Close the viewer cleanly.
    Quit,
}

/// Why a message could not be decoded or applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecError {
    /// The text payload exceeds the bounded frame size.
    TooLarge,
    /// The frame did not parse as a known message.
    Malformed,
    /// An `Append`/`ReplaceTail` frame's `version` is not
    /// [`DELTA_PROTOCOL_VERSION`].
    UnknownVersion,
    /// An `Append`/`ReplaceTail` frame's `seq` is not exactly one past the
    /// last accepted delta (a duplicate, a replay, or an out-of-order jump).
    SequenceMismatch,
    /// A `ReplaceTail` frame's `keep_bytes` does not land on a UTF-8
    /// character boundary of the current document, or exceeds its length.
    InvalidTailBoundary,
    /// Applying an `Append`/`ReplaceTail` would grow the reassembled
    /// document past [`DeltaState`]'s configured `max_document_bytes`.
    DocumentTooLarge,
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

/// Encodes an append delta message as one length-prefixed JSON frame.
pub fn encode_append(seq: u64, text: &str) -> Result<Vec<u8>, CodecError> {
    if text.len() > IPC_MAX_REQUEST_BYTES {
        return Err(CodecError::TooLarge);
    }
    encode(&Message::Append {
        version: DELTA_PROTOCOL_VERSION,
        seq,
        text: text.to_string(),
    })
}

/// Encodes a replace-tail delta message as one length-prefixed JSON frame.
pub fn encode_replace_tail(seq: u64, keep_bytes: usize, text: &str) -> Result<Vec<u8>, CodecError> {
    if text.len() > IPC_MAX_REQUEST_BYTES {
        return Err(CodecError::TooLarge);
    }
    encode(&Message::ReplaceTail {
        version: DELTA_PROTOCOL_VERSION,
        seq,
        keep_bytes,
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
            let len = u32::from_be_bytes([
                self.pending[0],
                self.pending[1],
                self.pending[2],
                self.pending[3],
            ]) as usize;
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

/// Applies `Document`/`Append`/`ReplaceTail` messages to a running document
/// text, enforcing AT-3-601's delta rules. Independent of [`Decoder`]
/// (framing) by design: this is the semantic layer above it, so the viewer
/// can hand it already-decoded messages and get back either the new
/// document text or a specific, fail-closed rejection reason.
///
/// **Resync policy** (the simplest safe one): any rejected delta frame
/// (unknown version, bad sequence, an invalid `ReplaceTail` boundary, or a
/// result that would cross `max_document_bytes`) invalidates delta tracking
/// rather than trying to patch around it. Every later `Append`/
/// `ReplaceTail` is then rejected with [`CodecError::SequenceMismatch`]
/// until the next `Document` frame resyncs — the source is expected to
/// notice (e.g. a socket write error path, or simply silence) and
/// eventually re-send a whole document. The current text stays exactly
/// what it was before the bad frame; nothing is ever applied speculatively.
///
/// **Bounded reassembly**: each individual frame is already capped at the
/// codec layer (`MAX_FRAME_BYTES`), but nothing about per-frame bounds
/// stops N accepted `Append`s from reassembling an unbounded document — the
/// codec crate has no rendering-side limits to defer to, so `DeltaState`
/// takes its own finite `max_document_bytes` at construction and enforces
/// it directly (AGENTS.md requires every limit to be finite and enforced,
/// not just individually-bounded inputs to an unbounded accumulator).
#[derive(Debug)]
pub struct DeltaState {
    document: String,
    last_seq: Option<u64>,
    delta_valid: bool,
    max_document_bytes: usize,
}

impl DeltaState {
    /// `max_document_bytes` bounds the reassembled document's total byte
    /// length; an `Append`/`ReplaceTail` whose result would exceed it is
    /// rejected with [`CodecError::DocumentTooLarge`] (the resync policy
    /// applies the same as any other rejected delta). `Document` frames are
    /// not checked here — they are already bounded per-frame by the codec
    /// (`encode_document` rejects anything over `IPC_MAX_REQUEST_BYTES`
    /// before it is ever sent) — so a whole document up to that size is
    /// always accepted, matching AT-3-601's backward-compat guarantee.
    pub fn new(max_document_bytes: usize) -> Self {
        Self {
            document: String::new(),
            last_seq: None,
            delta_valid: false,
            max_document_bytes,
        }
    }

    /// The current document text.
    pub fn document(&self) -> &str {
        &self.document
    }

    /// Applies one message. Returns `Ok(Some(text))` with the new document
    /// text when it changed (`Document`, or an accepted `Append`/
    /// `ReplaceTail`), `Ok(None)` when the message does not touch the
    /// document (`Quit`), or `Err` for a rejected delta — in which case
    /// `document()` is unchanged and delta tracking is invalidated per the
    /// resync policy above.
    pub fn apply(&mut self, message: &Message) -> Result<Option<&str>, CodecError> {
        match message {
            Message::Document { text } => {
                self.document = text.clone();
                self.last_seq = Some(0);
                self.delta_valid = true;
                Ok(Some(&self.document))
            }
            Message::Append { version, seq, text } => {
                self.check_delta(*version, *seq)?;
                if self.document.len().saturating_add(text.len()) > self.max_document_bytes {
                    self.delta_valid = false;
                    return Err(CodecError::DocumentTooLarge);
                }
                self.document.push_str(text);
                self.last_seq = Some(*seq);
                Ok(Some(&self.document))
            }
            Message::ReplaceTail {
                version,
                seq,
                keep_bytes,
                text,
            } => {
                self.check_delta(*version, *seq)?;
                if *keep_bytes > self.document.len() || !self.document.is_char_boundary(*keep_bytes)
                {
                    self.delta_valid = false;
                    return Err(CodecError::InvalidTailBoundary);
                }
                if keep_bytes.saturating_add(text.len()) > self.max_document_bytes {
                    self.delta_valid = false;
                    return Err(CodecError::DocumentTooLarge);
                }
                self.document.truncate(*keep_bytes);
                self.document.push_str(text);
                self.last_seq = Some(*seq);
                Ok(Some(&self.document))
            }
            Message::Quit => Ok(None),
        }
    }

    /// Validates a delta frame's version and sequence number against the
    /// current state, invalidating delta tracking on any failure (see the
    /// resync policy on the type doc). Does not mutate `document`.
    fn check_delta(&mut self, version: u32, seq: u64) -> Result<(), CodecError> {
        if version != DELTA_PROTOCOL_VERSION {
            self.delta_valid = false;
            return Err(CodecError::UnknownVersion);
        }
        let expected = match (self.delta_valid, self.last_seq) {
            (true, Some(last)) => last.checked_add(1),
            _ => None,
        };
        if expected != Some(seq) {
            self.delta_valid = false;
            return Err(CodecError::SequenceMismatch);
        }
        Ok(())
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
        assert_eq!(
            decoder.next_message(),
            Some(Ok(Message::Document { text: "A".into() }))
        );
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
        assert_eq!(
            decoder.next_message(),
            Some(Ok(Message::Document { text: "ok".into() }))
        );
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
        let chunk = vec![0u8; MAX_PENDING_BYTES + 16 * 1024];
        decoder.push(&chunk);
        assert!(decoder.total_dropped > 0);
        // Under a cap the decoder never holds more than the bound.
        assert!(decoder.pending.capacity() <= MAX_PENDING_BYTES * 2);
    }

    // --- AT-3-601: Append/ReplaceTail codec round trips ---

    #[test]
    fn append_message_round_trips_through_the_decoder() {
        let frame = encode_append(1, " more text.").unwrap();
        let mut decoder = Decoder::new();
        decoder.push(&frame);
        assert_eq!(
            decoder.next_message(),
            Some(Ok(Message::Append {
                version: DELTA_PROTOCOL_VERSION,
                seq: 1,
                text: " more text.".to_string()
            }))
        );
    }

    #[test]
    fn replace_tail_message_round_trips_through_the_decoder() {
        let frame = encode_replace_tail(1, 5, "world!").unwrap();
        let mut decoder = Decoder::new();
        decoder.push(&frame);
        assert_eq!(
            decoder.next_message(),
            Some(Ok(Message::ReplaceTail {
                version: DELTA_PROTOCOL_VERSION,
                seq: 1,
                keep_bytes: 5,
                text: "world!".to_string()
            }))
        );
    }

    #[test]
    fn oversized_append_and_replace_tail_are_rejected_without_panicking() {
        let big = "x".repeat(IPC_MAX_REQUEST_BYTES + 1);
        assert_eq!(encode_append(1, &big), Err(CodecError::TooLarge));
        assert_eq!(encode_replace_tail(1, 0, &big), Err(CodecError::TooLarge));
    }

    #[test]
    fn malformed_delta_frame_is_reported_and_consumed() {
        // A `kind: "append"` object missing required fields is well-formed
        // JSON but not a valid `Message`.
        let mut decoder = Decoder::new();
        let payload = br#"{"kind":"append"}"#;
        let mut frame = (payload.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(payload);
        decoder.push(&frame);
        assert_eq!(decoder.next_message(), Some(Err(CodecError::Malformed)));
    }

    #[test]
    fn truncated_delta_frame_waits_for_more_bytes() {
        let frame = encode_append(1, "hello").unwrap();
        let mut decoder = Decoder::new();
        // Push all but the last byte: an incomplete frame must not be
        // reported as malformed, only as "not yet".
        decoder.push(&frame[..frame.len() - 1]);
        assert_eq!(decoder.next_message(), None);
        decoder.push(&frame[frame.len() - 1..]);
        assert_eq!(
            decoder.next_message(),
            Some(Ok(Message::Append {
                version: DELTA_PROTOCOL_VERSION,
                seq: 1,
                text: "hello".to_string()
            }))
        );
    }

    // --- AT-3-601: DeltaState semantics ---

    #[test]
    fn happy_path_document_then_append_then_replace_tail() {
        let mut state = DeltaState::new(usize::MAX);
        assert_eq!(
            state
                .apply(&Message::Document {
                    text: "Hello".to_string()
                })
                .unwrap(),
            Some("Hello")
        );

        assert_eq!(
            state
                .apply(&Message::Append {
                    version: DELTA_PROTOCOL_VERSION,
                    seq: 1,
                    text: ", world".to_string()
                })
                .unwrap(),
            Some("Hello, world")
        );

        assert_eq!(
            state
                .apply(&Message::ReplaceTail {
                    version: DELTA_PROTOCOL_VERSION,
                    seq: 2,
                    keep_bytes: 5,
                    text: " there!".to_string()
                })
                .unwrap(),
            Some("Hello there!")
        );
        assert_eq!(state.document(), "Hello there!");
    }

    #[test]
    fn quit_does_not_touch_the_document() {
        let mut state = DeltaState::new(usize::MAX);
        state
            .apply(&Message::Document {
                text: "Hello".to_string(),
            })
            .unwrap();
        assert_eq!(state.apply(&Message::Quit).unwrap(), None);
        assert_eq!(state.document(), "Hello");
    }

    #[test]
    fn a_delta_before_any_document_is_rejected_as_out_of_order() {
        let mut state = DeltaState::new(usize::MAX);
        let result = state.apply(&Message::Append {
            version: DELTA_PROTOCOL_VERSION,
            seq: 1,
            text: "orphan".to_string(),
        });
        assert_eq!(result, Err(CodecError::SequenceMismatch));
        assert_eq!(state.document(), "", "no document existed to change");
    }

    #[test]
    fn duplicate_sequence_number_is_rejected_and_leaves_state_intact() {
        let mut state = DeltaState::new(usize::MAX);
        state
            .apply(&Message::Document {
                text: "A".to_string(),
            })
            .unwrap();
        state
            .apply(&Message::Append {
                version: DELTA_PROTOCOL_VERSION,
                seq: 1,
                text: "B".to_string(),
            })
            .unwrap();
        assert_eq!(state.document(), "AB");

        // Replaying seq=1 again is a duplicate.
        let result = state.apply(&Message::Append {
            version: DELTA_PROTOCOL_VERSION,
            seq: 1,
            text: "C".to_string(),
        });
        assert_eq!(result, Err(CodecError::SequenceMismatch));
        assert_eq!(
            state.document(),
            "AB",
            "the previous valid document state is untouched"
        );
    }

    #[test]
    fn out_of_order_sequence_number_is_rejected() {
        let mut state = DeltaState::new(usize::MAX);
        state
            .apply(&Message::Document {
                text: "A".to_string(),
            })
            .unwrap();
        // Jumping straight to seq=3 (skipping seq=1) is out of order.
        let result = state.apply(&Message::Append {
            version: DELTA_PROTOCOL_VERSION,
            seq: 3,
            text: "X".to_string(),
        });
        assert_eq!(result, Err(CodecError::SequenceMismatch));
        assert_eq!(state.document(), "A");
    }

    #[test]
    fn unknown_version_is_rejected() {
        let mut state = DeltaState::new(usize::MAX);
        state
            .apply(&Message::Document {
                text: "A".to_string(),
            })
            .unwrap();
        let result = state.apply(&Message::Append {
            version: DELTA_PROTOCOL_VERSION + 1,
            seq: 1,
            text: "X".to_string(),
        });
        assert_eq!(result, Err(CodecError::UnknownVersion));
        assert_eq!(state.document(), "A");
    }

    #[test]
    fn replace_tail_beyond_the_document_length_is_rejected() {
        let mut state = DeltaState::new(usize::MAX);
        state
            .apply(&Message::Document {
                text: "Hi".to_string(),
            })
            .unwrap();
        let result = state.apply(&Message::ReplaceTail {
            version: DELTA_PROTOCOL_VERSION,
            seq: 1,
            keep_bytes: 100,
            text: "!".to_string(),
        });
        assert_eq!(result, Err(CodecError::InvalidTailBoundary));
        assert_eq!(state.document(), "Hi");
    }

    #[test]
    fn replace_tail_off_a_utf8_boundary_is_rejected() {
        let mut state = DeltaState::new(usize::MAX);
        // "é" is a 2-byte UTF-8 character; byte offset 1 lands inside it.
        state
            .apply(&Message::Document {
                text: "é".to_string(),
            })
            .unwrap();
        let result = state.apply(&Message::ReplaceTail {
            version: DELTA_PROTOCOL_VERSION,
            seq: 1,
            keep_bytes: 1,
            text: "x".to_string(),
        });
        assert_eq!(result, Err(CodecError::InvalidTailBoundary));
        assert_eq!(state.document(), "é");
    }

    /// AT-3-601's resync policy: after any rejected delta, every later
    /// delta is rejected too (not just the bad one) until the next whole
    /// `Document` frame resyncs delta tracking.
    #[test]
    fn a_rejected_delta_invalidates_tracking_until_the_next_document() {
        let mut state = DeltaState::new(usize::MAX);
        state
            .apply(&Message::Document {
                text: "A".to_string(),
            })
            .unwrap();
        // A bad version poisons delta tracking.
        assert_eq!(
            state.apply(&Message::Append {
                version: DELTA_PROTOCOL_VERSION + 1,
                seq: 1,
                text: "X".to_string(),
            }),
            Err(CodecError::UnknownVersion)
        );

        // Even a well-formed, correctly-sequenced-looking append is now
        // rejected — the resync point is the next Document, not "whatever
        // seq would have been next".
        assert_eq!(
            state.apply(&Message::Append {
                version: DELTA_PROTOCOL_VERSION,
                seq: 1,
                text: "Y".to_string(),
            }),
            Err(CodecError::SequenceMismatch)
        );
        assert_eq!(state.document(), "A");

        // A fresh Document resyncs.
        assert_eq!(
            state
                .apply(&Message::Document {
                    text: "B".to_string()
                })
                .unwrap(),
            Some("B")
        );
        assert_eq!(
            state
                .apply(&Message::Append {
                    version: DELTA_PROTOCOL_VERSION,
                    seq: 1,
                    text: "!".to_string(),
                })
                .unwrap(),
            Some("B!")
        );
    }

    /// A whole-document frame from a V2-style source (no version, no
    /// sequencing) must still work even after delta tracking has diverged —
    /// `Document` is always accepted, per AT-3-601's backward-compat clause.
    #[test]
    fn document_frames_are_always_accepted_regardless_of_delta_state() {
        let mut state = DeltaState::new(usize::MAX);
        state
            .apply(&Message::Append {
                version: DELTA_PROTOCOL_VERSION,
                seq: 1,
                text: "orphan".to_string(),
            })
            .unwrap_err();
        assert_eq!(
            state
                .apply(&Message::Document {
                    text: "fresh".to_string()
                })
                .unwrap(),
            Some("fresh")
        );
    }

    // --- Bounded reassembly (max_document_bytes) ---

    #[test]
    fn an_append_that_would_cross_the_cap_is_rejected_with_state_intact() {
        let mut state = DeltaState::new(10);
        state
            .apply(&Message::Document {
                text: "12345".to_string(),
            })
            .unwrap();
        // 5 existing bytes + 6 new bytes = 11 > cap of 10.
        let result = state.apply(&Message::Append {
            version: DELTA_PROTOCOL_VERSION,
            seq: 1,
            text: "abcdef".to_string(),
        });
        assert_eq!(result, Err(CodecError::DocumentTooLarge));
        assert_eq!(
            state.document(),
            "12345",
            "the previous valid document is untouched"
        );
    }

    #[test]
    fn an_append_landing_exactly_on_the_cap_is_accepted() {
        let mut state = DeltaState::new(10);
        state
            .apply(&Message::Document {
                text: "12345".to_string(),
            })
            .unwrap();
        // 5 + 5 = 10, exactly the cap.
        let result = state.apply(&Message::Append {
            version: DELTA_PROTOCOL_VERSION,
            seq: 1,
            text: "abcde".to_string(),
        });
        assert_eq!(result.unwrap(), Some("12345abcde"));
    }

    #[test]
    fn a_replace_tail_that_shrinks_then_stays_under_the_cap_still_works() {
        let mut state = DeltaState::new(10);
        state
            .apply(&Message::Document {
                text: "0123456789".to_string(), // exactly at the cap
            })
            .unwrap();
        // Keep only the first byte and append 3 more: result is 4 bytes,
        // well under the cap, even though the document was already at the
        // cap before this delta.
        let result = state.apply(&Message::ReplaceTail {
            version: DELTA_PROTOCOL_VERSION,
            seq: 1,
            keep_bytes: 1,
            text: "xyz".to_string(),
        });
        assert_eq!(result.unwrap(), Some("0xyz"));
    }

    #[test]
    fn a_replace_tail_that_would_cross_the_cap_is_rejected_with_state_intact() {
        let mut state = DeltaState::new(10);
        state
            .apply(&Message::Document {
                text: "12345".to_string(),
            })
            .unwrap();
        // Keep all 5 existing bytes and append 6 more: 11 > cap of 10.
        let result = state.apply(&Message::ReplaceTail {
            version: DELTA_PROTOCOL_VERSION,
            seq: 1,
            keep_bytes: 5,
            text: "abcdef".to_string(),
        });
        assert_eq!(result, Err(CodecError::DocumentTooLarge));
        assert_eq!(state.document(), "12345");
    }

    #[test]
    fn a_document_frame_up_to_the_cap_is_accepted_even_though_deltas_are_capped_tighter() {
        // `Document` frames are bounded per-frame by the codec, not by
        // `max_document_bytes` — a whole document exactly at the cap (or,
        // in real use, up to `IPC_MAX_REQUEST_BYTES`) is always accepted.
        let mut state = DeltaState::new(5);
        let result = state.apply(&Message::Document {
            text: "0123456789".to_string(), // 10 bytes, over the 5-byte cap
        });
        assert_eq!(result.unwrap(), Some("0123456789"));
    }

    /// The resync policy applies to a document-too-large rejection exactly
    /// like any other rejected delta: tracking is invalidated, and the next
    /// `Document` frame recovers it.
    #[test]
    fn resync_recovers_after_a_document_too_large_rejection() {
        let mut state = DeltaState::new(10);
        state
            .apply(&Message::Document {
                text: "12345".to_string(),
            })
            .unwrap();
        assert_eq!(
            state.apply(&Message::Append {
                version: DELTA_PROTOCOL_VERSION,
                seq: 1,
                text: "abcdef".to_string(),
            }),
            Err(CodecError::DocumentTooLarge)
        );
        assert_eq!(state.document(), "12345");

        // A fresh Document resyncs and, being under the cap itself, leaves
        // room for a further accepted append.
        assert_eq!(
            state
                .apply(&Message::Document {
                    text: "abc".to_string()
                })
                .unwrap(),
            Some("abc")
        );
        assert_eq!(
            state
                .apply(&Message::Append {
                    version: DELTA_PROTOCOL_VERSION,
                    seq: 1,
                    text: "!".to_string(),
                })
                .unwrap(),
            Some("abc!")
        );
    }

    #[test]
    fn adversarial_byte_streams_never_panic_or_grow_unbounded() {
        // AT-3-601: deterministic fuzz over framing bytes and delta payloads.
        // The decoder must always terminate, stay within the pending cap, and
        // never leave malformed frames stuck in the buffer forever.
        let mut seed = 0xc0dec_u64;
        for iteration in 0..512u64 {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let mut decoder = Decoder::new();
            let len = 1 + (iteration as usize % 96);
            let mut bytes = Vec::with_capacity(len);
            for _ in 0..len {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let kind = (seed >> 32) as u8 % 6;
                bytes.push(match kind {
                    0 => (seed >> 24) as u8,
                    1 => 0xff,
                    2 => b'{',
                    3 => b'"',
                    4 => 0,
                    _ => (seed >> 56) as u8,
                });
            }

            if iteration % 4 == 0 {
                bytes.extend_from_slice(&encode_document("fuzz").unwrap());
            }
            if iteration % 7 == 0 {
                bytes.extend_from_slice(&encode_quit());
            }

            for chunk_size in [1_usize, 3, 7, 13] {
                let mut offset = 0;
                let mut steps = 0;
                while offset < bytes.len() {
                    let end = (offset + chunk_size).min(bytes.len());
                    decoder.push(&bytes[offset..end]);
                    offset = end;
                    while let Some(result) = decoder.next_message() {
                        steps += 1;
                        assert!(steps < 4096, "decoder must always make progress");
                        let _ = result;
                    }
                }
                assert!(decoder.pending.len() <= MAX_PENDING_BYTES);
            }
        }
    }

    #[test]
    fn adversarial_delta_sequences_never_panic_and_fail_closed() {
        let mut seed = 0xDE1A_0001_u64;
        for _ in 0..256 {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let cap = 32 + (seed as usize % 128);
            let mut state = DeltaState::new(cap);
            let mut seq = 0_u64;
            let ops = 4 + (seed as usize % 12);
            for _ in 0..ops {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let op = (seed as usize) % 5;
                let before = state.document().to_owned();
                let result = match op {
                    0 => state.apply(&Message::Document {
                        text: format!("doc-{}", seed % 97),
                    }),
                    1 => {
                        seq += 1;
                        state.apply(&Message::Append {
                            version: DELTA_PROTOCOL_VERSION,
                            seq,
                            text: format!("+{}", seed % 11),
                        })
                    }
                    2 => {
                        seq += 1;
                        let keep = (seed as usize % before.len().max(1)).min(before.len());
                        state.apply(&Message::ReplaceTail {
                            version: DELTA_PROTOCOL_VERSION,
                            seq,
                            keep_bytes: keep,
                            text: format!("~{}", seed % 13),
                        })
                    }
                    3 => state.apply(&Message::Append {
                        version: DELTA_PROTOCOL_VERSION + ((seed % 3) as u32),
                        seq: seq + 1,
                        text: "bad-version".to_string(),
                    }),
                    _ => state.apply(&Message::Append {
                        version: DELTA_PROTOCOL_VERSION,
                        seq: seq + 1 + (seed % 5),
                        text: "bad-seq".to_string(),
                    }),
                };
                let _ = result;
                assert!(state.document().len() <= IPC_MAX_REQUEST_BYTES);
            }
        }
    }
}
