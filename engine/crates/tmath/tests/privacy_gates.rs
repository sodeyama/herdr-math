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

/// Production-only source: everything before the first `#[cfg(test)]` module.
fn production_rust_source(path: &Path) -> String {
    let source = fs::read_to_string(path).unwrap();
    source
        .split_once("\n#[cfg(test)]")
        .map(|(production, _)| production.to_string())
        .unwrap_or(source)
}

fn transcript_agent_source_paths(root: &Path) -> Vec<PathBuf> {
    [
        root.join("engine/crates/tmath/src/transcript_adapter.rs"),
        root.join("engine/crates/tmath/src/agent_watcher.rs"),
    ]
    .into_iter()
    .filter(|path| path.is_file())
    .collect()
}

/// AT-3-605: the transcript adapter must never log or print assistant/user
/// transcript bytes — it has no logging surface at all in production code.
#[test]
fn transcript_adapter_has_no_logging_in_production() {
    let root = workspace_root();
    let path = root.join("engine/crates/tmath/src/transcript_adapter.rs");
    let source = production_rust_source(&path);
    for forbidden in ["eprintln!", "println!", "print!", "log::", "tracing::"] {
        assert!(
            !source.contains(forbidden),
            "transcript_adapter must not use {forbidden} (found in {})",
            path.display()
        );
    }
}

/// AT-3-605: watcher stderr may name events and bounded counts only.
#[test]
fn agent_watcher_stderr_lines_are_content_free() {
    let root = workspace_root();
    let path = root.join("engine/crates/tmath/src/agent_watcher.rs");
    let source = production_rust_source(&path);
    for line in source.lines() {
        if !line.contains("eprintln!") {
            continue;
        }
        assert!(
            !line.contains("answer.text"),
            "eprintln must not interpolate answer content: {line}"
        );
        assert!(
            !line.contains("{text}") && !line.contains(", text)") && !line.contains(", &text)"),
            "eprintln must not interpolate raw text: {line}"
        );
    }
    for forbidden in [
        "eprintln!(\"{}\", text)",
        "eprintln!(\"{}\", &text)",
        "eprintln!(\"{}\", document",
        "eprintln!(\"{}\", &document",
        "eprintln!(\"{}\", line",
    ] {
        assert!(
            !source.contains(forbidden),
            "agent_watcher must not log content via {forbidden}"
        );
    }
    assert!(
        source.contains("document_sent bytes={}"),
        "document byte counts remain the bounded watcher metric"
    );
}

/// AT-3-605: transcript-path sources must not persist transcript bytes.
#[test]
fn transcript_path_sources_never_write_transcript_bytes() {
    let root = workspace_root();
    for path in transcript_agent_source_paths(&root) {
        let source = production_rust_source(&path);
        assert!(
            !source.contains("fs::write(") && !source.contains("OpenOptions::new().write(true)"),
            "{} must not write transcript or pane content to disk",
            path.display()
        );
    }
}
