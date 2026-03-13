use crate::section::{InlineSpan, OrderedListDelimiter, Section, SpanSlice};
use crate::simd::find_byte;
use crate::special_char::SpecialChar;
use crate::inline::pool_offset;
use crate::{Inline, MarkdownFile};

// ---------------------------------------------------------------------------
// Pass 1: block-level parsing into RawSection (no inline parsing)
// ---------------------------------------------------------------------------

/// Intermediate section representation produced by pass 1 (block parsing).
/// Stores raw `&str` text that will be inline-parsed in pass 2.
enum RawSection<'src> {
    Heading {
        level: u8,
        text: &'src str,
    },
    Paragraph {
        text: &'src str,
    },
    CodeBlock {
        language: Option<&'src str>,
        code: &'src str,
    },
    UnorderedList {
        items_start: u32,
        items_len: u32,
    },
    OrderedList {
        start: u32,
        delimiter: OrderedListDelimiter,
        items_start: u32,
        items_len: u32,
    },
    Blockquote {
        lines_start: u32,
        lines_len: u32,
    },
    HorizontalRule,
}

/// Mutable parsing context for pass 1. Only collects raw sections — no inline
/// pool or span pool needed.
struct ParseCtx<'src> {
    input: &'src str,
    bytes: &'src [u8],
    sections: Vec<RawSection<'src>>,
    /// Shared pool for blockquote lines and list items, avoiding per-section
    /// `Vec<&str>` heap allocations.
    lines: Vec<&'src str>,
}

enum Accumulator<'src> {
    Empty,
    InBlockquote {
        lines_start: u32,
    },
    InUnorderedList {
        marker: SpecialChar,
        items_start: u32,
    },
    InOrderedList {
        start: u32,
        delimiter: OrderedListDelimiter,
        items_start: u32,
    },
    InParagraph {
        content: &'src str,
    },
}

impl<'src> Accumulator<'src> {
    const fn flush(self, lines_pool_len: u32) -> Option<RawSection<'src>> {
        match self {
            Self::Empty => None,
            Self::InBlockquote { lines_start } => Some(RawSection::Blockquote {
                lines_start,
                lines_len: lines_pool_len - lines_start,
            }),
            Self::InUnorderedList {
                items_start,
                ..
            } => Some(RawSection::UnorderedList {
                items_start,
                items_len: lines_pool_len - items_start,
            }),
            Self::InOrderedList {
                start,
                delimiter,
                items_start,
            } => Some(RawSection::OrderedList {
                start,
                delimiter,
                items_start,
                items_len: lines_pool_len - items_start,
            }),
            Self::InParagraph { content } => Some(RawSection::Paragraph { text: content }),
        }
    }

    fn flush_into(self, ctx: &mut ParseCtx<'src>) {
        let pool_len = lines_offset(ctx.lines.len());
        if let Some(section) = self.flush(pool_len) {
            ctx.sections.push(section);
        }
    }
}

/// Lines pool index as `u32`. Panics if the pool exceeds `u32::MAX` elements.
#[allow(clippy::inline_always)]
#[inline(always)]
fn lines_offset(len: usize) -> u32 {
    u32::try_from(len).expect("lines pool exceeds u32::MAX elements")
}

/// Check whether every byte in `bytes[start..end]` is ASCII whitespace.
#[inline]
fn is_blank_line(bytes: &[u8], start: usize, end: usize) -> bool {
    // Most blank lines are truly empty (start == end).
    if start >= end {
        return true;
    }
    bytes[start..end].iter().all(u8::is_ascii_whitespace)
}

// ---------------------------------------------------------------------------
// Pass 2: resolve inlines
// ---------------------------------------------------------------------------

