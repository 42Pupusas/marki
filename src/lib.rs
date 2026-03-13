mod block;
mod inline;
mod section;
pub(crate) mod simd;
mod special_char;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod fuzz_finds;

use std::borrow::Cow;

pub use inline::Inline;
pub use section::{InlineSpan, OrderedListDelimiter, Section, SpanSlice};
pub use special_char::SpecialChar;

/// Normalize line endings for parsing. Returns the input borrowed if it
/// contains no carriage returns, or an owned copy with `\r` stripped otherwise.
///
/// Use this before [`MarkdownFile::parse`] when the input may contain CRLF
/// line endings:
///
/// ```
/// let input = "# Hello\r\nWorld";
/// let normalized = marki::normalize(input);
/// let md: marki::MarkdownFile<'_> = marki::MarkdownFile::parse(&normalized);
/// ```
#[must_use]
pub fn normalize(input: &str) -> Cow<'_, str> {
    if simd::find_byte(input.as_bytes(), 0, SpecialChar::CarriageReturn.byte()).is_some() {
        Cow::Owned(input.replace('\r', ""))
    } else {
        Cow::Borrowed(input)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownFile<
    'src,
    const MAX_INLINE_DEPTH: u8 = 16,
    const INLINE_STACK_CAP: usize = 32,
> {
    pub sections: Vec<Section<'src>>,
    pool: Vec<Inline<'src>>,
    span_pool: Vec<InlineSpan>,
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
}

impl<'src, const MAX_INLINE_DEPTH: u8, const INLINE_STACK_CAP: usize>
    std::ops::Index<InlineSpan> for MarkdownFile<'src, MAX_INLINE_DEPTH, INLINE_STACK_CAP>
{
    type Output = [Inline<'src>];

    fn index(&self, span: InlineSpan) -> &[Inline<'src>] {
        let start = span.start as usize;
        let end = start + span.len as usize;
        &self.pool[start..end]
    }
}

impl<const MAX_INLINE_DEPTH: u8, const INLINE_STACK_CAP: usize>
    std::ops::Index<SpanSlice> for MarkdownFile<'_, MAX_INLINE_DEPTH, INLINE_STACK_CAP>
{
    type Output = [InlineSpan];

    fn index(&self, slice: SpanSlice) -> &[InlineSpan] {
        let start = slice.start as usize;
        let end = start + slice.len as usize;
        &self.span_pool[start..end]
    }
}
