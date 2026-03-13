use std::mem::MaybeUninit;

use crate::SpecialChar;
use crate::section::InlineSpan;
use crate::simd::{ByteSet, find_byte, find_byte_set};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inline<'src> {
    Text(&'src str),
    Bold(InlineSpan),
    Italic(InlineSpan),
    Link {
        text: InlineSpan,
        url: &'src str,
        title: Option<&'src str>,
    },
    Image {
        alt: &'src str,
        url: &'src str,
        title: Option<&'src str>,
    },
    Code(&'src str),
    SoftBreak,
    HardBreak,
}

impl<'src> From<&'src str> for Inline<'src> {
    fn from(s: &'src str) -> Self {
        Self::Text(s)
    }
}

/// SIMD-accelerated byte set for inline special characters.
static SPECIAL_SET: ByteSet = ByteSet::new(&[
    SpecialChar::Newline.byte(),
    SpecialChar::Asterisk.byte(),
    SpecialChar::Underscore.byte(),
    SpecialChar::OpenBracket.byte(),
    SpecialChar::ExclamationMark.byte(),
    SpecialChar::Backslash.byte(),
    SpecialChar::Backtick.byte(),
]);

/// Pre-computed byte sets for `find_matching_close` — avoids rebuilding
/// the 256-byte lookup table on every call.
static BRACKET_CLOSE_SET: ByteSet = ByteSet::new(&[
    SpecialChar::OpenBracket.byte(),
    SpecialChar::CloseBracket.byte(),
    SpecialChar::Backslash.byte(),
]);
static PAREN_CLOSE_SET: ByteSet = ByteSet::new(&[
    SpecialChar::OpenParen.byte(),
    SpecialChar::CloseParen.byte(),
    SpecialChar::Backslash.byte(),
]);

/// Pre-computed byte sets for `try_parse_delimited` — one per delimiter type.
static STAR_DELIM_SET: ByteSet =
    ByteSet::new(&[SpecialChar::Asterisk.byte(), SpecialChar::Backslash.byte()]);
static UNDER_DELIM_SET: ByteSet = ByteSet::new(&[
    SpecialChar::Underscore.byte(),
    SpecialChar::Backslash.byte(),
]);

/// Character classification for `CommonMark` emphasis flanking rules.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Whitespace,
    Punctuation,
    Other,
}

impl CharClass {
    const fn of(ch: char) -> Self {
        if ch.is_whitespace() {
            Self::Whitespace
        } else if ch.is_ascii_punctuation() || unicode_punctuation(ch) {
            Self::Punctuation
        } else {
            Self::Other
        }
    }

    /// Fast classification for ASCII bytes, avoiding UTF-8 decode.
    #[inline]
    const fn of_ascii(b: u8) -> Self {
        if b.is_ascii_whitespace() {
            Self::Whitespace
        } else if b.is_ascii_punctuation() {
            Self::Punctuation
        } else {
            Self::Other
        }
    }
}

/// Check if a character is Unicode punctuation (general categories P or S)
/// beyond ASCII punctuation. Covers the most common cases without a dependency.
/// A full implementation would use unicode-general-category crate.
const fn unicode_punctuation(ch: char) -> bool {
    if ch.is_ascii() {
        return false;
    }
    matches!(ch,
        '\u{00A1}'..='\u{00BF}' // Latin punctuation/symbols
        | '\u{2010}'..='\u{2027}' // General punctuation (dashes, quotes, etc.)
        | '\u{2030}'..='\u{205E}' // More general punctuation
        | '\u{2190}'..='\u{23FF}' // Arrows, math operators, misc technical
        | '\u{2500}'..='\u{2BFF}' // Box drawing, block elements, symbols
        | '\u{3000}'..='\u{303F}' // CJK symbols and punctuation
        | '\u{FE30}'..='\u{FE6F}' // CJK compatibility forms, small forms
        | '\u{FF01}'..='\u{FF0F}' // Fullwidth punctuation
        | '\u{FF1A}'..='\u{FF20}' // More fullwidth punctuation
        | '\u{FF3B}'..='\u{FF40}' // Fullwidth brackets
        | '\u{FF5B}'..='\u{FF65}' // Fullwidth punctuation
    )
}

