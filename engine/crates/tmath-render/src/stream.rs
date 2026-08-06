use crate::{
    content_hash, open_display_math_start, parse_blocks_limited, Block, BlockKind, Limits,
    RenderError, RenderOptions,
};

/// The current semantic block revision for a byte stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Revision {
    pub blocks: Vec<Block>,
    pub stable_prefix: usize,
    pub tail_open: bool,
}

/// Incrementally decodes and re-splits a Markdown byte stream.
pub struct StreamSplitter {
    text: String,
    carry: Vec<u8>,
    limits: Limits,
    previous_hashes: Vec<[u8; 32]>,
    failure: Option<RenderError>,
    finished: bool,
}

impl StreamSplitter {
    pub fn new(limits: Limits) -> Self {
        Self {
            text: String::new(),
            carry: Vec::with_capacity(3),
            limits,
            previous_hashes: Vec::new(),
            failure: None,
            finished: false,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<Revision, RenderError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        if self.finished {
            return self.fail(finished_stream_error());
        }

        self.decode(chunk, false);
        self.revise(false)
    }

    pub fn finish(&mut self) -> Result<Revision, RenderError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        if !self.finished {
            self.decode(&[], true);
            self.finished = true;
        }
        self.revise(true)
    }

    fn decode(&mut self, chunk: &[u8], eof: bool) {
        let mut bytes = Vec::with_capacity(self.carry.len().saturating_add(chunk.len()));
        bytes.append(&mut self.carry);
        bytes.extend_from_slice(chunk);
        let mut offset = 0;

        loop {
            match std::str::from_utf8(&bytes[offset..]) {
                Ok(valid) => {
                    self.text.push_str(valid);
                    return;
                }
                Err(error) => {
                    let valid_up_to = error.valid_up_to();
                    if valid_up_to > 0 {
                        let valid = std::str::from_utf8(&bytes[offset..offset + valid_up_to])
                            .expect("valid_up_to must end at a UTF-8 boundary");
                        self.text.push_str(valid);
                    }
                    offset = offset.saturating_add(valid_up_to);

                    match error.error_len() {
                        Some(invalid_len) => {
                            self.text.push('\u{fffd}');
                            offset = offset.saturating_add(invalid_len);
                        }
                        None if eof => {
                            self.text
                                .push_str(String::from_utf8_lossy(&bytes[offset..]).as_ref());
                            return;
                        }
                        None => {
                            self.carry.extend_from_slice(&bytes[offset..]);
                            return;
                        }
                    }
                }
            }
        }
    }

    fn revise(&mut self, eof: bool) -> Result<Revision, RenderError> {
        // While the stream is still open, an unclosed display-math opener's
        // raw LaTeX must never reach pulldown-cmark: line-leading `+`, `-`,
        // `#`, and `---` inside the still-growing formula body would be
        // misread as list items, headings, or thematic breaks, splitting the
        // eventual one-block formula into several (see
        // `specs/stream-open-tail-v1/plans/main.md`'s root-cause chain). Bundle
        // everything from the opener to the end of the buffer into a single
        // synthetic `Paragraph` tail block instead, so every revision while
        // the formula is open is a same-position tail replace. At EOF the
        // opener is final and never bundled: parse the full text exactly as
        // the one-shot parser would (AT-S-105).
        let bundled_open_tail = if eof {
            None
        } else {
            open_display_math_start(&self.text)
        };

        let (blocks, bundling_engaged) = match bundled_open_tail {
            Some(start) => match self.bundle_open_tail(start) {
                Ok(blocks) => (blocks, true),
                Err(error) => return self.fail(error),
            },
            None => match parse_blocks_limited(&self.text, &self.limits) {
                Ok(blocks) => (blocks, false),
                Err(error) => return self.fail(error),
            },
        };

        // Stream layout options are fixed; callers cannot vary them between revisions.
        let options = RenderOptions::default();
        let hashes = blocks
            .iter()
            .map(|block| content_hash(block, &options))
            .collect::<Vec<_>>();
        let stable_prefix = self
            .previous_hashes
            .iter()
            .zip(&hashes)
            .take_while(|(previous, current)| previous == current)
            .count();
        let tail_open = !eof
            && (!self.carry.is_empty()
                || bundling_engaged
                || tail_is_open(&self.text, blocks.last()));
        self.previous_hashes = hashes;

        Ok(Revision {
            blocks,
            stable_prefix,
            tail_open,
        })
    }