/// Convert raw sections from pass 1 into final sections with inline parsing.
/// Separating passes lets us pre-size the output pools from the raw section
/// count and avoid interleaving block and inline allocation patterns.
fn resolve_inlines<'src, const MAX_DEPTH: u8, const CAP: usize>(
    raw: Vec<RawSection<'src>>,
    lines: &[&'src str],
    pool: &mut Vec<Inline<'src>>,
    span_pool: &mut Vec<InlineSpan>,
) -> Vec<Section<'src>> {
    let mut sections = Vec::with_capacity(raw.len());
    for raw_section in raw {
        match raw_section {
            RawSection::Heading { level, text } => {
                sections.push(Section::Heading {
                    level,
                    content: Inline::parse_configured::<MAX_DEPTH, CAP>(text, pool),
                });
            }
            RawSection::Paragraph { text } => {
                sections.push(Section::Paragraph {
                    content: Inline::parse_configured::<MAX_DEPTH, CAP>(text, pool),
                });
            }
            RawSection::CodeBlock { language, code } => {
                sections.push(Section::CodeBlock { language, code });
            }
            RawSection::UnorderedList {
                items_start,
                items_len,
            } => {
                let raw_items =
                    lines.get(items_start as usize..(items_start + items_len) as usize)
                        .unwrap_or(&[]);
                let start = pool_offset(span_pool.len());
                for item in raw_items {
                    let span = Inline::parse_configured::<MAX_DEPTH, CAP>(item, pool);
                    span_pool.push(span);
                }
                let len = pool_offset(span_pool.len()) - start;
                sections.push(Section::UnorderedList {
                    items: SpanSlice::new(start, len),
                });
            }
            RawSection::OrderedList {
                start,
                delimiter,
                items_start,
                items_len,
            } => {
                let raw_items =
                    lines.get(items_start as usize..(items_start + items_len) as usize)
                        .unwrap_or(&[]);
                let sp_start = pool_offset(span_pool.len());
                for item in raw_items {
                    let span = Inline::parse_configured::<MAX_DEPTH, CAP>(item, pool);
                    span_pool.push(span);
                }
                let sp_len = pool_offset(span_pool.len()) - sp_start;
                sections.push(Section::OrderedList {
                    start,
                    delimiter,
                    items: SpanSlice::new(sp_start, sp_len),
                });
            }
            RawSection::Blockquote {
                lines_start,
                lines_len,
            } => {
                let raw_lines = lines
                    .get(lines_start as usize..(lines_start + lines_len) as usize)
                    .unwrap_or(&[]);
                let start = pool_offset(pool.len());
                for (i, line) in raw_lines.iter().enumerate() {
                    if i > 0 {
                        pool.push(Inline::Text("\n"));
                    }
                    Inline::parse_flat_into_configured::<MAX_DEPTH, CAP>(line, pool);
                }
                let len = pool_offset(pool.len()) - start;
                sections.push(Section::Blockquote {
                    content: InlineSpan::new(start, len),
                });
            }
            RawSection::HorizontalRule => {
                sections.push(Section::HorizontalRule);
            }
        }
    }
    sections
}

// ---------------------------------------------------------------------------
// BlockBytes trait — block-level helpers on byte slices.
// ---------------------------------------------------------------------------

/// Lookup table: true for bytes that could start a block-level element
/// (blockquote, list marker, HR character, or digit for ordered lists).
const COULD_START_BLOCK: [bool; 256] = {
    let mut table = [false; 256];
    table[SpecialChar::GreaterThan.byte() as usize] = true;
    table[SpecialChar::Dash.byte() as usize] = true;
    table[SpecialChar::Asterisk.byte() as usize] = true;
    table[SpecialChar::Plus.byte() as usize] = true;
    table[SpecialChar::Underscore.byte() as usize] = true;
    let mut d = SpecialChar::Zero.byte();
    while d <= b'9' {
        table[d as usize] = true;
        d += 1;
    }
    table
};

trait BlockBytes {
    fn code_fence_len(&self) -> usize;
    fn is_horizontal_rule(&self) -> bool;
    fn try_parse_heading<'src>(&self, input: &'src str, line_offset: usize) -> Option<(u8, &'src str)>;
    fn extract_code_language<'src>(&self, input: &'src str) -> Option<&'src str>;
    fn try_parse_unordered_item(&self) -> Option<(SpecialChar, usize)>;
    fn try_parse_ordered_item(&self) -> Option<(u32, OrderedListDelimiter, usize)>;
    fn could_start_block(&self) -> bool;
}

