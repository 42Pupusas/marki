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

/// A range of `&str` lines stored contiguously in the line pool.
///
/// Used by code blocks nested inside list items or blockquotes, whose content
/// lines are dedented (and therefore no longer a single contiguous source
/// slice).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineRange {
    pub start: u32,
    pub len: u32,
}

impl LineRange {
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

/// A pre-order sub-forest of [`Section`]s in the section pool.
///
/// Used by blockquotes, list items, and lists to reference their child blocks
/// without a per-block heap allocation, keeping [`Section`] itself `Copy`.
///
/// The pool is laid out in **pre-order**: a container is immediately followed
/// by its entire subtree, so `start` is the index of the container's first
/// child and `len` counts *every* entry in the sub-forest (direct children
/// **and** their descendants). Direct children are therefore not adjacent —
/// iterate them with [`child_sections`](crate::MarkdownFile::child_sections),
/// which skips each child's subtree via [`Section::pool_span`].
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

impl Section<'_> {
    /// Number of pool entries this section occupies, including itself and its
    /// entire pre-order subtree. Leaves span 1; containers span `1 + len` of
    /// their child sub-forest. Used to skip whole subtrees when walking direct
    /// children.
    #[must_use]
    pub const fn pool_span(&self) -> usize {
        match self {
            Section::UnorderedList { items, .. } | Section::OrderedList { items, .. } => {
                1 + items.len as usize
            }
            Section::ListItem { children } | Section::Blockquote { children } => {
                1 + children.len as usize
            }
            _ => 1,
        }
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
    /// An indented code block (`CommonMark` §4.4). `code` is the verbatim
    /// source span, still carrying its leading indentation; the renderer
    /// strips up to four leading spaces per line.
    IndentedCode {
        code: &'src str,
    },
    /// An unordered (bullet) list (`CommonMark` §5.3). `items` is a range of
    /// [`Section::ListItem`] sections in the document's section pool. `tight`
    /// controls rendering: a tight list emits its items' paragraph children
    /// without `<p>` wrappers.
    UnorderedList {
        tight: bool,
        items: SectionRange,
    },
    /// An ordered list (`CommonMark` §5.3). See [`Section::UnorderedList`].
    OrderedList {
        start: u32,
        delimiter: OrderedListDelimiter,
        tight: bool,
        items: SectionRange,
    },
    /// A single list item (`CommonMark` §5.2), containing child block-level
    /// sections stored as a range in the document's section pool.
    ListItem {
        children: SectionRange,
    },
    /// A code block whose content lines were dedented out of a container (list
    /// item or blockquote) and so live in the line pool rather than as one
    /// contiguous source slice. `language` is `None` for indented code.
    CodeLines {
        language: Option<&'src str>,
        lines: LineRange,
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
