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

/// A range of [`Section`]s stored contiguously in the section pool.
///
/// Used by blockquotes (and later, list items) to reference their child blocks
/// without a per-block heap allocation, keeping [`Section`] itself `Copy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionRange {
    pub start: u32,
    pub len: u32,
}

impl SectionRange {
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

/// The delimiter used after the number in an ordered list item (`1.` vs `1)`).
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

/// A block-level element of a Markdown document.
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
    /// A blockquote (`CommonMark` §5.1) containing child block-level sections,
    /// stored as a range in the document's section pool.
    Blockquote {
        children: SectionRange,
    },
    /// A raw HTML block (`CommonMark` §4.6). The slice is emitted verbatim,
    /// without escaping or inline parsing.
    HtmlBlock {
        html: &'src str,
    },
    HorizontalRule,
}
