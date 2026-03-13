use crate::Inline;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum OrderedListDelimiter {
    Dot = b'.',
    Paren = b')',
}

impl OrderedListDelimiter {
    #[must_use]
    pub const fn byte(self) -> u8 {
        self as u8
    }

    #[must_use]
    pub const fn from_byte(b: u8) -> Option<Self> {
        match b {
            b'.' => Some(Self::Dot),
            b')' => Some(Self::Paren),
            _ => None,
        }
    }
}

impl PartialEq<u8> for OrderedListDelimiter {
    fn eq(&self, other: &u8) -> bool {
        self.byte() == *other
    }
}

impl PartialEq<OrderedListDelimiter> for u8 {
    fn eq(&self, other: &OrderedListDelimiter) -> bool {
        *self == other.byte()
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
