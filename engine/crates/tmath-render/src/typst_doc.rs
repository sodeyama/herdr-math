//! Converts parsed Markdown events into Typst source.
//!
//! User-controlled text is emitted only as escaped Typst string literals inside
//! `#text("...")` or `#raw("...")` calls. Structural Typst syntax is selected
//! solely from pulldown-cmark event variants, so Markdown text cannot become
//! Typst markup.

use std::iter::Peekable;

use pulldown_cmark::{Alignment, CodeBlockKind, CowStr, Event, HeadingLevel, Options, Parser, Tag};

use crate::{
    limits::{render_guard, RenderDeadline},
    math::render_formula_with_deadline,
    scan_latex, Block, BlockKind, ErrorCode, Formula, Limits, MathImage, RenderError,
    RenderOptions, SafeErrorRecord, ScannerLimits, DARK_THEME_TEXT_COLOR, TABLE_STROKE_COLOR,
};

/// Line rhythm (D-LINE): the em multiples that produce the composed page's
/// text metrics. Chosen for comfortable, EVEN spacing across mixed
/// Japanese/Latin prose at typical terminal font sizes (the live 17pt
/// build), not to minimize image height.
///
/// The original values (`top-edge`/`bottom-edge: "bounds"`, `leading:
/// 0.35em`) sized each line box to the tightest possible ink bounding box
/// with a below-default gap between boxes — this reads fine for all-Latin
/// text (short ascenders/descenders, most glyphs share a similar ink
/// height) but is uneven and cramped for Japanese: CJK glyphs are close to
/// full-em height with no ascender/descender rhythm, so a `bounds`-sized
/// line box varies per line depending on which glyphs happen to appear,
/// and the tight leading left no room to absorb that variance. There is no
/// code comment or commit message recording why `bounds`/`0.35em` were
/// chosen (`git blame`/`git log -S` traced to the T3-103 introducing
/// commit with no rationale) — the working assumption is that "bounds"
/// keeps the placed image as short as possible.
///
/// Typst's `par.leading` is the gap from one line's `bottom-edge` to the
/// next line's `top-edge` (see the Typst reference for `par`/`text`), so
/// baseline-to-baseline distance equals `TEXT_TOP_EDGE_EM -
/// TEXT_BOTTOM_EDGE_EM + PAR_LEADING_EM`. CJK typography convention calls
/// for roughly 1.5-1.7x the font size between baselines (vs. Latin's usual
/// ~1.2x); this picks the middle of that range.
pub(crate) const TARGET_LINE_ADVANCE_EM: f64 = 1.6;
/// Top edge fixed at a constant fraction of an em (rather than `"bounds"`)
/// so every line reserves the same headroom regardless of which glyphs it
/// contains — close to a typical ascender, comfortably above CJK full-em
/// glyphs.
const TEXT_TOP_EDGE_EM: f64 = 0.8;
/// Bottom edge fixed the same way, extending slightly below the baseline
/// (negative = below) to give descenders (Latin) and the bottom stroke
/// weight of CJK glyphs even clearance.
const TEXT_BOTTOM_EDGE_EM: f64 = -0.3;
/// `par.leading` is derived so the two edges above plus this leading sum to
/// exactly `TARGET_LINE_ADVANCE_EM`.
const PAR_LEADING_EM: f64 = TARGET_LINE_ADVANCE_EM - TEXT_TOP_EDGE_EM + TEXT_BOTTOM_EDGE_EM;

/// Undoes Typst's built-in `raw` show rule, which sets inline AND block code
/// to `0.8em` of the surrounding text size by default (mono fonts read
/// visually larger at equal pt than proportional ones, per Typst's own
/// rationale — but that assumption doesn't hold against M PLUS 2/NewCM10 at
/// this renderer's sizes: empirically measured ink-row height for an
/// inline `` `code` `` span and a fenced code block were both ~0.83x a
/// plain-text control at the live 15pt/dpr2 geometry, matching the 0.8em
/// figure). A `show raw: set text(size: ...)` rule's `em` is relative to
/// raw's OWN already-0.8x'd context, so `1.0 / 0.8` restores exactly the
/// surrounding body text size for both inline and block code alike — per
/// the task's finding that inline and block measured the same reduction,
/// there is no reason to keep a separate, smaller block-code size.
const RAW_TEXT_SIZE_EM: f64 = 1.0 / 0.8;

/// The primary (Latin/math) prose font. Fixed — `RenderOptions` only ever
/// selects among embedded CJK families (`CjkFont`); Latin coverage does not
/// vary per session.
const PRIMARY_FONT: &str = "NewCM10";

/// Builds the exact `#set text(font: (...))` fallback-list argument for
/// `options.cjk_font` — the ONE place this list is constructed (D-CONFIG
/// phase 2), so every block's composed Typst source and any future
/// multi-family selection stay derived from a single source of truth
/// instead of a second hard-coded family name drifting out of sync.
pub(crate) fn font_fallback_list(cjk_font: crate::CjkFont) -> String {
    let mut list = String::from("(\"");
    escape_typst_string_into(PRIMARY_FONT, &mut list);
    list.push_str("\", \"");
    escape_typst_string_into(cjk_font.typst_family_name(), &mut list);
    list.push_str("\")");
    list
}

