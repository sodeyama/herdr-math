use std::ops::Range;

use pulldown_cmark::{Event, Options, Parser, Tag};

use crate::{scan_latex, Block, BlockKind, Formula, Limits, RenderError, ScannerLimits};

/// Splits Markdown into semantic prose blocks.
pub fn parse_blocks(input: &str) -> Result<Vec<Block>, RenderError> {
    parse_blocks_limited(input, &Limits::default())
}

/// Brackets a display-formula placeholder line: two Unicode Private Use Area
/// codepoints (never occurring in ordinary Markdown/LaTeX source, matching
/// `typst_doc.rs`'s inline placeholder convention) wrapping the formula's
/// index, on its own line with a blank line on each side. A display formula
/// spanning a blank line — `[ \bar{x}\n\n\frac1n\sum ... ]` or
/// `\[\n...\n\n...\n\]`, both of which real agents emit — would otherwise be
/// split into multiple paragraphs by pulldown-cmark's blank-line-delimited
/// paragraph rule before `scan_latex`'s span is ever consulted (Markdown
/// syntax, unlike `$...$`/`$$...$$`, has no concept of a formula's own
/// boundary). Placing the placeholder on an isolated line with blank-line
/// padding guarantees pulldown-cmark always emits it as exactly one
/// paragraph, regardless of the replaced formula's internal newlines.
const FORMULA_BLOCK_PLACEHOLDER_START: char = '\u{E002}';
const FORMULA_BLOCK_PLACEHOLDER_END: char = '\u{E003}';

/// Splits Markdown while enforcing document and per-block source caps.
///
/// Phase 1 fails the whole parse with a safe error record. Replacing only the
/// offending block with an error entry is intentionally staged for Phase 2,
/// when incremental block state exists.
pub fn parse_blocks_limited(input: &str, limits: &Limits) -> Result<Vec<Block>, RenderError> {
    // AGENTS.md's Product Boundaries requires `$...$`/`$$...$$` (and the
    // retained `\(...\)`/`\[...\]`) to be parsed FIRST, before Markdown: scan
    // the whole document for display formulas before pulldown-cmark ever
    // splits it into paragraphs, so a formula that itself contains a blank
    // line is never torn across multiple blocks. See
    // `FORMULA_BLOCK_PLACEHOLDER_START`'s doc comment for why a blank line
    // inside the formula defeats the previous "parse blocks, then check if
    // one exactly matches a formula" approach.
    let formulas = scan_latex(input, &ScannerLimits::default())?;
    let display_formulas: Vec<&Formula> = formulas.iter().filter(|f| f.display).collect();
    let protected_input = protect_display_formula_spans(input, &display_formulas);

    let parser = Parser::new_ext(&protected_input, Options::ENABLE_TABLES).into_offset_iter();
    let mut blocks = Vec::new();
    let mut open_block: Option<(BlockKind, usize)> = None;
    let mut depth = 0_usize;

    for (event, range) in parser {
        match event {
            Event::Start(tag) => {
                if depth == 0 {
                    open_block = Some((block_kind(&tag), range.start));
                }
                depth += 1;
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    if let Some((kind, start)) = open_block.take() {
                        push_block(&mut blocks, &protected_input, kind, start..range.end, limits)?;
                    }
                }
            }
            Event::Rule if depth == 0 => {
                push_block(
                    &mut blocks,
                    &protected_input,
                    BlockKind::ThematicBreak,
                    range.clone(),
                    limits,
                )?;
            }
            Event::Html(_) | Event::InlineHtml(_) if depth == 0 => {
                push_block(
                    &mut blocks,
                    &protected_input,
                    BlockKind::Paragraph,
                    range.clone(),
                    limits,
                )?;
            }
            Event::Text(_)
            | Event::Code(_)
            | Event::SoftBreak
            | Event::HardBreak
            | Event::TaskListMarker(_)
                if depth == 0 =>
            {
                push_block(
                    &mut blocks,
                    &protected_input,
                    BlockKind::Paragraph,
                    range.clone(),
                    limits,
                )?;
            }
            _ => {}
        }
    }

    // The Markdown parser owns prose boundaries; the scanner then upgrades
    // a block into display math either because it is a placeholder standing
    // in for a formula that spanned a blank line, or (the common case, no
    // blank line inside the formula) because the whole block source happens
    // to be exactly one complete display formula.
    for block in &mut blocks {
        if block.kind != BlockKind::Paragraph {
            continue;
        }
        if let Some(formula) = resolve_placeholder_formula(&block.source, &display_formulas) {
            // Restore the exact original source slice (delimiters included),
            // matching what `is_complete_display_math`'s branch below already
            // preserves for a formula with no internal blank line.
            block.source = input[formula.start..formula.end].to_owned();
            block.kind = BlockKind::DisplayMath;
        } else if is_complete_display_math(&block.source) {
            block.kind = BlockKind::DisplayMath;
        }
    }
    for (index, block) in blocks.iter_mut().enumerate() {
        block.index = index;
    }
    Ok(blocks)
}