    /// Parses only the prefix before the open formula's opener, then appends
    /// the still-open span as one synthetic `Paragraph` block, subject to the
    /// same per-block byte cap and document block-count cap
    /// `parse_blocks_limited` would enforce (AT-S-107): a cap violation fails
    /// closed exactly like an oversized ordinary block.
    fn bundle_open_tail(&self, start: usize) -> Result<Vec<Block>, RenderError> {
        let mut blocks = parse_blocks_limited(&self.text[..start], &self.limits)?;
        let tail_source = self.text[start..].to_owned();
        self.limits
            .check_source_bytes_per_block(tail_source.len() as u64)?;
        self.limits
            .check_blocks_per_document((blocks.len() as u64).saturating_add(1))?;
        blocks.push(Block {
            index: blocks.len(),
            kind: BlockKind::Paragraph,
            source: tail_source,
        });
        Ok(blocks)
    }

    fn fail<T>(&mut self, error: RenderError) -> Result<T, RenderError> {
        self.failure = Some(error.clone());
        Err(error)
    }
}

fn finished_stream_error() -> RenderError {
    RenderError::new(
        crate::SafeErrorRecord {
            code: crate::ErrorCode::InternalError,
            retryable: false,
            details: None,
        },
        "cannot push after the stream is finished",
    )
}

fn tail_is_open(text: &str, last_block: Option<&Block>) -> bool {
    let Some(last_block) = last_block else {
        return !text.is_empty();
    };

    match last_block.kind {
        BlockKind::CodeBlock => code_tail_is_open(text, &last_block.source),
        BlockKind::DisplayMath => !ends_with_closed_display_math(text, &last_block.source),
        BlockKind::ThematicBreak | BlockKind::Heading => !ends_with_line_terminator(text),
        BlockKind::Paragraph | BlockKind::List | BlockKind::Quote | BlockKind::Table => {
            !ends_with_blank_line(text)
        }
    }
}

fn code_tail_is_open(text: &str, source: &str) -> bool {
    let first_line = source.lines().next().unwrap_or_default();
    let source_indent = first_line.bytes().take_while(|byte| *byte == b' ').count();
    let candidate = &first_line[source_indent..];
    let omitted_indent = text.rfind(source).map_or(0, |start| {
        let line_start = text[..start].rfind('\n').map_or(0, |index| index + 1);
        text[line_start..start]
            .bytes()
            .take_while(|byte| *byte == b' ')
            .count()
    });
    let total_indent = source_indent.saturating_add(omitted_indent);
    let is_fenced =
        total_indent <= 3 && (candidate.starts_with("```") || candidate.starts_with("~~~"));

    if is_fenced {
        !ends_with_closed_fence(text)
    } else {
        !ends_with_blank_line(text)
    }
}

fn ends_with_line_terminator(text: &str) -> bool {
    text.ends_with('\n') || text.ends_with('\r')
}

fn ends_with_blank_line(text: &str) -> bool {
    let without_terminator = if let Some(text) = text.strip_suffix("\r\n") {
        text
    } else if let Some(text) = text.strip_suffix(['\n', '\r']) {
        text
    } else {
        return false;
    };
    let final_line = without_terminator
        .rsplit_once(['\n', '\r'])
        .map_or(without_terminator, |(_, line)| line);

    final_line.bytes().all(|byte| matches!(byte, b' ' | b'\t'))
}

/// A `DisplayMath` block's source is always the exact closing-delimiter-
/// inclusive slice of the input (`parse_blocks_limited` restores it verbatim,
/// including for a bare-bracket formula, whose closer is a plain `]` rather
/// than `\]`/`$$`). Comparing against the block's own source, instead of a
/// fixed set of delimiter strings, is what makes this correct for all three
/// display-math openers (fixes the bare-bracket `tail_open` gap noted in the
/// plan's Secondary defect).
fn ends_with_closed_display_math(text: &str, block_source: &str) -> bool {
    let trimmed = text.trim_end_matches([' ', '\t', '\r', '\n']);
    trimmed.ends_with(block_source)
}