/// Inter-block vertical margin (D-LINE, uniform inter-block spacing): each
/// semantic block (heading, paragraph, list, standalone display-math, ...)
/// renders as its own `#set page(margin: 0pt, height: auto, ...)` Typst
/// document, and the viewer/stream emitter then stacks those independently
/// rendered block images with zero gap between them. Before this constant
/// existed, that meant the visual gap BETWEEN two blocks was always exactly
/// zero — regardless of `TARGET_LINE_ADVANCE_EM`/`PAR_LEADING_EM` — while the
/// gap BETWEEN two lines *within* one block (a `#linebreak()` or wrapped
/// line) got the full designed `PAR_LEADING_EM` line-box-to-line-box gap.
/// That inconsistency, not weight/bold at all, is what a live-run bug
/// report perceived as "bold breaks the line spacing": headings and
/// bold-led lines are disproportionately followed by paragraph *blocks*
/// (new Markdown block boundaries), so the report's true confound was block
/// adjacency, not bold — see `prose.rs`'s
/// `bold_spans_do_not_change_the_per_line_advance` test, which already
/// disproved a bold-specific line-metric effect at the render layer.
///
/// The fix applies this margin, split evenly top and bottom, to every block
/// page: stacking block A (bottom margin `INTER_BLOCK_MARGIN_EM`) directly
/// against block B (top margin `INTER_BLOCK_MARGIN_EM`) at the viewer's
/// existing zero gap yields a combined `2 * INTER_BLOCK_MARGIN_EM =
/// PAR_LEADING_EM` gap between their ink — the same line-box-to-line-box
/// gap Typst's own `par.leading` inserts between two lines in one block.
///
/// The SAME value is also applied left and right (pane-edge margins,
/// queued separately): "half a line-gap" reads naturally as a horizontal
/// rhythm too, and reusing this constant rather than inventing a second one
/// means both axes retune together if `TARGET_LINE_ADVANCE_EM`/the edges
/// ever change. For a prose block, whose PNG then gets right-trimmed to its
/// actual ink (`prose.rs::trim_transparent_right`), the trim boundary is
/// deliberately stopped `INTER_BLOCK_MARGIN_EM * font_size_pt` short of the
/// content edge instead of at the bare content edge, so the intended right
/// margin survives trimming instead of being cropped away as "just more
/// transparent space" (see `trim_transparent_right`'s doc comment). The
/// left margin needs no such adjustment: nothing trims the left edge.
///
/// Derived from the existing line-metric constants above, not a new magic
/// number, so a future retune of `TARGET_LINE_ADVANCE_EM`/edges
/// automatically keeps inter-block, intra-block, AND pane-edge rhythm
/// consistent.
pub(crate) const INTER_BLOCK_MARGIN_EM: f64 = PAR_LEADING_EM / 2.0;

/// A complete, self-contained Typst source document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypstSource {
    pub source: String,
    pub(crate) static_files: Vec<(String, Vec<u8>)>,
    pub(crate) formula_errors: Vec<SafeErrorRecord>,
}

impl TypstSource {
    pub fn as_str(&self) -> &str {
        &self.source
    }

    pub fn into_string(self) -> String {
        self.source
    }
}

/// Composes one Markdown block into a self-contained Typst document.
pub fn compose_block(block: &Block, options: &RenderOptions) -> Result<TypstSource, RenderError> {
    let _guard = render_guard()?;
    let limits = Limits::default();
    let deadline = RenderDeadline::new(limits.render_duration_ms);
    compose_block_with_deadline(block, options, &limits, &deadline)
}

pub(crate) fn compose_block_with_deadline(
    block: &Block,
    options: &RenderOptions,
    limits: &Limits,
    deadline: &RenderDeadline,
) -> Result<TypstSource, RenderError> {
    limits.check_source_bytes_per_block(block.source.len() as u64)?;
    validate_options(options)?;
    // AGENTS.md's Product Boundaries requires `$...$`/`$$...$$` to be parsed
    // FIRST, before Markdown — scanned here, over the whole block source, so
    // every formula's span is known before pulldown-cmark ever sees the
    // text. This is not optional plumbing: pulldown-cmark's inline parser
    // splits a block's text into multiple `Event::Text`/`Event::InlineHtml`
    // pieces at `_`, `*`, and `<` (emphasis/HTML-tag candidates), so a
    // formula spanning one of those characters — `$Z_{in}$`, `$a*b$`,
    // `$0 \le r < \infty$` — would land in more than one `Node::Text` and
    // never look contiguous to a per-node scan. `protect_formula_spans`
    // below replaces each formula's exact source bytes with an opaque
    // placeholder token before handing the text to pulldown-cmark, so no
    // Markdown-significant character from inside a formula ever reaches the
    // inline parser; `push_text_with_math` restores the real formula from
    // `context.formulas` by index wherever a placeholder survives into a
    // `Node::Text`.
    let formulas = if supports_math_embedding(block.kind) {
        scan_latex(&block.source, &ScannerLimits::default())?
    } else {
        Vec::new()
    };
    let protected_source = protect_formula_spans(&block.source, &formulas);
    deadline.checkpoint()?;

    let mut events = Parser::new_ext(&protected_source, Options::ENABLE_TABLES).peekable();
    let nodes = parse_nodes(&mut events);
    let mut body = String::new();
    let mut context = MathContext::new(block.index, options, limits, deadline, formulas);
    render_nodes(&nodes, &mut body, &mut context)?;
    if body.is_empty() {
        body.push_str("#text(\"\")");
    }

    // `#set page(margin: ...)` is evaluated before `#set text(size: ...)` in
    // the source below, so an em-unit margin there would resolve against
    // Typst's default text size, not `options.font_size_pt`. Compute the
    // margin as an absolute pt value in Rust instead, exactly like every
    // other size in this module (`image.width_pt`, etc.), so it always
    // tracks the block's actual font size unambiguously. The SAME value is
    // used on all four sides (see `INTER_BLOCK_MARGIN_EM`'s doc comment for
    // why horizontal reuses the vertical constant rather than a new one).
    let block_margin_pt = INTER_BLOCK_MARGIN_EM * options.font_size_pt;

    Ok(TypstSource {
        source: format!(
            "#set page(width: {width}pt, height: auto, margin: {block_margin}pt, fill: none)\n\
             #set text(font: {fonts}, size: {font_size}pt, \
             fill: rgb(\"{color}\"), top-edge: {top_edge}em, bottom-edge: {bottom_edge}em)\n\
             #set par(leading: {leading}em)\n\
             #show raw: set text(size: {raw_size_em}em)\n\
             {body}\n",
            width = options.content_width_pt,
            block_margin = block_margin_pt,
            fonts = font_fallback_list(options.cjk_font),
            font_size = options.font_size_pt,
            color = DARK_THEME_TEXT_COLOR,
            top_edge = TEXT_TOP_EDGE_EM,
            bottom_edge = TEXT_BOTTOM_EDGE_EM,
            leading = PAR_LEADING_EM,
            raw_size_em = RAW_TEXT_SIZE_EM,
        ),
        static_files: context.static_files,
        formula_errors: context.formula_errors,
    })
}

