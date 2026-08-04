//! Static privacy and security gates for the Rust/TS split.
//!
//! These assert the source invariants that cannot change without review: the
//! terminal-facing crates never import a network socket API, never evaluate
//! user-provided strings as commands, and never embed an absolute home path in
//! committed source. They scan the workspace Rust sources so a regression trips
//! a normal `cargo test`.

use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .canonicalize()
        .unwrap()
}

fn rust_sources(root: &Path) -> Vec<PathBuf> {
    let mut sources = Vec::new();
    collect_rs(&root.join("engine"), &mut sources);
    sources
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs")
            && !path.ends_with("build.rs")
            && !path.ends_with("privacy_gates.rs")
        {
            out.push(path);
        }
    }
}

#[test]
fn no_network_sockets_in_the_terminal_crates() {
    let root = workspace_root();
    let sources = rust_sources(&root);
    assert!(!sources.is_empty(), "found Rust sources to audit");
    for path in sources {
        let source = fs::read_to_string(&path).unwrap();
        if source.contains("std::net")
            || source.contains("TcpStream")
            || source.contains("UdpSocket")
            || source.contains("reqwest")
        {
            panic!("network socket import in {}", path.display());
        }
    }
}

#[test]
fn no_shell_eval_of_user_input() {
    let root = workspace_root();
    for path in rust_sources(&root) {
        let source = fs::read_to_string(&path).unwrap();
        // Test modules contain inert adversarial input fixtures such as
        // `#eval(...)`; audit only the compiled production portion.
        let source = source
            .split_once("\n#[cfg(test)]")
            .map_or(source.as_str(), |(production, _)| production);
        // The documented renderer/native-helper spawns use fixed paths; an eval
        // or variable-driven shell invocation would be a new threat surface.
        if source.contains("eval(") || source.contains("sh -c") {
            panic!("shell-eval pattern in {}", path.display());
        }
    }
}

#[test]
fn no_absolute_user_paths_in_committed_source() {
    let root = workspace_root();
    for path in rust_sources(&root) {
        let source = fs::read_to_string(&path).unwrap();
        if source.contains("/Users/") || source.contains("/home/") {
            panic!("absolute home path in {}", path.display());
        }
    }
}