/// Replaces each display formula's exact source span (delimiters included)
/// with an isolated placeholder line, blank-line-padded so pulldown-cmark
/// always treats it as a single paragraph. See
/// `FORMULA_BLOCK_PLACEHOLDER_START`'s doc comment for why.
fn protect_display_formula_spans(source: &str, formulas: &[&Formula]) -> String {
    if formulas.is_empty() {
        return source.to_owned();
    }
    let mut protected = String::with_capacity(source.len());
    let mut cursor = 0;
    for (index, formula) in formulas.iter().enumerate() {
        if formula.start < cursor {
            // Overlapping/out-of-order spans should never occur from
            // `scan_latex`, but fail closed by skipping rather than
            // corrupting the byte stream with a negative-length slice.
            continue;
        }
        protected.push_str(&source[cursor..formula.start]);
        protected.push('\n');
        protected.push(FORMULA_BLOCK_PLACEHOLDER_START);
        protected.push_str(&index.to_string());
        protected.push(FORMULA_BLOCK_PLACEHOLDER_END);
        protected.push('\n');
        cursor = formula.end;
    }
    protected.push_str(&source[cursor..]);
    protected
}

/// Recognizes a block whose entire (trimmed) source is exactly one
/// placeholder token, and resolves it back to the [`Formula`] it stands in
/// for. Returns `None` for any other shape (fails closed): a placeholder can
/// only ever come from this module's own `protect_display_formula_spans` in
/// this same call, so a malformed or partial one is treated as ordinary text
/// rather than risking a panic or dropped content.
fn resolve_placeholder_formula<'a>(source: &str, formulas: &[&'a Formula]) -> Option<&'a Formula> {
    let trimmed = source.trim();
    let inner = trimmed
        .strip_prefix(FORMULA_BLOCK_PLACEHOLDER_START)?
        .strip_suffix(FORMULA_BLOCK_PLACEHOLDER_END)?;
    let index: usize = inner.parse().ok()?;
    formulas.get(index).copied()
}

fn is_complete_display_math(source: &str) -> bool {
    let candidate = source.trim();
    let Ok(formulas) = scan_latex(candidate, &ScannerLimits::default()) else {
        return false;
    };
    matches!(
        formulas.as_slice(),
        [formula] if formula.display && formula.start == 0 && formula.end == candidate.len()
    )
}

fn block_kind(tag: &Tag<'_>) -> BlockKind {
    match tag {
        Tag::Heading { .. } => BlockKind::Heading,
        Tag::List(_) => BlockKind::List,
        Tag::BlockQuote(_) => BlockKind::Quote,
        Tag::Table(_) => BlockKind::Table,
        Tag::CodeBlock(_) => BlockKind::CodeBlock,
        Tag::HtmlBlock | Tag::Paragraph => BlockKind::Paragraph,
        _ => BlockKind::Paragraph,
    }
}