fn supports_math_embedding(kind: BlockKind) -> bool {
    matches!(
        kind,
        BlockKind::Paragraph
            | BlockKind::Heading
            | BlockKind::List
            | BlockKind::Quote
            | BlockKind::Table
    )
}

/// Brackets a formula placeholder token: two Unicode Private Use Area
/// codepoints that never occur in ordinary Markdown/LaTeX source, so a
/// placeholder can never collide with real text and is never itself
/// Markdown-significant (no `_`, `*`, `<`, backtick, etc. inside it) —
/// pulldown-cmark passes it through as an ordinary character in a `Text`
/// event, never splitting on it.
const FORMULA_PLACEHOLDER_START: char = '\u{E000}';
const FORMULA_PLACEHOLDER_END: char = '\u{E001}';

/// Replaces each formula's exact source span (delimiters included) with an
/// opaque placeholder token encoding its index into `formulas` — see the
/// doc comment on the `protect_formula_spans` call site in
/// `compose_block_with_deadline` for why this must happen before
/// pulldown-cmark ever sees the text. Byte-range replacement, driven
/// entirely by `Formula::start`/`end` (already validated UTF-8-safe byte
/// offsets from `scan_latex`), so this never needs to re-parse LaTeX or
/// second-guess the scanner's delimiter matching.
fn protect_formula_spans(source: &str, formulas: &[Formula]) -> String {
    if formulas.is_empty() {
        return source.to_string();
    }
    let mut protected = String::with_capacity(source.len());
    let mut cursor = 0;
    for (index, formula) in formulas.iter().enumerate() {
        protected.push_str(&source[cursor..formula.start]);
        protected.push(FORMULA_PLACEHOLDER_START);
        protected.push_str(&index.to_string());
        protected.push(FORMULA_PLACEHOLDER_END);
        cursor = formula.end;
    }
    protected.push_str(&source[cursor..]);
    protected
}

/// Scans `text` for `protect_formula_spans`' placeholder tokens, splitting
/// it into an ordered sequence of literal text runs and formula indices.
/// Any `FORMULA_PLACEHOLDER_START` not immediately followed by digits and a
/// matching `FORMULA_PLACEHOLDER_END` is treated as literal text (fail
/// closed — a placeholder can only ever come from this module's own
/// `protect_formula_spans`, but a malformed one must never panic or drop
/// content).
fn split_formula_placeholders(text: &str) -> Vec<TextSegment<'_>> {
    let mut segments = Vec::new();
    let mut literal_start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((byte_index, character)) = chars.next() {
        if character != FORMULA_PLACEHOLDER_START {
            continue;
        }
        let digits_start = byte_index + character.len_utf8();
        let mut digits_end = digits_start;
        while let Some(&(_, next_char)) = chars.peek() {
            if next_char.is_ascii_digit() {
                digits_end += next_char.len_utf8();
                chars.next();
            } else {
                break;
            }
        }
        let Some(&(end_index, FORMULA_PLACEHOLDER_END)) = chars.peek() else {
            continue;
        };
        if digits_end == digits_start {
            continue;
        }
        let Ok(formula_index) = text[digits_start..digits_end].parse::<usize>() else {
            continue;
        };
        chars.next();
        let placeholder_end = end_index + FORMULA_PLACEHOLDER_END.len_utf8();
        if literal_start < byte_index {
            segments.push(TextSegment::Literal(&text[literal_start..byte_index]));
        }
        segments.push(TextSegment::Formula(formula_index));
        literal_start = placeholder_end;
    }
    if literal_start < text.len() {
        segments.push(TextSegment::Literal(&text[literal_start..]));
    }
    segments
}

enum TextSegment<'a> {
    Literal(&'a str),
    Formula(usize),
}

fn validate_options(options: &RenderOptions) -> Result<(), RenderError> {
    if options.content_width_pt.is_finite()
        && options.content_width_pt > 0.0
        && options.font_size_pt.is_finite()
        && options.font_size_pt > 0.0
    {
        Ok(())
    } else {
        Err(RenderError::new(
            SafeErrorRecord {
                code: ErrorCode::InternalError,
                retryable: false,
                details: None,
            },
            "invalid render options",
        ))
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Node {
    Text(String),
    SoftBreak,
    HardBreak,
    Paragraph(Vec<Node>),
    Heading(u8, Vec<Node>),
    Strong(Vec<Node>),
    Emphasis(Vec<Node>),
    InlineCode(String),
    List {
        start: Option<u64>,
        items: Vec<Vec<Node>>,
    },
    ListItem(Vec<Node>),
    Quote(Vec<Node>),
    Table {
        alignments: Vec<Alignment>,
        sections: Vec<Node>,
    },
    TableHead(Vec<Node>),
    TableRow(Vec<Node>),
    TableCell(Vec<Node>),
    CodeBlock {
        language: Option<String>,
        text: String,
    },
    Group(Vec<Node>),
    Rule,
}

fn parse_nodes<'a, I>(events: &mut Peekable<I>) -> Vec<Node>
where
    I: Iterator<Item = Event<'a>>,
{
    let mut nodes = Vec::new();
    while let Some(event) = events.next() {
        match event {
            Event::Start(tag) => nodes.push(parse_tag(tag, events)),
            Event::End(_) => break,
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                nodes.push(Node::Text(text.into_string()));
            }
            Event::Code(text) => nodes.push(Node::InlineCode(text.into_string())),
            Event::SoftBreak => nodes.push(Node::SoftBreak),
            Event::HardBreak => nodes.push(Node::HardBreak),
            Event::Rule => nodes.push(Node::Rule),
            Event::TaskListMarker(checked) => {
                nodes.push(Node::Text(if checked { "☑ " } else { "☐ " }.to_owned()));
            }
            Event::FootnoteReference(label) => {
                nodes.push(Node::Text(label.into_string()));
            }
            Event::InlineMath(text) | Event::DisplayMath(text) => {
                nodes.push(Node::Text(text.into_string()));
            }
        }
    }
    nodes
}

fn parse_tag<'a, I>(tag: Tag<'a>, events: &mut Peekable<I>) -> Node
where
    I: Iterator<Item = Event<'a>>,
{
    match tag {
        Tag::Paragraph => Node::Paragraph(parse_nodes(events)),
        Tag::Heading { level, .. } => Node::Heading(heading_level(level), parse_nodes(events)),
        Tag::BlockQuote(_) => Node::Quote(parse_nodes(events)),
        Tag::CodeBlock(kind) => {
            let language = match kind {
                CodeBlockKind::Fenced(info) => allowed_language(&info),
                CodeBlockKind::Indented => None,
            };
            Node::CodeBlock {
                language,
                text: collect_code_text(events),
            }
        }
        Tag::List(start) => {
            let children = parse_nodes(events);
            let items = children
                .into_iter()
                .filter_map(|node| match node {
                    Node::ListItem(children) => Some(children),
                    _ => None,
                })
                .collect();
            Node::List { start, items }
        }
        Tag::Item => Node::ListItem(parse_nodes(events)),
        Tag::Table(alignments) => Node::Table {
            alignments,
            sections: parse_nodes(events),
        },
        Tag::TableHead => Node::TableHead(parse_nodes(events)),
        Tag::TableRow => Node::TableRow(parse_nodes(events)),
        Tag::TableCell => Node::TableCell(parse_nodes(events)),
        Tag::Emphasis => Node::Emphasis(parse_nodes(events)),
        Tag::Strong => Node::Strong(parse_nodes(events)),
        // Link destinations and image sources are deliberately discarded.
        Tag::Link { .. } | Tag::Image { .. } => Node::Group(parse_nodes(events)),
        _ => Node::Group(parse_nodes(events)),
    }
}