impl BlockBytes for [u8] {
    /// Returns the fence length (number of backticks) if this line is a
    /// valid code fence (`CommonMark` §4.5), or 0 if not. A valid fence has 3+
    /// backticks with no backticks in the info string.
    #[inline]
    fn code_fence_len(&self) -> usize {
        // Skip leading whitespace.
        let mut first = 0;
        while first < self.len() && self[first].is_ascii_whitespace() {
            first += 1;
        }
        // Quick reject: first non-whitespace byte must be a backtick.
        if self.get(first) != SpecialChar::Backtick {
            return 0;
        }
        // Count backticks directly from the offset we already found.
        let len = SpecialChar::Backtick.count_leading_bytes(&self[first..]);
        if len >= 3 && !self[first + len..].contains(&SpecialChar::Backtick.byte()) {
            len
        } else {
            0
        }
    }

    /// Check whether this line is a thematic break / horizontal rule
    /// (`CommonMark` §4.1): three or more matching `-`, `*`, or `_` characters,
    /// optionally separated by spaces, with nothing else on the line.
    fn is_horizontal_rule(&self) -> bool {
        let mut rule_byte = 0u8;
        let mut count = 0u32;
        for &b in self {
            if b.is_ascii_whitespace() {
                continue;
            }
            if rule_byte == 0 {
                if b != SpecialChar::Dash
                    && b != SpecialChar::Asterisk
                    && b != SpecialChar::Underscore
                {
                    return false;
                }
                rule_byte = b;
            }
            if b != rule_byte {
                return false;
            }
            count += 1;
        }
        count >= 3
    }

    /// Check whether this byte slice is an ATX heading (`CommonMark` §4.2).
    /// Returns `(level, text)` without performing any inline parsing.
    fn try_parse_heading<'src>(&self, input: &'src str, line_offset: usize) -> Option<(u8, &'src str)> {
        let level = SpecialChar::Hash.count_leading_bytes(self);
        if !(1..=6).contains(&level) || self.get(level) != SpecialChar::Space {
            return None;
        }
        // Trim leading whitespace after '#'s.
        let mut start = level;
        while start < self.len() && self[start].is_ascii_whitespace() {
            start += 1;
        }
        let mut end = self.len();
        while end > start && self[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
        // Strip optional closing # sequence per CommonMark §4.2:
        // trailing #s are removed only if preceded by whitespace (or they
        // are the entire content after the opening).
        let mut stripped_end = end;
        while stripped_end > start && self.get(stripped_end - 1) == SpecialChar::Hash {
            stripped_end -= 1;
        }
        if stripped_end == start
            || self.get(stripped_end - 1) == SpecialChar::Space
            || self.get(stripped_end - 1) == SpecialChar::Tab
        {
            // Trim whitespace before the closing hashes.
            end = stripped_end;
            while end > start && self[end - 1].is_ascii_whitespace() {
                end -= 1;
            }
        }
        let text = input.get(line_offset + start..line_offset + end)?;
        let level = u8::try_from(level).expect("heading level already validated 1..=6");
        Some((level, text))
    }

    /// Extract the language tag from a code fence line, returning
    /// a `&str` slice from `input`.
    fn extract_code_language<'src>(&self, input: &'src str) -> Option<&'src str> {
        // Find the start of the info string: skip whitespace then backticks.
        let mut i = 0;
        while self.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        i += SpecialChar::Backtick.count_leading_bytes(&self[i..]);
        // Trim remaining whitespace around the info string.
        while self.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        let mut end = self.len();
        while end > i && self[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
        if i >= end {
            None
        } else {
            // Compute the absolute offset into input. self is a subslice of
            // input.as_bytes(), so pointer arithmetic gives us the offset.
            let line_offset = self.as_ptr() as usize - input.as_ptr() as usize;
            input.get(line_offset + i..line_offset + end)
        }
    }

    /// Try to parse an unordered list item.
    /// Returns `(marker, item_byte_offset)` where offset is relative to line start.
    fn try_parse_unordered_item(&self) -> Option<(SpecialChar, usize)> {
        let &first = self.first()?;
        let marker = SpecialChar::from_byte(first)?;
        if !marker.is_list_char() {
            return None;
        }
        if self.get(1) == SpecialChar::Space {
            Some((marker, 2))
        } else {
            None
        }
    }

    /// Try to parse an ordered list item.
    /// Returns `(number, delimiter, item_byte_offset)` where offset is relative
    /// to line start.
    fn try_parse_ordered_item(&self) -> Option<(u32, OrderedListDelimiter, usize)> {
        let mut num: u32 = 0;
        let mut digits = 0usize;
        for &b in self {
            if b.is_ascii_digit() {
                digits += 1;
                if digits > 9 {
                    return None;
                }
                num = num * 10 + u32::from(b - SpecialChar::Zero.byte());
            } else {
                break;
            }
        }
        if digits == 0 {
            return None;
        }
        let delimiter = OrderedListDelimiter::from_byte(self.get(digits).copied()?)?;
        if self.get(digits + 1) != SpecialChar::Space {
            return None;
        }
        let item_offset = digits + 2;
        if item_offset >= self.len() {
            return None;
        }
        Some((num, delimiter, item_offset))
    }

    /// Check whether the first byte of this line could start a block-level element.
    #[inline]
    fn could_start_block(&self) -> bool {
        self.first().is_some_and(|&b| COULD_START_BLOCK[b as usize])
    }
}

