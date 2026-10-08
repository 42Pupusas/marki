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

/// One entry in the code/HTML line pool: a borrowed source slice plus a count
/// of *synthetic* leading spaces to emit before it.
///
/// The `pad` is almost always `0` — the common case is a zero-copy source
/// slice. It is non-zero only when a container prefix (a blockquote `>`
/// padding, a list content column, or the 4-space indented-code strip) ends
/// partway through a tab: the leftover columns of that tab must be rendered as
/// spaces that exist in no source substring (`CommonMark` §2.2 tab expansion).
/// Carrying the count keeps the slice itself borrowed (`&'src str`), preserving
/// the parser's zero-copy guarantee — we never have to own a padded string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolLine<'src> {
    /// Synthetic leading spaces to emit before [`text`](Self::text).
    pub pad: u8,
    /// The borrowed source slice (already dedented to a byte boundary).
    pub text: &'src str,
}

impl<'src> PoolLine<'src> {
    /// A zero-copy line with no synthetic padding (the overwhelmingly common
    /// case).
    #[inline]
    #[must_use]
    pub const fn plain(text: &'src str) -> Self {
        Self { pad: 0, text }
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

/// Column alignment for a GFM table (`CommonMark`/GFM §4.10), derived from the
/// colons in the delimiter row: `:---` left, `:--:` center, `---:` right, and
/// `---` (no colon) [`None`](Self::None).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColumnAlignment {
    /// No explicit alignment (`---`): the renderer picks its default.
    #[default]
    None,
    /// Left-aligned (`:---`).
    Left,
    /// Center-aligned (`:--:`).
    Center,
    /// Right-aligned (`---:`).
    Right,
}

impl ColumnAlignment {
    /// Derive the alignment from a single delimiter-row cell's trimmed bytes
    /// (e.g. `:--:`). A leading `:` means left, a trailing `:` means right,
    /// both means center. Returns [`None`](Self::None) when neither is present.
    #[must_use]
    pub fn from_delimiter(cell: &[u8]) -> Self {
        let left = cell.first() == Some(&b':');
        let right = cell.last() == Some(&b':');
        match (left, right) {
            (true, true) => Self::Center,
            (true, false) => Self::Left,
            (false, true) => Self::Right,
            (false, false) => Self::None,
        }
    }
}

/// The kind of a GFM alert (`> [!NOTE]`, `> [!TIP]`, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl AlertKind {
    /// The title rendered when the alert has no custom one.
    #[must_use]
    pub const fn default_title(self) -> &'static str {
        match self {
            Self::Note => "Note",
            Self::Tip => "Tip",
            Self::Important => "Important",
            Self::Warning => "Warning",
            Self::Caution => "Caution",
        }
    }

    /// The lowercase keyword, used for the `markdown-alert-…` CSS class.
    #[must_use]
    pub const fn keyword(self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Tip => "tip",
            Self::Important => "important",
            Self::Warning => "warning",
            Self::Caution => "caution",
        }
    }
}

/// The checkbox of a GFM task-list item (`- [ ]` / `- [x]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checkbox {
    Unchecked,
    Checked,
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
            Section::ListItem { children, .. }
            | Section::Blockquote { children }
            | Section::Alert { children, .. }
            | Section::FootnoteDefinition { children, .. } => 1 + children.len as usize,
            Section::Table { rows, .. } => 1 + rows.len as usize,
            Section::TableRow { cells, .. } => 1 + cells.len as usize,
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
    /// sections stored as a range in the document's section pool. `task` is
    /// the GFM task-list checkbox, when the item starts with `[ ]` or `[x]`.
    ListItem {
        task: Option<Checkbox>,
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
    /// A GFM alert: a blockquote whose first line is `[!NOTE]`, `[!TIP]`,
    /// `[!IMPORTANT]`, `[!WARNING]`, or `[!CAUTION]`. `title` is the raw text
    /// after the marker, if any.
    Alert {
        kind: AlertKind,
        title: Option<&'src str>,
        children: SectionRange,
    },
    /// A GFM footnote definition (`[^label]: …`). It stays at its source
    /// position; renderers collect referenced definitions at the document end.
    /// `number` is the 1-based position in the footnote list, assigned in
    /// order of first reference, and `references` counts the references to
    /// it. Both are `0` for a definition that is never referenced.
    FootnoteDefinition {
        label: &'src str,
        number: u32,
        references: u32,
        children: SectionRange,
    },
    /// A raw HTML block (`CommonMark` §4.6). The slice is emitted verbatim,
    /// without escaping or inline parsing.
    HtmlBlock {
        html: &'src str,
    },
    /// A raw HTML block whose content lines were dedented out of a container
    /// (list item or blockquote) and so live in the line pool rather than as
    /// one contiguous source slice. Emitted verbatim, line by line.
    HtmlLines {
        lines: LineRange,
    },
    HorizontalRule,
    /// A GFM table (`CommonMark`/GFM §4.10). `columns` is the column count
    /// fixed by the header/delimiter rows; `rows` is a [`SectionRange`] of
    /// [`Section::TableRow`] children in the section pool (the header row first,
    /// then any body rows). Iterate them with
    /// [`child_sections`](crate::MarkdownFile::child_sections).
    Table {
        columns: u16,
        rows: SectionRange,
    },
    /// One row of a [`Section::Table`]. `header` marks the first (heading) row.
    /// `cells` is a [`SectionRange`] of [`Section::TableCell`] children, always
    /// exactly the table's `columns` long (short source rows are padded with
    /// empty cells, long ones truncated, per GFM).
    TableRow {
        header: bool,
        cells: SectionRange,
    },
    /// One cell of a [`Section::TableRow`]. `align` is the column's alignment
    /// (from the delimiter row); `content` is the cell text parsed as inlines.
    TableCell {
        align: ColumnAlignment,
        content: InlineSpan,
    },
}