/// What emphasis types remain possible for a given delimiter character.
/// Tracks delimiter availability per character type, avoiding O(n²)
/// re-scanning in both top-level and recursive parse calls.
#[derive(Clone, Copy)]
enum DelimiterAvail {
    /// Both bold and italic are still possible.
    Both,
    /// Bold failed; only italic can be attempted.
    ItalicOnly,
    /// Italic failed; only bold can be attempted.
    BoldOnly,
    /// Neither bold nor italic can succeed.
    None,
}

impl DelimiterAvail {
    const fn can_bold(self) -> bool {
        matches!(self, Self::Both | Self::BoldOnly)
    }

    const fn can_italic(self) -> bool {
        matches!(self, Self::Both | Self::ItalicOnly)
    }

    const fn bold_failed(&mut self) {
        *self = match *self {
            Self::Both => Self::ItalicOnly,
            Self::BoldOnly => Self::None,
            other => other,
        };
    }

    const fn italic_failed(&mut self) {
        *self = match *self {
            Self::Both => Self::BoldOnly,
            Self::ItalicOnly => Self::None,
            other => other,
        };
    }

    const fn from_count(count: usize) -> Self {
        match count {
            0 | 1 => Self::None,
            2 | 3 => Self::ItalicOnly,
            _ => Self::Both,
        }
    }
}

struct EmphasisState {
    star: DelimiterAvail,
    under: DelimiterAvail,
}

impl EmphasisState {
    const fn assume_both() -> Self {
        Self {
            star: DelimiterAvail::Both,
            under: DelimiterAvail::Both,
        }
    }

    fn from_bytes(bytes: &[u8]) -> Self {
        static EMPH_SET: ByteSet =
            ByteSet::new(&[SpecialChar::Asterisk.byte(), SpecialChar::Underscore.byte()]);
        let mut stars: u8 = 0;
        let mut unders: u8 = 0;
        let mut i = 0;
        loop {
            let Some(pos) = find_byte_set(bytes, i, &EMPH_SET) else {
                break;
            };
            if bytes[pos] == SpecialChar::Asterisk {
                stars += 1;
            } else {
                unders += 1;
            }
            if stars >= 4 && unders >= 4 {
                break;
            }
            i = pos + 1;
        }
        Self {
            star: DelimiterAvail::from_count(stars as usize),
            under: DelimiterAvail::from_count(unders as usize),
        }
    }

    const fn avail_mut(&mut self, is_star: bool) -> &mut DelimiterAvail {
        if is_star {
            &mut self.star
        } else {
            &mut self.under
        }
    }
}

/// Maximum inline elements per parse level stored on the stack.
/// Falls back to heap if exceeded (extremely rare — 32 inlines on one line).
const STACK_CAP: usize = 32;

/// Stack-allocated buffer for collecting inline elements without heap allocation.
/// Uses `MaybeUninit` to avoid zeroing the 32-element stack array on every parse call.
struct InlineBuf<'src> {
    stack: [MaybeUninit<Inline<'src>>; STACK_CAP],
    len: usize,
    overflow: Vec<Inline<'src>>,
}

impl<'src> InlineBuf<'src> {
    #[inline]
    const fn new() -> Self {
        Self {
            // SAFETY: An array of MaybeUninit does not require initialization.
            stack: [const { MaybeUninit::uninit() }; STACK_CAP],
            len: 0,
            overflow: Vec::new(),
        }
    }

    #[allow(clippy::inline_always)]
    #[inline(always)]
    fn push(&mut self, item: Inline<'src>) {
        if self.len < STACK_CAP {
            self.stack[self.len] = MaybeUninit::new(item);
            self.len += 1;
        } else {
            self.push_slow(item);
        }
    }

