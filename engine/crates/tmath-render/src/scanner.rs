use crate::{ErrorCode, RenderError, SafeLimitKind};

/// A LaTeX formula found in an input document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Formula {
    pub latex: String,
    pub display: bool,
    /// Byte offset of the opening delimiter.
    pub start: usize,
    /// Byte offset immediately after the closing delimiter.
    pub end: usize,
    // Unlike the TypeScript contract, the native pipeline does not retain a
    // delimiter-name field; `display` and the source byte range are sufficient.
}

/// Finite scanner limits mirrored from the TypeScript V2 policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScannerLimits {
    pub max_input_bytes: usize,
    pub max_delimiter_runs: usize,
    pub max_delimiter_run_length: usize,
    pub max_formula_count: usize,
    pub max_formula_characters: usize,
}

impl Default for ScannerLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 160 * 1024 * 1024,
            max_delimiter_runs: 655_360,
            max_delimiter_run_length: 8,
            max_formula_count: 5000,
            max_formula_characters: 80_000,
        }
    }
}

/// Scans an input document for supported LaTeX delimiters.
///
/// # Panics
///
/// Panics when any configured limit is zero, matching the TypeScript scanner's
/// rejection of invalid limit configuration before scanning.
pub fn scan_latex(input: &str, limits: &ScannerLimits) -> Result<Vec<Formula>, RenderError> {
    validate_limits(limits);
    assert_input_bytes(input, limits.max_input_bytes)?;

    let bytes = input.as_bytes();
    let mut formulas = Vec::new();
    let mut delimiter_runs = 0;
    let mut index = 0;
    let mut fence: Option<Fence> = None;
    let mut inline_ticks = 0;

    while index < bytes.len() {
        if is_line_start(bytes, index) {
            if let Some(detected) = read_fence(bytes, index) {
                match fence {
                    None => fence = Some(detected),
                    Some(active)
                        if active.marker == detected.marker && detected.length >= active.length =>
                    {
                        fence = None;
                    }
                    Some(_) => {}
                }
                index = skip_line(bytes, index);
                continue;
            }
        }

        if fence.is_some() {
            index += 1;
            continue;
        }

        if bytes[index] == b'`' && !is_escaped(bytes, index) {
            let ticks = count_run(bytes, index, b'`');
            if inline_ticks == 0 {
                inline_ticks = ticks;
            } else if ticks == inline_ticks {
                inline_ticks = 0;
            }
            index += ticks;
            continue;
        }

        if inline_ticks > 0 {
            index += 1;
            continue;
        }

        if bytes[index] == b'\\' && !is_escaped(bytes, index) {
            if let Some(next) = bytes.get(index + 1) {
                if *next == b'(' || *next == b'[' {
                    let closing = if *next == b'(' {
                        [b'\\', b')']
                    } else {
                        [b'\\', b']']
                    };
                    let content_start = index + 2;
                    let mut cursor = content_start;

                    while cursor < bytes.len() {
                        if bytes[cursor] == b'\n' && *next == b'(' {
                            break;
                        }
                        if bytes[cursor..].starts_with(&closing) {
                            let latex = trim_javascript_whitespace(&input[content_start..cursor]);
                            let formula_characters = latex.chars().count();
                            check_limit(
                                formula_characters,
                                limits.max_formula_characters,
                                SafeLimitKind::FormulaCharacters,
                            )?;
                            if !latex.is_empty() {
                                formulas.push(Formula {
                                    latex: latex.to_owned(),
                                    display: *next == b'[',
                                    start: index,
                                    end: cursor + closing.len(),
                                });
                                check_limit(
                                    formulas.len(),
                                    limits.max_formula_count,
                                    SafeLimitKind::FormulaCount,
                                )?;
                            }
                            cursor += closing.len();
                            break;
                        }
                        cursor += 1;
                    }
                    index = cursor;
                    continue;
                }
            }
        }

        if bytes[index] == b'[' && !is_escaped(bytes, index) && is_line_start(bytes, index) {
            if let Some((formula, next_index)) = scan_bare_bracket_math(input, bytes, index, limits)? {
                formulas.push(formula);
                check_limit(
                    formulas.len(),
                    limits.max_formula_count,
                    SafeLimitKind::FormulaCount,
                )?;
                index = next_index;
                continue;
            }
        }

        if bytes[index] != b'$' || is_escaped(bytes, index) {
            index += 1;
            continue;
        }

        let run_length = count_run(bytes, index, b'$');
        consume_delimiter_run(&mut delimiter_runs, run_length, limits)?;
        if run_length > 2 {
            index += run_length;
            continue;
        }

        let candidate = scan_candidate(input, index, run_length, &mut delimiter_runs, limits)?;
        if let Some(formula) = candidate.formula {
            formulas.push(formula);
            check_limit(
                formulas.len(),
                limits.max_formula_count,
                SafeLimitKind::FormulaCount,
            )?;
        }
        index = candidate.next_index;
    }

    Ok(formulas)
}

#[derive(Clone, Copy)]
struct Fence {
    marker: u8,
    length: usize,
}

struct CandidateResult {
    formula: Option<Formula>,
    next_index: usize,
}