fn collect_code_text<'a, I>(events: &mut Peekable<I>) -> String
where
    I: Iterator<Item = Event<'a>>,
{
    let mut text = String::new();
    for event in events.by_ref() {
        match event {
            Event::End(_) => break,
            Event::Text(value)
            | Event::Code(value)
            | Event::Html(value)
            | Event::InlineHtml(value) => text.push_str(&value),
            Event::SoftBreak | Event::HardBreak => text.push('\n'),
            Event::TaskListMarker(checked) => {
                text.push_str(if checked { "☑ " } else { "☐ " });
            }
            Event::FootnoteReference(value)
            | Event::InlineMath(value)
            | Event::DisplayMath(value) => text.push_str(&value),
            Event::Start(_) | Event::Rule => {}
        }
    }
    text
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

struct MathContext<'a> {
    block_index: usize,
    /// Every formula `scan_latex` found in the whole block source, indexed
    /// by the placeholder tokens `protect_formula_spans` wrote in their
    /// place. `push_text_with_math` looks a formula up by this index rather
    /// than re-scanning `text` — the whole point of the placeholder
    /// protocol is that `text` (one `Node::Text`'s contents) may no longer
    /// contain a formula's Markdown-significant characters contiguously,
    /// so re-scanning it here would reproduce the original bug.
    formulas: Vec<Formula>,
    /// Tracks which `formulas` indices have already been rendered, so a
    /// placeholder that somehow appears twice (never expected in practice,
    /// but not provably impossible if a future Markdown construct clones
    /// text) renders its image at most once rather than re-running
    /// `render_formula_with_deadline` — that call is not free, and
    /// `static_files` entries are keyed by `(block_index, rendered
    /// count)`, not by formula index, so a duplicate render would silently
    /// double-count against the render-guard/deadline budget for no
    /// visible benefit.
    rendered: Vec<bool>,
    options: &'a RenderOptions,
    limits: &'a Limits,
    deadline: &'a RenderDeadline,
    static_files: Vec<(String, Vec<u8>)>,
    formula_errors: Vec<SafeErrorRecord>,
}

impl<'a> MathContext<'a> {
    fn new(
        block_index: usize,
        options: &'a RenderOptions,
        limits: &'a Limits,
        deadline: &'a RenderDeadline,
        formulas: Vec<Formula>,
    ) -> Self {
        let rendered = vec![false; formulas.len()];
        Self {
            block_index,
            formulas,
            rendered,
            options,
            limits,
            deadline,
            static_files: Vec::new(),
            formula_errors: Vec::new(),
        }
    }

    /// Renders `text` (one `Node::Text`'s contents) into the composed Typst
    /// body: literal runs go through the usual escaped `#text(...)` call,
    /// and each formula-placeholder token is resolved by index against
    /// `self.formulas` and rendered as an image (or an `[invalid latex]`
    /// badge, matching the pre-fix per-formula error contract). A
    /// placeholder index out of range or already consumed is treated as
    /// literal text — defensive only; `protect_formula_spans` never
    /// produces such a token, but this keeps the function total rather
    /// than panicking on a state it should never reach.
    fn push_text_with_math(&mut self, text: &str, output: &mut String) -> Result<(), RenderError> {
        for segment in split_formula_placeholders(text) {
            match segment {
                TextSegment::Literal(literal) => push_text_call(output, literal),
                TextSegment::Formula(formula_index) => {
                    let already_rendered = self.rendered.get(formula_index).copied();
                    let Some(formula) = (already_rendered == Some(false))
                        .then(|| self.formulas.get(formula_index))
                        .flatten()
                        .cloned()
                    else {
                        // Out of range or already consumed: fall back to
                        // literal text rather than silently dropping it.
                        push_text_call(
                            output,
                            &format!(
                                "{FORMULA_PLACEHOLDER_START}{formula_index}{FORMULA_PLACEHOLDER_END}"
                            ),
                        );
                        continue;
                    };
                    self.rendered[formula_index] = true;
                    let rendered_count = self.static_files.len();
                    match render_formula_with_deadline(
                        &formula.latex,
                        formula.display,
                        self.options,
                        self.limits,
                        self.deadline,
                    ) {
                        Ok(image) => {
                            let name = format!("math-{}-{rendered_count}.svg", self.block_index);
                            push_math_image(output, &name, &image, formula.display);
                            self.static_files.push((name, image.svg));
                        }
                        Err(error) if error.safe_record().code == ErrorCode::InvalidLatex => {
                            self.formula_errors.push(error.into_safe_record());
                            push_raw_call(output, "[invalid latex]", false, None);
                        }
                        Err(error) => return Err(error),
                    }
                }
            }
        }
        Ok(())
    }
}