    #[cold]
    fn push_slow(&mut self, item: Inline<'src>) {
        if self.overflow.is_empty() {
            // Spill stack to heap
            self.overflow = Vec::with_capacity(STACK_CAP * 2);
            // SAFETY: elements 0..self.len were initialized via push.
            // Use a raw pointer to avoid borrow conflict with self.overflow.
            let len = self.len;
            let ptr = self.stack.as_ptr().cast::<Inline>();
            let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
            self.overflow.extend_from_slice(slice);
        }
        self.overflow.push(item);
    }

    /// Get initialized stack elements as a slice.
    #[inline]
    const fn initialized_stack(&self) -> &[Inline<'src>] {
        // SAFETY: all elements 0..self.len have been initialized via push.
        unsafe { std::slice::from_raw_parts(self.stack.as_ptr().cast::<Inline>(), self.len) }
    }

    #[inline]
    fn flush_to_pool(self, pool: &mut Vec<Inline<'src>>) -> InlineSpan {
        let start = pool_offset(pool.len());
        if self.overflow.is_empty() {
            pool.extend_from_slice(self.initialized_stack());
            InlineSpan::new(start, pool_offset(self.len))
        } else {
            let len = pool_offset(self.overflow.len());
            pool.extend(self.overflow);
            InlineSpan::new(start, len)
        }
    }
}

/// Pool index as `u32`. Panics if the pool exceeds 4 GiB of elements
/// (unreachable in practice — that would require billions of inline nodes).
#[allow(clippy::inline_always)]
#[inline(always)]
pub fn pool_offset(pool_len: usize) -> u32 {
    u32::try_from(pool_len).expect("inline pool exceeds u32::MAX elements")
}

impl<'src> Inline<'src> {
    /// Threshold below which the emphasis pre-scan costs more than it saves.
    const EMPH_SCAN_THRESHOLD: usize = 256;

    /// Parse inline elements and store them in the pool. Returns a span.
    #[must_use]
    pub fn parse(input: &'src str, pool: &mut Vec<Self>) -> InlineSpan {
        let bytes = input.as_bytes();
        // Fast path: if no special bytes exist, the entire input is plain text.
        if find_byte_set(bytes, 0, &SPECIAL_SET).is_none() {
            if input.is_empty() {
                return InlineSpan::EMPTY;
            }
            let start = pool_offset(pool.len());
            pool.push(Self::Text(input));
            return InlineSpan::new(start, 1);
        }
        let emph = if bytes.len() < Self::EMPH_SCAN_THRESHOLD {
            EmphasisState::assume_both()
        } else {
            EmphasisState::from_bytes(bytes)
        };
        Self::parse_with_emph(input, bytes, emph, pool)
    }

    fn parse_inner(input: &'src str, pool: &mut Vec<Self>) -> InlineSpan {
        let bytes = input.as_bytes();
        Self::parse_with_emph(input, bytes, EmphasisState::assume_both(), pool)
    }

    fn parse_with_emph(
        input: &'src str,
        bytes: &[u8],
        emph: EmphasisState,
        pool: &mut Vec<Self>,
    ) -> InlineSpan {
        let mut buf = InlineBuf::new();
        Self::parse_into_buf(input, bytes, emph, pool, &mut buf);
        buf.flush_to_pool(pool)
    }

    /// Push parsed inline elements directly into the pool without wrapping
    /// in a span. Used for blockquote multi-line accumulation where the caller
    /// manages span boundaries.
    pub fn parse_flat_into(input: &'src str, pool: &mut Vec<Self>) {
        let bytes = input.as_bytes();
        // Fast path: no special bytes means plain text.
        if find_byte_set(bytes, 0, &SPECIAL_SET).is_none() {
            if !input.is_empty() {
                pool.push(Self::Text(input));
            }
            return;
        }
        let emph = if bytes.len() < Self::EMPH_SCAN_THRESHOLD {
            EmphasisState::assume_both()
        } else {
            EmphasisState::from_bytes(bytes)
        };
        // Parse directly into a buf that flushes to pool (flat, no span wrapper).
        let mut buf = InlineBuf::new();
        Self::parse_into_buf(input, bytes, emph, pool, &mut buf);
        // Flush buf directly to pool (not wrapped in a span).
        if buf.overflow.is_empty() {
            pool.extend_from_slice(buf.initialized_stack());
        } else {
            pool.extend(buf.overflow);
        }
    }