/// Scan forward from `start` to find a closing code fence of at least
/// `fence_len` backticks. Returns `(code_content, resume_position)`.
fn scan_code_block_fast<'src>(
    input: &'src str,
    bytes: &[u8],
    start: usize,
    fence_len: usize,
) -> (&'src str, usize) {
    let mut pos = start;
    while pos < bytes.len() {
        let line_end = find_byte(bytes, pos, SpecialChar::Newline.byte()).unwrap_or(bytes.len());

        // Check for closing fence: first non-whitespace byte must be backtick.
        let first = bytes.get(pos).copied();
        if (first == SpecialChar::Backtick || first.is_some_and(|b| b.is_ascii_whitespace()))
            && bytes[pos..line_end].code_fence_len() >= fence_len
        {
            // Content is everything between opening and closing fence.
            let code = if start < pos {
                input.get(start..pos - 1).unwrap_or("")
            } else {
                ""
            };
            return (code, line_end + 1);
        }
        pos = line_end + 1;
    }
    // Unclosed code block: content runs to end of input.
    let code = input.get(start..).unwrap_or("");
    (code, bytes.len())
}

/// Merge two subslices of `base` into one contiguous slice spanning from the
/// start of `a` to the end of `b`.
fn merge_slices<'src>(base: &'src str, a: &str, b: &str) -> Option<&'src str> {
    let base_start = base.as_ptr() as usize;
    let a_start = a.as_ptr() as usize;
    let b_end = b.as_ptr() as usize + b.len();

    if a_start < base_start || b_end > base_start + base.len() || b_end < a_start {
        return None;
    }

    base.get(a_start - base_start..b_end - base_start)
}

// ---------------------------------------------------------------------------
// MarkdownFile: public API
// ---------------------------------------------------------------------------

