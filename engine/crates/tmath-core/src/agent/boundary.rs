//! Answer-boundary detection for the tmux agent viewer.
//!
//! `tmath agent` captures the agent pane at a bounded poll interval and passes
//! each new snapshot through [`find_answer`], which returns the display text of
//! the most recent agent answer when a boundary can be proven. The logic is
//! conservative: ambiguity, truncation, or pure repaint returns `None` so the
//! watcher fails closed and never renders a partial or shifted answer.
//!
//! The detection mirrors the strategies recorded in
//! `tests/fixtures/agents/answer-corpus.json` (exact prefix, stable prefix with
//! volatile working frames, and no-provenance rejection) but returns display
//! text: trailing prompt glyphs and repainted working frames are stripped, and
//! a prompt-only tail yields no answer.

/// A detected answer region, ready for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub text: String,
}

/// Whether a line is a recognized terminal prompt glyph line. These are treated
/// as answer boundaries, never as answer content.
///
/// Recognized prompts: `❯` (Claude Code), `›` (Codex), and opencode's
/// `┃ prompt:` marker. Agent prompts that are plain text with an inline
/// marker (for example pi's `Current prompt > ...`) are not recognized yet.
pub fn is_prompt_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("❯") || trimmed.starts_with("›") || trimmed.starts_with("┃ prompt:")
}

