//! Terminal-facing core for the standalone `tmath` math/document renderer.
//!
//! Phase 0 ports the pieces of the reference pixel-core crate that own the
//! terminal: Kitty graphics escape construction, terminal init/reset, mouse
//! parsing, and the scroll state machine. This crate has no Herdr coupling and
//! never touches a socket, manifest, or plugin runtime.

pub mod kitty;
pub mod mouse;
pub mod scroll;
pub mod terminal;

/// Returns the crate version from the package manifest.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::version;

    #[test]
    fn version_is_not_empty() {
        assert!(!version().is_empty());
    }
}
