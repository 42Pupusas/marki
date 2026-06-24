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
//! [`MarkdownFile::inlines`] and [`MarkdownFile::item_spans`] (or index with
//! [`InlineSpan`] / [`SpanSlice`]) to retrieve them:
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
mod html;
mod inline;
mod link_def;
pub(crate) mod raw_html;
mod section;
pub(crate) mod simd;
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
pub use section::{InlineSpan, OrderedListDelimiter, Section, SpanSlice};
pub use special_char::SpecialChar;

use std::borrow::Cow;

/// A parsed Markdown document.
///
/// Contains the block-level [`Section`]s and the internal pools that store
/// [`Inline`] elements. Use [`inlines`](Self::inlines) and
/// [`item_spans`](Self::item_spans) to access inline content referenced by
/// sections.
///
/// The const generics `MAX_INLINE_DEPTH` and `INLINE_STACK_CAP` control
/// recursion depth and stack-allocation size for emphasis parsing. The
/// defaults (`16` and `32`) are suitable for virtually all real-world input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownFile<'src, const MAX_INLINE_DEPTH: u8 = 16, const INLINE_STACK_CAP: usize = 32> {
    /// The block-level sections of the document, in order.
    pub sections: Vec<Section<'src>>,
    pool: Vec<Inline<'src>>,
    span_pool: Vec<InlineSpan>,
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

    /// Get the item spans referenced by a `SpanSlice` (list items).
    #[must_use]
    pub fn item_spans(&self, slice: SpanSlice) -> &[InlineSpan] {
        &self[slice]
    }

    /// Walk every section and dereference every inline span. Used in tests and
    /// fuzz targets to assert the parser does not panic on arbitrary input.
    #[cfg(test)]
    pub(crate) fn walk_all_inlines(&self) {
        for section in &self.sections {
            match section {
                Section::UnorderedList { items } | Section::OrderedList { items, .. } => {
                    for &span in self.item_spans(*items) {
                        let _ = self.inlines(span);
                    }
                }
                Section::Heading { content, .. }
                | Section::Paragraph { content }
                | Section::Blockquote { content } => {
                    let _ = self.inlines(*content);
                }
                Section::CodeBlock { .. }
                | Section::HtmlBlock { .. }
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

impl<const MAX_INLINE_DEPTH: u8, const INLINE_STACK_CAP: usize> std::ops::Index<SpanSlice>
    for MarkdownFile<'_, MAX_INLINE_DEPTH, INLINE_STACK_CAP>
{
    type Output = [InlineSpan];

    fn index(&self, slice: SpanSlice) -> &[InlineSpan] {
        let start = slice.start as usize;
        let end = start + slice.len as usize;
        &self.span_pool[start..end]
    }
}