    fn parse_into_buf(
        input: &'src str,
        bytes: &[u8],
        mut emph: EmphasisState,
        pool: &mut Vec<Self>,
        buf: &mut InlineBuf<'src>,
    ) {
        let mut plain_start = 0;
        let mut i = 0;

        // SIMD-accelerated scan: find next special byte.
        while let Some(pos) = find_byte_set(bytes, i, &SPECIAL_SET) {
            i = pos;
            let b = bytes[i];

            if b == SpecialChar::Newline {
                Self::emit_line_break(input, bytes, plain_start, i, buf);
                plain_start = i + 1;
                i = plain_start;
                continue;
            }

            // Backslash escape: only ASCII punctuation can be escaped (CommonMark spec).
            // For non-punctuation, the backslash is kept as literal text.
            if b == SpecialChar::Backslash
                && let Some(&next) = bytes.get(i + 1)
                && next.is_ascii_punctuation()
            {
                if plain_start < i {
                    buf.push(Self::Text(&input[plain_start..i]));
                }
                plain_start = i + 1;
                i += 2;
                continue;
            }

            // Inline code: `code` or ``code``
            if b == SpecialChar::Backtick
                && let Some((code, end)) = Self::try_parse_inline_code(input, bytes, i)
            {
                if plain_start < i {
                    buf.push(Self::Text(&input[plain_start..i]));
                }
                buf.push(Self::Code(code));
                plain_start = end;
                i = end;
                continue;
            }

            // Image: ![alt](url "title")
            if b == SpecialChar::ExclamationMark
                && bytes.get(i + 1) == SpecialChar::OpenBracket
                && let Some((alt, url, title, end)) =
                    Self::try_parse_bracket_paren(input, bytes, i + 1)
            {
                if plain_start < i {
                    buf.push(Self::Text(&input[plain_start..i]));
                }
                buf.push(Self::Image { alt, url, title });
                plain_start = end;
                i = end;
                continue;
            }

            // Link: [text](url "title")
            if b == SpecialChar::OpenBracket
                && let Some((text_str, url, title, end)) =
                    Self::try_parse_bracket_paren(input, bytes, i)
            {
                if plain_start < i {
                    buf.push(Self::Text(&input[plain_start..i]));
                }
                let text_span = Self::parse_inner(text_str, pool);
                buf.push(Self::Link {
                    text: text_span,
                    url,
                    title,
                });
                plain_start = end;
                i = end;
                continue;
            }

            // Bold/Italic: ** __ * _
            if let Some((elem, end)) = Self::try_parse_emphasis(input, bytes, i, b, &mut emph, pool)
            {
                if plain_start < i {
                    buf.push(Self::Text(&input[plain_start..i]));
                }
                buf.push(elem);
                plain_start = end;
                i = end;
                continue;
            }

            i += 1;
        }

        if plain_start < input.len() {
            buf.push(Self::Text(&input[plain_start..]));
        }
    }

    /// Emit a hard or soft line break at a newline position.
    /// Hard break if preceded by trailing `\` or 2+ spaces; soft break otherwise.
    #[inline]
    fn emit_line_break(
        input: &'src str,
        bytes: &[u8],
        plain_start: usize,
        newline_pos: usize,
        buf: &mut InlineBuf<'src>,
    ) {
        let preceding = &bytes[plain_start..newline_pos];
        let (trim_end, is_hard) = if preceding.last() == SpecialChar::Backslash {
            (newline_pos - 1, true)
        } else {
            // Count trailing spaces with a simple backward loop.
            let mut spaces = 0;
            let mut j = preceding.len();
            while j > 0 && preceding[j - 1] == b' ' {
                spaces += 1;
                j -= 1;
            }
            if spaces >= 2 {
                (newline_pos - spaces, true)
            } else {
                (newline_pos, false)
            }
        };
        if plain_start < trim_end {
            buf.push(Self::Text(&input[plain_start..trim_end]));
        }
        buf.push(if is_hard {
            Self::HardBreak
        } else {
            Self::SoftBreak
        });
    }

