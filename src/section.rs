use crate::Inline;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Section<'src> {
    Heading {
        level: u8,
        content: Vec<Inline<'src>>,
    },
    Paragraph {
        content: Vec<Inline<'src>>,
    },
    CodeBlock {
        language: Option<&'src str>,
        code: &'src str,
    },
    UnorderedList {
        items: Vec<Vec<Inline<'src>>>,
    },
    OrderedList {
        items: Vec<Vec<Inline<'src>>>,
    },
    Blockquote {
        content: Vec<Inline<'src>>,
    },
    HorizontalRule,
}
