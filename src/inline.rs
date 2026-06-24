use std::mem::MaybeUninit;

use crate::OffsetExt;
use crate::SpecialChar;
use crate::link_def::{LinkDefs, normalize_label};
use crate::section::InlineSpan;
use crate::simd::{ByteSet, ByteSliceExt};

/// An inline element within a Markdown block.
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
    /// An autolink `<uri>` or `<email>` (`CommonMark` §6.5). `is_email`
    /// distinguishes the two so the renderer can add the `mailto:` scheme.
    Autolink {
        target: &'src str,
        is_email: bool,
    },
    /// A raw inline HTML tag/comment/etc. (`CommonMark` §6.6), emitted verbatim.
    RawHtml(&'src str),
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
    SpecialChar::LessThan.byte(),
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
        } else if ch.is_ascii_punctuation() || Self::unicode_punctuation(ch) {
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

    /// Returns `true` for Unicode punctuation/symbol characters beyond ASCII.
    /// Covers general categories P and S without an external crate.
    const fn unicode_punctuation(ch: char) -> bool {
        if ch.is_ascii() {
            return false;
        }
        matches!(ch,
            '\u{00A1}'..='\u{00BF}' // Latin punctuation/symbols
            | '\u{2010}'..='\u{2027}' // General punctuation
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
            2 => Self::BoldOnly,
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
        while let Some(pos) = bytes.find_byte_set(i, &EMPH_SET) {
            if bytes[pos] == SpecialChar::Asterisk {
                stars = stars.saturating_add(1);
            } else {
                unders = unders.saturating_add(1);
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

/// Stack-allocated buffer for collecting inline elements without heap allocation.
/// Uses `MaybeUninit` to avoid zeroing the stack array on every parse call.
/// The capacity `CAP` is configurable via `MarkdownFile`'s `INLINE_STACK_CAP`
/// const generic — falls back to heap if exceeded.
struct InlineBuf<'src, const CAP: usize> {
    stack: [MaybeUninit<Inline<'src>>; CAP],
    len: usize,
    overflow: Vec<Inline<'src>>,
}

impl<'src, const CAP: usize> InlineBuf<'src, CAP> {
    #[inline]
    const fn new() -> Self {
        Self {
            // SAFETY: An array of MaybeUninit does not require initialization.
            stack: [const { MaybeUninit::uninit() }; CAP],
            len: 0,
            overflow: Vec::new(),
        }
    }

    #[allow(clippy::inline_always)]
    #[inline(always)]
    fn push(&mut self, item: Inline<'src>) {
        if self.len < CAP {
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
            self.overflow = Vec::with_capacity(CAP * 2);
            // SAFETY: elements 0..self.len were initialized via push.
            // Use a raw pointer to avoid borrow conflict with self.overflow.
            let len = self.len;
            let ptr = self.stack.as_ptr().cast::<Inline>();
            let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
            self.overflow.extend_from_slice(slice);
        }
        self.overflow.push(item);
    }

    /// Drop the last element if it is a soft or hard line break. Used to
    /// discard a trailing break at the end of a paragraph (`CommonMark` strips
    /// trailing whitespace, so `foo  \n` renders as `foo`, not `foo<br />`).
    fn pop_trailing_break(&mut self) {
        if !self.overflow.is_empty() {
            if matches!(
                self.overflow.last(),
                Some(Inline::SoftBreak | Inline::HardBreak)
            ) {
                self.overflow.pop();
            }
        } else if self.len > 0 {
            // SAFETY: element len-1 was initialized via push.
            let last = unsafe { self.stack[self.len - 1].assume_init_ref() };
            if matches!(last, Inline::SoftBreak | Inline::HardBreak) {
                self.len -= 1;
            }
        }
    }

    /// Get initialized stack elements as a slice.
    #[inline]
    const fn initialized_stack(&self) -> &[Inline<'src>] {
        // SAFETY: all elements 0..self.len have been initialized via push.
        unsafe { std::slice::from_raw_parts(self.stack.as_ptr().cast::<Inline>(), self.len) }
    }

    #[inline]
    fn flush_to_pool(self, pool: &mut Vec<Inline<'src>>) -> InlineSpan {
        let start = pool.len().pool_offset();
        if self.overflow.is_empty() {
            pool.extend_from_slice(self.initialized_stack());
            InlineSpan::new(start, self.len.pool_offset())
        } else {
            let len = self.overflow.len().pool_offset();
            pool.extend(self.overflow);
            InlineSpan::new(start, len)
        }
    }
}

/// Threshold below which the emphasis pre-scan costs more than it saves.
const EMPH_SCAN_THRESHOLD: usize = 256;

/// Stateful parser for a single inline parse pass.
///
/// Holds the input slice and a mutable reference to the output pool so that
/// recursive/nested parsing can share the same pool without threading the
/// pool through every helper.
pub struct InlineParser<'src, 'pool, const MAX_DEPTH: u8, const CAP: usize> {
    input: &'src str,
    pool: &'pool mut Vec<Inline<'src>>,
    defs: &'pool LinkDefs<'src>,
    /// When true, strip leading whitespace at each line start and trailing
    /// whitespace at each line end (`CommonMark` §4.8 / §6.8). Enabled for
    /// paragraph bodies, where joined lines are reflowed; disabled for nested
    /// contexts like link text and image alt, which preserve whitespace.
    strip_lines: bool,
}

impl<'src, 'pool, const MAX_DEPTH: u8, const CAP: usize> InlineParser<'src, 'pool, MAX_DEPTH, CAP> {
    const fn new(
        input: &'src str,
        pool: &'pool mut Vec<Inline<'src>>,
        defs: &'pool LinkDefs<'src>,
    ) -> Self {
        Self {
            input,
            pool,
            defs,
            strip_lines: false,
        }
    }

    /// Parse inline elements with configurable depth and stack limits.
    pub(crate) fn parse_configured(
        input: &'src str,
        pool: &'pool mut Vec<Inline<'src>>,
        defs: &'pool LinkDefs<'src>,
    ) -> InlineSpan {
        Self::new(input, pool, defs).parse()
    }

    /// Like [`parse_configured`](Self::parse_configured), but treats the input
    /// as a paragraph body: leading whitespace at each line start and trailing
    /// whitespace at each line end are stripped (`CommonMark` reflows joined
    /// paragraph lines).
    pub(crate) fn parse_paragraph_configured(
        input: &'src str,
        pool: &'pool mut Vec<Inline<'src>>,
        defs: &'pool LinkDefs<'src>,
    ) -> InlineSpan {
        let mut parser = Self::new(input, pool, defs);
        parser.strip_lines = true;
        parser.parse()
    }

    /// Parse a sequence of paragraph lines (joined by soft breaks) into one
    /// span. Each line is parsed into a shared [`InlineBuf`] so emphasis bodies
    /// land in the pool *below* the top-level run — the span returned covers
    /// only the top-level elements, never their nested children.
    pub(crate) fn parse_lines_configured(
        lines: &[&'src str],
        pool: &'pool mut Vec<Inline<'src>>,
        defs: &'pool LinkDefs<'src>,
    ) -> InlineSpan {
        let mut buf = InlineBuf::<CAP>::new();
        for (idx, line) in lines.iter().enumerate() {
            if idx > 0 {
                buf.push(Inline::SoftBreak);
            }
            let bytes = line.as_bytes();
            if bytes.find_byte_set(0, &SPECIAL_SET).is_none() {
                if !line.is_empty() {
                    buf.push(Inline::Text(line));
                }
                continue;
            }
            let emph = if bytes.len() < EMPH_SCAN_THRESHOLD {
                EmphasisState::assume_both()
            } else {
                EmphasisState::from_bytes(bytes)
            };
            // Reborrow the pool for just this line so the next iteration (and
            // the final flush) can borrow it again.
            let mut parser: InlineParser<'src, '_, MAX_DEPTH, CAP> =
                InlineParser::new(line, &mut *pool, defs);
            parser.parse_into_buf(bytes, emph, &mut buf, 0);
        }
        buf.flush_to_pool(pool)
    }

    /// Parse inline elements and store them in the pool. Returns a span.
    ///
    /// Uses default limits (`MAX_INLINE_DEPTH = 16`, `INLINE_STACK_CAP = 32`).
    /// For custom limits, use [`crate::MarkdownFile::parse`] with const generics.
    #[must_use]
    fn parse(&mut self) -> InlineSpan {
        self.parse_at_depth(0)
    }

    fn parse_at_depth(&mut self, depth: u8) -> InlineSpan {
        let bytes = self.input.as_bytes();
        // Fast path: if no special bytes exist, the entire input is plain text.
        if bytes.find_byte_set(0, &SPECIAL_SET).is_none() {
            // A single line with no newline: paragraph reflow trims both ends.
            let text = if self.strip_lines {
                self.input.trim_matches([' ', '\t'])
            } else {
                self.input
            };
            if text.is_empty() {
                return InlineSpan::EMPTY;
            }
            let start = self.pool.len().pool_offset();
            self.pool.push(Inline::Text(text));
            return InlineSpan::new(start, 1);
        }
        let emph = if bytes.len() < EMPH_SCAN_THRESHOLD {
            EmphasisState::assume_both()
        } else {
            EmphasisState::from_bytes(bytes)
        };
        let mut buf = InlineBuf::<CAP>::new();
        self.parse_into_buf(bytes, emph, &mut buf, depth);
        // A paragraph never ends with a dangling break (trailing whitespace is
        // stripped, so `foo  \n` is `foo`, not `foo<br />`).
        if self.strip_lines {
            buf.pop_trailing_break();
        }
        buf.flush_to_pool(self.pool)
    }

    /// Skip leading spaces/tabs from `p` (a line start). Used for paragraph
    /// reflow, where each continuation line's indentation is removed.
    #[inline]
    fn skip_line_leading_ws(bytes: &[u8], mut p: usize) -> usize {
        while matches!(bytes.get(p), Some(b' ' | b'\t')) {
            p += 1;
        }
        p
    }

    fn parse_inner(&mut self, input: &'src str, depth: u8) -> InlineSpan {
        InlineParser::<MAX_DEPTH, CAP> {
            input,
            pool: self.pool,
            defs: self.defs,
            // Nested contexts (link text, image alt, emphasis bodies) preserve
            // whitespace; only top-level paragraph reflow strips it.
            strip_lines: false,
        }
        .parse_at_depth(depth)
    }

    #[allow(clippy::too_many_lines)]
    fn parse_into_buf(
        &mut self,
        bytes: &[u8],
        mut emph: EmphasisState,
        buf: &mut InlineBuf<'src, CAP>,
        depth: u8,
    ) {
        // Paragraph reflow strips indentation at the start of the first line.
        let mut plain_start = if self.strip_lines {
            Self::skip_line_leading_ws(bytes, 0)
        } else {
            0
        };
        let mut i = plain_start;

        // SIMD-accelerated scan: find next special byte.
        while let Some(pos) = bytes.find_byte_set(i, &SPECIAL_SET) {
            i = pos;
            let b = bytes[i];

            if b == SpecialChar::Newline {
                self.emit_line_break(bytes, plain_start, i, buf);
                plain_start = i + 1;
                // Paragraph reflow strips indentation at each line start.
                if self.strip_lines {
                    plain_start = Self::skip_line_leading_ws(bytes, plain_start);
                }
                i = plain_start;
                continue;
            }

            // Backslash escape: only ASCII punctuation can be escaped (CommonMark spec).
            // For non-punctuation, the backslash is kept as literal text.
            if b == SpecialChar::Backslash
                && let Some(&next) = bytes.get(i + 1)
                && next.is_ascii_punctuation()
            {
                if let Some(text) = self.input.get(plain_start..i)
                    && !text.is_empty()
                {
                    buf.push(Inline::Text(text));
                }
                plain_start = i + 1;
                i += 2;
                continue;
            }

            // Inline code: `code` or ``code``
            if b == SpecialChar::Backtick
                && let Some((code, end)) = Self::try_parse_inline_code(self.input, bytes, i)
            {
                if let Some(text) = self.input.get(plain_start..i)
                    && !text.is_empty()
                {
                    buf.push(Inline::Text(text));
                }
                buf.push(Inline::Code(code));
                plain_start = end;
                i = end;
                continue;
            }

            // Image: ![alt](url "title") or reference ![alt][label]
            if b == SpecialChar::ExclamationMark
                && bytes.get(i + 1) == SpecialChar::OpenBracket
                && let Some((alt, url, title, end)) =
                    Self::try_parse_bracket_paren(self.input, bytes, i + 1)
            {
                if let Some(text) = self.input.get(plain_start..i)
                    && !text.is_empty()
                {
                    buf.push(Inline::Text(text));
                }
                buf.push(Inline::Image { alt, url, title });
                plain_start = end;
                i = end;
                continue;
            }
            if b == SpecialChar::ExclamationMark
                && bytes.get(i + 1) == SpecialChar::OpenBracket
                && let Some((text_str, url, title, end)) = self.try_parse_reference(bytes, i + 1)
            {
                if let Some(text) = self.input.get(plain_start..i)
                    && !text.is_empty()
                {
                    buf.push(Inline::Text(text));
                }
                buf.push(Inline::Image {
                    alt: text_str,
                    url,
                    title,
                });
                plain_start = end;
                i = end;
                continue;
            }

            // Link: [text](url "title")
            if b == SpecialChar::OpenBracket
                && let Some((text_str, url, title, end)) =
                    Self::try_parse_bracket_paren(self.input, bytes, i)
            {
                if let Some(text) = self.input.get(plain_start..i)
                    && !text.is_empty()
                {
                    buf.push(Inline::Text(text));
                }
                let text_span = self.parse_inner(text_str, depth.saturating_add(1));
                buf.push(Inline::Link {
                    text: text_span,
                    url,
                    title,
                });
                plain_start = end;
                i = end;
                continue;
            }

            // Reference link: [text][label], [label][], or [label]
            if b == SpecialChar::OpenBracket
                && let Some((text_str, url, title, end)) = self.try_parse_reference(bytes, i)
            {
                if let Some(text) = self.input.get(plain_start..i)
                    && !text.is_empty()
                {
                    buf.push(Inline::Text(text));
                }
                let text_span = self.parse_inner(text_str, depth.saturating_add(1));
                buf.push(Inline::Link {
                    text: text_span,
                    url,
                    title,
                });
                plain_start = end;
                i = end;
                continue;
            }

            // Autolinks `<uri>` / `<email>` and raw inline HTML (§6.5, §6.6).
            if b == SpecialChar::LessThan
                && let Some((elem, end)) = Self::try_parse_angle(self.input, bytes, i)
            {
                if let Some(text) = self.input.get(plain_start..i)
                    && !text.is_empty()
                {
                    buf.push(Inline::Text(text));
                }
                buf.push(elem);
                plain_start = end;
                i = end;
                continue;
            }

            // Bold/Italic: ** __ * _
            if let Some((elem, end)) = self.try_parse_emphasis(bytes, i, b, &mut emph, depth) {
                if let Some(text) = self.input.get(plain_start..i)
                    && !text.is_empty()
                {
                    buf.push(Inline::Text(text));
                }
                buf.push(elem);
                plain_start = end;
                i = end;
                continue;
            }

            i += 1;
        }

        // Trailing segment: paragraph reflow strips whitespace at the end of
        // the final line.
        let tail = if self.strip_lines {
            self.input.get(plain_start..).map(|t| t.trim_end_matches([' ', '\t']))
        } else {
            self.input.get(plain_start..)
        };
        if let Some(text) = tail
            && !text.is_empty()
        {
            buf.push(Inline::Text(text));
        }
    }

    /// Emit a hard or soft line break at a newline position.
    /// Hard break if preceded by trailing `\` or 2+ spaces; soft break otherwise.
    #[inline]
    fn emit_line_break(
        &self,
        bytes: &[u8],
        plain_start: usize,
        newline_pos: usize,
        buf: &mut InlineBuf<'src, CAP>,
    ) {
        let preceding = bytes.get(plain_start..newline_pos).unwrap_or_default();
        let (mut trim_end, is_hard) = if preceding.last() == SpecialChar::Backslash {
            (newline_pos - 1, true)
        } else {
            // Count trailing spaces with a simple backward loop.
            let mut spaces = 0;
            let mut j = preceding.len();
            while j > 0 && preceding[j - 1] == SpecialChar::Space {
                spaces += 1;
                j -= 1;
            }
            if spaces >= 2 {
                (newline_pos - spaces, true)
            } else {
                (newline_pos, false)
            }
        };
        // Paragraph reflow strips trailing whitespace from each line, even the
        // single trailing space that would otherwise survive a soft break.
        if self.strip_lines {
            while trim_end > plain_start
                && matches!(bytes.get(trim_end - 1), Some(b' ' | b'\t'))
            {
                trim_end -= 1;
            }
        }
        if let Some(text) = self.input.get(plain_start..trim_end)
            && !text.is_empty()
        {
            buf.push(Inline::Text(text));
        }
        buf.push(if is_hard {
            Inline::HardBreak
        } else {
            Inline::SoftBreak
        });
    }

    #[inline]
    fn try_parse_emphasis(
        &mut self,
        bytes: &[u8],
        i: usize,
        b: u8,
        emph: &mut EmphasisState,
        depth: u8,
    ) -> Option<(Inline<'src>, usize)> {
        let is_star = b == SpecialChar::Asterisk;
        if !is_star && b != SpecialChar::Underscore {
            return None;
        }
        // Depth limit: treat as plain text to prevent stack overflow.
        if depth >= MAX_DEPTH {
            return None;
        }
        let avail = emph.avail_mut(is_star);
        let open_run = if is_star {
            SpecialChar::Asterisk.count_leading_bytes(&bytes[i..])
        } else {
            SpecialChar::Underscore.count_leading_bytes(&bytes[i..])
        };

        // Triple runs (*** or ___) can open/close both emphasis and strong
        // emphasis. Match them as strong nested inside emphasis so that
        // ***text*** becomes Italic(Bold(text)) instead of being split.
        if open_run >= 3 && avail.can_bold() && avail.can_italic() {
            if let Some((inner, end)) = Self::try_parse_delimited(self.input, bytes, i, b, 3) {
                // Only match when the closing run is exactly three characters,
                // leaving longer runs (e.g. ****text****) to the strong/italic
                // logic below.
                let close_run_start = end - 3;
                let exact_close =
                    bytes.get(close_run_start - 1) != Some(&b) && bytes.get(end) != Some(&b);
                if exact_close {
                    let inner_span = self.parse_inner(inner, depth + 1);
                    let bold_start = self.pool.len().pool_offset();
                    self.pool.push(Inline::Bold(inner_span));
                    let bold_span = InlineSpan::new(bold_start, 1);
                    return Some((Inline::Italic(bold_span), end));
                }
            }
            // A triple run exists but couldn't be matched; strong and italic
            // may still succeed from the same starting position.
        }

        // Bold: ** or __
        if avail.can_bold() && bytes.get(i + 1) == Some(&b) {
            if let Some((inner, end)) = Self::try_parse_delimited(self.input, bytes, i, b, 2) {
                let span = self.parse_inner(inner, depth + 1);
                return Some((Inline::Bold(span), end));
            }
            avail.bold_failed();
        }

        // Italic: * or _
        if avail.can_italic() {
            if let Some((inner, end)) = Self::try_parse_delimited(self.input, bytes, i, b, 1) {
                let span = self.parse_inner(inner, depth + 1);
                return Some((Inline::Italic(span), end));
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
        let mut nested = 0u32;
        let mut j = start;
        loop {
            let pos = bytes.find_byte_set(j, set)?;
            let b = bytes[pos];
            if b == SpecialChar::Backslash
                && bytes.get(pos + 1).is_some_and(u8::is_ascii_punctuation)
            {
                j = pos + 2;
                continue;
            }
            if b == open {
                nested += 1;
            } else if b == close {
                if nested == 0 {
                    return Some(pos);
                }
                nested -= 1;
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

    /// Try to parse a reference link/image at `start` (the `[`). Handles all
    /// three `CommonMark` §6.3 reference forms:
    ///  - full:      `[text][label]`
    ///  - collapsed: `[label][]`
    ///  - shortcut:  `[label]`
    ///
    /// Returns `(text_to_render, url, title, resume)` when the label resolves
    /// against the definition registry. `text_to_render` is the bracket text
    /// (for full references) or the label text itself (collapsed/shortcut).
    fn try_parse_reference(
        &self,
        bytes: &[u8],
        start: usize,
    ) -> Option<(&'src str, &'src str, Option<&'src str>, usize)> {
        // Every reference form requires a matching definition; if the registry
        // is empty there is nothing to resolve, so skip the bracket scanning.
        if self.defs.is_empty() {
            return None;
        }
        if bytes.get(start) != SpecialChar::OpenBracket {
            return None;
        }
        let first_start = start + 1;
        let first_end = Self::find_matching_close(
            bytes,
            first_start,
            SpecialChar::OpenBracket,
            SpecialChar::CloseBracket,
        )?;
        let first_text = self.input.get(first_start..first_end)?;

        // Is there a second bracket pair `[...]` immediately after?
        let after_first = first_end + 1;
        if bytes.get(after_first) == SpecialChar::OpenBracket {
            let second_start = after_first + 1;
            let second_end = Self::find_matching_close(
                bytes,
                second_start,
                SpecialChar::OpenBracket,
                SpecialChar::CloseBracket,
            )?;
            let second_text = self.input.get(second_start..second_end)?;
            if second_text.trim().is_empty() {
                // Collapsed reference `[label][]`: label is the first text.
                let (url, title) = self.lookup(first_text)?;
                return Some((first_text, url, title, second_end + 1));
            }
            // Full reference `[text][label]`: label is the second text.
            let (url, title) = self.lookup(second_text)?;
            return Some((first_text, url, title, second_end + 1));
        }

        // Shortcut reference `[label]`.
        let (url, title) = self.lookup(first_text)?;
        Some((first_text, url, title, after_first))
    }

    /// Look up a label in the definition registry after normalization.
    ///
    /// Fast-exits when there are no definitions (the common case), avoiding the
    /// allocation + lowercasing that `normalize_label` performs.
    fn lookup(&self, label: &str) -> Option<(&'src str, Option<&'src str>)> {
        if self.defs.is_empty() {
            return None;
        }
        self.defs.get(&normalize_label(label)).copied()
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
                    let url = trimmed.get(..j).unwrap_or(trimmed).trim_end();
                    let title = trimmed.get(j + 1..bytes.len() - 1).unwrap_or("");
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
        while let Some(pos) = bytes.find_byte_set(i, delim_set) {
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

    /// Parse an angle-bracket construct at `start` (a `<`): an absolute-URI
    /// autolink, an email autolink, or a raw inline HTML tag. Returns the
    /// parsed inline and the index just past it.
    fn try_parse_angle(
        input: &'src str,
        bytes: &[u8],
        start: usize,
    ) -> Option<(Inline<'src>, usize)> {
        // Autolink: <scheme:...> with no spaces or `<`, scheme 2-32 chars.
        if let Some(close) = Self::scan_autolink_uri(bytes, start) {
            let target = input.get(start + 1..close)?;
            return Some((
                Inline::Autolink {
                    target,
                    is_email: false,
                },
                close + 1,
            ));
        }
        // Email autolink.
        if let Some(close) = Self::scan_autolink_email(bytes, start) {
            let target = input.get(start + 1..close)?;
            return Some((
                Inline::Autolink {
                    target,
                    is_email: true,
                },
                close + 1,
            ));
        }
        // Raw inline HTML.
        if let Some(len) = crate::raw_html::scan_inline_html(&bytes[start..]) {
            let html = input.get(start..start + len)?;
            return Some((Inline::RawHtml(html), start + len));
        }
        None
    }

    /// Scan an absolute-URI autolink body. Returns the index of the closing
    /// `>` if `bytes[start..]` is `<scheme:chars>` per `CommonMark` §6.5.
    fn scan_autolink_uri(bytes: &[u8], start: usize) -> Option<usize> {
        let mut i = start + 1;
        // Scheme: ASCII letter then 1-31 of [A-Za-z0-9+.-], total 2-32.
        if !bytes.get(i).is_some_and(u8::is_ascii_alphabetic) {
            return None;
        }
        i += 1;
        let scheme_start = start + 1;
        while bytes.get(i).is_some_and(|&b| {
            b.is_ascii_alphanumeric() || b == b'+' || b == b'.' || b == b'-'
        }) {
            i += 1;
        }
        let scheme_len = i - scheme_start;
        if bytes.get(i) != Some(&b':') || !(2..=32).contains(&scheme_len) {
            return None;
        }
        i += 1;
        // Body: no whitespace, no `<`, up to `>`.
        while let Some(&b) = bytes.get(i) {
            match b {
                b'>' => return Some(i),
                b'<' => return None,
                _ if b.is_ascii_whitespace() => return None,
                _ => i += 1,
            }
        }
        None
    }

    /// Scan an email autolink body per the `CommonMark` §6.5 email regex.
    fn scan_autolink_email(bytes: &[u8], start: usize) -> Option<usize> {
        let mut i = start + 1;
        let local_start = i;
        while bytes.get(i).is_some_and(|&b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'.' | b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+'
                        | b'/' | b'=' | b'?' | b'^' | b'_' | b'`' | b'{' | b'|'
                        | b'}' | b'~' | b'-'
                )
        }) {
            i += 1;
        }
        if i == local_start || bytes.get(i) != Some(&b'@') {
            return None;
        }
        i += 1;
        // One or more dot-separated labels of [A-Za-z0-9-] (max 63, no leading/
        // trailing hyphen). We keep it permissive but require at least one.
        loop {
            let label_start = i;
            if !bytes.get(i).is_some_and(u8::is_ascii_alphanumeric) {
                return None;
            }
            while bytes
                .get(i)
                .is_some_and(|&b| b.is_ascii_alphanumeric() || b == b'-')
            {
                i += 1;
            }
            // No trailing hyphen.
            if bytes.get(i - 1) == Some(&b'-') {
                return None;
            }
            let _ = label_start;
            match bytes.get(i) {
                Some(&b'.') => i += 1,
                Some(&b'>') => return Some(i),
                _ => return None,
            }
        }
    }

    /// Parse inline code spans (`CommonMark` §6.1).
    /// The opening and closing backtick sequences must have the same length.
    /// Content is taken verbatim (no backslash escaping inside code spans).
    fn try_parse_inline_code(
        input: &'src str,
        bytes: &[u8],
        start: usize,
    ) -> Option<(&'src str, usize)> {
        let backtick_count = SpecialChar::Backtick.count_leading_bytes(&bytes[start..]);
        if backtick_count == 0 {
            return None;
        }

        let content_start = start + backtick_count;
        let mut i = content_start;
        while i < bytes.len() {
            // SIMD-accelerated backtick scan.
            i = bytes.find_byte(i, SpecialChar::Backtick.byte())?;

            // Count consecutive backticks
            let close_count = SpecialChar::Backtick.count_leading_bytes(&bytes[i..]);

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
