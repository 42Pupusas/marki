use crate::Inline;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Section {
    Heading { level: u8, content: Vec<Inline> },
    Paragraph { content: Vec<Inline> },
    CodeBlock { language: Option<String>, code: String },
    UnorderedList { items: Vec<Vec<Inline>> },
    OrderedList { items: Vec<Vec<Inline>> },
    Blockquote { content: Vec<Inline> },
    HorizontalRule,
}