fn scan_candidate(
    input: &str,
    start: usize,
    delimiter_length: usize,
    delimiter_runs: &mut usize,
    limits: &ScannerLimits,
) -> Result<CandidateResult, RenderError> {
    let bytes = input.as_bytes();
    let content_start = start + delimiter_length;
    let mut cursor = content_start;

    while cursor < bytes.len() {
        if delimiter_length == 1 && bytes[cursor] == b'\n' {
            return Ok(CandidateResult {
                formula: None,
                next_index: cursor + 1,
            });
        }
        if bytes[cursor] != b'$' || is_escaped(bytes, cursor) {
            cursor += 1;
            continue;
        }

        let run_length = count_run(bytes, cursor, b'$');
        check_limit(
            run_length,
            limits.max_delimiter_run_length,
            SafeLimitKind::DelimiterRunLength,
        )?;
        if run_length > 2 {
            consume_delimiter_run(delimiter_runs, run_length, limits)?;
            cursor += run_length;
            continue;
        }
        if run_length != delimiter_length {
            return Ok(CandidateResult {
                formula: None,
                next_index: cursor,
            });
        }

        let latex = trim_javascript_whitespace(&input[content_start..cursor]);
        let formula_characters = latex.chars().count();
        check_limit(
            formula_characters,
            limits.max_formula_characters,
            SafeLimitKind::FormulaCharacters,
        )?;
        if delimiter_length == 1 && looks_like_next_dollar_value(input, content_start, cursor) {
            return Ok(CandidateResult {
                formula: None,
                next_index: cursor,
            });
        }
        if latex.is_empty() || (delimiter_length == 1 && !is_plausible_inline_latex(latex)) {
            return Ok(CandidateResult {
                formula: None,
                next_index: cursor,
            });
        }

        consume_delimiter_run(delimiter_runs, run_length, limits)?;
        return Ok(CandidateResult {
            formula: Some(Formula {
                latex: latex.to_owned(),
                display: delimiter_length == 2,
                start,
                end: cursor + delimiter_length,
            }),
            next_index: cursor + delimiter_length,
        });
    }

    Ok(CandidateResult {
        formula: None,
        next_index: bytes.len(),
    })
}

/// Finds the earliest display-math opener in `text` that never sees its
/// closing delimiter before the end of the text.
///
/// This supports the streaming open-tail bundling fix
/// (`specs/stream-open-tail-v1`): while a display formula is still being
/// streamed in, the splitter must withhold everything from the opener to
/// the end of the buffer as one provisional tail block, instead of letting
/// pulldown-cmark misread the raw, still-unclosed LaTeX as Markdown
/// structure (line-leading `+`, `-`, `#`, `---`).
///
/// Recognized openers, in the same precedence order `scan_latex` uses:
///
/// - `$$` (a delimiter run of exactly 2 dollars; longer runs are never
///   openers, matching `scan_latex`'s treatment of ambiguous runs)
/// - `\[` (unescaped)
/// - a line-leading, unescaped bare `[` that passes the scanner's existing
///   bare-bracket plausibility heuristics (`has_bare_bracket_latex_hint`,
///   no [`is_japanese_punctuation`]) applied to the text from just after
///   the `[` to the end of the input
///
/// Inline math (`$...$`, `\(...\)`) is out of scope: an unclosed inline
/// opener is never reported, since inline math never meaningfully spans
/// stream revisions. Delimiters inside a fenced code block (`` ``` ``/`~~~`)
/// or inline code span, and escaped delimiters, are never treated as
/// openers, reusing the same fence/backtick/escape tracking `scan_latex`
/// uses so fenced content and code spans are immune (mirrors AT-S-106).
///
/// Returns the byte offset of the opening delimiter's first byte, or
/// `None` when every opener in `text` closes before EOF (including when
/// there is no opener at all). Runs in O(n) time over `text` and never
/// panics.
///
/// # Complexity
///
/// A *successful* closer search advances `index` past the match, so the
/// total work across all successful searches telescopes to O(n). A failed
/// `\]` or `$$` search returns immediately (an unclosed `\[`/`$$` is
/// always the answer), so those two never repeat. A *rejected* bare `[`
/// is the one case that falls through without consuming a closer —
/// scanning resumes one byte at a time — so a document with many
/// line-leading `[` openers whose closer search also fails (no `]` at
/// all before EOF) would otherwise re-run `find_unescaped` to EOF for
/// every one of them, making the function O(n²) on pathological input
/// (and the stream splitter calls this once per revision on accumulated
/// text, which would amplify the cost further). Each bare-bracket closer
/// search always starts at a strictly later offset than the previous one,
/// so once one such search fails to find a `]` before EOF, no later
/// search can find one either; `no_bare_closer` records that fact and
/// turns every later search into an O(1) lookup.
pub fn open_display_math_start(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = 0;
    let mut fence: Option<Fence> = None;
    let mut inline_ticks = 0;
    let mut no_bare_closer = false;

    while index < bytes.len() {
        if is_line_start(bytes, index) {
            if let Some(detected) = read_fence(bytes, index) {
                match fence {
                    None => fence = Some(detected),
                    Some(active)
                        if active.marker == detected.marker && detected.length >= active.length =>
                    {
                        fence = None;
                    }
                    Some(_) => {}
                }
                index = skip_line(bytes, index);
                continue;
            }
        }

        if fence.is_some() {
            index += 1;
            continue;
        }

        if bytes[index] == b'`' && !is_escaped(bytes, index) {
            let ticks = count_run(bytes, index, b'`');
            if inline_ticks == 0 {
                inline_ticks = ticks;
            } else if ticks == inline_ticks {
                inline_ticks = 0;
            }
            index += ticks;
            continue;
        }

        if inline_ticks > 0 {
            index += 1;
            continue;
        }

        if bytes[index] == b'\\' && !is_escaped(bytes, index) && bytes.get(index + 1) == Some(&b'[')
        {
            let content_start = index + 2;
            return match find_unescaped(bytes, content_start, b"\\]") {
                Some(closer_end) => {
                    index = closer_end;
                    continue;
                }
                None => Some(index),
            };
        }

        if bytes[index] == b'[' && !is_escaped(bytes, index) && is_line_start(bytes, index) {
            let content_start = index + 1;
            let closer = if no_bare_closer {
                None
            } else {
                find_unescaped(bytes, content_start, b"]")
            };
            match closer {
                Some(closer_end) => {
                    // Only treat this as having consumed a real opener when
                    // it also passes the same plausibility heuristics the
                    // scanner applies to a closed bare bracket; otherwise it
                    // is ordinary prose and scanning continues one byte at a
                    // time, exactly like `scan_latex` falls through.
                    let latex = &text[content_start..closer_end - 1];
                    if is_plausible_block_latex(latex) && bytes.get(closer_end) != Some(&b'(') {
                        index = closer_end;
                        continue;
                    }
                }
                None => {
                    no_bare_closer = true;
                    // No closing `]` before EOF: only an open tail when the
                    // remaining text still looks like plausible LaTeX (the
                    // same guard `is_plausible_block_latex` applies to a
                    // closed bracket, minus the "must not contain `[`/`]`"
                    // check, which cannot be evaluated without a closer).
                    let remainder = &text[content_start..];
                    if has_bare_bracket_latex_hint(remainder)
                        && !remainder.chars().any(is_japanese_punctuation)
                    {
                        return Some(index);
                    }
                }
            }
        }

        if bytes[index] != b'$' || is_escaped(bytes, index) {
            index += 1;
            continue;
        }

        let run_length = count_run(bytes, index, b'$');
        if run_length != 2 {
            index += run_length;
            continue;
        }

        let content_start = index + 2;
        match find_unescaped(bytes, content_start, b"$$") {
            Some(closer_end) => {
                index = closer_end;
                continue;
            }
            None => return Some(index),
        }
    }

    None
}