fn render_nodes(
    nodes: &[Node],
    output: &mut String,
    context: &mut MathContext<'_>,
) -> Result<(), RenderError> {
    for node in nodes {
        render_node(node, output, context)?;
    }
    Ok(())
}

fn render_node(
    node: &Node,
    output: &mut String,
    context: &mut MathContext<'_>,
) -> Result<(), RenderError> {
    match node {
        Node::Text(text) => context.push_text_with_math(text, output)?,
        Node::SoftBreak => push_text_call(output, " "),
        Node::HardBreak => output.push_str("#linebreak()"),
        Node::Paragraph(children) => {
            render_nodes(children, output, context)?;
            output.push_str("#parbreak()");
        }
        Node::Heading(level, children) => {
            let size = match level {
                1 => 1.5,
                2 => 1.3,
                3 => 1.15,
                _ => 1.0,
            };
            output.push_str("#block(width: 100%)[#text(size: ");
            output.push_str(&size.to_string());
            output.push_str("em, weight: \"bold\")[");
            render_nodes(children, output, context)?;
            output.push_str("]]");
        }
        Node::Strong(children) => {
            output.push_str("#strong[");
            render_nodes(children, output, context)?;
            output.push(']');
        }
        Node::Emphasis(children) => {
            output.push_str("#emph[");
            render_nodes(children, output, context)?;
            output.push(']');
        }
        Node::InlineCode(text) => push_raw_call(output, text, false, None),
        Node::List { start, items } => {
            if let Some(start) = start {
                output.push_str("#enum(start: ");
                output.push_str(&start.to_string());
                output.push(',');
            } else {
                output.push_str("#list(");
            }
            for item in items {
                output.push('[');
                render_nodes(item, output, context)?;
                output.push_str("],");
            }
            output.push(')');
        }
        Node::ListItem(children) => render_nodes(children, output, context)?,
        Node::Quote(children) => {
            output.push_str("#quote(block: true)[");
            render_nodes(children, output, context)?;
            output.push(']');
        }
        Node::Table {
            alignments,
            sections,
        } => render_table(alignments, sections, output, context)?,
        Node::TableHead(children) | Node::TableRow(children) | Node::TableCell(children) => {
            render_nodes(children, output, context)?;
        }
        Node::CodeBlock { language, text } => {
            push_raw_call(output, text, true, language.as_deref());
        }
        Node::Group(children) => render_nodes(children, output, context)?,
        Node::Rule => output.push_str("#block(width: 100%, height: 1pt, fill: rgb(\"#e6edf3\"))"),
    }
    Ok(())
}

fn render_table(
    alignments: &[Alignment],
    sections: &[Node],
    output: &mut String,
    context: &mut MathContext<'_>,
) -> Result<(), RenderError> {
    let mut header_cells = Vec::new();
    let mut body_rows = Vec::new();
    for section in sections {
        match section {
            Node::TableHead(children) => {
                header_cells = table_cells(children);
            }
            Node::TableRow(children) => body_rows.push(table_cells(children)),
            _ => {}
        }
    }
    let columns = alignments
        .len()
        .max(header_cells.len())
        .max(body_rows.iter().map(Vec::len).max().unwrap_or(0))
        .max(1);

    output.push_str("#table(columns: ");
    output.push_str(&columns.to_string());
    // Explicit stroke + no fill (AT-1-101 dark-theme table legibility): the
    // rendered page has a transparent background (`#set page(fill: none)`),
    // so any cell fill here would show the terminal's own background
    // through it inconsistently rather than blending — cell shading is out
    // of scope, only the border needs to be visible. Typst's own default
    // stroke is `1pt + black`, invisible on a dark terminal background;
    // `TABLE_STROKE_COLOR` is a mid-luminance gray chosen to read clearly
    // there (see its doc comment for the derivation).
    output.push_str(", stroke: 1pt + rgb(\"");
    output.push_str(TABLE_STROKE_COLOR);
    output.push_str("\"), fill: none");
    if !alignments.is_empty() {
        output.push_str(", align: (");
        for alignment in alignments {
            output.push_str(match alignment {
                Alignment::None | Alignment::Left => "left,",
                Alignment::Center => "center,",
                Alignment::Right => "right,",
            });
        }
        output.push(')');
    }
    output.push(',');
    if !header_cells.is_empty() {
        output.push_str("table.header(");
        render_cells(&header_cells, output, context)?;
        output.push_str("),");
    }
    for row in body_rows {
        render_cells(&row, output, context)?;
    }
    output.push(')');
    Ok(())
}

fn table_cells(nodes: &[Node]) -> Vec<Vec<Node>> {
    nodes
        .iter()
        .filter_map(|node| match node {
            Node::TableCell(children) => Some(children.clone()),
            _ => None,
        })
        .collect()
}

fn render_cells(
    cells: &[Vec<Node>],
    output: &mut String,
    context: &mut MathContext<'_>,
) -> Result<(), RenderError> {
    for cell in cells {
        output.push('[');
        render_nodes(cell, output, context)?;
        output.push_str("],");
    }
    Ok(())
}

