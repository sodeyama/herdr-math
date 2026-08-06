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
/// Recognized prompts: `❯` (Claude Code), `›` (Codex), opencode's
/// `┃ prompt:` marker, and pi's `Current prompt > ...` marker.
pub fn is_prompt_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("❯")
        || trimmed.starts_with("›")
        || trimmed.starts_with("┃ prompt:")
        || trimmed.starts_with("Current prompt >")
}

/// Whether a line is a volatile "working frame" that agents repaint in place
/// while they are still working. A changed working frame is not answer content.
pub fn is_status_line(line: &str) -> bool {
    let lower = line.trim_start().to_lowercase();
    if is_reasoning_or_completion_status(line) {
        return true;
    }
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

    let bl: Vec<&str> = baseline.lines().collect();
    let cl: Vec<&str> = completion.lines().collect();
    if cl.iter().any(|line| is_delimited_prompt_line(line)) {
        if let Some(answer) = prompt_delimited_answer(&cl) {
            if prompt_delimited_answer(&bl).as_ref() == Some(&answer) {
                return None;
            }
            return Some(answer);
        }
        if let Some(answer) = claude_in_progress_answer(&cl) {
            let baseline_answer = prompt_delimited_answer(&bl)
                .or_else(|| claude_in_progress_answer(&bl));
            if baseline_answer.as_ref() == Some(&answer) {
                return None;
            }
            return Some(answer);
        }
        // Idle prompt-delimited extraction failed (streaming command output
        // under a non-idle `❯ cmd`, or Claude still repainting). Fall through
        // to prefix-based detection so growing pane snapshots can still emit
        // incrementally; wholesale repaints without a shared prefix remain
        // fail-closed (see `claude_repaint_without_idle_prompt_is_not_complete`).
    }

    let (tail, _) = if completion.starts_with(baseline) {
        if !completion.contains('⏺') {
            if baseline.lines().last().is_some_and(is_user_input_line) {
                return None;
            }
            if !tail_contains_newline_after_prefix(baseline, completion)
                && baseline
                    .lines()
                    .last()
                    .is_some_and(is_claude_compose_input_line)
            {
                return None;
            }
        }
        (
            completion
                .strip_prefix(baseline)
                .map(str::to_string)
                .unwrap_or_default(),
            0,
        )
    } else {
        let mut index = 0;
        while index < bl.len() && index < cl.len() && bl[index] == cl[index] {
            index += 1;
        }
        if index >= cl.len() {
            return None;
        }
        // When the baseline shares no leading line with the completion, the
        // pane was rewritten wholesale and no answer boundary can be proven:
        // prefer explicit prompt delimiters. A recognized non-idle final
        // prompt means the agent is still working, so fail closed rather than
        // accepting a weak overlap with repeated TUI chrome.
        if index == 0 && !bl.is_empty() {
            return contextual_answer(&bl, &cl).or_else(|| sliding_window_answer(&bl, &cl));
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

    if !completion.contains('⏺')
        && (tail_is_only_user_input(&tail) || tail_is_non_answer_noise(&tail))
    {
        return None;
    }

    if let Some(text) = clean_tail(&tail) {
        let mut text = strip_chrome_and_system_lines(&text);
        if !completion.contains('⏺') {
            text = strip_claude_compose_lines(&text);
            if is_prompt_suffix_fragment(&text) {
                return None;
            }
        }
        if text.trim().is_empty() {
            return None;
        }
        Some(Answer {
            text: strip_tool_activity_prefix(&text),
        })
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

/// Extracts the response between the preceding user prompt and the final idle
/// prompt used by Claude Code, Codex, and opencode after completion.
fn prompt_delimited_answer(completion: &[&str]) -> Option<Answer> {
    let end = completion
        .iter()
        .rposition(|line| is_delimited_prompt_line(line))?;
    if !is_idle_prompt_line(completion[end]) {
        return None;
    }
    let start = completion[..end]
        .iter()
        .rposition(|line| is_delimited_prompt_line(line))?
        + 1;
    let mut body = completion[start..end].to_vec();
    body = filter_answer_body_lines(body);
    let text = clean_tail(&body.join("\n"))?;
    (!text.trim().is_empty()).then(|| Answer {
        text: normalize_captured_answer(&strip_tool_activity_prefix(&text)),
    })
}

/// Extracts a Claude Code answer that is still streaming: the pane shows the
/// user prompt and growing `⏺` body but not yet the idle trailing `❯`.
fn claude_in_progress_answer(completion: &[&str]) -> Option<Answer> {
    if !completion
        .iter()
        .any(|line| line.trim_start().starts_with('⏺'))
    {
        return None;
    }
    let start = completion
        .iter()
        .rposition(|line| is_delimited_prompt_line(line) && !is_idle_prompt_line(line))?
        + 1;
    let body = filter_answer_body_lines(completion.get(start..)?.to_vec());
    if !body
        .iter()
        .any(|line| line.trim_start().starts_with('⏺'))
    {
        return None;
    }
    let text = clean_tail(&body.join("\n"))?;
    (!text.trim().is_empty()).then(|| Answer {
        text: normalize_captured_answer(&strip_tool_activity_prefix(&text)),
    })
}

fn is_delimited_prompt_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("❯") || trimmed.starts_with("›") || trimmed.starts_with("┃ prompt:")
}

/// Whether a line is user input being composed, not assistant answer text.
fn is_user_input_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with('⏺') || is_idle_prompt_line(line) {
        return false;
    }
    is_claude_compose_input_line(line)
        || trimmed.starts_with('❯')
        || trimmed.starts_with('›')
        || trimmed.starts_with("┃ prompt:")
}

/// Claude Code's `>` compose prompt, distinct from the `❯` delimiter glyph.
fn is_claude_compose_input_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with('>') && !trimmed.starts_with('⏺')
}

fn tail_is_only_user_input(tail: &str) -> bool {
    let lines: Vec<&str> = tail.lines().filter(|line| !line.trim().is_empty()).collect();
    lines.is_empty() || lines.iter().all(|line| is_user_input_line(line))
}

fn tail_is_non_answer_noise(tail: &str) -> bool {
    let lines: Vec<&str> = tail.lines().filter(|line| !line.trim().is_empty()).collect();
    lines.is_empty()
        || lines.iter().all(|line| {
            is_agent_chrome_or_system_line(line)
                || is_reasoning_or_completion_status(line)
                || is_claude_compose_input_line(line)
                || is_tool_activity_line(line.trim())
        })
}

fn tail_contains_newline_after_prefix(baseline: &str, completion: &str) -> bool {
    completion
        .strip_prefix(baseline)
        .is_some_and(|suffix| suffix.starts_with('\n') || suffix.starts_with("\r\n"))
}

/// Claude Code header chrome, environment hints, and other non-answer TUI lines.
fn is_agent_chrome_or_system_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.starts_with('→') {
        return true;
    }
    if trimmed.contains("git:(")
        && trimmed.contains('[')
        && (trimmed.contains("/rc") || trimmed.contains("for agents"))
    {
        return true;
    }
    if trimmed.contains("for agents") {
        return true;
    }
    if trimmed.contains("auto mode on")
        || trimmed.contains("auto-mode on")
        || trimmed.contains("shift+tab to cycle")
    {
        return true;
    }
    if trimmed.starts_with('▸') {
        return true;
    }
    if trimmed.contains("tmux focus-events")
        || trimmed.contains("~/.tmux.conf")
        || trimmed.contains("reattach for focus")
    {
        return true;
    }
    if trimmed.contains("Claude Code v") || trimmed.contains("MCP servers need authentication") {
        return true;
    }
    trimmed.starts_with("tmath agent:")
}