impl<'src, const MAX_INLINE_DEPTH: u8, const INLINE_STACK_CAP: usize>
    MarkdownFile<'src, MAX_INLINE_DEPTH, INLINE_STACK_CAP>
{
    #[must_use]
    pub fn parse(input: &'src str) -> Self {
        let bytes = input.as_bytes();

        // --- Pass 1: block-level parsing (no inline work) ---
        let mut ctx = ParseCtx {
            input,
            bytes,
            // Rough heuristic: ~50 bytes per section on average.
            sections: Vec::with_capacity(input.len() / 50 + 1),
            lines: Vec::with_capacity(input.len() / 80 + 1),
        };
        let mut acc = Accumulator::Empty;
        let mut pos = 0;

        while pos < bytes.len() {
            let line_end = find_byte(bytes, pos, SpecialChar::Newline.byte()).unwrap_or(bytes.len());

            // Fast-path: when we detect a code fence opening, scan ahead for
            // the closing fence in one shot instead of processing line-by-line.
            // CommonMark §4.5: a code fence can be indented 0-3 spaces, so we
            // only need to check if a backtick appears within the first 4 bytes.
            let first = bytes.get(pos).copied();
            if first == SpecialChar::Backtick
                || (first == SpecialChar::Space
                    && bytes[pos..line_end]
                        .get(..4)
                        .is_some_and(|w| w.contains(&SpecialChar::Backtick.byte())))
            {
                let fence_len = bytes[pos..line_end].code_fence_len();
                if fence_len > 0 {
                    let language = bytes[pos..line_end].extract_code_language(input);
                    acc.flush_into(&mut ctx);
                    let content_start = line_end + 1;
                    let (code, resume) =
                        scan_code_block_fast(input, bytes, content_start, fence_len);
                    ctx.sections
                        .push(RawSection::CodeBlock { language, code });
                    pos = resume;
                    acc = Accumulator::Empty;
                    continue;
                }
            }

            acc = ctx.fold_line(acc, pos, line_end);
            pos = line_end + 1;
        }

        acc.flush_into(&mut ctx);

        // --- Pass 2: inline parsing ---
        let mut pool = Vec::with_capacity(input.len() / 20);
        let mut span_pool = Vec::with_capacity(input.len() / 100 + 1);
        let sections = resolve_inlines::<MAX_INLINE_DEPTH, INLINE_STACK_CAP>(
            ctx.sections,
            &ctx.lines,
            &mut pool,
            &mut span_pool,
        );

        Self {
            sections,
            pool,
            span_pool,
        }
    }
}

// ---------------------------------------------------------------------------
// ParseCtx methods — pass 1 line-level fold logic
// ---------------------------------------------------------------------------

impl<'src> ParseCtx<'src> {
    /// Process one line given as byte range `[pos..line_end)`.
    /// Operates on `&[u8]` throughout; converts to `&str` only when storing.
    ///
    /// Code fence opening is handled by the fast-path in `parse()` before this
    /// method is called, so no code-block state is tracked here.
    fn fold_line(
        &mut self,
        acc: Accumulator<'src>,
        pos: usize,
        line_end: usize,
    ) -> Accumulator<'src> {
        let first = self.bytes.get(pos).copied();

        if first.is_some_and(|b| b.is_ascii_whitespace()) && is_blank_line(self.bytes, pos, line_end) {
            acc.flush_into(self);
            return Accumulator::Empty;
        }

        // ATX headings (CommonMark §4.2): only if line starts with '#'.
        if first == SpecialChar::Hash
            && let Some((level, text)) = self.bytes[pos..line_end].try_parse_heading(
                self.input,
                pos,
            )
        {
            acc.flush_into(self);
            self.sections.push(RawSection::Heading { level, text });
            return Accumulator::Empty;
        }