fn ends_with_closed_fence(text: &str) -> bool {
    let mut active: Option<(u8, usize)> = None;

    for line in text.split_inclusive('\n') {
        let line = line.trim_end_matches(['\r', '\n']);
        let indent = line.bytes().take_while(|byte| *byte == b' ').count();
        if indent > 3 {
            continue;
        }
        let candidate = &line.as_bytes()[indent..];
        let Some(marker) = candidate
            .first()
            .copied()
            .filter(|byte| matches!(byte, b'`' | b'~'))
        else {
            continue;
        };
        let length = candidate.iter().take_while(|byte| **byte == marker).count();
        if length < 3 {
            continue;
        }

        match active {
            None => active = Some((marker, length)),
            Some((open_marker, open_length))
                if marker == open_marker
                    && length >= open_length
                    && candidate[length..].iter().all(u8::is_ascii_whitespace) =>
            {
                active = None;
            }
            Some(_) => {}
        }
    }

    active.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        content_hash, parse_blocks_limited, BlockKind, PlacementPlanner, PlanOp, RenderOptions,
    };

    fn assert_equivalent(input: &str, stride: usize) {
        let limits = Limits::default();
        let expected = parse_blocks_limited(input, &limits).unwrap();
        let mut splitter = StreamSplitter::new(limits);

        for chunk in input.as_bytes().chunks(stride) {
            splitter.push(chunk).unwrap();
        }
        let actual = splitter.finish().unwrap();

        assert_eq!(actual.blocks, expected, "stride {stride}");
        assert!(!actual.tail_open);
    }

    #[test]
    fn at_3_401_final_revision_matches_one_shot_for_every_chunk_stride() {
        let documents = [
            concat!(
                "Prose with $x+y$.\n\n",
                "$$a^2+b^2=c^2$$\n\n",
                "```rust\nfn main() {}\n```\n\n",
                "| A | B |\n| - | - |\n| 1 | 2 |\n"
            ),
            "日本語と絵文字 🦀 の段落。\n\n> 引用 😀\n\n$$東京+大阪$$\n",
            "- outer\n  - nested one\n  - nested two\n- final\n\nAfter the list.\n",
        ];

        for document in documents {
            for stride in [1, 3, 7] {
                assert_equivalent(document, stride);
            }
        }
    }

    #[test]
    fn mid_utf8_and_mid_display_delimiters_never_corrupt_valid_input() {
        let input = "Prefix 🦀\n\n$$a+b$$\n";
        let crab = input.find('🦀').unwrap();
        let closing = input.rfind("$$").unwrap();
        let split_offsets = [crab + 1, crab + 3, closing + 1];
        let mut splitter = StreamSplitter::new(Limits::default());
        let mut start = 0;

        for end in split_offsets.into_iter().chain([input.len()]) {
            let revision = splitter.push(&input.as_bytes()[start..end]).unwrap();
            assert!(revision
                .blocks
                .iter()
                .all(|block| !block.source.contains('\u{fffd}')));
            start = end;
        }

        assert_eq!(
            splitter.finish().unwrap().blocks,
            parse_blocks_limited(input, &Limits::default()).unwrap()
        );
    }

    #[test]
    fn invalid_utf8_is_replaced_only_after_it_cannot_become_valid() {
        let mut splitter = StreamSplitter::new(Limits::default());

        let incomplete = splitter.push(&[b'a', 0xe2]).unwrap();
        assert_eq!(incomplete.blocks[0].source, "a");
        assert!(!incomplete.blocks[0].source.contains('\u{fffd}'));

        let invalid = splitter.push(b"x").unwrap();
        assert_eq!(invalid.blocks[0].source, "a\u{fffd}x");
    }

    #[test]
    fn at_3_404_unclosed_display_math_upgrades_when_closed() {
        let mut splitter = StreamSplitter::new(Limits::default());
        let first = splitter.push(b"Earlier.\n\n$$a+b").unwrap();

        assert_eq!(first.blocks.len(), 2);
        assert_eq!(first.blocks[1].kind, BlockKind::Paragraph);
        assert_eq!(first.blocks[1].source, "$$a+b");
        assert!(first.tail_open);
        let earlier_hash = content_hash(&first.blocks[0], &RenderOptions::default());

        let second = splitter.push(b"$$\n\n").unwrap();
        assert_eq!(second.blocks[1].kind, BlockKind::DisplayMath);
        assert!(!second.tail_open);
        assert_eq!(second.stable_prefix, 1);
        assert_eq!(
            content_hash(&second.blocks[0], &RenderOptions::default()),
            earlier_hash
        );
    }

    #[test]
    fn unclosed_display_math_remains_open_even_after_a_blank_line() {
        // Strengthened for T-S-201/202: the blank line inside the still-open
        // formula must not fool pulldown-cmark into splitting the tail into
        // more than one block (previously only kind/tail_open were checked,
        // which a broken-apart-but-still-Paragraph-first split would have
        // passed). Bundling keeps the whole open span as exactly one
        // synthetic Paragraph tail block, byte-for-byte.
        let mut splitter = StreamSplitter::new(Limits::default());
        let revision = splitter.push(b"$$a+b\n\n").unwrap();

        assert_eq!(revision.blocks.len(), 1);
        assert_eq!(revision.blocks[0].kind, BlockKind::Paragraph);
        assert_eq!(revision.blocks[0].source, "$$a+b\n\n");
        assert!(revision.tail_open);

        // Pushing more text that still doesn't close the formula must keep
        // the span as one block across the blank line.
        let second = splitter.push(b"more\n\nlines\n").unwrap();
        assert_eq!(second.blocks.len(), 1);
        assert_eq!(second.blocks[0].kind, BlockKind::Paragraph);
        assert!(second.tail_open);
    }

    #[test]
    fn unclosed_fence_stays_open_and_upgrades_when_closed() {
        let mut splitter = StreamSplitter::new(Limits::default());
        let first = splitter.push(b"Before.\n\n```rust\nfn main() {}").unwrap();

        assert_eq!(first.blocks.len(), 2);
        assert_eq!(first.blocks[1].kind, BlockKind::CodeBlock);
        assert!(first.tail_open);

        let second = splitter.push(b"\n```\n").unwrap();
        assert_eq!(second.blocks[1].kind, BlockKind::CodeBlock);
        assert!(!second.tail_open);
        assert_eq!(second.stable_prefix, 1);
    }

    #[test]
    fn indented_code_requires_a_blank_line_to_close() {
        let mut splitter = StreamSplitter::new(Limits::default());
        let open = splitter.push(b"    literal code\n").unwrap();
        assert_eq!(open.blocks[0].kind, BlockKind::CodeBlock);
        assert!(open.tail_open);

        let closed = splitter.push(b"\n").unwrap();
        assert!(!closed.tail_open);
    }

    #[test]
    fn indented_code_starting_with_backticks_is_not_a_fenced_block() {
        let mut splitter = StreamSplitter::new(Limits::default());
        let open = splitter.push(b"    ```\n    literal code\n").unwrap();
        assert_eq!(open.blocks[0].kind, BlockKind::CodeBlock);
        assert!(open.tail_open);

        let closed = splitter.push(b"\n").unwrap();
        assert!(!closed.tail_open);
    }

    #[test]
    fn thematic_break_closes_only_after_its_line_terminator() {
        let mut splitter = StreamSplitter::new(Limits::default());
        let open = splitter.push(b"---").unwrap();
        assert_eq!(open.blocks[0].kind, BlockKind::ThematicBreak);
        assert!(open.tail_open);

        let closed = splitter.push(b"\n").unwrap();
        assert!(!closed.tail_open);
    }

    #[test]
    fn whitespace_only_blank_line_closes_a_prose_tail() {
        let mut splitter = StreamSplitter::new(Limits::default());
        let closed = splitter.push(b"Paragraph.\n \t\n").unwrap();

        assert_eq!(closed.blocks[0].kind, BlockKind::Paragraph);
        assert!(!closed.tail_open);
    }

    #[test]
    fn stable_prefix_covers_unchanged_blocks_and_empty_pushes() {
        let mut splitter = StreamSplitter::new(Limits::default());
        let first = splitter.push(b"First.\n\n").unwrap();
        assert_eq!(first.stable_prefix, 0);
        assert!(!first.tail_open);

        let unchanged = splitter.push(b"").unwrap();
        assert_eq!(unchanged.stable_prefix, first.blocks.len());

        let appended = splitter.push(b"Second.\n\n").unwrap();
        assert_eq!(appended.stable_prefix, first.blocks.len());
        assert_eq!(appended.blocks.len(), 2);
        assert!(!appended.tail_open);
    }

    #[test]
    fn finish_closes_tail_without_changing_content() {
        let mut splitter = StreamSplitter::new(Limits::default());
        let open = splitter.push(b"Still growing").unwrap();
        assert!(open.tail_open);

        let finished = splitter.finish().unwrap();
        assert_eq!(finished.blocks, open.blocks);
        assert_eq!(finished.stable_prefix, open.blocks.len());
        assert!(!finished.tail_open);
    }

    #[test]
    fn incomplete_utf8_bytes_keep_the_tail_open_until_finish() {
        let mut splitter = StreamSplitter::new(Limits::default());
        let open = splitter.push(&[0xf0, 0x9f]).unwrap();
        assert!(open.blocks.is_empty());
        assert!(open.tail_open);

        let finished = splitter.finish().unwrap();
        assert_eq!(finished.blocks[0].source, "\u{fffd}");
        assert!(!finished.tail_open);
    }

    #[test]
    fn limit_failure_is_sticky() {
        let limits = Limits {
            source_bytes_per_block: 4,
            ..Limits::default()
        };
        let mut splitter = StreamSplitter::new(limits);
        let first = splitter.push(b"12345").unwrap_err();
        let second = splitter.push(b"ignored").unwrap_err();

        assert_eq!(second, first);
        assert_eq!(splitter.finish().unwrap_err(), first);
    }

    #[test]
    fn block_count_limit_failure_is_also_sticky() {
        // The document block cap applies to the entire buffered stream, and
        // push/finish keep returning the same error after the first failure.
        let limits = Limits {
            blocks_per_document: 1,
            ..Limits::default()
        };
        let mut splitter = StreamSplitter::new(limits);
        let first = splitter.push(b"First.\n\nSecond.\n\n").unwrap_err();
        let second = splitter.push(b"Third.\n\n").unwrap_err();

        assert_eq!(second, first);
        assert_eq!(splitter.finish().unwrap_err(), first);
    }

    #[test]
    fn finish_is_idempotent_after_eof() {
        // Repeated finish calls return the same finalized revision.
        let mut splitter = StreamSplitter::new(Limits::default());
        splitter.push(b"Prose.\n\n$$a+b").unwrap();

        let first = splitter.finish().unwrap();
        let second = splitter.finish().unwrap();

        assert_eq!(first.blocks, second.blocks);
        assert!(!first.tail_open);
        assert!(!second.tail_open);
        assert_eq!(
            first.blocks,
            parse_blocks_limited("Prose.\n\n$$a+b", &Limits::default()).unwrap()
        );
    }

    #[test]
    fn adversarial_chunk_streams_never_panic_or_diverge_from_one_shot_parse() {
        // AT-3-601: deterministic fuzz over chunk boundaries and markdown
        // tokens. The splitter must always terminate, stay within limits, and
        // either match the one-shot parser or fail closed with the same limit.
        let mut seed = 0x57EA_0001_u64;
        let tokens = [
            "# ",
            "## ",
            "- ",
            "1. ",
            "> ",
            "| A | B |\n| - | - |\n| 1 | 2 |\n",
            "```\n",
            "\n```\n",
            "$",
            "$$",
            "\n\n",
            " ",
            "x",
            "🦀",
            "a+b",
            "\\(",
            "\\)",
            "plain",
        ];

        for iteration in 0..256u64 {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let length = 1 + (iteration as usize % 48);
            let mut document = String::new();
            for _ in 0..length {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                document.push_str(tokens[(seed as usize) % tokens.len()]);
            }

            let limits = Limits::default();
            let expected = parse_blocks_limited(&document, &limits);
            let stride = 1 + ((seed >> 16) as usize % 11);
            let mut splitter = StreamSplitter::new(limits);
            let mut push_result = Ok(());
            for chunk in document.as_bytes().chunks(stride) {
                push_result = splitter.push(chunk).map(|_| ());
            }
            let finish_result = splitter.finish();

            match (expected, push_result, finish_result) {
                (Ok(blocks), Ok(()), Ok(revision)) => {
                    assert_eq!(revision.blocks, blocks, "iteration {iteration}");
                    assert!(
                        !revision
                            .blocks
                            .iter()
                            .any(|block| block.source.contains('\u{fffd}')),
                        "valid UTF-8 input must not pick up replacement chars"
                    );
                }
                (Err(expected_error), Err(push_error), _) => {
                    assert_eq!(push_error, expected_error);
                }
                (Err(expected_error), Ok(()), Err(finish_error)) => {
                    assert_eq!(finish_error, expected_error);
                }
                other => panic!("splitter/one-shot mismatch at iteration {iteration}: {other:?}"),
            }
        }
    }

    // --- Open-tail bundling (specs/stream-open-tail-v1, T-S-201/202/203) ---

    #[test]
    fn at_s_101_unclosed_backslash_bracket_formula_stays_one_paragraph_tail_across_strides() {
        // AT-S-101: line-leading `+`, `-`, `#`, and `---` inside the still-open
        // formula body must never split it into multiple blocks or get
        // misclassified as List/Heading/ThematicBreak while unclosed.
        let opener = "Update:\n\\[\n";
        let body =
            "\\Lambda_{n+1} = \\Lambda_n\n+ \\alpha\n- \\beta\n# not a heading\n---\nmore terms\n";
        let input = format!("{opener}{body}");

        for stride in [1, 3, 7, 64] {
            let mut splitter = StreamSplitter::new(Limits::default());
            for (end, chunk) in input
                .as_bytes()
                .chunks(stride)
                .scan(0usize, |offset, chunk| {
                    *offset += chunk.len();
                    Some((*offset, chunk))
                })
            {
                let revision = splitter.push(chunk).unwrap();
                if end < opener.len() {
                    // The opener itself has not fully arrived yet.
                    continue;
                }
                let tail = revision
                    .blocks
                    .last()
                    .expect("at least one block once the opener has arrived");
                assert_eq!(
                    tail.kind,
                    BlockKind::Paragraph,
                    "stride {stride} at byte {end}: open span must stay Paragraph, got {:?} in {:?}",
                    tail.kind,
                    revision.blocks.iter().map(|b| b.kind).collect::<Vec<_>>()
                );
                assert!(
                    revision.tail_open,
                    "stride {stride} at byte {end}: tail_open must be true while the formula is unclosed"
                );
                assert!(
                    revision
                        .blocks
                        .iter()
                        .all(|block| !matches!(
                            block.kind,
                            BlockKind::List | BlockKind::Heading | BlockKind::ThematicBreak
                        )),
                    "stride {stride} at byte {end}: no block may be misclassified as List/Heading/ThematicBreak while the formula is open"
                );
            }
        }
    }

    #[test]
    fn at_s_102_formula_completion_is_a_pure_tail_update_with_stable_prefix_covering_the_prefix() {
        // AT-S-102: when the closer arrives, stable_prefix must cover every
        // block before the formula, and the formula itself collapses to one
        // DisplayMath block (a pure same-position tail replace).
        let mut splitter = StreamSplitter::new(Limits::default());
        let opening = splitter
            .push(b"Intro paragraph.\n\nSecond paragraph.\n\n\\[\n\\Lambda_n\n+ \\alpha\n")
            .unwrap();

        assert_eq!(opening.blocks.len(), 3);
        assert_eq!(opening.blocks[2].kind, BlockKind::Paragraph);
        let blocks_before_formula = opening.blocks.len() - 1;

        let closed = splitter.push(b"- \\beta\n\\]\n\n").unwrap();

        assert_eq!(closed.stable_prefix, blocks_before_formula);
        assert_eq!(closed.blocks.len(), 3);
        assert_eq!(closed.blocks[2].kind, BlockKind::DisplayMath);
        assert!(!closed.tail_open);
    }

    #[test]
    fn at_s_103_bare_bracket_formula_spanning_a_blank_line_bundles_and_reports_tail_open() {
        // AT-S-103: bare-bracket display math had no unclosed detection at
        // all before this fix; it must now get the same one-block-tail and
        // tail_open guarantees as \[ and $$, including across a blank line
        // inside the still-open body.
        let mut splitter = StreamSplitter::new(Limits::default());
        let opening = splitter
            .push(b"Density is\n\n[ \\boldsymbol{V}_n\n\nkeeps growing\n")
            .unwrap();

        let tail = opening.blocks.last().unwrap();
        assert_eq!(tail.kind, BlockKind::Paragraph);
        assert!(opening.tail_open);
        let blocks_before_formula = opening.blocks.len() - 1;

        let closed = splitter.push(b"still open\n").unwrap();
        assert_eq!(closed.blocks.last().unwrap().kind, BlockKind::Paragraph);
        assert!(closed.tail_open);
        assert_eq!(closed.blocks.len(), opening.blocks.len());

        let finished = splitter.push(b" ]\n\n").unwrap();
        assert_eq!(finished.stable_prefix, blocks_before_formula);
        assert_eq!(finished.blocks.last().unwrap().kind, BlockKind::DisplayMath);
        assert!(!finished.tail_open);
    }

    #[test]
    fn at_s_104_dollar_dollar_formula_across_chunks_keeps_the_same_guarantees() {
        // AT-S-104: regression guard for the delimiter that already worked
        // before this fix.
        let mut splitter = StreamSplitter::new(Limits::default());
        let opening = splitter
            .push(b"Before.\n\n$$\na + b\n- c\n# not a heading\n")
            .unwrap();

        assert_eq!(opening.blocks.len(), 2);
        assert_eq!(opening.blocks[1].kind, BlockKind::Paragraph);
        assert!(opening.tail_open);

        let closed = splitter.push(b"---\nd\n$$\n\n").unwrap();
        assert_eq!(closed.blocks.len(), 2);
        assert_eq!(closed.blocks[1].kind, BlockKind::DisplayMath);
        assert_eq!(closed.stable_prefix, 1);
        assert!(!closed.tail_open);
    }

    #[test]
    fn at_s_105_eof_with_an_unclosed_opener_matches_the_one_shot_parser_block_for_block() {
        // AT-S-105: at EOF an unclosed opener is final; finish() must parse
        // the full text exactly like parse_blocks_limited, never leaving a
        // stuck synthetic tail block.
        let input = "Prose.\n\n\\[\n\\Lambda_n\n+ \\alpha\n- \\beta\n# not a heading\n---\nstill unclosed\n";
        let mut splitter = StreamSplitter::new(Limits::default());
        for chunk in input.as_bytes().chunks(5) {
            splitter.push(chunk).unwrap();
        }
        let finished = splitter.finish().unwrap();

        let expected = parse_blocks_limited(input, &Limits::default()).unwrap();
        assert_eq!(finished.blocks, expected);
        assert!(!finished.tail_open);
    }

    #[test]
    fn at_s_106_fenced_code_containing_openers_is_never_bundled() {
        // AT-S-106: `\[` and `$$` inside an unclosed fence are not openers;
        // the fence must keep using ordinary CodeBlock tail-open detection,
        // not the bundling path, and the final fence block is CodeBlock.
        let mut splitter = StreamSplitter::new(Limits::default());
        let opening = splitter
            .push(b"Before.\n\n```text\n\\[\nnever closes here\n$$\nalso not a formula\n")
            .unwrap();

        assert_eq!(opening.blocks.len(), 2);
        assert_eq!(opening.blocks[1].kind, BlockKind::CodeBlock);
        assert!(opening.tail_open);

        let closed = splitter.push(b"```\n").unwrap();
        assert_eq!(closed.blocks.len(), 2);
        assert_eq!(closed.blocks[1].kind, BlockKind::CodeBlock);
        assert!(!closed.tail_open);
    }

    // --- Corpus replay (specs/stream-open-tail-v1, T-S-301/302) ---

    /// A fixture modeled on the field answer that triggered the 2026-08-06
    /// incident (see `specs/stream-open-tail-v1/plans/main.md`'s incident
    /// summary): Japanese prose, headings, a list, thematic breaks, four
    /// consecutive `\[...\]` display formulas whose bodies contain
    /// line-leading `+`, `-`, `(`, and `{` lines (the exact pulldown-cmark
    /// misread pattern that produced 7 non-tail replaces before the fix), one
    /// `$$...$$` formula, and one bare-bracket formula with a blank line
    /// inside its body. Shared by AT-S-201 (block-for-block equivalence) and
    /// AT-S-202 (no-interior-divergence planner replay).
    const AT_S_201_CORPUS: &str = concat!(
        "# 事後分布の更新\n\n",
        "正規逆ウィシャート分布の事後パラメータは、観測データを用いて逐次的に更新される。\n",
        "以下の手順で計算する。\n\n",
        "## 更新式\n\n",
        "- 平均パラメータ `mu` を更新する\n",
        "- 精度パラメータ `kappa` を更新する\n",
        "- 自由度 `nu` を更新する\n\n",
        "---\n\n",
        "まず平均の更新式は次の通り。\n\n",
        "\\[\n",
        "\\boldsymbol{\\mu}_n\n",
        "=\n",
        "\\frac{\\kappa_0 \\boldsymbol{\\mu}_0 + n \\bar{\\boldsymbol{x}}}{\\kappa_0 + n}\n",
        "+\n",
        "(\\boldsymbol{0})\n",
        "\\]\n\n",
        "次に精度パラメータの更新式。\n\n",
        "\\[\n",
        "\\kappa_n\n",
        "=\n",
        "\\kappa_0\n",
        "+ n\n",
        "- 0\n",
        "\\]\n\n",
        "続いて自由度の更新式。\n\n",
        "\\[\n",
        "\\nu_n\n",
        "=\n",
        "\\nu_0\n",
        "+ n\n",
        "{\\nu_0}\n",
        "\\]\n\n",
        "最後に散布行列の更新式。\n\n",
        "\\[\n",
        "\\boldsymbol{\\Lambda}_n\n",
        "=\n",
        "\\boldsymbol{\\Lambda}_0\n",
        "+\\boldsymbol{S}\n",
        "+\n",
        "\\frac{\\kappa_0 n}{\\kappa_0 + n}\n",
        "(\\bar{\\boldsymbol{x}} - \\boldsymbol{\\mu}_0)\n",
        "\\]\n\n",
        "これらをまとめると、同時分布は次のように書ける。\n\n",
        "$$\n",
        "p(\\boldsymbol{\\mu}, \\boldsymbol{\\Lambda}) \\propto\n",
        "|\\boldsymbol{\\Lambda}|^{(\\nu_0 - d - 1)/2}\n",
        "$$\n\n",
        "エージェントによっては `\\[` を落として次のように出力することがある。\n\n",
        "[ \\boldsymbol{x}_n\n",
        "\n",
        "= \\boldsymbol{\\mu}_n + \\boldsymbol{\\epsilon}_n ]\n\n",
        "以上が更新手順の全体像である。\n"
    );

    #[test]
    fn at_s_201_corpus_replay_final_revision_matches_one_shot_for_every_stride() {
        // AT-S-201: for every stride, streaming the corpus chunk by chunk
        // then finishing must yield the same block list (kind + source,
        // block for block) as the one-shot parser over the full text.
        let limits = Limits::default();
        let expected = parse_blocks_limited(AT_S_201_CORPUS, &limits).unwrap();

        for stride in [1usize, 3, 7, 64, 1024] {
            let mut splitter = StreamSplitter::new(limits);
            for chunk in AT_S_201_CORPUS.as_bytes().chunks(stride) {
                splitter.push(chunk).unwrap();
            }
            let finished = splitter.finish().unwrap();

            assert_eq!(finished.blocks, expected, "stride {stride}");
            assert!(!finished.tail_open, "stride {stride}");
        }
    }

    #[test]
    fn at_s_202_corpus_replay_never_produces_a_non_tail_replace_or_any_remove() {
        // AT-S-202: across every revision of the AT-S-201 replay (every
        // stride), every `Replace` in the planner-derived plan must target
        // the block that was the last planned block of the previous
        // revision (a pure tail replace), and no `Remove` may ever be
        // planned. This is the regression guard for the incident's root
        // cause: before the open-tail bundling fix, the same corpus
        // produced interior (non-tail) replaces when a split-apart open
        // formula later merged into one DisplayMath block.
        let limits = Limits::default();
        let options = RenderOptions::default();

        for stride in [1usize, 3, 7, 64, 1024] {
            let mut splitter = StreamSplitter::new(limits);
            let mut planner = PlacementPlanner::new();

            for chunk in AT_S_201_CORPUS.as_bytes().chunks(stride) {
                let revision = splitter.push(chunk).unwrap();
                assert_planner_replay_is_tail_only(&mut planner, &revision, &options, stride);
            }
            let finished = splitter.finish().unwrap();
            assert_planner_replay_is_tail_only(&mut planner, &finished, &options, stride);
        }
    }

    /// Feeds one revision's blocks into `planner` the same way
    /// `native_stream.rs::apply_revision` does (hash each block with
    /// `content_hash`, pair it with placeholder width/height dimensions —
    /// the planner is dimension-agnostic for this invariant, so a fixed
    /// stand-in is sufficient — and call `PlacementPlanner::plan`), then
    /// asserts every `Replace` targets the id that was the previous
    /// revision's last planned block and no `Remove` is ever produced.
    fn assert_planner_replay_is_tail_only(
        planner: &mut PlacementPlanner,
        revision: &Revision,
        options: &RenderOptions,
        stride: usize,
    ) {
        let previous_last_id = planner.blocks().last().map(|block| block.id);
        let inputs: Vec<([u8; 32], u32, u32)> = revision
            .blocks
            .iter()
            .map(|block| (content_hash(block, options), 320, 20))
            .collect();

        let plan = planner.plan(&inputs);

        for op in &plan.ops {
            match op {
                PlanOp::Replace { old_id, .. } => {
                    assert_eq!(
                        Some(*old_id),
                        previous_last_id,
                        "stride {stride}: Replace targeted a non-tail block (old_id {old_id}, \
                         previous tail was {previous_last_id:?})"
                    );
                }
                PlanOp::Remove { id } => {
                    panic!("stride {stride}: unexpected Remove of block {id}");
                }
                PlanOp::Keep { .. } | PlanOp::Append { .. } => {}
            }
        }
    }

    #[test]
    fn at_s_107_oversized_open_tail_fails_closed_and_stays_sticky() {
        // AT-S-107: an opener followed by more bytes than
        // source_bytes_per_block without closing must report the same
        // stable limit error the one-shot parser reports for an oversized
        // block, and the failure must be sticky on subsequent pushes,
        // matching existing splitter semantics (limit_failure_is_sticky).
        let limits = Limits {
            source_bytes_per_block: 16,
            ..Limits::default()
        };
        let mut splitter = StreamSplitter::new(limits);
        let oversized_tail = b"\\[\nthis body is deliberately longer than the byte cap";

        let error = splitter.push(oversized_tail).unwrap_err();
        let expected = parse_blocks_limited(std::str::from_utf8(oversized_tail).unwrap(), &limits)
            .unwrap_err();
        assert_eq!(error, expected);

        let second = splitter.push(b"ignored").unwrap_err();
        assert_eq!(second, error);
        assert_eq!(splitter.finish().unwrap_err(), error);
    }
}
