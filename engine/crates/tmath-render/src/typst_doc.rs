//! Converts parsed Markdown events into Typst source.
//!
//! User-controlled text is emitted only as escaped Typst string literals inside
//! `#text("...")` or `#raw("...")` calls. Structural Typst syntax is selected
//! solely from pulldown-cmark event variants, so Markdown text cannot become
//! Typst markup.

use std::iter::Peekable;

use pulldown_cmark::{Alignment, CodeBlockKind, CowStr, Event, HeadingLevel, Options, Parser, Tag};

use crate::{
    Block, ErrorCode, Limits, RenderError, RenderOptions, SafeErrorRecord, DARK_THEME_TEXT_COLOR,
};

/// A complete, self-contained Typst source document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypstSource {
    pub source: String,
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
    Limits::default().check_source_bytes_per_block(block.source.len() as u64)?;
    validate_options(options)?;

    let mut events = Parser::new_ext(&block.source, Options::ENABLE_TABLES).peekable();
    let nodes = parse_nodes(&mut events);
    let mut body = String::new();
    render_nodes(&nodes, &mut body);
    if body.is_empty() {
        body.push_str("#text(\"\")");
    }

    Ok(TypstSource {
        source: format!(
            "#set page(width: {width}pt, height: auto, margin: 0pt, fill: none)\n\
             #set text(font: \"NewCM10\", size: {font_size}pt, fill: rgb(\"{color}\"), \
             top-edge: \"bounds\", bottom-edge: \"bounds\")\n\
             #set par(leading: 0.35em)\n\
             {body}\n",
            width = options.content_width_pt,
            font_size = options.font_size_pt,
            color = DARK_THEME_TEXT_COLOR,
        ),
    })
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

fn render_nodes(nodes: &[Node], output: &mut String) {
    for node in nodes {
        render_node(node, output);
    }
}

fn render_node(node: &Node, output: &mut String) {
    match node {
        Node::Text(text) => push_text_call(output, text),
        Node::SoftBreak => push_text_call(output, " "),
        Node::HardBreak => output.push_str("#linebreak()"),
        Node::Paragraph(children) => {
            render_nodes(children, output);
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
            render_nodes(children, output);
            output.push_str("]]");
        }
        Node::Strong(children) => {
            output.push_str("#strong[");
            render_nodes(children, output);
            output.push(']');
        }
        Node::Emphasis(children) => {
            output.push_str("#emph[");
            render_nodes(children, output);
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
                render_nodes(item, output);
                output.push_str("],");
            }
            output.push(')');
        }
        Node::ListItem(children) => render_nodes(children, output),
        Node::Quote(children) => {
            output.push_str("#quote(block: true)[");
            render_nodes(children, output);
            output.push(']');
        }
        Node::Table {
            alignments,
            sections,
        } => render_table(alignments, sections, output),
        Node::TableHead(children) | Node::TableRow(children) | Node::TableCell(children) => {
            render_nodes(children, output);
        }
        Node::CodeBlock { language, text } => {
            push_raw_call(output, text, true, language.as_deref());
        }
        Node::Group(children) => render_nodes(children, output),
        Node::Rule => output.push_str("#block(width: 100%, height: 1pt, fill: rgb(\"#e6edf3\"))"),
    }
}

fn render_table(alignments: &[Alignment], sections: &[Node], output: &mut String) {
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
        render_cells(&header_cells, output);
        output.push_str("),");
    }
    for row in body_rows {
        render_cells(&row, output);
    }
    output.push(')');
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

fn render_cells(cells: &[Vec<Node>], output: &mut String) {
    for cell in cells {
        output.push('[');
        render_nodes(cell, output);
        output.push_str("],");
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
}