        self.fold_block_element(acc, pos, line_end)
    }

    #[inline]
    fn fold_block_element(
        &mut self,
        acc: Accumulator<'src>,
        pos: usize,
        line_end: usize,
    ) -> Accumulator<'src> {
        let line_bytes = &self.bytes[pos..line_end];

        // Fast-path: if we're in a paragraph and the line can't start a block
        // element, skip all the block-level checks and extend the paragraph.
        if let Accumulator::InParagraph { .. } = acc
            && !line_bytes.is_empty()
            && !line_bytes.could_start_block()
        {
            return self.fold_paragraph(acc, pos, line_end);
        }

        if line_bytes.first() == SpecialChar::GreaterThan {
            let content_start = pos + 1;
            let content = if self.bytes.get(content_start) == SpecialChar::Space {
                self.input.get(content_start + 1..line_end).unwrap_or("")
            } else {
                self.input.get(content_start..line_end).unwrap_or("")
            };
            if let Accumulator::InBlockquote { lines_start } = acc {
                self.lines.push(content);
                return Accumulator::InBlockquote { lines_start };
            }
            acc.flush_into(self);
            let lines_start = lines_offset(self.lines.len());
            self.lines.push(content);
            return Accumulator::InBlockquote { lines_start };
        }

        // Blockquote lazy continuation (CommonMark §5.1): a non-blank line
        // that doesn't start a new block-level construct continues the
        // current blockquote.
        let acc = if let Accumulator::InBlockquote { lines_start } = acc {
            // Fast reject: if first byte can't start a block element, continue.
            let continues = if !line_bytes.is_empty()
                && !line_bytes.could_start_block()
            {
                true
            } else {
                !line_bytes.is_horizontal_rule()
                    && line_bytes.try_parse_heading(self.input, pos).is_none()
                    && line_bytes.code_fence_len() == 0
                    && line_bytes.try_parse_unordered_item().is_none()
                    && line_bytes.try_parse_ordered_item().is_none()
            };
            if continues {
                self.lines
                    .push(self.input.get(pos..line_end).unwrap_or(""));
                return Accumulator::InBlockquote { lines_start };
            }
            // Line starts a new block — flush the blockquote and fall through.
            Accumulator::InBlockquote { lines_start }.flush_into(self);
            Accumulator::Empty
        } else {
            acc
        };

        // Horizontal rules (CommonMark §4.1): three or more -, *, or _
        // characters (optionally with spaces) on a line by themselves.
        if line_bytes.is_horizontal_rule() {
            acc.flush_into(self);
            self.sections.push(RawSection::HorizontalRule);
            return Accumulator::Empty;
        }

        if let Some((marker, item_offset)) =
            line_bytes.try_parse_unordered_item()
        {
            let item = self.input.get(pos + item_offset..line_end).unwrap_or("");
            return self.fold_unordered_list(acc, marker, item);
        }

        if let Some((num, delim, item_offset)) =
            line_bytes.try_parse_ordered_item()
        {
            let item = self.input.get(pos + item_offset..line_end).unwrap_or("");
            return self.fold_ordered_list(acc, num, delim, item);
        }

        self.fold_paragraph(acc, pos, line_end)
    }

    #[inline]
    fn fold_unordered_list(
        &mut self,
        acc: Accumulator<'src>,
        marker: SpecialChar,
        item: &'src str,
    ) -> Accumulator<'src> {
        if let Accumulator::InUnorderedList {
            marker: m,
            items_start,
        } = acc
        {
            if m == marker {
                self.lines.push(item);
                return Accumulator::InUnorderedList { marker, items_start };
            }
            Accumulator::InUnorderedList {
                marker: m,
                items_start,
            }
            .flush_into(self);
        } else {
            acc.flush_into(self);
        }
        let items_start = lines_offset(self.lines.len());
        self.lines.push(item);
        Accumulator::InUnorderedList {
            marker,
            items_start,
        }
    }

    #[inline]
    fn fold_ordered_list(
        &mut self,
        acc: Accumulator<'src>,
        num: u32,
        delim: OrderedListDelimiter,
        item: &'src str,
    ) -> Accumulator<'src> {
        if let Accumulator::InOrderedList {
            start,
            delimiter,
            items_start,
        } = acc
        {
            if delimiter == delim {
                self.lines.push(item);
                return Accumulator::InOrderedList {
                    start,
                    delimiter,
                    items_start,
                };
            }
            Accumulator::InOrderedList {
                start,
                delimiter,
                items_start,
            }
            .flush_into(self);
        } else {
            acc.flush_into(self);
        }
        let items_start = lines_offset(self.lines.len());
        self.lines.push(item);
        Accumulator::InOrderedList {
            start: num,
            delimiter: delim,
            items_start,
        }
    }

    #[inline]
    fn fold_paragraph(
        &mut self,
        acc: Accumulator<'src>,
        pos: usize,
        line_end: usize,
    ) -> Accumulator<'src> {
        let line_str = self.input.get(pos..line_end).unwrap_or("");
        if let Accumulator::InParagraph { content } = acc {
            return merge_slices(self.input, content, line_str).map_or_else(
                || {
                    self.sections.push(RawSection::Paragraph { text: content });
                    Accumulator::InParagraph { content: line_str }
                },
                |merged| Accumulator::InParagraph { content: merged },
            );
        }
        acc.flush_into(self);
        Accumulator::InParagraph { content: line_str }
    }
}
