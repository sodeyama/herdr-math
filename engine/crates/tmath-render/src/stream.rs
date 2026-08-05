use crate::{
    content_hash, parse_blocks_limited, scan_latex, Block, BlockKind, Limits, RenderError,
    RenderOptions, ScannerLimits,
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
        let blocks = match parse_blocks_limited(&self.text, &self.limits) {
            Ok(blocks) => blocks,
            Err(error) => return self.fail(error),
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
                || has_unclosed_display_math(&self.text)
                || tail_is_open(&self.text, blocks.last()));
        self.previous_hashes = hashes;

        Ok(Revision {
            blocks,
            stable_prefix,
            tail_open,
        })
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
        BlockKind::DisplayMath => !ends_with_closed_display_math(text),
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

fn has_unclosed_display_math(text: &str) -> bool {
    completes_formula_with(text, "$$") || completes_formula_with(text, "\\]")
}

fn completes_formula_with(text: &str, closing: &str) -> bool {
    let original_len = text.len();
    let mut completed = String::with_capacity(original_len.saturating_add(closing.len()));
    completed.push_str(text);
    completed.push_str(closing);

    scan_latex(&completed, &ScannerLimits::default()).map_or(true, |formulas| {
        formulas
            .iter()
            .any(|formula| formula.display && formula.end > original_len)
    })
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

fn ends_with_closed_display_math(text: &str) -> bool {
    let trimmed = text.trim_end_matches([' ', '\t', '\r', '\n']);
    trimmed.ends_with("$$") || trimmed.ends_with("\\]")
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
    use crate::{content_hash, parse_blocks_limited, BlockKind, RenderOptions};

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
        let mut splitter = StreamSplitter::new(Limits::default());
        let revision = splitter.push(b"$$a+b\n\n").unwrap();

        assert_eq!(revision.blocks[0].kind, BlockKind::Paragraph);
        assert!(revision.tail_open);
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
}
