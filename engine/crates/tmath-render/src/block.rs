/// The semantic kind of a renderable document block.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BlockKind {
    Heading,
    Paragraph,
    List,
    Quote,
    Table,
    CodeBlock,
    DisplayMath,
    ThematicBreak,
}

impl BlockKind {
    pub(crate) const fn hash_tag(self) -> u8 {
        match self {
            Self::Heading => 0,
            Self::Paragraph => 1,
            Self::List => 2,
            Self::Quote => 3,
            Self::Table => 4,
            Self::CodeBlock => 5,
            Self::DisplayMath => 6,
            Self::ThematicBreak => 7,
        }
    }
}

/// A renderable block whose source is the exact slice from the input document.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Block {
    pub index: usize,
    pub kind: BlockKind,
    pub source: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_preserves_exact_source() {
        let block = Block {
            index: 7,
            kind: BlockKind::Paragraph,
            source: "  exact source\n".to_owned(),
        };

        assert_eq!(block.index, 7);
        assert_eq!(block.kind, BlockKind::Paragraph);
        assert_eq!(block.source, "  exact source\n");
    }
}