/// Finds the byte offset just past the first unescaped occurrence of
/// `needle` at or after `start`, or `None` if it never occurs.
fn find_unescaped(bytes: &[u8], start: usize, needle: &[u8]) -> Option<usize> {
    let mut cursor = start;
    while cursor + needle.len() <= bytes.len() {
        if bytes[cursor..].starts_with(needle) && !is_escaped(bytes, cursor) {
            return Some(cursor + needle.len());
        }
        cursor += 1;
    }
    None
}

fn validate_limits(limits: &ScannerLimits) {
    assert!(
        limits.max_input_bytes > 0
            && limits.max_delimiter_runs > 0
            && limits.max_delimiter_run_length > 0
            && limits.max_formula_count > 0
            && limits.max_formula_characters > 0,
        "scanner limits must be positive"
    );
}

fn assert_input_bytes(input: &str, limit: usize) -> Result<(), RenderError> {
    let mut bytes = 0;
    for character in input.chars() {
        bytes += character.len_utf8();
        if bytes > limit {
            return Err(limit_error(SafeLimitKind::InputBytes, limit, bytes));
        }
    }
    Ok(())
}

fn consume_delimiter_run(
    runs: &mut usize,
    run_length: usize,
    limits: &ScannerLimits,
) -> Result<(), RenderError> {
    check_limit(
        run_length,
        limits.max_delimiter_run_length,
        SafeLimitKind::DelimiterRunLength,
    )?;
    *runs += 1;
    check_limit(
        *runs,
        limits.max_delimiter_runs,
        SafeLimitKind::DelimiterRuns,
    )
}

fn check_limit(actual: usize, limit: usize, limit_kind: SafeLimitKind) -> Result<(), RenderError> {
    if actual <= limit {
        Ok(())
    } else {
        Err(limit_error(limit_kind, limit, actual))
    }
}

fn limit_error(limit_kind: SafeLimitKind, limit: usize, actual: usize) -> RenderError {
    RenderError::limit_exceeded(
        ErrorCode::ScannerInputLimit,
        limit_kind,
        u64::try_from(limit).unwrap_or(u64::MAX),
        u64::try_from(actual).unwrap_or(u64::MAX),
    )
}

fn is_plausible_inline_latex(latex: &str) -> bool {
    if latex.contains('\n') || latex.contains('$') {
        return false;
    }
    !latex.chars().any(is_japanese_prose_character)
}

/// Some agents drop the backslash and emit `[ \boldsymbol{x} ]` instead of the
/// documented `\[ ... \]`. Only treat this as math at the start of a line, and
/// only when the content looks like LaTeX and the closing bracket isn't
/// immediately followed by `(` (a Markdown link).
fn is_plausible_block_latex(latex: &str) -> bool {
    if latex.is_empty() || latex.contains('[') || latex.contains(']') {
        return false;
    }
    // Unlike inline `$...$` math, a display block may legitimately carry a
    // Japanese label inside `\text{...}` (e.g. `\text{事後精度}`); only
    // full-width punctuation is treated as a sign of ordinary prose, not any
    // CJK character.
    if latex.chars().any(is_japanese_punctuation) {
        return false;
    }
    has_bare_bracket_latex_hint(latex)
}

fn is_japanese_punctuation(character: char) -> bool {
    matches!(character, '\u{3001}' | '\u{3002}')
}

fn has_bare_bracket_latex_hint(latex: &str) -> bool {
    let bytes = latex.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 1 < bytes.len() && bytes[index + 1].is_ascii_alphabetic() {
            return true;
        }
        if (bytes[index] == b'_' || bytes[index] == b'^') && bytes.get(index + 1) == Some(&b'{') {
            return true;
        }
        index += 1;
    }
    false
}

fn scan_bare_bracket_math(
    input: &str,
    bytes: &[u8],
    start: usize,
    limits: &ScannerLimits,
) -> Result<Option<(Formula, usize)>, RenderError> {
    let content_start = start + 1;
    let mut cursor = content_start;
    while cursor < bytes.len() {
        if bytes[cursor] == b']' && !is_escaped(bytes, cursor) {
            if bytes.get(cursor + 1) == Some(&b'(') {
                return Ok(None);
            }
            let latex = trim_javascript_whitespace(&input[content_start..cursor]);
            if !is_plausible_block_latex(latex) {
                return Ok(None);
            }
            let formula_characters = latex.chars().count();
            check_limit(
                formula_characters,
                limits.max_formula_characters,
                SafeLimitKind::FormulaCharacters,
            )?;
            return Ok(Some((
                Formula {
                    latex: latex.to_owned(),
                    display: true,
                    start,
                    end: cursor + 1,
                },
                cursor + 1,
            )));
        }
        cursor += 1;
    }
    Ok(None)
}