    #[inline]
    fn try_parse_emphasis(
        input: &'src str,
        bytes: &[u8],
        i: usize,
        b: u8,
        emph: &mut EmphasisState,
        pool: &mut Vec<Self>,
    ) -> Option<(Self, usize)> {
        let is_star = b == SpecialChar::Asterisk;
        if !is_star && b != SpecialChar::Underscore {
            return None;
        }
        let avail = emph.avail_mut(is_star);

        // Bold: ** or __
        if avail.can_bold() && bytes.get(i + 1) == Some(&b) {
            if let Some((inner, end)) = Self::try_parse_delimited(input, bytes, i, b, 2) {
                let span = Self::parse_inner(inner, pool);
                return Some((Self::Bold(span), end));
            }
            avail.bold_failed();
        }

        // Italic: * or _
        if avail.can_italic() {
            if let Some((inner, end)) = Self::try_parse_delimited(input, bytes, i, b, 1) {
                let span = Self::parse_inner(inner, pool);
                return Some((Self::Italic(span), end));
            }
            avail.italic_failed();
        }

        None
    }

    /// Find the position of a matching closing delimiter, handling backslash
    /// escapes and nested pairs.
    fn find_matching_close(
        bytes: &[u8],
        start: usize,
        open: SpecialChar,
        close: SpecialChar,
    ) -> Option<usize> {
        // Select pre-computed static ByteSet instead of building one each call.
        let set = if open == SpecialChar::OpenBracket {
            &BRACKET_CLOSE_SET
        } else {
            &PAREN_CLOSE_SET
        };
        let mut depth = 0u32;
        let mut j = start;
        loop {
            let pos = find_byte_set(bytes, j, set)?;
            let b = bytes[pos];
            if b == SpecialChar::Backslash
                && bytes.get(pos + 1).is_some_and(u8::is_ascii_punctuation)
            {
                j = pos + 2;
                continue;
            }
            if b == open {
                depth += 1;
            } else if b == close {
                if depth == 0 {
                    return Some(pos);
                }
                depth -= 1;
            }
            j = pos + 1;
        }
    }

    fn try_parse_bracket_paren(
        input: &'src str,
        bytes: &[u8],
        start: usize,
    ) -> Option<(&'src str, &'src str, Option<&'src str>, usize)> {
        if bytes.get(start) != SpecialChar::OpenBracket {
            return None;
        }

        let bracket_start = start + 1;
        let bracket_end = Self::find_matching_close(
            bytes,
            bracket_start,
            SpecialChar::OpenBracket,
            SpecialChar::CloseBracket,
        )?;

        let paren_pos = bracket_end + 1;
        if bytes.get(paren_pos) != SpecialChar::OpenParen {
            return None;
        }

        let paren_start = paren_pos + 1;
        let paren_end = Self::find_matching_close(
            bytes,
            paren_start,
            SpecialChar::OpenParen,
            SpecialChar::CloseParen,
        )?;

        let paren_content = input.get(paren_start..paren_end)?;
        let (url, title) = Self::split_url_title(paren_content);

        Some((
            input.get(bracket_start..bracket_end)?,
            url,
            title,
            paren_end + 1,
        ))
    }

