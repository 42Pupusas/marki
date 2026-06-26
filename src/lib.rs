//! A fast, zero-copy `CommonMark` parser with SIMD-accelerated scanning.
//!
//! `marki-parse` parses Markdown into structured [`Section`] and [`Inline`] elements,
//! borrowing directly from the input string with no intermediate allocations for
//! text content.
//!
//! # Quick start
//!
//! ```
//! use marki_parse::MarkdownFile;
//!
//! let md: MarkdownFile<'_> = MarkdownFile::parse("# Hello\n\nSome **bold** text.");
//! for section in &md.sections {
//!     println!("{section:?}");
//! }
//! ```
//!
//! # CRLF input
//!
//! The parser operates on LF (`\n`) line endings. For input that may contain
//! `\r\n`, call [`MarkdownFile::normalize`] first — it returns the input borrowed when no
//! `\r` is present (zero cost):
//!
//! ```
//! let input = "# Hello\r\nWorld";
//! let normalized = marki_parse::MarkdownFile::normalize(input);
//! let md: marki_parse::MarkdownFile<'_> = marki_parse::MarkdownFile::parse(&normalized);
//! ```
//!
//! # Accessing inline elements
//!
//! Inline elements are stored in a flat pool for cache efficiency. Use
//! [`MarkdownFile::inlines`] (or index with [`InlineSpan`]) to retrieve them:
//!
//! ```
//! use marki_parse::{MarkdownFile, Section};
//!
//! let md: MarkdownFile<'_> = MarkdownFile::parse("Hello **world**");
//! if let Some(Section::Paragraph { content }) = md.sections.first() {
//!     for inline in md.inlines(*content) {
//!         println!("{inline:?}");
//!     }
//! }
//! ```

mod block;
pub(crate) mod entities_table;
pub(crate) mod entity;
mod html;
mod inline;
mod link_def;
pub(crate) mod raw_html;
mod section;
pub(crate) mod simd;
mod small_bool;
mod special_char;

#[cfg(test)]
mod fuzz_finds;
#[cfg(test)]
mod tests;

pub use inline::Inline;

/// Convert collection lengths into the `u32` offsets used by spans.
pub(crate) trait OffsetExt {
    fn pool_offset(self) -> u32;
    fn lines_offset(self) -> u32;
}

impl OffsetExt for usize {
    fn pool_offset(self) -> u32 {
        u32::try_from(self).expect("inline pool exceeds u32::MAX elements")
    }

    fn lines_offset(self) -> u32 {
        u32::try_from(self).expect("lines pool exceeds u32::MAX elements")
    }
}
use crate::simd::ByteSliceExt;
pub use section::{InlineSpan, LineRange, OrderedListDelimiter, PoolLine, Section, SectionRange};
pub use special_char::SpecialChar;

use std::borrow::Cow;

/// A parsed Markdown document.
///
/// Contains the block-level [`Section`]s and the internal pools that store
/// [`Inline`] elements. Use [`inlines`](Self::inlines) to access inline
/// content referenced by sections, and [`child_sections`](Self::child_sections)
/// for the child blocks of blockquotes and list items.
///
/// The const generics `MAX_INLINE_DEPTH` and `INLINE_STACK_CAP` control
/// recursion depth and stack-allocation size for emphasis parsing. The
/// defaults (`16` and `32`) are suitable for virtually all real-world input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownFile<'src, const MAX_INLINE_DEPTH: u8 = 16, const INLINE_STACK_CAP: usize = 32> {
    /// The block-level sections of the document, in order.
    pub sections: Vec<Section<'src>>,
    pool: Vec<Inline<'src>>,
    /// Pool of child sections referenced by [`SectionRange`] (blockquote
    /// interiors and list items). Kept flat so [`Section`] stays `Copy`.
    section_pool: Vec<Section<'src>>,
    /// Pool of dedented code lines referenced by [`LineRange`] (code blocks
    /// nested inside list items or blockquotes). Each entry carries the slice
    /// plus any synthetic leading-space [`pad`](PoolLine::pad).
    line_pool: Vec<PoolLine<'src>>,
}

/// On the default `MarkdownFile` (depth=16, cap=32) we expose `normalize` as
/// an associated fn so callers don't need to import a free function.
impl MarkdownFile<'_, 16, 32> {
    /// Normalize line endings for parsing. Converts `\r\n` to `\n` and bare
    /// `\r` (classic Mac) to `\n`. Returns the input borrowed if no carriage
    /// returns are found (zero cost).
    ///
    /// Call this before [`MarkdownFile::parse`] when input may contain CRLF:
    ///
    /// ```
    /// let input = "# Hello\r\nWorld";
    /// let normalized = marki_parse::MarkdownFile::normalize(input);
    /// let md: marki_parse::MarkdownFile<'_> = marki_parse::MarkdownFile::parse(&normalized);
    /// ```
    #[must_use]
    pub fn normalize(input: &str) -> Cow<'_, str> {
        let bytes = input.as_bytes();
        if bytes
            .find_byte(0, SpecialChar::CarriageReturn.byte())
            .is_none()
        {
            return Cow::Borrowed(input);
        }
        let mut out = String::with_capacity(input.len());
        let mut start = 0;
        while let Some(cr) = bytes.find_byte(start, SpecialChar::CarriageReturn.byte()) {
            out.push_str(&input[start..cr]);
            out.push('\n');
            start = cr + 1;
            if bytes.get(start) == Some(&SpecialChar::Newline.byte()) {
                start += 1;
            }
        }
        out.push_str(&input[start..]);
        Cow::Owned(out)
    }
}