fn is_japanese_prose_character(character: char) -> bool {
    matches!(
        character,
        '\u{2e80}'..='\u{2e99}'
            | '\u{2e9b}'..='\u{2ef3}'
            | '\u{2f00}'..='\u{2fd5}'
            | '\u{3001}'
            | '\u{3002}'
            | '\u{3005}'
            | '\u{3007}'
            | '\u{3021}'..='\u{3029}'
            | '\u{3038}'..='\u{303b}'
            | '\u{3041}'..='\u{3096}'
            | '\u{309d}'..='\u{309f}'
            | '\u{30a1}'..='\u{30fa}'
            | '\u{30fd}'..='\u{30ff}'
            | '\u{31f0}'..='\u{31ff}'
            | '\u{32d0}'..='\u{32fe}'
            | '\u{3300}'..='\u{3357}'
            | '\u{3400}'..='\u{4dbf}'
            | '\u{4e00}'..='\u{9fff}'
            | '\u{f900}'..='\u{fa6d}'
            | '\u{fa70}'..='\u{fad9}'
            | '\u{ff66}'..='\u{ff6f}'
            | '\u{ff71}'..='\u{ff9d}'
            | '\u{16fe2}'..='\u{16fe3}'
            | '\u{16ff0}'..='\u{16ff1}'
            | '\u{1aff0}'..='\u{1aff3}'
            | '\u{1aff5}'..='\u{1affb}'
            | '\u{1affd}'..='\u{1affe}'
            | '\u{1b000}'..='\u{1b122}'
            | '\u{1b132}'
            | '\u{1b150}'..='\u{1b152}'
            | '\u{1b155}'
            | '\u{1b164}'..='\u{1b167}'
            | '\u{1f200}'
            | '\u{20000}'..='\u{2a6df}'
            | '\u{2a700}'..='\u{2b739}'
            | '\u{2b740}'..='\u{2b81d}'
            | '\u{2b820}'..='\u{2cea1}'
            | '\u{2ceb0}'..='\u{2ebe0}'
            | '\u{2ebf0}'..='\u{2ee5d}'
            | '\u{2f800}'..='\u{2fa1d}'
            | '\u{30000}'..='\u{3134a}'
            | '\u{31350}'..='\u{323af}'
    )
}

fn looks_like_next_dollar_value(input: &str, content_start: usize, candidate_index: usize) -> bool {
    let bytes = input.as_bytes();
    let Some(&next) = bytes.get(candidate_index + 1) else {
        return false;
    };
    if !next.is_ascii_alphanumeric() && next != b'_' {
        return false;
    }

    let content = trim_javascript_whitespace(&input[content_start..candidate_index]);
    if content.is_empty()
        || content.bytes().any(|byte| {
            matches!(
                byte,
                b'\\'
                    | b'^'
                    | b'_'
                    | b'='
                    | b'+'
                    | b'*'
                    | b'/'
                    | b'<'
                    | b'>'
                    | b'('
                    | b')'
                    | b'['
                    | b']'
                    | b'{'
                    | b'}'
            )
        })
    {
        return false;
    }

    has_javascript_whitespace(content)
        || content.as_bytes()[0].is_ascii_digit()
        || is_uppercase_shell_name(content)
}

fn has_javascript_whitespace(content: &str) -> bool {
    content.chars().any(is_javascript_whitespace)
}

fn trim_javascript_whitespace(content: &str) -> &str {
    content.trim_matches(is_javascript_whitespace)
}