fn strip_claude_compose_lines(text: &str) -> String {
    text.lines()
        .filter(|line| !is_claude_compose_input_line(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_prompt_suffix_fragment(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.lines().count() == 1
        && !trimmed.starts_with('#')
        && !trimmed.starts_with("**")
        && !trimmed.contains('$')
        && !trimmed.starts_with('|')
        && !trimmed.starts_with('•')
        && !trimmed.starts_with("❯")
        && trimmed.chars().count() <= 16
}

fn strip_chrome_and_system_lines(text: &str) -> String {
    text.lines()
        .filter(|line| !is_agent_chrome_or_system_line(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_idle_prompt_line(line: &str) -> bool {
    let trimmed = line.trim();
    matches!(trimmed, "❯" | "›" | "┃ prompt:")
}

fn is_completion_status(line: &str) -> bool {
    line.starts_with('✻')
}

/// Claude Code reasoning/timing frames (`* Pontificating…`, `✻ Brewed…`, etc.).
fn is_reasoning_or_completion_status(line: &str) -> bool {
    let trimmed = line.trim();
    if !(trimmed.starts_with('✻') || trimmed.starts_with('*')) {
        return false;
    }
    let lower = trimmed.to_lowercase();
    [
        "pontificat",
        "thinking",
        "tokens",
        "brewed",
        "sautéed",
        "sauteed",
        "churned",
        "cooked",
        "working",
        "escuargot",
        "ferment",
        "marinating",
        "simmer",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn answer_line_text(line: &str) -> &str {
    line.trim_start()
        .strip_prefix("⏺ ")
        .or_else(|| line.trim_start().strip_prefix('⏺'))
        .map(str::trim_start)
        .unwrap_or_else(|| line.trim())
}

fn filter_answer_body_lines(body: Vec<&str>) -> Vec<&str> {
    let flags: Vec<bool> = body
        .iter()
        .enumerate()
        .map(|(index, line)| {
            if line.trim().is_empty() {
                let prev_display = body[..index]
                    .iter()
                    .rev()
                    .find(|candidate| !candidate.trim().is_empty())
                    .is_some_and(|line| is_display_answer_line(line));
                let next_display = body[index + 1..]
                    .iter()
                    .find(|candidate| !candidate.trim().is_empty())
                    .is_some_and(|line| is_display_answer_line(line));
                prev_display && next_display
            } else {
                is_display_answer_line(line)
            }
        })
        .collect();
    body.into_iter()
        .zip(flags)
        .filter_map(|(line, keep)| keep.then_some(line))
        .collect()
}

fn is_display_answer_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty()
        || is_user_input_line(line)
        || is_claude_compose_input_line(line)
        || is_idle_prompt_line(line)
        || is_tui_rule(trimmed)
        || is_agent_chrome_or_system_line(line)
    {
        return false;
    }
    let content = answer_line_text(trimmed);
    if content.is_empty() {
        return false;
    }
    if is_reasoning_or_completion_status(trimmed)
        || is_reasoning_or_completion_status(content)
        || is_status_line(content)
        || is_completion_status(content)
        || is_tool_activity_line(content)
        || content.starts_with("$ ")
    {
        return false;
    }
    true
}

fn is_tui_rule(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.chars().count() >= 20
        && trimmed
            .chars()
            .all(|character| matches!(character, '─' | '━'))
}

/// Converts presentation-only structures emitted by coding-agent TUIs back to
/// the allowlisted Markdown understood by the renderer.
fn normalize_captured_answer(text: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if let Some(first) = lines.first_mut() {
        let trimmed = first.trim_start();
        if let Some(rest) = trimmed.strip_prefix("⏺ ") {
            *first = rest.to_string();
        }
    }

    let mut output = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if is_box_top(&lines[index]) {
            let mut cursor = index + 1;
            let mut rows = Vec::new();
            while cursor < lines.len() {
                if is_box_bottom(&lines[cursor]) {
                    break;
                }
                if let Some(cells) = box_row_cells(&lines[cursor]) {
                    rows.push(cells);
                }
                cursor += 1;
            }
            if cursor < lines.len() && rows.len() >= 2 {
                output.push(markdown_row(&rows[0]));
                output.push(format!(
                    "| {} |",
                    rows[0]
                        .iter()
                        .map(|_| "---")
                        .collect::<Vec<_>>()
                        .join(" | ")
                ));
                output.extend(rows[1..].iter().map(|row| markdown_row(row)));
                index = cursor + 1;
                continue;
            }
        }
        output.push(lines[index].clone());
        index += 1;
    }
    output.join("\n")
}

fn is_box_top(line: &str) -> bool {
    line.trim().starts_with('┌')
}

fn is_box_bottom(line: &str) -> bool {
    line.trim().starts_with('└')
}

fn box_row_cells(line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim();
    if !trimmed.starts_with('│') || !trimmed.ends_with('│') {
        return None;
    }
    let cells: Vec<String> = trimmed
        .trim_matches('│')
        .split('│')
        .map(|cell| cell.trim().replace('|', "\\|"))
        .collect();
    (cells.len() >= 2).then_some(cells)
}

fn markdown_row(cells: &[String]) -> String {
    format!("| {} |", cells.join(" | "))
}

/// Recovers a bounded tail when tmux capture history slides: a suffix of the
/// baseline must exactly match the completion prefix before new lines.
fn sliding_window_answer(baseline: &[&str], completion: &[&str]) -> Option<Answer> {
    for start in 1..baseline.len() {
        let suffix = &baseline[start..];
        if suffix.len() < 2
            || completion.len() <= suffix.len()
            || completion.get(..suffix.len()) != Some(suffix)
        {
            continue;
        }
        let text = clean_tail(&completion[suffix.len()..].join("\n"))?;
        if text.trim().is_empty() {
            return None;
        }
        return Some(Answer {
            text: strip_tool_activity_prefix(&text),
        });
    }
    None
}

/// Recovers an answer from TUIs such as pi that repaint/truncate the full
/// screen but repeat the exact current-prompt line around the completed answer.
fn contextual_answer(baseline: &[&str], completion: &[&str]) -> Option<Answer> {
    let anchor = baseline
        .iter()
        .rev()
        .find(|line| line.trim_start().starts_with("Current prompt >"))?;
    let mut matches = completion
        .iter()
        .enumerate()
        .filter_map(|(index, line)| (*line == *anchor).then_some(index));
    let start = matches.next()? + 1;
    let end = matches.next().unwrap_or(completion.len());
    let text = clean_tail(&completion[start..end].join("\n"))?;
    (!text.trim().is_empty()).then(|| Answer {
        text: strip_tool_activity_prefix(&text),
    })
}

/// Cursor CLI can append bounded tool-activity lines immediately before its
/// final bullet. Strip that prefix only when every preceding non-empty line is
/// recognizably tool/status output, preserving ordinary Markdown answers.
fn strip_tool_activity_prefix(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let Some(final_index) = lines
        .iter()
        .rposition(|line| line.trim_start().starts_with("• "))
    else {
        return text.to_string();
    };
    if final_index == 0 {
        return text.to_string();
    }
    let prefix_is_tools = lines[..final_index].iter().all(|line| {
        let trimmed = line.trim();
        trimmed.is_empty() || is_status_line(trimmed) || is_tool_activity_line(trimmed)
    });
    if prefix_is_tools {
        let mut answer = lines[final_index..].to_vec();
        answer[0] = answer[0]
            .trim_start()
            .strip_prefix("• ")
            .unwrap_or(answer[0]);
        answer.join("\n")
    } else {
        text.to_string()
    }
}

fn is_tool_activity_line(line: &str) -> bool {
    [
        "Read ",
        "Grepped ",
        "Searched ",
        "Searched for ",
        "Listed ",
        "Ran ",
        "Running shell command",
        "Running ",
        "Edited ",
        "Wrote ",
        "Viewed ",
    ]
    .iter()
    .any(|prefix| line.starts_with(prefix))
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
        assert_eq!(
            answer(baseline, &completion).as_deref(),
            Some("The relation is $E=mc^2$.")
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
    fn pi_repeated_contextual_prompt_recovers_the_answer() {
        let baseline = "┌ Agent pane ┐\nPrevious prompt > summarize\nPrevious answer\nCurrent prompt > solve unique request 1234567890\nWorking…\n└────────────┘\n";
        let completion = "Previous answer\nCurrent prompt > solve unique request 1234567890\nSolution: $$x=4$$\nCurrent prompt > solve unique request 1234567890\n└────────────┘\n";
        assert_eq!(
            answer(baseline, completion).as_deref(),
            Some("Solution: $$x=4$$")
        );
    }

    #[test]
    fn claude_whole_screen_repaint_uses_prompt_delimiters() {
        let baseline = "Old answer\n────────────────────────\n❯\n────────────────────────\n";
        let completion = "Earlier screen content\n❯ Show equations\n\n⏺ $$x^2 - 4 = 0$$\n\n  Answer: $x = \\pm 2$\n\n✻ Brewed for 6s\n\n────────────────────────\n❯\n────────────────────────\n";
        assert_eq!(
            answer(baseline, completion).as_deref(),
            Some("$$x^2 - 4 = 0$$\n\n  Answer: $x = \\pm 2$")
        );
    }

    #[test]
    fn claude_repaint_without_idle_prompt_still_exposes_streaming_text() {
        let baseline = "Old answer\n────────────────────────\n❯\n────────────────────────\n";
        let completion =
            "Earlier screen content\n❯ Show equations\n\n⏺ Partial answer\n✻ Churned for 2s\n";
        assert_eq!(
            answer(baseline, completion).as_deref(),
            Some("Partial answer")
        );
    }

    #[test]
    fn claude_reasoning_and_tool_lines_are_not_answer_content() {
        let baseline = "❯ 数学アプリの6章表示して\n";
        let completion = "❯ 数学アプリの6章表示して\n\n* Pontificating... (25s · ↓ 536 tokens · thinking with high effort)\nSearched for 1 pattern, read 1 file\n$ open \"https://math-notes.example/ch6\"\n";
        assert_eq!(answer(baseline, completion), None);
    }

    #[test]
    fn claude_final_answer_strips_reasoning_tool_and_timing_lines() {
        let baseline = "❯ 数学アプリの6章表示して\n";
        let completion = "\
❯ 数学アプリの6章表示して\n\n\
* Pontificating... (35s · ↓ 824 tokens)\n\
Searched for 1 pattern, read 1 file\n\
$ open \"https://math-notes.example/ch6\"\n\
⏺ 6章を math-notes で開きました。\n\
✻ Sautéed for 35s\n\n\
────────────────────────\n\
❯\n\
────────────────────────\n";
        assert_eq!(
            answer(baseline, completion).as_deref(),
            Some("6章を math-notes で開きました。")
        );
    }

    #[test]
    fn claude_code_input_line_growth_is_not_answer_content() {
        let baseline = "Earlier screen content\n> 数学";
        let completion = "Earlier screen content\n> 数学アプリの6章表示して";
        assert_eq!(answer(baseline, completion), None);
    }

    #[test]
    fn claude_code_same_line_prompt_suffix_is_not_answer_content() {
        let baseline = "→ obsidian git:(main) [Sonnet 5] ← for agents\n> 数";
        let completion = "→ obsidian git:(main) [Sonnet 5] ← for agents\n> 数式";
        assert_eq!(answer(baseline, completion), None);
    }

    #[test]
    fn claude_code_chrome_and_system_lines_are_not_answer_content() {
        let baseline = "❯\n";
        let completion = "\
→ obsidian git:(main) [Sonnet 5] /rc auto-mode on (shift+tab to cycle) ← for agents\n\
> 数式を出して\n\
tmux focus-events off · add 'set -g focus-events on' to ~/.tmux.conf and reattach for focus tracking\n";
        assert_eq!(answer(baseline, completion), None);
    }

    #[test]
    fn claude_code_status_bar_without_arrow_suffix_is_not_answer_content() {
        let baseline = "Earlier screen\n";
        let completion = "\
→ terminal-math git:(main) [Sonnet 5]                      /rc\n\
> 数式を５０個だして\n\
▸▸ auto mode on (shift+tab to cycle)\n";
        assert_eq!(answer(baseline, completion), None);
    }

    #[test]
    fn claude_code_compose_prompt_growth_is_not_answer_content() {
        let baseline = "→ terminal-math git:(main) [Sonnet 5] /rc\n> s";
        let completion = "→ terminal-math git:(main) [Sonnet 5] /rc\n> suu";
        assert_eq!(answer(baseline, completion), None);
    }

    #[test]
    fn claude_code_prompt_suffix_fragment_is_not_answer_content() {
        let baseline = "→ terminal-math git:(main) [Sonnet 5] /rc\n> 数";
        let completion = "→ terminal-math git:(main) [Sonnet 5] /rc\n> 数式を";
        assert_eq!(answer(baseline, completion), None);
    }

    #[test]
    fn typing_a_new_prompt_does_not_reemit_a_prior_streaming_answer() {
        let baseline = "❯ Old question\n\n⏺ Old answer text\n✻ Done\n────────────────────────\n❯\n> 数";
        let completion =
            "❯ Old question\n\n⏺ Old answer text\n✻ Done\n────────────────────────\n❯\n> 数学";
        assert_eq!(answer(baseline, completion), None);
    }

    #[test]
    fn claude_streaming_text_grows_incrementally_before_the_idle_prompt() {
        let baseline =
            "Earlier screen content\n❯ Show equations\n\n⏺ Partial answer\n✻ Churned for 2s\n";
        let completion =
            "Earlier screen content\n❯ Show equations\n\n⏺ Partial answer continues with more math $x=2$.\n✻ Churned for 3s\n";
        assert_eq!(
            answer(baseline, completion).as_deref(),
            Some("Partial answer continues with more math $x=2$.")
        );
    }

    #[test]
    fn streaming_command_output_grows_by_exact_prefix_while_prompt_is_busy() {
        let baseline = "❯ \n";
        let completion =
            "❯ \n❯ python3 demo-stream-answer.py bayes-ja\n# ベイズ統計 — 要点まとめ\n";
        assert_eq!(
            answer(baseline, completion).as_deref(),
            Some("❯ python3 demo-stream-answer.py bayes-ja\n# ベイズ統計 — 要点まとめ")
        );
    }

    #[test]
    fn streaming_command_output_appends_japanese_lines_incrementally() {
        let baseline = "❯ \n❯ python3 demo.py bayes-ja\n# ベイズ統計 — 要点まとめ\n";
        let completion = format!(
            "{baseline}\n**事後分布**は **事前分布** と **尤度** から更新される:\n"
        );
        assert_eq!(
            answer(baseline, &completion).as_deref(),
            Some("**事後分布**は **事前分布** と **尤度** から更新される:")
        );
    }

    #[test]
    fn claude_partial_prefix_repaint_still_uses_latest_prompts() {
        let baseline =
            "Shared history\nOld answer\n❯ Old prompt\nOld response\n────────────────────────\n❯\n";
        let completion = "Shared history\nReflowed old answer\n❯ Show a table\n\n⏺ |式|結果|\n|---|---|\n|$x^2$|$4$|\n\n✻ Cooked for 3s\n────────────────────────\n❯\n";
        assert_eq!(
            answer(baseline, completion).as_deref(),
            Some("|式|結果|\n|---|---|\n|$x^2$|$4$|")
        );
    }

    #[test]
    fn repaint_of_the_same_latest_answer_is_not_reemitted() {
        let baseline = "Old layout\n❯ Show equations\n\n⏺ $$x=1$$\n────────────────────────\n❯\n";
        let completion =
            "Reflowed layout\n❯ Show equations\n\n⏺ $$x=1$$\n────────────────────────\n❯\n";
        assert_eq!(answer(baseline, completion), None);
    }

    #[test]
    fn captured_box_table_is_restored_to_markdown() {
        let baseline = "Old answer\n❯\n";
        let completion = "Old answer\n❯ Show table\n\n⏺ ┌────────┬────────┐\n  │ 式     │ 結果   │\n  ├────────┼────────┤\n  │ $x^2$  │ $4$    │\n  └────────┴────────┘\n\n✻ Cooked for 2s\n────────────────────────\n❯\n";
        assert_eq!(
            answer(baseline, completion).as_deref(),
            Some("| 式 | 結果 |\n| --- | --- |\n| $x^2$ | $4$ |")
        );
    }

    #[test]
    fn prompt_and_status_detection() {
        assert!(is_prompt_line("❯ Derive"));
        assert!(is_prompt_line("  › codex"));
        assert!(is_prompt_line("┃ prompt: integrate"));
        assert!(is_prompt_line("Current prompt > integrate"));
        assert!(!is_prompt_line("┃ answer: $$x$$"));
        assert!(!is_prompt_line("> a blockquote"));
        assert!(is_status_line("• Working frame 09"));
        assert!(is_status_line("  Working on the answer."));
        assert!(!is_status_line("• First point"));
        assert!(!is_status_line("• Release notes"));
        assert!(!is_status_line("The answer is $x=2$."));
        assert!(!is_status_line("Read package.json"));
    }

    #[test]
    fn fixture_boundary_matrix_matches_display_answers() {
        let corpus: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../tests/fixtures/agents/answer-corpus.json"
        ))
        .unwrap();
        for case in corpus["boundaryCases"].as_array().unwrap() {
            let id = case["id"].as_str().unwrap();
            let baseline = case["baseline"].as_str().unwrap();
            let completion = case["completion"].as_str().unwrap();
            let expected = case["expectedDisplayAnswer"].as_str().unwrap();
            assert_eq!(
                answer(baseline, completion).as_deref(),
                Some(expected),
                "fixture {id}"
            );
        }
    }
}