/// Embeds one RaTeX formula into the Typst source as an `#image(...)` box at
/// its exact logical baseline metrics. `name` carries a `.svg` extension
/// (the [`MathImage::svg`] static file registered alongside this call);
/// Typst rasterizes the SVG's glyph outlines directly into the composed
/// page at final resolution — the same one-shot path prose text takes — so
/// math no longer gets a second, blur-inducing resample the way the old
/// pre-rasterized PNG embedding did (see the `MathImage` doc comment in
/// `math.rs`). The box/baseline math below is otherwise unchanged from the
/// PNG embedding this replaces.
fn push_math_image(output: &mut String, name: &str, image: &MathImage, display: bool) {
    let total_height = image.height_pt + image.depth_pt;
    if display {
        output.push_str("#block(width: 100%, align(center)[#image(\"");
        escape_typst_string_into(name, output);
        output.push_str("\", width: ");
        output.push_str(&image.width_pt.to_string());
        output.push_str("pt, height: ");
        output.push_str(&total_height.to_string());
        output.push_str("pt, fit: \"stretch\")])");
    } else {
        output.push_str("#box(width: ");
        output.push_str(&image.width_pt.to_string());
        output.push_str("pt, height: ");
        output.push_str(&total_height.to_string());
        output.push_str("pt, baseline: ");
        output.push_str(&image.depth_pt.to_string());
        output.push_str("pt, image(\"");
        escape_typst_string_into(name, output);
        output.push_str("\", width: ");
        output.push_str(&image.width_pt.to_string());
        output.push_str("pt, height: ");
        output.push_str(&total_height.to_string());
        output.push_str("pt, fit: \"stretch\"))");
    }
}

fn push_text_call(output: &mut String, text: &str) {
    output.push_str("#text(\"");
    escape_typst_string_into(text, output);
    output.push_str("\")");
}

fn push_raw_call(output: &mut String, text: &str, block: bool, language: Option<&str>) {
    output.push_str("#raw(\"");
    escape_typst_string_into(text, output);
    output.push_str("\", block: ");
    output.push_str(if block { "true" } else { "false" });
    if let Some(language) = language {
        output.push_str(", lang: \"");
        output.push_str(language);
        output.push('"');
    }
    output.push(')');
}

fn escape_typst_string_into(value: &str, output: &mut String) {
    for character in value.chars() {
        match character {
            '\\' => output.push_str("\\\\"),
            '"' => output.push_str("\\\""),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                output.push_str("\\u{");
                output.push_str(&format!("{:x}", character as u32));
                output.push('}');
            }
            character => output.push(character),
        }
    }
}