    /// Split the content inside `(...)` into a URL and optional title
    /// (`CommonMark` §6.3).
    ///
    /// Titles are delimited by `"..."`, `'...'`, or `(...)`.
    ///
    /// We scan **backwards** because the title, if present, is always at the
    /// end. The algorithm:
    ///  1. Check the last byte for a closing title delimiter (`"`, `'`, `)`).
    ///  2. Walk backwards to find the matching opener.
    ///  3. The opener must be preceded by whitespace — this separates the URL
    ///     from the title. If no whitespace is found, there is no title.
    ///  4. For **paired** delimiters (`(…)`), if the first candidate opener
    ///     lacks preceding whitespace we keep scanning for an earlier `(`
    ///     that does. For **same-char** delimiters (`"…"`, `'…'`), the first
    ///     match is the only candidate (no nesting possible).
    fn split_url_title(content: &'src str) -> (&'src str, Option<&'src str>) {
        let trimmed = content.trim();
        // A valid title needs at minimum: url, space, open+close quotes (e.g. `u "t"`).
        // With fewer than 3 bytes the backward scan would underflow.
        if trimmed.len() < 3 {
            return (trimmed, None);
        }

        let bytes = trimmed.as_bytes();
        let last = bytes[bytes.len() - 1];
        let (open, close) = match SpecialChar::from_byte(last) {
            Some(SpecialChar::DoubleQuote) => (SpecialChar::DoubleQuote, SpecialChar::DoubleQuote),
            Some(SpecialChar::SingleQuote) => (SpecialChar::SingleQuote, SpecialChar::SingleQuote),
            Some(SpecialChar::CloseParen) => (SpecialChar::OpenParen, SpecialChar::CloseParen),
            // No trailing title delimiter — the entire content is the URL.
            _ => return (trimmed, None),
        };

        // Scan backwards for the matching opening delimiter.
        let mut j = bytes.len() - 2;
        loop {
            if bytes[j] == open {
                // Whitespace before the opener separates URL from title.
                if j > 0 && bytes[j - 1].is_ascii_whitespace() {
                    let url = trimmed[..j].trim_end();
                    let title = &trimmed[j + 1..bytes.len() - 1];
                    return (url, Some(title));
                }
                // For paired delimiters (open != close), keep scanning for an
                // earlier opener that *does* have preceding whitespace.
                if open != close {
                    if j == 0 {
                        break;
                    }
                    j -= 1;
                    continue;
                }
                // Same-char delimiter: first match is the only candidate.
                break;
            }
            if j == 0 {
                break;
            }
            j -= 1;
        }

        // No valid title found — treat entire content as URL.
        (trimmed, None)
    }

    #[inline]
    /// Classify the character before a position for flanking delimiter rules.
    /// Returns `CharClass::Whitespace` at start-of-input (treated as if preceded by newline).
    fn char_class_before(bytes: &[u8], pos: usize) -> CharClass {
        if pos == 0 {
            return CharClass::Whitespace;
        }
        let b = bytes[pos - 1];
        // Fast path: ASCII bytes need no UTF-8 decoding.
        if b < 0x80 {
            return CharClass::of_ascii(b);
        }
        // Walk back to find UTF-8 codepoint start.
        let mut start = pos - 1;
        while start > 0 && bytes[start] & 0xC0 == 0x80 {
            start -= 1;
        }
        let ch = std::str::from_utf8(&bytes[start..pos])
            .ok()
            .and_then(|s| s.chars().next())
            .unwrap_or(' ');
        CharClass::of(ch)
    }

    #[inline]
    /// Classify the character after a position for flanking delimiter rules.
    /// Returns `CharClass::Whitespace` at end-of-input (treated as if followed by newline).
    fn char_class_after(bytes: &[u8], pos: usize) -> CharClass {
        if pos >= bytes.len() {
            return CharClass::Whitespace;
        }
        let b = bytes[pos];
        // Fast path: ASCII bytes need no UTF-8 decoding.
        if b < 0x80 {
            return CharClass::of_ascii(b);
        }
        // Decode the UTF-8 codepoint starting at `pos`.
        let ch = std::str::from_utf8(&bytes[pos..])
            .ok()
            .and_then(|s| s.chars().next())
            .unwrap_or(' ');
        CharClass::of(ch)
    }

