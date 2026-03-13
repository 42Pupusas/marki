/// A range of inline elements stored contiguously in the inline pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InlineSpan {
    pub start: u32,
    pub len: u32,
}

impl InlineSpan {
    pub const EMPTY: Self = Self { start: 0, len: 0 };

    #[inline]
    #[must_use]
    pub const fn new(start: u32, len: u32) -> Self {
        Self { start, len }
    }

    #[inline]
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.len == 0
    }
}

/// A range of `InlineSpan` elements stored contiguously in the span pool.
/// Used by list sections to avoid per-list `Vec<InlineSpan>` heap allocations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpanSlice {
    pub start: u32,
    pub len: u32,
}

impl SpanSlice {
    pub const EMPTY: Self = Self { start: 0, len: 0 };

    #[inline]
    #[must_use]
    pub const fn new(start: u32, len: u32) -> Self {
        Self { start, len }
    }

    #[inline]
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.len == 0
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section<'src> {
    Heading {
        level: u8,
        content: InlineSpan,
    },
    Paragraph {
        content: InlineSpan,
    },
    CodeBlock {
        language: Option<&'src str>,
        code: &'src str,
    },
    UnorderedList {
        items: SpanSlice,
    },
    OrderedList {
        start: u32,
        delimiter: OrderedListDelimiter,
        items: SpanSlice,
    },
    Blockquote {
        content: InlineSpan,
    },
    HorizontalRule,
}