impl<'src, const MAX_INLINE_DEPTH: u8, const INLINE_STACK_CAP: usize>
    MarkdownFile<'src, MAX_INLINE_DEPTH, INLINE_STACK_CAP>
{
    /// Get the inline elements referenced by a span.
    #[must_use]
    pub fn inlines(&self, span: InlineSpan) -> &[Inline<'src>] {
        &self[span]
    }

    /// Iterate the *direct* child sections referenced by a [`SectionRange`]
    /// (list items, blockquote and list-item interiors).
    ///
    /// The section pool is pre-order, so a container's descendants are
    /// interleaved between its direct children; this iterator skips each
    /// child's subtree via [`Section::pool_span`] to yield only the immediate
    /// children.
    pub fn child_sections(&self, range: SectionRange) -> impl Iterator<Item = &Section<'src>> {
        ChildSections {
            forest: &self[range],
            cursor: 0,
        }
    }

    /// Get the dedented code lines referenced by a [`LineRange`]. Each entry is
    /// a [`PoolLine`] carrying the borrowed slice plus its synthetic
    /// leading-space pad.
    #[must_use]
    pub fn code_lines(&self, range: LineRange) -> &[PoolLine<'src>] {
        &self[range]
    }

    /// Walk every section and dereference every inline span. Used in tests and
    /// fuzz targets to assert the parser does not panic on arbitrary input.
    #[cfg(test)]
    pub(crate) fn walk_all_inlines(&self) {
        self.walk_sections(self.sections.iter());
    }

    #[cfg(test)]
    fn walk_sections<'a>(&'a self, sections: impl Iterator<Item = &'a Section<'src>>)
    where
        'src: 'a,
    {
        for section in sections {
            match section {
                Section::UnorderedList { items, .. } | Section::OrderedList { items, .. } => {
                    self.walk_sections(self.child_sections(*items));
                }
                Section::Heading { content, .. } | Section::Paragraph { content } => {
                    let _ = self.inlines(*content);
                }
                Section::Blockquote { children } | Section::ListItem { children } => {
                    self.walk_sections(self.child_sections(*children));
                }
                Section::CodeBlock { .. }
                | Section::CodeLines { .. }
                | Section::IndentedCode { .. }
                | Section::HtmlBlock { .. }
                | Section::HtmlLines { .. }
                | Section::HorizontalRule => {}
            }
        }
    }
}

impl<'src, const MAX_INLINE_DEPTH: u8, const INLINE_STACK_CAP: usize> std::ops::Index<InlineSpan>
    for MarkdownFile<'src, MAX_INLINE_DEPTH, INLINE_STACK_CAP>
{
    type Output = [Inline<'src>];

    fn index(&self, span: InlineSpan) -> &[Inline<'src>] {
        let start = span.start as usize;
        let end = start + span.len as usize;
        &self.pool[start..end]
    }
}

impl<'src, const MAX_INLINE_DEPTH: u8, const INLINE_STACK_CAP: usize> std::ops::Index<SectionRange>
    for MarkdownFile<'src, MAX_INLINE_DEPTH, INLINE_STACK_CAP>
{
    type Output = [Section<'src>];

    fn index(&self, range: SectionRange) -> &[Section<'src>] {
        let start = range.start as usize;
        let end = start + range.len as usize;
        &self.section_pool[start..end]
    }
}

/// Iterator over the direct children of a pre-order sub-forest, skipping each
/// child's interleaved subtree. Produced by
/// [`child_sections`](MarkdownFile::child_sections).
struct ChildSections<'a, 'src> {
    forest: &'a [Section<'src>],
    cursor: usize,
}

impl<'a, 'src> Iterator for ChildSections<'a, 'src> {
    type Item = &'a Section<'src>;

    fn next(&mut self) -> Option<Self::Item> {
        let child = self.forest.get(self.cursor)?;
        self.cursor += child.pool_span();
        Some(child)
    }
}

impl<'src, const MAX_INLINE_DEPTH: u8, const INLINE_STACK_CAP: usize> std::ops::Index<LineRange>
    for MarkdownFile<'src, MAX_INLINE_DEPTH, INLINE_STACK_CAP>
{
    type Output = [PoolLine<'src>];

    fn index(&self, range: LineRange) -> &[PoolLine<'src>] {
        let start = range.start as usize;
        let end = start + range.len as usize;
        &self.line_pool[start..end]
    }
}