    fn try_parse_delimited(
        input: &'src str,
        bytes: &[u8],
        start: usize,
        marker: u8,
        count: usize,
    ) -> Option<(&'src str, usize)> {
        let inner_start = start + count;
        bytes.get(inner_start)?;

        let is_star = marker == SpecialChar::Asterisk;

        // CommonMark §6.2 — emphasis flanking rules:
        // A left-flanking delimiter run must not be followed by whitespace,
        // and must not be followed by punctuation unless preceded by whitespace
        // or punctuation. For `_`, it must also not be right-flanking (unless
        // preceded by punctuation), preventing intra-word emphasis.
        let before_open = Self::char_class_before(bytes, start);
        let after_open = Self::char_class_after(bytes, inner_start);

        let left_flanking = after_open != CharClass::Whitespace
            && (after_open != CharClass::Punctuation || before_open != CharClass::Other);
        if !left_flanking {
            return None;
        }
        if !is_star {
            // _ can open only if left-flanking AND (not right-flanking OR preceded by punctuation)
            let right_flanking_open = before_open != CharClass::Whitespace
                && (before_open != CharClass::Punctuation || after_open != CharClass::Other);
            if right_flanking_open && before_open != CharClass::Punctuation {
                return None;
            }
        }

        // Select pre-computed static ByteSet instead of building one each call.
        let delim_set = if is_star {
            &STAR_DELIM_SET
        } else {
            &UNDER_DELIM_SET
        };

        let mut i = inner_start;
        loop {
            let Some(pos) = find_byte_set(bytes, i, delim_set) else {
                break;
            };
            i = pos;
            let b = bytes[i];

            if b == SpecialChar::Backslash && bytes.get(i + 1).is_some_and(u8::is_ascii_punctuation)
            {
                i += 2;
                continue;
            }

            if b != marker {
                i += 1;
                continue;
            }

            // Found a marker byte — check for a valid closing run.
            let all_match = (1..count).all(|j| bytes.get(i + j) == Some(&marker));
            if !all_match {
                i += 1;
                continue;
            }

            let close_end = i + count;
            let before_close = Self::char_class_before(bytes, i);
            let after_close = Self::char_class_after(bytes, close_end);

            // CommonMark §6.2 — closing delimiter must be right-flanking:
            // not preceded by whitespace, and not preceded by punctuation
            // unless followed by whitespace or punctuation. For `_`, must
            // also not be left-flanking (unless followed by punctuation).
            let right_flanking = before_close != CharClass::Whitespace
                && (before_close != CharClass::Punctuation || after_close != CharClass::Other);
            if !right_flanking {
                i += 1;
                continue;
            }
            if !is_star {
                // _ can close only if right-flanking AND (not left-flanking OR followed by punctuation)
                let left_flanking_close = after_close != CharClass::Whitespace
                    && (after_close != CharClass::Punctuation || before_close != CharClass::Other);
                if left_flanking_close && after_close != CharClass::Punctuation {
                    i += 1;
                    continue;
                }
            }

            return Some((input.get(inner_start..i)?, close_end));
        }

        None
    }

    /// Parse inline code spans (`CommonMark` §6.1).
    /// The opening and closing backtick sequences must have the same length.
    /// Content is taken verbatim (no backslash escaping inside code spans).
    fn try_parse_inline_code(
        input: &'src str,
        bytes: &[u8],
        start: usize,
    ) -> Option<(&'src str, usize)> {
        let backtick = SpecialChar::Backtick.byte();
        let mut backtick_count = 0;
        while bytes.get(start + backtick_count).copied() == Some(backtick) {
            backtick_count += 1;
        }
        if backtick_count == 0 {
            return None;
        }

        let content_start = start + backtick_count;
        let mut i = content_start;
        while i < bytes.len() {
            // SIMD-accelerated backtick scan.
            i = find_byte(bytes, i, backtick)?;

            // Count consecutive backticks
            let mut close_count = 0;
            while bytes.get(i + close_count).copied() == Some(backtick) {
                close_count += 1;
            }

            if close_count == backtick_count {
                // CommonMark §6.1: strip one leading and one trailing space
                // when the content both starts and ends with a space.
                let mut cs = content_start;
                let mut ce = i;
                if ce - cs >= 2
                    && bytes.get(cs) == SpecialChar::Space
                    && bytes.get(ce - 1) == SpecialChar::Space
                {
                    cs += 1;
                    ce -= 1;
                }
                return Some((input.get(cs..ce)?, i + close_count));
            }
            i += close_count;
        }

        None
    }
}
