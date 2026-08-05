use std::ops::Range;

use pulldown_cmark::{Event, Options, Parser, Tag};

use crate::{scan_latex, Block, BlockKind, Limits, RenderError, ScannerLimits};

/// Splits Markdown into semantic prose blocks.
pub fn parse_blocks(input: &str) -> Result<Vec<Block>, RenderError> {
    parse_blocks_limited(input, &Limits::default())
}

/// Splits Markdown while enforcing document and per-block source caps.
///
/// Phase 1 fails the whole parse with a safe error record. Replacing only the
/// offending block with an error entry is intentionally staged for Phase 2,
/// when incremental block state exists.
pub fn parse_blocks_limited(input: &str, limits: &Limits) -> Result<Vec<Block>, RenderError> {
    let parser = Parser::new_ext(input, Options::ENABLE_TABLES).into_offset_iter();
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
                        push_block(&mut blocks, input, kind, start..range.end, limits)?;
                    }
                }
            }
            Event::Rule if depth == 0 => {
                push_block(
                    &mut blocks,
                    input,
                    BlockKind::ThematicBreak,
                    range.clone(),
                    limits,
                )?;
            }
            Event::Html(_) | Event::InlineHtml(_) if depth == 0 => {
                push_block(
                    &mut blocks,
                    input,
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
                    input,
                    BlockKind::Paragraph,
                    range.clone(),
                    limits,
                )?;
            }
            _ => {}
        }
    }

    // The Markdown parser owns prose boundaries; the scanner then upgrades only
    // a complete, closed top-level display formula.
    for block in &mut blocks {
        if block.kind == BlockKind::Paragraph && is_complete_display_math(&block.source) {
            block.kind = BlockKind::DisplayMath;
        }
    }
    for (index, block) in blocks.iter_mut().enumerate() {
        block.index = index;
    }
    Ok(blocks)
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
        assert_eq!(blocks[0].source, "$$x+y$$\n");

        let bracketed = parse_blocks("\\[\nx+y\n\\]\n").unwrap();
        assert_eq!(bracketed.len(), 1);
        assert_eq!(bracketed[0].kind, BlockKind::DisplayMath);

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
