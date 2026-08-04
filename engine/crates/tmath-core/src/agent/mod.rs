//! Agent integration: tmux pane wiring, answer-boundary detection, and the
//! bounded message channel that feeds rendered documents to the viewer pane.
//!
//! Everything here is local: the watcher captures a tmux pane, proves a new
//! answer boundary, sends the answer document to a viewer process over a Unix
//! socket, and never persists or logs answer content. No agent-specific API or
//! network service is involved.

pub mod boundary;
pub mod codec;
pub mod tmux;

pub use boundary::{find_answer, is_prompt_line, is_status_line, Answer};
pub use codec::{
    encode_append, encode_document, encode_quit, encode_replace_tail, CodecError, Decoder,
    DeltaState, Message, DELTA_PROTOCOL_VERSION,
};
pub use tmux::{
    capture, display_pane, kill_pane, shell_quote, split_viewer, valid_pane_id, PaneId,
};
