mod block;
mod inline;
mod section;
pub(crate) mod simd;
mod special_char;

#[cfg(test)]
mod tests;

use std::borrow::Cow;

pub use inline::Inline;
pub use section::{InlineSpan, OrderedListDelimiter, Section};
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
/// let md = marki::MarkdownFile::parse(&normalized);
/// ```
#[must_use]
pub fn normalize(input: &str) -> Cow<'_, str> {
    if input.as_bytes().contains(&SpecialChar::CarriageReturn.byte()) {
        Cow::Owned(input.replace('\r', ""))
    } else {
        Cow::Borrowed(input)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownFile<'src> {
    pub sections: Vec<Section<'src>>,
    pool: Vec<Inline<'src>>,
}

impl<'src> MarkdownFile<'src> {
    /// Get the inline elements referenced by a span.
    #[must_use]
    pub fn inlines(&self, span: InlineSpan) -> &[Inline<'src>] {
        &self[span]
    }
}

impl<'src> std::ops::Index<InlineSpan> for MarkdownFile<'src> {
    type Output = [Inline<'src>];

    fn index(&self, span: InlineSpan) -> &[Inline<'src>] {
        let start = span.start as usize;
        let end = start + span.len as usize;
        &self.pool[start..end]
    }
}
