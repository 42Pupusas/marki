use crate::Inline;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum OrderedListDelimiter {
    Dot = b'.',
    Paren = b')',
}

impl PartialEq<u8> for OrderedListDelimiter {
    fn eq(&self, other: &u8) -> bool {
        *self as u8 == *other
    }
}

impl PartialEq<OrderedListDelimiter> for u8 {
    fn eq(&self, other: &OrderedListDelimiter) -> bool {
        *self == *other as Self
    }
}

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
        start: u32,
        delimiter: OrderedListDelimiter,
        items: Vec<Vec<Inline<'src>>>,
    },
    Blockquote {
        content: Vec<Inline<'src>>,
    },
    HorizontalRule,
}