fn push_block(
    blocks: &mut Vec<Block>,
    input: &str,
    kind: BlockKind,
    range: Range<usize>,
    limits: &Limits,
) -> Result<(), RenderError> {
    if range.is_empty() {
        return Ok(());
    }
    let mut source = &input[range];
    while source.ends_with("\n\n") {
        source = &source[..source.len() - 1];
    }
    while source.ends_with("\r\n\r\n") {
        source = &source[..source.len() - 2];
    }
    if source.is_empty() {
        return Ok(());
    }
    limits.check_source_bytes_per_block(source.len() as u64)?;
    limits.check_blocks_per_document((blocks.len() as u64).saturating_add(1))?;
    blocks.push(Block {
        index: blocks.len(),
        kind,
        source: source.to_owned(),
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlockKind, ErrorCode, SafeLimitKind};

    #[test]
    fn bare_bracket_display_math_spanning_a_blank_line_stays_one_block() {
        let input = "標本平均を\n\n[ \\bar{\\boldsymbol{x}}\n\n\\frac1n\\sum_{i=1}^n\\boldsymbol{x}_i ]\n\n標本散布行列を";
        let blocks = parse_blocks(input).unwrap();
        assert_eq!(
            blocks.iter().map(|b| b.kind).collect::<Vec<_>>(),
            vec![
                BlockKind::Paragraph,
                BlockKind::DisplayMath,
                BlockKind::Paragraph
            ]
        );
        assert_eq!(
            blocks[1].source,
            "[ \\bar{\\boldsymbol{x}}\n\n\\frac1n\\sum_{i=1}^n\\boldsymbol{x}_i ]"
        );
    }

    #[test]
    fn backslash_bracket_display_math_spanning_a_blank_line_stays_one_block() {
        let input =
            "標本平均を\n\n\\[\n\\bar{\\boldsymbol{x}}\n=\n\\frac1n\\sum_{i=1}^n\\boldsymbol{x}_i\n\\]\n\n標本散布行列を";
        let blocks = parse_blocks(input).unwrap();
        assert_eq!(
            blocks.iter().map(|b| b.kind).collect::<Vec<_>>(),
            vec![
                BlockKind::Paragraph,
                BlockKind::DisplayMath,
                BlockKind::Paragraph
            ]
        );
        assert_eq!(
            blocks[1].source,
            "\\[\n\\bar{\\boldsymbol{x}}\n=\n\\frac1n\\sum_{i=1}^n\\boldsymbol{x}_i\n\\]"
        );
    }

    #[test]
    fn multiple_blank_line_spanning_display_formulas_all_resolve_correctly() {
        let input = "[ \\boldsymbol{V}_n\n\n\\left( \\boldsymbol{V}_0 \\right)^{-1} ]\n\n[ \\boldsymbol{m}_n\n\n\\boldsymbol{V}_n \\boldsymbol{m}_0 ]";
        let blocks = parse_blocks(input).unwrap();
        assert_eq!(
            blocks.iter().map(|b| b.kind).collect::<Vec<_>>(),
            vec![BlockKind::DisplayMath, BlockKind::DisplayMath]
        );
        assert_eq!(
            blocks[0].source,
            "[ \\boldsymbol{V}_n\n\n\\left( \\boldsymbol{V}_0 \\right)^{-1} ]"
        );
        assert_eq!(
            blocks[1].source,
            "[ \\boldsymbol{m}_n\n\n\\boldsymbol{V}_n \\boldsymbol{m}_0 ]"
        );
    }

    #[test]
    fn splits_a_mixed_document_into_exact_source_blocks() {
        let input = concat!(
            "# Heading\n\n",
            "Paragraph with *emphasis*.\n\n",
            "- one\n- two\n\n",
            "> quote\n\n",
            "| A | B |\n| - | - |\n| 1 | 2 |\n\n",
            "```rust\nfn main() {}\n```\n\n",
            "---\n"
        );

        let blocks = parse_blocks(input).unwrap();
        assert_eq!(
            blocks.iter().map(|block| block.kind).collect::<Vec<_>>(),
            vec![
                BlockKind::Heading,
                BlockKind::Paragraph,
                BlockKind::List,
                BlockKind::Quote,
                BlockKind::Table,
                BlockKind::CodeBlock,
                BlockKind::ThematicBreak,
            ]
        );
        assert_eq!(blocks[0].source, "# Heading\n");
        assert_eq!(blocks[2].source, "- one\n- two\n");
        assert_eq!(blocks[5].source, "```rust\nfn main() {}\n```");
        assert_eq!(
            blocks.iter().map(|block| block.index).collect::<Vec<_>>(),
            (0..blocks.len()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn recognizes_closed_display_math_and_keeps_unclosed_text_literal() {
        let blocks = parse_blocks("$$x+y$$\n").unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, BlockKind::DisplayMath);
        assert_eq!(blocks[0].source, "$$x+y$$");

        let bracketed = parse_blocks("\\[\nx+y\n\\]\n").unwrap();
        assert_eq!(bracketed.len(), 1);
        assert_eq!(bracketed[0].kind, BlockKind::DisplayMath);
        assert_eq!(bracketed[0].source, "\\[\nx+y\n\\]");

        let unclosed = parse_blocks("$$x+y\n").unwrap();
        assert_eq!(unclosed.len(), 1);
        assert_eq!(unclosed[0].kind, BlockKind::Paragraph);
    }

    #[test]
    fn treats_raw_html_as_literal_paragraph_source() {
        let blocks = parse_blocks("<script>alert(1)</script>\n").unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, BlockKind::Paragraph);
        assert_eq!(blocks[0].source, "<script>alert(1)</script>\n");
    }

    #[test]
    fn limited_parser_rejects_a_block_at_split_time() {
        let limits = Limits {
            source_bytes_per_block: 4,
            ..Limits::default()
        };
        let error = parse_blocks_limited("12345\n", &limits).unwrap_err();
        assert_eq!(error.safe_record().code, ErrorCode::RendererInputLimit);
        assert_eq!(
            error.safe_record().details.as_ref().unwrap().limit_kind,
            Some(SafeLimitKind::ResponseDocumentBytes)
        );
    }

    #[test]
    fn limited_parser_accepts_empty_input_and_exact_caps() {
        assert!(parse_blocks_limited("", &Limits::default())
            .unwrap()
            .is_empty());

        let limits = Limits {
            source_bytes_per_block: 6,
            blocks_per_document: 1,
            ..Limits::default()
        };
        let blocks = parse_blocks_limited("12345\n", &limits).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].source.len(), 6);
    }

    #[test]
    fn limited_parser_rejects_the_first_block_past_the_document_cap() {
        let limits = Limits {
            blocks_per_document: 1,
            ..Limits::default()
        };
        let error = parse_blocks_limited("first\n\nsecond\n", &limits).unwrap_err();
        assert_eq!(error.safe_record().code, ErrorCode::RendererInputLimit);
        assert_eq!(
            error.safe_record().details.as_ref().unwrap().limit_kind,
            Some(SafeLimitKind::ResponseDocumentBlocks)
        );
    }
}