fn allowed_language(info: &CowStr<'_>) -> Option<String> {
    let candidate = info.split_whitespace().next()?.to_ascii_lowercase();
    if candidate.is_empty()
        || !candidate
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "+#-".contains(character))
    {
        return None;
    }

    const LANGUAGES: &[&str] = &[
        "bash",
        "c",
        "c#",
        "c++",
        "clojure",
        "cpp",
        "csharp",
        "css",
        "diff",
        "dockerfile",
        "elixir",
        "erlang",
        "go",
        "haskell",
        "html",
        "java",
        "javascript",
        "js",
        "json",
        "jsx",
        "kotlin",
        "latex",
        "lua",
        "makefile",
        "markdown",
        "md",
        "objective-c",
        "perl",
        "php",
        "plaintext",
        "python",
        "r",
        "ruby",
        "rust",
        "scala",
        "scss",
        "shell",
        "sh",
        "sql",
        "swift",
        "toml",
        "ts",
        "tsx",
        "typescript",
        "xml",
        "yaml",
        "yml",
        "zig",
    ];
    LANGUAGES.contains(&candidate.as_str()).then_some(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BlockKind;

    fn block(source: &str) -> Block {
        Block {
            index: 0,
            kind: BlockKind::Paragraph,
            source: source.to_owned(),
        }
    }

    #[test]
    fn typst_string_escaper_covers_syntax_and_control_characters() {
        let original =
            "\" slash: \\\\ braces: {} # $ newline:\n tab:\t return:\r bell:\u{7} CJK: 数学 emoji: 🧮";
        let mut escaped = String::new();
        escape_typst_string_into(original, &mut escaped);
        assert_eq!(decode_test_escape(&escaped), original);

        let source = compose_block(&block(original), &RenderOptions::default()).unwrap();
        assert!(source.source.contains("\\\""));
        assert!(source.source.contains("\\\\"));
        assert!(source.source.contains("\\u{7}"));
        assert!(source.source.contains("数学"));
        assert!(source.source.contains('🧮'));
    }

    fn decode_test_escape(value: &str) -> String {
        let mut output = String::new();
        let mut characters = value.chars().peekable();
        while let Some(character) = characters.next() {
            if character != '\\' {
                output.push(character);
                continue;
            }
            match characters.next().unwrap() {
                '\\' => output.push('\\'),
                '"' => output.push('"'),
                'n' => output.push('\n'),
                'r' => output.push('\r'),
                't' => output.push('\t'),
                'u' => {
                    assert_eq!(characters.next(), Some('{'));
                    let mut digits = String::new();
                    for digit in characters.by_ref() {
                        if digit == '}' {
                            break;
                        }
                        digits.push(digit);
                    }
                    let codepoint = u32::from_str_radix(&digits, 16).unwrap();
                    output.push(char::from_u32(codepoint).unwrap());
                }
                unexpected => panic!("unexpected escape: {unexpected}"),
            }
        }
        output
    }

    #[test]
    fn link_and_image_destinations_are_absent_from_typst_source() {
        let source = compose_block(
            &block("[visible](https://example.com/secret) ![alt](file:///private/path)"),
            &RenderOptions::default(),
        )
        .unwrap();
        assert!(source.source.contains("visible"));
        assert!(source.source.contains("alt"));
        assert!(!source.source.contains("example.com"));
        assert!(!source.source.contains("/private/path"));
    }

    #[test]
    fn known_fence_language_enables_typst_raw_highlighting() {
        let source = compose_block(
            &Block {
                index: 0,
                kind: BlockKind::CodeBlock,
                source: "```Rust\nfn main() {}\n```".to_owned(),
            },
            &RenderOptions::default(),
        )
        .unwrap();
        assert!(source.source.contains("lang: \"rust\""));
    }

    #[test]
    fn rejects_a_block_over_the_source_limit() {
        let error = compose_block(
            &block(&"x".repeat(64 * 1024 + 1)),
            &RenderOptions::default(),
        )
        .unwrap_err();
        assert_eq!(
            error.safe_record().code,
            crate::ErrorCode::RendererInputLimit
        );
        assert_eq!(
            error.safe_record().details.as_ref().unwrap().limit_kind,
            Some(crate::SafeLimitKind::ResponseDocumentBytes)
        );
    }

    // --- D-CONFIG phase 2: cjk_font selection ---

    #[test]
    fn font_fallback_list_always_starts_with_the_primary_latin_font() {
        let list = font_fallback_list(crate::CjkFont::MPlus2);
        assert_eq!(list, "(\"NewCM10\", \"M PLUS 2\")");
    }

    /// The resolved fallback list must carry the SELECTED family's exact
    /// Typst name, not a hard-coded one — this is the render-level proof
    /// `RenderOptions::cjk_font` actually reaches the composed Typst
    /// source's `#set text(font: ...)` rule (the only place it can matter).
    #[test]
    fn composed_source_carries_the_selected_cjk_font_family_name() {
        let options = RenderOptions::default().with_cjk_font(crate::CjkFont::MPlus2);
        let source = compose_block(&block("hello"), &options).unwrap();
        assert!(
            source
                .source
                .contains("#set text(font: (\"NewCM10\", \"M PLUS 2\")"),
            "{}",
            source.source
        );
    }

    // --- Scanner recognition gaps (D-SCAN): formula spans crossing a
    // Markdown-significant character must still be recognized as one
    // formula, not split into literal text by pulldown-cmark's inline
    // parser. See the doc comment on `protect_formula_spans`'s call site in
    // `compose_block_with_deadline` for the mechanism.

    /// Whether `source` (one whole paragraph) was recognized as containing
    /// a `$...$` formula span by `scan_latex`, whether or not that LaTeX
    /// was itself valid — a recognized-but-invalid formula still renders
    /// an `[invalid latex]` badge (`formula_errors` non-empty) rather than
    /// falling all the way through to literal text, which is what
    /// distinguishes "the scanner didn't see this as math at all" (the bug
    /// this fix addresses) from "the scanner saw it but the LaTeX itself
    /// was bad" (a separate, expected outcome — see
    /// `prose.rs::one_invalid_formula_becomes_a_badge_without_harming_siblings`).
    fn recognized_as_math(source: &str) -> bool {
        let composed = compose_block(&block(source), &RenderOptions::default()).unwrap();
        composed.source.contains("image(\"math-") || !composed.formula_errors.is_empty()
    }

    #[test]
    fn formulas_crossing_underscore_asterisk_or_angle_bracket_are_recognized() {
        // Each of these previously split across multiple pulldown-cmark
        // Text/InlineHtml events and fell through to literal text.
        let cases = [
            r"$Z_{in}$",                      // braced subscript
            r"$V_{BE}$",                      // braced subscript, EE notation
            r"$r_\pi$",                       // subscript then a command
            r"$\{x\}$",                       // escaped brace
            r"$V_T \approx 26\,\mathrm{mV}$", // thin space
            r"$a*b$",                         // bare asterisk
            r"$0 \le r < \infty$",            // bare less-than
            r"$a<b>c$",                       // less-than mixed with greater-than
        ];
        for case in cases {
            assert!(
                recognized_as_math(case),
                "expected math recognition: {case}"
            );
        }
    }

    #[test]
    fn previously_working_recognition_cases_do_not_regress() {
        let cases = [
            r"$x_i$",
            r"$x^2$",
            r"$x^{2}$",
            r"$|x|$",
            r"$a>b$", // bare greater-than alone was already fine
            r"$a \le b \times c$",
            r"$a \badcmd b$", // an unknown command is still a formula span
        ];
        for case in cases {
            assert!(
                recognized_as_math(case),
                "expected math recognition: {case}"
            );
        }
    }

    #[test]
    fn currency_and_js_template_text_still_reads_as_literal() {
        // The desired rejections: these must never be mistaken for math,
        // with or without the placeholder protocol in the way. The
        // Japanese-currency phrasing is the exact live-corpus shape (a
        // dollar sign immediately followed by a number and full-width
        // Japanese text, not a plausible LaTeX body) — the scanner's own
        // heuristics decide these are not formulas; this test only proves
        // the placeholder-protocol change did not alter that outcome.
        let cases = [
            "$10/月、Business $20/月",
            "js template ${x}",
            "price list: $5, $10, $15",
        ];
        for case in cases {
            assert!(
                !recognized_as_math(case),
                "expected literal text, not math: {case}"
            );
        }
    }

    #[test]
    fn a_paragraph_mixing_recognized_math_rejected_currency_and_multibyte_context_composes_correctly(
    ) {
        // Mirrors the live failure shape: Japanese prose (with full-width
        // punctuation) containing both a formula that used to fail to
        // scan and a currency-shaped span that must keep failing to scan,
        // in the same paragraph.
        let source = "抵抗値は$Z_{in}$で、価格は$10/月です（$20/月ではありません）。$a_i$も使う。";
        let composed = compose_block(&block(source), &RenderOptions::default()).unwrap();
        // Two real formulas ($Z_{in}$ and $a_i$) -> two rendered images.
        let image_count = composed.source.matches("image(\"math-").count();
        assert_eq!(
            image_count, 2,
            "expected exactly the two real formulas to render: {}",
            composed.source
        );
        // The currency text must survive as literal, escaped text — not
        // consumed as (part of) a formula and not dropped.
        assert!(composed.source.contains("価格"));
        assert!(composed.source.contains("月です"));
    }

    #[test]
    fn greater_than_ampersand_and_html_looking_text_inside_math_render_as_math() {
        // `>`, `&`, and HTML-tag-shaped text INSIDE a formula span must all
        // resolve as part of the one recognized formula, not be peeled off
        // by the Markdown inline parser the way `<` alone used to be.
        let cases = [r"$a > b \& c$", r"$a < b$", r"$\langle x \rangle$"];
        for case in cases {
            assert!(
                recognized_as_math(case),
                "expected math recognition: {case}"
            );
        }
    }

    #[test]
    fn a_literal_less_than_outside_math_stays_inert_text_never_html() {
        // AGENTS.md: the renderer never executes/interprets raw HTML. A
        // bare `<` outside any `$...$` span must render as literal escaped
        // text, not become (or be swallowed as) HTML — this is the
        // "generic tags stay literal" half of AT-3-701's injection
        // contract, exercised here specifically against the placeholder
        // protocol added by this fix (a stray, unprotected `<` in the
        // non-formula part of the source must still go through
        // `push_text_call`, not leak past it).
        //
        // pulldown-cmark still splits this into several `Text` events
        // around the bare `<` — that is pre-existing, unrelated to this
        // fix (only formula spans need protecting), and harmless: each
        // piece still comes out through the same escaped `#text(...)`
        // call, so Typst concatenates them back into one line visually.
        // The assertion below checks each literal piece is present and
        // properly escaped as text, not that they are joined into one
        // Rust string.
        let source = "1 < 2 and no math here";
        let composed = compose_block(&block(source), &RenderOptions::default()).unwrap();
        assert!(!composed.source.contains("image(\"math-"));
        assert!(!composed.source.contains("#eval"));
        assert!(composed.source.contains("#text(\"1 \")"));
        assert!(composed.source.contains("#text(\"<\")"));
        assert!(composed.source.contains("#text(\" 2 and no math here\")"));
        // Never a bare, unescaped HTML-tag-shaped Typst call.
        assert!(!composed.source.contains("<2"));
    }

    #[test]
    fn a_malformed_placeholder_like_string_in_ordinary_text_is_not_mistaken_for_a_formula() {
        // Defense-in-depth: `split_formula_placeholders` must never treat
        // stray Private Use Area characters typed by a user as a formula
        // reference. This can only arise if a user's own text happens to
        // contain the placeholder codepoints; confirms the fail-closed
        // "keep as literal" behavior documented on
        // `split_formula_placeholders`.
        let source = "weird chars: \u{E000}not-a-formula\u{E001} end";
        let composed = compose_block(&block(source), &RenderOptions::default()).unwrap();
        assert!(!composed.source.contains("image(\"math-"));
    }

    // --- Table stroke visibility on dark backgrounds (task #23) ---

    fn table_block(source: &str) -> Block {
        Block {
            index: 0,
            kind: BlockKind::Table,
            source: source.to_owned(),
        }
    }

    /// Source-level check: the composed Typst source for a table must
    /// carry an explicit visible stroke and no cell fill — Typst's own
    /// default stroke (`1pt + black`) is invisible on a dark terminal
    /// background, which is the reported defect.
    #[test]
    fn table_emission_carries_an_explicit_visible_stroke_and_no_fill() {
        let source = "| A | B |\n|---|---|\n| 1 | 2 |\n";
        let composed = compose_block(&table_block(source), &RenderOptions::default()).unwrap();
        assert!(
            composed
                .source
                .contains(&format!("stroke: 1pt + rgb(\"{TABLE_STROKE_COLOR}\")")),
            "table source must set an explicit non-default stroke: {}",
            composed.source
        );
        assert!(
            composed.source.contains("fill: none"),
            "table source must not fill cells (transparent PNGs sit on the \
             terminal's own background): {}",
            composed.source
        );
        // The stroke color must not be Typst's invisible-on-dark default.
        assert_ne!(TABLE_STROKE_COLOR, "#000000");
    }

    /// Pixel-level check: a rendered table must actually contain ink at
    /// (or very near) the chosen stroke color, proving the stroke is not
    /// just present in source but visible in the output raster. Uses a
    /// tolerance band around the exact stroke RGB because Typst's
    /// rasterizer antialiases border pixels, so few pixels (if any) will
    /// be the mathematically exact stroke color — most border pixels blend
    /// with the transparent background at partial alpha.
    #[test]
    fn a_rendered_table_contains_ink_near_the_stroke_color() {
        use std::io::Cursor;

        let source = "| A | B |\n|---|---|\n| 1 | 2 |\n";
        let image =
            crate::prose::render_prose_block(&table_block(source), &RenderOptions::default())
                .unwrap();

        let mut decoder = png::Decoder::new(Cursor::new(&image.png));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
        let mut reader = decoder.read_info().unwrap();
        let mut output = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut output).unwrap();
        let bytes = &output[..info.buffer_size()];
        assert_eq!(info.color_type, png::ColorType::Rgba);

        let stroke_rgb = (0x69u8, 0x6fu8, 0x75u8); // TABLE_STROKE_COLOR
        let tolerance: i32 = 12;
        let near_stroke = bytes.chunks_exact(4).any(|pixel| {
            pixel[3] > 200 // opaque enough that alpha blending with a
                // transparent background hasn't diluted the color much
                && (i32::from(pixel[0]) - i32::from(stroke_rgb.0)).abs() <= tolerance
                && (i32::from(pixel[1]) - i32::from(stroke_rgb.1)).abs() <= tolerance
                && (i32::from(pixel[2]) - i32::from(stroke_rgb.2)).abs() <= tolerance
        });
        assert!(
            near_stroke,
            "rendered table PNG must contain at least one near-opaque pixel \
             close to the stroke color {stroke_rgb:?} (tolerance {tolerance})"
        );
    }

    // --- Raw text size balance (task #24): undo Typst's 0.8em raw default ---

    /// Source-level check: every composed block carries a `show raw` rule
    /// that restores raw text to the surrounding body size — Typst's own
    /// default show rule sets `raw` to 0.8em, which reads visibly smaller
    /// than surrounding prose (measured empirically at the live 15pt/dpr2
    /// geometry, see `prose.rs`'s ink-level raw-size tests).
    #[test]
    fn every_block_carries_a_show_rule_restoring_raw_to_body_size() {
        let composed = compose_block(
            &block("Prose with `inline code` in it."),
            &RenderOptions::default(),
        )
        .unwrap();
        assert!(
            composed
                .source
                .contains(&format!("#show raw: set text(size: {RAW_TEXT_SIZE_EM}em)")),
            "composed source must restore raw's font size: {}",
            composed.source
        );
    }
}
