//! tmux command construction for the agent watcher.
//!
//! The watcher shells out to the `tmux` binary with a fixed, allowlisted set of
//! subcommands and flags; the only variable inputs are a validated pane id and
//! our own controlled socket/exe paths. Nothing user-controlled reaches the
//! shell, so no pipeline involves shell evaluation of agent content.

/// A validated tmux pane id such as `%37`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneId(String);

impl PaneId {
    /// Builds a pane id from a raw string, rejecting anything that is not a
    /// `%` followed by digits (the `$TMUX_PANE` format).
    pub fn new(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        valid_pane_id(trimmed).then(|| Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A pane id is `%` followed by one or more decimal digits (tmux's numeric
/// pane ids assigned to every pane).
pub fn valid_pane_id(raw: &str) -> bool {
    let trimmed = raw.trim();
    let mut chars = trimmed.chars();
    chars.next() == Some('%') && chars.all(|c| c.is_ascii_digit()) && !trimmed.is_empty()
}

/// Splits the source pane to the right, running `viewer_command` in the new
/// pane, and prints the new pane id so the watcher can track it.
pub fn split_viewer(percent: u32, source: &PaneId, viewer_command: &str) -> Vec<String> {
    vec![
        "split-window".into(),
        "-h".into(),
        "-p".into(),
        percent.to_string(),
        "-P".into(),
        "-F".into(),
        "#{pane_id}".into(),
        "-t".into(),
        source.as_str().into(),
        viewer_command.into(),
    ]
}

/// Captures the source pane to stdout, including up to `history` lines of
/// scrollback above the visible area so long answers are not silently lost.
pub fn capture(source: &PaneId, history: u32) -> Vec<String> {
    vec![
        "capture-pane".into(),
        "-p".into(),
        "-t".into(),
        source.as_str().into(),
        "-S".into(),
        format!("-{history}"),
    ]
}

/// Asks tmux to print the pane id, succeeding only when the pane still exists.
pub fn display_pane(source: &PaneId) -> Vec<String> {
    vec![
        "display-message".into(),
        "-p".into(),
        "-t".into(),
        source.as_str().into(),
        "#{pane_id}".into(),
    ]
}

/// Kills a viewer pane by id.
pub fn kill_pane(pane: &PaneId) -> Vec<String> {
    vec!["kill-pane".into(), "-t".into(), pane.as_str().into()]
}

/// Quotes a single argument for the shell that tmux uses to run the split
/// command. Only our own controlled paths are quoted here.
pub fn shell_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for ch in value.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_ids_are_percent_digits() {
        assert!(valid_pane_id("%37"));
        assert!(valid_pane_id("%0"));
        assert!(PaneId::new("%37").is_some());
        assert!(PaneId::new("0").is_none(), "missing %");
        assert!(PaneId::new("%abc").is_none(), "non-digits");
        assert!(PaneId::new("").is_none());
        assert!(PaneId::new("%37\n").is_some(), "trailing newline trimmed");
    }

    #[test]
    fn split_viewer_targets_the_source_pane() {
        let pane = PaneId::new("%9").unwrap();
        let cmd = split_viewer(35, &pane, "/usr/local/bin/tmath agent-viewer /tmp/x.sock");
        assert_eq!(
            cmd,
            vec![
                "split-window",
                "-h",
                "-p",
                "35",
                "-P",
                "-F",
                "#{pane_id}",
                "-t",
                "%9",
                "/usr/local/bin/tmath agent-viewer /tmp/x.sock"
            ]
        );
    }

    #[test]
    fn capture_request_pulls_bounded_history() {
        let pane = PaneId::new("%2").unwrap();
        let cmd = capture(&pane, 500);
        assert_eq!(cmd, vec!["capture-pane", "-p", "-t", "%2", "-S", "-500"]);
    }

    #[test]
    fn display_and_kill_are_scoped_to_the_pane() {
        let pane = PaneId::new("%5").unwrap();
        assert_eq!(
            display_pane(&pane),
            vec!["display-message", "-p", "-t", "%5", "#{pane_id}"]
        );
        assert_eq!(kill_pane(&pane), vec!["kill-pane", "-t", "%5"]);
    }

    #[test]
    fn shell_quote_handles_single_quotes() {
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote("/tmp/tmath.sock"), "'/tmp/tmath.sock'");
    }
}