fn is_javascript_whitespace(character: char) -> bool {
    matches!(
        character,
        '\u{0009}'..='\u{000d}'
            | '\u{0020}'
            | '\u{00a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'..='\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    )
}

fn is_uppercase_shell_name(content: &str) -> bool {
    let mut bytes = content.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first.is_ascii_uppercase() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

fn is_escaped(input: &[u8], index: usize) -> bool {
    let mut slashes = 0;
    let mut cursor = index;
    while cursor > 0 && input[cursor - 1] == b'\\' {
        slashes += 1;
        cursor -= 1;
    }
    slashes % 2 == 1
}

fn is_line_start(input: &[u8], index: usize) -> bool {
    index == 0 || input[index - 1] == b'\n'
}

fn read_fence(input: &[u8], index: usize) -> Option<Fence> {
    let mut cursor = index;
    let mut spaces = 0;
    while spaces < 4 && input.get(cursor) == Some(&b' ') {
        spaces += 1;
        cursor += 1;
    }
    if spaces > 3 {
        return None;
    }

    let marker = *input.get(cursor)?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let length = count_run(input, cursor, marker);
    (length >= 3).then_some(Fence { marker, length })
}

fn skip_line(input: &[u8], index: usize) -> usize {
    input[index..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(input.len(), |position| index + position + 1)
}

fn count_run(input: &[u8], index: usize, character: u8) -> usize {
    input[index..]
        .iter()
        .take_while(|byte| **byte == character)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ErrorCode, SafeLimitKind};

    fn summaries(formulas: &[Formula]) -> Vec<(&str, bool)> {
        formulas
            .iter()
            .map(|formula| (formula.latex.as_str(), formula.display))
            .collect()
    }

    fn assert_limit_error(error: RenderError, kind: SafeLimitKind) {
        let record = error.safe_record();
        assert_eq!(record.code, ErrorCode::ScannerInputLimit);
        let details = record.details.as_ref().expect("limit details");
        assert_eq!(details.limit_kind, Some(kind));
        assert!(details.actual > details.limit);
        assert!(!error.to_string().contains('$'));
    }

    #[test]
    fn matches_the_synthetic_corpus_case_claude_inline() {
        let formulas = scan_latex("The relation is $E=mc^2$.", &ScannerLimits::default()).unwrap();
        assert_eq!(summaries(&formulas), vec![("E=mc^2", false)]);
    }

    #[test]
    fn matches_the_synthetic_corpus_case_codex_display_multiline() {
        let formulas = scan_latex(
            "Integrate term by term:\n$$\n\\int_0^1 x^2\\,dx\n= \\frac{1}{3}\n$$\nThis is the result.",
            &ScannerLimits::default(),
        )
        .unwrap();
        assert_eq!(
            summaries(&formulas),
            vec![("\\int_0^1 x^2\\,dx\n= \\frac{1}{3}", true)]
        );
    }

    #[test]
    fn matches_the_synthetic_corpus_case_pi_code_and_prose() {
        let formulas = scan_latex(
            "Inline code `const price = '$20';` is not math.\n```text\n$HOME and $$not_math$$\n```\nOutside code, $a+b=c$ comes before $$c^2=a^2+b^2$$.",
            &ScannerLimits::default(),
        )
        .unwrap();
        assert_eq!(
            summaries(&formulas),
            vec![("a+b=c", false), ("c^2=a^2+b^2", true)]
        );
    }

    #[test]
    fn matches_the_synthetic_corpus_case_opencode_prices_and_shell() {
        let formulas = scan_latex(
            "The estimates are $10 and $20. Paths use $HOME and $PATH. The placeholder $VALUE is unclosed.",
            &ScannerLimits::default(),
        )
        .unwrap();
        assert!(formulas.is_empty());
    }

    #[test]
    fn matches_the_synthetic_corpus_case_claude_escaped_and_recovery() {
        let formulas = scan_latex(
            "A literal \\$ is escaped. This opening $ never closes before the line ends.\nA later formula $x+1$ is valid.",
            &ScannerLimits::default(),
        )
        .unwrap();
        assert_eq!(summaries(&formulas), vec![("x+1", false)]);
    }

    #[test]
    fn matches_the_synthetic_corpus_case_codex_numeric() {
        let formulas = scan_latex(
            "The smallest test expression is $1$.",
            &ScannerLimits::default(),
        )
        .unwrap();
        assert_eq!(summaries(&formulas), vec![("1", false)]);
    }

    #[test]
    fn matches_the_synthetic_corpus_case_pi_unicode() {
        let formulas = scan_latex(
            "日本語の前置き $\\alpha+\\beta$ and mixed text 数式 $$y=x^2$$ の後書き。",
            &ScannerLimits::default(),
        )
        .unwrap();
        assert_eq!(
            summaries(&formulas),
            vec![("\\alpha+\\beta", false), ("y=x^2", true)]
        );
    }

    #[test]
    fn matches_the_synthetic_corpus_case_opencode_delimiter_runs() {
        let formulas = scan_latex(
            "Repeated delimiters are ambiguous: $$$$ and $$$value$$$.",
            &ScannerLimits::default(),
        )
        .unwrap();
        assert!(formulas.is_empty());
    }

    #[test]
    fn matches_the_synthetic_corpus_case_cursor_inline() {
        let formulas = scan_latex("The relation is $E=mc^2$.", &ScannerLimits::default()).unwrap();
        assert_eq!(summaries(&formulas), vec![("E=mc^2", false)]);
    }

    #[test]
    fn matches_the_synthetic_corpus_case_cursor_paren_bracket() {
        let formulas = scan_latex(
            "The relation is \\(E=mc^2\\) and display \\[a^2+b^2=c^2\\].",
            &ScannerLimits::default(),
        )
        .unwrap();
        assert_eq!(
            summaries(&formulas),
            vec![("E=mc^2", false), ("a^2+b^2=c^2", true)]
        );
    }

    #[test]
    fn matches_the_synthetic_corpus_case_cursor_bracket_multiline() {
        let formulas = scan_latex(
            "Integrate:\n\\[\n\\int_0^1 x^2 dx\n\\]\nDone.",
            &ScannerLimits::default(),
        )
        .unwrap();
        assert_eq!(summaries(&formulas), vec![("\\int_0^1 x^2 dx", true)]);
    }

    #[test]
    fn matches_bare_bracket_math_when_the_backslash_is_dropped() {
        let formulas = scan_latex(
            "Density is\n\n[ p(\\boldsymbol{x}\\mid\\boldsymbol{\\mu}) ]\n\nDone.",
            &ScannerLimits::default(),
        )
        .unwrap();
        assert_eq!(
            summaries(&formulas),
            vec![("p(\\boldsymbol{x}\\mid\\boldsymbol{\\mu})", true)]
        );
    }

    #[test]
    fn ignores_bare_brackets_that_look_like_markdown_links_or_prose() {
        let formulas = scan_latex(
            "See the [link](https://example.com) and a [note] in prose.",
            &ScannerLimits::default(),
        )
        .unwrap();
        assert!(formulas.is_empty());
    }

    #[test]
    fn ignores_bare_brackets_not_at_the_start_of_a_line() {
        let formulas = scan_latex(
            "Inline text with [ \\alpha ] mid-sentence is not display math.",
            &ScannerLimits::default(),
        )
        .unwrap();
        assert!(formulas.is_empty());
    }

    #[test]
    fn matches_bare_bracket_math_with_a_japanese_text_label_across_multiple_lines() {
        // \text{...} may legitimately carry a Japanese label; only full-width
        // punctuation (a sign of ordinary prose) should reject the block.
        let formulas = scan_latex(
            "ただし、\n\n[ \\text{事後精度}\n\n\\text{事前精度} + \\text{データの精度} ]\n\nです。",
            &ScannerLimits::default(),
        )
        .unwrap();
        assert_eq!(
            summaries(&formulas),
            vec![(
                "\\text{事後精度}\n\n\\text{事前精度} + \\text{データの精度}",
                true
            )]
        );
    }

    #[test]
    fn ignores_bare_brackets_containing_full_width_punctuation() {
        let formulas = scan_latex(
            "[ \\text{これは、文章です} ]\n\nDone.",
            &ScannerLimits::default(),
        )
        .unwrap();
        assert!(formulas.is_empty());
    }

    #[test]
    fn returns_delimiter_offsets_around_unicode_prose() {
        let input = "前🙂 The relation is $E=mc^2$. 後";
        let start = input.find("$E=mc^2$").unwrap();
        let formulas = scan_latex(input, &ScannerLimits::default()).unwrap();

        assert_eq!(
            formulas,
            vec![Formula {
                latex: "E=mc^2".to_owned(),
                display: false,
                start,
                end: start + "$E=mc^2$".len(),
            }]
        );
        assert!(input.is_char_boundary(formulas[0].start));
        assert!(input.is_char_boundary(formulas[0].end));
    }

    #[test]
    fn preserves_internal_display_newlines_while_trimming_outer_whitespace() {
        let formulas = scan_latex(
            "Before\n$$\n a+b\n= c\n$$\nAfter",
            &ScannerLimits::default(),
        )
        .unwrap();
        assert_eq!(summaries(&formulas), vec![("a+b\n= c", true)]);
    }

    #[test]
    fn trims_the_same_bom_whitespace_as_javascript() {
        let formulas = scan_latex(
            "\u{feff}$\u{feff}x+1\u{feff}$\u{feff}",
            &ScannerLimits::default(),
        )
        .unwrap();
        assert_eq!(summaries(&formulas), vec![("x+1", false)]);
    }

    #[test]
    fn recovers_from_an_unclosed_inline_delimiter_at_a_newline() {
        let formulas = scan_latex(
            "Opening $ never closes.\nLater $x+1$ is valid.",
            &ScannerLimits::default(),
        )
        .unwrap();
        assert_eq!(summaries(&formulas), vec![("x+1", false)]);
    }

    #[test]
    fn recovers_from_an_ambiguous_shell_value_before_a_same_line_formula() {
        let formulas = scan_latex(
            "Use $HOME, then calculate $x+1$.",
            &ScannerLimits::default(),
        )
        .unwrap();
        assert_eq!(summaries(&formulas), vec![("x+1", false)]);
    }

    #[test]
    fn ignores_tilde_fences_and_honors_even_escape_runs() {
        let input = "~~~text\n$hidden$\n~~~\n\\\\$x$";
        let formulas = scan_latex(input, &ScannerLimits::default()).unwrap();
        assert_eq!(summaries(&formulas), vec![("x", false)]);
    }

    #[test]
    fn rejects_input_above_the_utf_8_byte_limit() {
        let limits = ScannerLimits {
            max_input_bytes: 10,
            ..ScannerLimits::default()
        };
        let error = scan_latex(&"界".repeat(4), &limits).unwrap_err();
        assert_limit_error(error.clone(), SafeLimitKind::InputBytes);
        assert_eq!(
            error.safe_record().details.as_ref().unwrap().actual,
            Some(12)
        );
    }

    #[test]
    fn rejects_excessive_delimiter_runs_and_long_runs() {
        let run_limits = ScannerLimits {
            max_delimiter_runs: 5,
            ..ScannerLimits::default()
        };
        assert_limit_error(
            scan_latex("$a$ $b$ $c$", &run_limits).unwrap_err(),
            SafeLimitKind::DelimiterRuns,
        );

        let input = "$".repeat(ScannerLimits::default().max_delimiter_run_length + 1);
        assert_limit_error(
            scan_latex(&input, &ScannerLimits::default()).unwrap_err(),
            SafeLimitKind::DelimiterRunLength,
        );
    }

    #[test]
    fn rejects_excessive_formula_count_and_formula_length() {
        let count_limits = ScannerLimits {
            max_formula_count: 1,
            ..ScannerLimits::default()
        };
        assert_limit_error(
            scan_latex("$a$ $b$", &count_limits).unwrap_err(),
            SafeLimitKind::FormulaCount,
        );

        let character_limits = ScannerLimits {
            max_formula_characters: 3,
            ..ScannerLimits::default()
        };
        assert_limit_error(
            scan_latex("$abcd$", &character_limits).unwrap_err(),
            SafeLimitKind::FormulaCharacters,
        );
    }

    #[test]
    fn rejects_invalid_limit_configuration() {
        let limits = ScannerLimits {
            max_delimiter_runs: 0,
            ..ScannerLimits::default()
        };
        let result = std::panic::catch_unwind(|| scan_latex("$x$", &limits));
        assert!(result.is_err());
    }

    #[test]
    fn handles_a_large_bounded_prose_input_without_changing_output() {
        let input = format!("{}$x$", "plain text ".repeat(20_000));
        let formulas = scan_latex(&input, &ScannerLimits::default()).unwrap();
        assert_eq!(summaries(&formulas), vec![("x", false)]);
    }

    #[test]
    fn never_panics_or_reports_invalid_offsets_under_deterministic_fuzz() {
        let mut seed = 0xc0ff_ee42_u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            seed
        };
        let tokens = [
            "$", "$$", "\\(", "\\)", "\\[", "\\]", "\\$", "`", "```", "\n", " ", "x_", "{}", "界",
            "a$",
        ];

        for _ in 0..512 {
            let length = 1 + next() % 40;
            let mut text = String::new();
            for _ in 0..length {
                text.push_str(tokens[(next() as usize) % tokens.len()]);
            }

            match scan_latex(&text, &ScannerLimits::default()) {
                Ok(formulas) => {
                    for formula in formulas {
                        assert!(formula.end > formula.start);
                        let source = text
                            .get(formula.start..formula.end)
                            .expect("formula offsets must be UTF-8 boundaries");
                        assert!(source.contains(&formula.latex));
                    }
                }
                Err(error) => assert_eq!(error.safe_record().code, ErrorCode::ScannerInputLimit),
            }
        }
    }

    #[test]
    fn fails_closed_without_caching_content_when_scanning_over_limit_fuzz_input() {
        let mut seed = 7_u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            seed
        };
        let alphabet = ["$", "\\", "`", "\n", "中", "{", "}", "${"];
        let mut text = String::new();
        for _ in 0..2000 {
            text.push_str(alphabet[(next() as usize) % alphabet.len()]);
        }

        if let Err(error) = scan_latex(&text, &ScannerLimits::default()) {
            assert_eq!(error.safe_record().code, ErrorCode::ScannerInputLimit);
        }
    }

    #[test]
    fn keeps_cjk_content_and_byte_offsets_for_display_delimiters() {
        let input = "前文 $$ 数式=値 $$ 後文";
        let start = input.find("$$").unwrap();
        let formula = scan_latex(input, &ScannerLimits::default())
            .unwrap()
            .pop()
            .unwrap();

        assert_eq!(formula.latex, "数式=値");
        assert!(formula.display);
        assert_eq!(&input[formula.start..formula.end], "$$ 数式=値 $$");
        assert_eq!(formula.start, start);
        assert!(input.is_char_boundary(formula.start));
        assert!(input.is_char_boundary(formula.end));
    }

    #[test]
    fn keeps_byte_offsets_after_an_emoji() {
        let input = "🙂 before $x+α$ after";
        let formula = scan_latex(input, &ScannerLimits::default())
            .unwrap()
            .pop()
            .unwrap();

        assert_eq!(formula.latex, "x+α");
        assert_eq!(&input[formula.start..formula.end], "$x+α$");
        assert!(input.is_char_boundary(formula.start));
        assert!(input.is_char_boundary(formula.end));
    }

    #[test]
    fn accepts_exact_limits_and_rejects_one_past_each_limit() {
        let input_limits = ScannerLimits {
            max_input_bytes: 3,
            ..ScannerLimits::default()
        };
        assert!(scan_latex("界", &input_limits).is_ok());
        assert_limit_error(
            scan_latex("界a", &input_limits).unwrap_err(),
            SafeLimitKind::InputBytes,
        );

        let run_count_limits = ScannerLimits {
            max_delimiter_runs: 2,
            ..ScannerLimits::default()
        };
        assert_eq!(
            summaries(&scan_latex("$x$", &run_count_limits).unwrap()),
            vec![("x", false)]
        );
        assert_limit_error(
            scan_latex("$x$ $", &run_count_limits).unwrap_err(),
            SafeLimitKind::DelimiterRuns,
        );

        let run_length_limits = ScannerLimits {
            max_delimiter_run_length: 3,
            ..ScannerLimits::default()
        };
        assert!(scan_latex("$$$", &run_length_limits).is_ok());
        assert_limit_error(
            scan_latex("$$$$", &run_length_limits).unwrap_err(),
            SafeLimitKind::DelimiterRunLength,
        );

        let formula_count_limits = ScannerLimits {
            max_formula_count: 1,
            ..ScannerLimits::default()
        };
        assert!(scan_latex("$a$", &formula_count_limits).is_ok());
        assert_limit_error(
            scan_latex("$a$ $b$", &formula_count_limits).unwrap_err(),
            SafeLimitKind::FormulaCount,
        );

        let formula_character_limits = ScannerLimits {
            max_formula_characters: 3,
            ..ScannerLimits::default()
        };
        assert!(scan_latex("$a界b$", &formula_character_limits).is_ok());
        assert_limit_error(
            scan_latex("$$a界bc$$", &formula_character_limits).unwrap_err(),
            SafeLimitKind::FormulaCharacters,
        );
    }

    #[test]
    fn synthetic_corpus_fixture_stays_in_sync_with_the_port() {
        // The per-case tests above transcribe the corpus for readability; this
        // guard re-checks every case straight from the shared fixture so the
        // Rust port can never drift from the V2 corpus of record.
        let corpus: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/agents/answer-corpus.json"
        ))
        .expect("The answer corpus fixture must parse");
        let cases = corpus["scannerCases"]
            .as_array()
            .expect("scannerCases must be an array");
        assert!(!cases.is_empty());
        for case in cases {
            let id = case["id"].as_str().expect("case id");
            let answer = case["answer"].as_str().expect("case answer");
            let expected = case["expected"]
                .as_array()
                .expect("case expected")
                .iter()
                .map(|entry| {
                    (
                        entry["latex"].as_str().expect("expected latex").to_string(),
                        entry["display"].as_bool().expect("expected display"),
                    )
                })
                .collect::<Vec<_>>();
            let actual = scan_latex(answer, &ScannerLimits::default())
                .unwrap_or_else(|error| panic!("case {id} failed to scan: {error}"))
                .iter()
                .map(|formula| (formula.latex.clone(), formula.display))
                .collect::<Vec<_>>();
            assert_eq!(actual, expected, "corpus case {id} diverged");
        }
    }

    // --- open_display_math_start (specs/stream-open-tail-v1) ---

    #[test]
    fn open_display_math_start_reports_an_unclosed_bracket_formula_with_markdown_like_lines() {
        let input = "Update:\n\\[\n\\Lambda_{n+1} = \\Lambda_n\n+ \\alpha\n- \\beta\n# not a heading\n---\n";
        let expected = input.find("\\[").unwrap();
        assert_eq!(open_display_math_start(input), Some(expected));
    }

    #[test]
    fn open_display_math_start_returns_the_second_opener_after_a_closed_one() {
        let input = "\\[a+b\\]\n\nThen an unclosed one:\n\\[\nc + d\n";
        let expected = input.rfind("\\[").unwrap();
        assert_eq!(open_display_math_start(input), Some(expected));
    }

    #[test]
    fn open_display_math_start_returns_none_for_a_fully_closed_document() {
        let input = concat!(
            "Inline $x$ stays inline.\n\n",
            "$$a^2+b^2=c^2$$\n\n",
            "\\[\\int_0^1 x\\,dx\\]\n\n",
            "[ \\boldsymbol{x} ]\n\n",
            "Done.\n"
        );
        assert_eq!(open_display_math_start(input), None);
    }

    #[test]
    fn open_display_math_start_reports_an_unclosed_double_dollar_run() {
        assert_eq!(
            open_display_math_start("Before\n\n$$a+b\n"),
            Some("Before\n\n".len())
        );
    }

    #[test]
    fn open_display_math_start_returns_none_for_a_closed_double_dollar_run() {
        assert_eq!(open_display_math_start("Before\n\n$$a+b$$\nAfter\n"), None);
    }

    #[test]
    fn open_display_math_start_reports_an_unclosed_bare_bracket_with_a_latex_hint() {
        let input = "Density is\n\n[ p(\\boldsymbol{x}\\mid\\boldsymbol{\\mu})\nkeeps growing\n";
        let expected = input.find('[').unwrap();
        assert_eq!(open_display_math_start(input), Some(expected));
    }

    #[test]
    fn open_display_math_start_ignores_an_unclosed_bare_bracket_that_looks_like_prose() {
        let input = "Notes:\n\n[note this line never closes and has no latex hint\n";
        assert_eq!(open_display_math_start(input), None);
    }

    #[test]
    fn open_display_math_start_ignores_an_unclosed_bare_bracket_with_japanese_punctuation() {
        let input = "ただし、\n\n[ \\text{事後精度}\nこれは、まだ続く文章です\n";
        assert_eq!(open_display_math_start(input), None);
    }

    #[test]
    fn open_display_math_start_ignores_openers_inside_an_unclosed_fenced_code_block() {
        let input = "Before.\n\n```text\n\\[\nnever closes\n$$\nalso never closes\n";
        assert_eq!(open_display_math_start(input), None);
    }

    #[test]
    fn open_display_math_start_ignores_an_escaped_backslash_before_a_bare_bracket() {
        // `\\` is an escaped backslash (a literal `\`), so the following `[`
        // is a plain, unescaped bare bracket to the scanner's escape rule
        // (an even run of backslashes escapes only itself). It still needs
        // a LaTeX hint and line-leading position to count as an opener; this
        // input has neither the hint nor a line-leading `[`, so it must not
        // be reported.
        assert_eq!(
            open_display_math_start("prose \\\\[ \\alpha never closes"),
            None
        );
    }

    #[test]
    fn open_display_math_start_ignores_a_mid_line_bare_bracket() {
        assert_eq!(
            open_display_math_start("Inline text with [ \\alpha never closes mid-sentence"),
            None
        );
    }

    #[test]
    fn open_display_math_start_returns_none_for_empty_text() {
        assert_eq!(open_display_math_start(""), None);
    }

    #[test]
    fn open_display_math_start_returns_none_for_plain_prose() {
        assert_eq!(
            open_display_math_start("Just an ordinary paragraph with no delimiters at all."),
            None
        );
    }

    #[test]
    fn open_display_math_start_ignores_delimiter_runs_longer_than_two_dollars() {
        assert_eq!(
            open_display_math_start("Ambiguous run: $$$ never closes"),
            None
        );
    }

    #[test]
    fn open_display_math_start_ignores_unclosed_inline_math() {
        assert_eq!(open_display_math_start("Inline $x+y never closes"), None);
        assert_eq!(open_display_math_start("Inline \\(x+y never closes"), None);
    }

    #[test]
    fn open_display_math_start_stays_correct_across_many_rejected_bare_brackets() {
        // Every line-leading `[` here has no closing `]` anywhere in the
        // rest of the document, and no LaTeX hint, so each one is rejected
        // by the bare-bracket branch and falls through without consuming a
        // closer. Before the no_bare_closer memo this drove one O(n) scan
        // to EOF per rejected `[`; the memo turns every search after the
        // first failure into an O(1) lookup. The whole document still has
        // no genuine opener, so the correct result is None.
        let mut input = String::new();
        for line in 0..2000 {
            input.push_str(&format!("[note line {line} with no closing bracket\n"));
        }
        assert_eq!(open_display_math_start(&input), None);
    }

    #[test]
    fn open_display_math_start_memo_does_not_hide_a_later_genuine_opener() {
        // Every bare `[...]` here closes (so the no_bare_closer memo is
        // never armed) but is rejected as implausible prose, exercising the
        // ordinary reject-and-fall-through path many times in a row before
        // a later, genuinely unclosed `\[` appears. The `\]` closer search
        // for that final formula must still run to completion and report
        // its offset — proving the bare-bracket memo (armed only by an
        // *unclosed* bare bracket) never interferes with `\[` detection.
        let mut input = String::new();
        for line in 0..500 {
            input.push_str(&format!("[note {line}]\n"));
        }
        input.push_str("\\[\n\\Lambda_n = \\Lambda_{n-1} + 1\n");
        let expected = input.rfind("\\[").unwrap();
        assert_eq!(open_display_math_start(&input), Some(expected));
    }

    #[test]
    fn open_display_math_start_bare_bracket_memo_does_not_hide_a_later_genuine_bare_bracket() {
        // The bare-bracket branch is checked before the `\[`/`$$` branches
        // in the loop, so it needs its own direct proof: many earlier bare
        // brackets are rejected because their remainder (up to EOF) lacks a
        // LaTeX hint at the time they are scanned, arming no_bare_closer —
        // but the true test is that a later bare bracket whose own closer
        // search would still succeed, or whose remainder is plausible, is
        // not masked by an over-eager memo. Here every earlier bracket
        // closes and is rejected for implausibility (never touching
        // no_bare_closer), and the final bracket is unclosed with a
        // genuine hint, so it must still be reported.
        let mut input = String::new();
        for line in 0..500 {
            input.push_str(&format!("[note {line}]\n"));
        }
        input.push_str("[ \\boldsymbol{x} stays open\n");
        let expected = input.rfind('[').unwrap();
        assert_eq!(open_display_math_start(&input), Some(expected));
    }

    #[test]
    fn open_display_math_start_never_panics_under_deterministic_fuzz() {
        let mut seed = 0x0BED_5EED_u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            seed
        };
        let tokens = [
            "$", "$$", "\\[", "\\]", "[", "]", "\\", "\\\\", "`", "```", "\n", " ", "\\alpha", "+",
            "-", "#", "---", "。", "、", "x",
        ];

        for _ in 0..512 {
            let length = 1 + next() % 40;
            let mut text = String::new();
            for _ in 0..length {
                text.push_str(tokens[(next() as usize) % tokens.len()]);
            }
            if let Some(offset) = open_display_math_start(&text) {
                assert!(text.is_char_boundary(offset));
            }
        }
    }
}