/// Whether a line is a volatile "working frame" that agents repaint in place
/// while they are still working. A changed working frame is not answer content.
pub fn is_status_line(line: &str) -> bool {
    let lower = line.trim_start().to_lowercase();
    [
        "• working",
        "* working",
        "working on the answer",
        "working…",
        "working...",
        "┃ working",
        "… working",
        "▌ working",
        "■ working",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
}

/// Returns the newest answer text between two consecutive pane snapshots, or
/// `None` when no answer boundary can be proven.
pub fn find_answer(baseline: &str, completion: &str) -> Option<Answer> {
    if baseline == completion {
        return None;
    }

    let (tail, _) = if completion.starts_with(baseline) {
        (
            completion
                .strip_prefix(baseline)
                .map(str::to_string)
                .unwrap_or_default(),
            0,
        )
    } else {
        let bl: Vec<&str> = baseline.lines().collect();
        let cl: Vec<&str> = completion.lines().collect();
        let mut index = 0;
        while index < bl.len() && index < cl.len() && bl[index] == cl[index] {
            index += 1;
        }
        if index >= cl.len() {
            return None;
        }
        // When the baseline shares no leading line with the completion, the
        // pane was rewritten wholesale and no answer boundary can be proven:
        // fail closed rather than render the full replacement.
        if index == 0 && !bl.is_empty() {
            return None;
        }
        let mut tail_lines: Vec<&str> = cl[index..].to_vec();
        if start_is_repaint(&bl, index, tail_lines.first().copied().unwrap_or("")) {
            tail_lines.remove(0);
        }
        if tail_lines.is_empty() {
            return None;
        }
        (tail_lines.join("\n"), index)
    };

    if let Some(text) = clean_tail(&tail) {
        if text.trim().is_empty() {
            return None;
        }
        Some(Answer { text })
    } else {
        // The first line in the tail is a repainted working frame even on the
        // exact-prefix path, so retry after dropping it.
        let first = tail.lines().next().unwrap_or("");
        if is_status_line(first) {
            let rest = tail.lines().skip(1).collect::<Vec<_>>().join("\n");
            clean_tail(&rest).map(|text| Answer { text })
        } else {
            None
        }
    }
}

/// Whether the first new line is a working frame being repainted in place
/// rather than fresh answer content.
fn start_is_repaint(bl: &[&str], index: usize, first: &str) -> bool {
    if !is_status_line(first) {
        return false;
    }
    if bl.get(index).is_some_and(|line| is_status_line(line)) {
        return true;
    }
    if index > 0 && bl.get(index - 1).is_some_and(|line| is_status_line(line)) {
        return true;
    }
    // A baseline that held only working frames is entirely volatile.
    bl.is_empty() || bl.iter().all(|line| is_status_line(line))
}

/// Strips leading blank lines, trailing prompt lines, and trailing blank lines
/// from a candidate answer tail.
fn clean_tail(tail: &str) -> Option<String> {
    let lines: Vec<&str> = tail.lines().collect();
    let first = lines.iter().position(|line| !line.trim().is_empty())?;
    let mut out: Vec<&str> = lines[first..].to_vec();
    while let Some(last) = out.last() {
        if last.trim().is_empty() || is_prompt_line(last) {
            out.pop();
        } else {
            break;
        }
    }
    if out.is_empty() {
        return None;
    }
    Some(out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(baseline: &str, completion: &str) -> Option<String> {
        find_answer(baseline, completion).map(|a| a.text)
    }

    #[test]
    fn no_change_has_no_answer() {
        let snapshot = "❯ Derive the result.\n";
        assert_eq!(find_answer(snapshot, snapshot), None);
    }

    #[test]
    fn appending_content_is_an_exact_prefix_answer() {
        // claude-prompt-only-exact from answer-corpus.json.
        let baseline = "❯ Derive the result.\n";
        let completion = "❯ Derive the result.\nThe answer is $x=2$.\n❯ ";
        assert_eq!(
            answer(baseline, completion).as_deref(),
            Some("The answer is $x=2$.")
        );
    }

    #[test]
    fn a_repainted_working_frame_is_not_answer_content() {
        // codex-repaint-stable-prefix from answer-corpus.json.
        let baseline = {
            let mut lines = Vec::new();
            for i in 1..=5 {
                lines.push(format!("Stable history {i:02} abcdefghijklmnopqrstuvwxyz"));
            }
            lines.push("› Compute the integral.".into());
            lines.push("• Working frame 01".into());
            lines.join("\n")
        };
        let completion = baseline
            .lines()
            .map(|line| {
                if line == "• Working frame 01" {
                    "• Working frame 02"
                } else {
                    line
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\nResult: $1/3$.\n› ";
        let expected = "Result: $1/3$.";
        assert_eq!(answer(&baseline, &completion).as_deref(), Some(expected));
    }

    #[test]
    fn opencode_answer_line_after_a_stable_prefix() {
        let baseline = "┃ verified window 01 abcdefghijklmnopqrstuvwxyz\n┃ prompt: integrate\n";
        let completion = format!("{baseline}┃ answer: $$\\frac{{1}}{{3}}$$\n┃ prompt: \n");
        assert_eq!(
            answer(baseline, &completion).as_deref(),
            Some("┃ answer: $$\\frac{1}{3}$$")
        );
    }

    #[test]
    fn appended_content_uses_the_last_prompt_as_the_boundary() {
        // cursor-tool-then-final from answer-corpus.json.
        let baseline = "Grepped pattern in src\n\n  • Working on the answer.\n";
        let completion =
            format!("{baseline}\nRead package.json\n\n  • The relation is $E=mc^2$.\n");
        // MVP keeps the tool-activity line; the answer boundary is still the
        // appended tail after the previous capture.
        assert_eq!(
            answer(baseline, &completion).as_deref(),
            Some("Read package.json\n\n  • The relation is $E=mc^2$.")
        );
    }

    #[test]
    fn an_inline_formula_inside_the_answer_is_kept() {
        let baseline = "❯ Compute.\n";
        let completion = "❯ Compute.\nThe relation is $E=mc^2$. and display $$a^2+b^2=c^2$$.\n❯ ";
        assert_eq!(
            answer(baseline, completion).as_deref(),
            Some("The relation is $E=mc^2$. and display $$a^2+b^2=c^2$$.")
        );
    }

    #[test]
    fn a_prompt_only_tail_is_not_an_answer() {
        let baseline = "❯ Run it.\n";
        let completion = "❯ Run it.\n❯ ";
        assert_eq!(answer(baseline, completion), None);
    }

    #[test]
    fn a_solitary_repainted_frame_settles_to_nothing() {
        let baseline = "• Working frame 01\n";
        let completion = "• Working frame 02\n";
        assert_eq!(answer(baseline, completion), None);
    }

    #[test]
    fn an_unrecoverable_rewrite_is_rejected() {
        // The pane content is completely rewritten with no common prefix or
        // overlap; the boundary cannot be proven, so we fail closed.
        let baseline = "Old line A\nOld line B\n❯ prompt\nWorking…\n└────────────┘\n";
        let completion = "Previous answer\nCurrent prompt > solve unique request 1234567890\nSolution: $$x=4$$\n";
        assert_eq!(answer(baseline, completion), None);
    }

    #[test]
    fn prompt_and_status_detection() {
        assert!(is_prompt_line("❯ Derive"));
        assert!(is_prompt_line("  › codex"));
        assert!(is_prompt_line("┃ prompt: integrate"));
        assert!(!is_prompt_line("┃ answer: $$x$$"));
        assert!(!is_prompt_line("> a blockquote"));
        assert!(is_status_line("• Working frame 09"));
        assert!(is_status_line("  Working on the answer."));
        assert!(!is_status_line("• First point"));
        assert!(!is_status_line("• Release notes"));
        assert!(!is_status_line("The answer is $x=2$."));
        assert!(!is_status_line("Read package.json"));
    }
}
