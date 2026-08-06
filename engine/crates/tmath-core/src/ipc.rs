//! Shared input-size limits for document paths that accept user text.

/// Maximum bytes accepted for a document payload. Sized above the native
/// scanner input limit to leave room for envelope overhead when needed.
pub const IPC_MAX_REQUEST_BYTES: usize = 192 * 1024 * 1024;
