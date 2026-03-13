use crate::section::{InlineSpan, OrderedListDelimiter, Section, SpanSlice};
use crate::simd::find_byte;
use crate::special_char::SpecialChar;
use crate::inline::pool_offset;
use crate::{Inline, MarkdownFile};

/// Mutable parsing context that bundles the output vectors, reducing the number
/// of parameters threaded through every `fold_*` call.
struct ParseCtx<'src> {
    input: &'src str,
    bytes: &'src [u8],
    sections: Vec<Section<'src>>,
    pool: Vec<Inline<'src>>,
    span_pool: Vec<InlineSpan>,
}

enum Accumulator<'src> {
    Empty,
    InCodeBlock {
        language: Option<&'src str>,
        content: Option<&'src str>,
        fence_len: usize,
    },
    InBlockquote {
        lines: Vec<&'src str>,
    },
    InUnorderedList {
        marker: SpecialChar,
        items: Vec<&'src str>,
    },
    InOrderedList {
        start: u32,
        delimiter: OrderedListDelimiter,
        items: Vec<&'src str>,
    },
    InParagraph {
        content: &'src str,
    },
}

impl<'src> Accumulator<'src> {
    fn flush(
        self,
        pool: &mut Vec<Inline<'src>>,
        span_pool: &mut Vec<InlineSpan>,
    ) -> Option<Section<'src>> {
        match self {
            Self::Empty => None,
            Self::InCodeBlock {
                language, content, ..
            } => Some(Section::CodeBlock {
                language,
                code: content.unwrap_or(""),
            }),
            Self::InBlockquote { lines } => {
                let start = pool_offset(pool.len());
                for (i, line) in lines.iter().enumerate() {
                    if i > 0 {
                        pool.push(Inline::Text("\n"));
                    }
                    Inline::parse_flat_into(line, pool);
                }
                let len = pool_offset(pool.len()) - start;
                Some(Section::Blockquote {
                    content: InlineSpan::new(start, len),
                })
            }
            Self::InUnorderedList { items, .. } => {
                let start = pool_offset(span_pool.len());
                for item in &items {
                    let span = Inline::parse(item, pool);
                    span_pool.push(span);
                }
                let len = pool_offset(span_pool.len()) - start;
                Some(Section::UnorderedList {
                    items: SpanSlice::new(start, len),
                })
            }
            Self::InOrderedList {
                start,
                delimiter,
                items,
            } => {
                let sp_start = pool_offset(span_pool.len());
                for item in &items {
                    let span = Inline::parse(item, pool);
                    span_pool.push(span);
                }
                let sp_len = pool_offset(span_pool.len()) - sp_start;
                Some(Section::OrderedList {
                    start,
                    delimiter,
                    items: SpanSlice::new(sp_start, sp_len),
                })
            }
            Self::InParagraph { content } => Some(Section::Paragraph {
                content: Inline::parse(content, pool),
            }),
        }
    }

    fn flush_into(self, ctx: &mut ParseCtx<'src>) {
        if let Some(section) = self.flush(&mut ctx.pool, &mut ctx.span_pool) {
            ctx.sections.push(section);
        }
    }
}

/// Create a `Vec` with pre-allocated capacity and a single initial element.
fn vec_with_first<T>(first: T) -> Vec<T> {
    let mut v = Vec::with_capacity(4);
    v.push(first);
    v
}

/// Convert a byte range from the input into a `&str` without char-boundary
/// checks. All call sites split at ASCII byte boundaries (newlines, spaces,
/// digits, punctuation), so the invariant is guaranteed.
///
/// # Safety
/// `start` and `end` must lie on UTF-8 char boundaries within `input`.
#[allow(clippy::inline_always)]
#[inline(always)]
unsafe fn str_from_range(input: &str, start: usize, end: usize) -> &str {
    debug_assert!(start <= end && end <= input.len());
    debug_assert!(input.is_char_boundary(start));
    debug_assert!(input.is_char_boundary(end));
    unsafe { input.get_unchecked(start..end) }
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

impl<'src> MarkdownFile<'src> {
    #[must_use]
    pub fn parse(input: &'src str) -> Self {
        let bytes = input.as_bytes();
        let mut ctx = ParseCtx {
            input,
            bytes,
            // Rough heuristic: ~50 bytes per section on average.
            sections: Vec::with_capacity(input.len() / 50 + 1),
            pool: Vec::with_capacity(input.len() / 20),
            span_pool: Vec::with_capacity(input.len() / 100 + 1),
        };
        let mut acc = Accumulator::Empty;
        let mut pos = 0;

        while pos < bytes.len() {
            let line_end = find_byte(bytes, pos, b'\n').unwrap_or(bytes.len());

            // Fast-path: when we detect a code fence opening, scan ahead for
            // the closing fence in one shot instead of processing line-by-line.
            // CommonMark §4.5: a code fence can be indented 0-3 spaces, so we
            // only need to check if a backtick appears within the first 4 bytes.
            if !matches!(acc, Accumulator::InCodeBlock { .. }) {
                let first = bytes.get(pos).copied().unwrap_or(SpecialChar::Space.byte());
                let backtick = SpecialChar::Backtick.byte();
                if first == backtick
                    || (first == SpecialChar::Space
                        && bytes[pos..line_end]
                            .get(..4)
                            .is_some_and(|w| w.contains(&backtick)))
                {
                    let fence_len = Self::code_fence_len_bytes(&bytes[pos..line_end]);
                    if fence_len > 0 {
                        let language =
                            Self::extract_code_language_bytes(input, &bytes[pos..line_end]);
                        acc.flush_into(&mut ctx);
                        let content_start = line_end + 1;
                        let (code, resume) =
                            Self::scan_code_block_fast(input, bytes, content_start, fence_len);
                        ctx.sections.push(Section::CodeBlock { language, code });
                        pos = resume;
                        acc = Accumulator::Empty;
                        continue;
                    }
                }
            }

            acc = ctx.fold_line(acc, pos, line_end);
            pos = line_end + 1;
        }

        acc.flush_into(&mut ctx);
        Self {
            sections: ctx.sections,
            pool: ctx.pool,
            span_pool: ctx.span_pool,
        }
    }

    /// Scan forward from `start` to find a closing code fence of at least
    /// `fence_len` backticks. Returns `(code_content, resume_position)`.
    fn scan_code_block_fast(
        input: &'src str,
        bytes: &[u8],
        start: usize,
        fence_len: usize,
    ) -> (&'src str, usize) {
        let mut pos = start;
        while pos < bytes.len() {
            let line_end = find_byte(bytes, pos, b'\n').unwrap_or(bytes.len());

            // Check for closing fence: first non-whitespace byte must be backtick.
            let first = bytes.get(pos).copied().unwrap_or(0);
            if (first == SpecialChar::Backtick.byte() || first.is_ascii_whitespace())
                && Self::code_fence_len_bytes(&bytes[pos..line_end]) >= fence_len
            {
                // Content is everything between opening and closing fence.
                let code = if start < pos {
                    // Trim the trailing newline before the closing fence.
                    // SAFETY: start is after a newline, pos-1 is before a newline.
                    unsafe { str_from_range(input, start, pos - 1) }
                } else {
                    ""
                };
                return (code, line_end + 1);
            }
            pos = line_end + 1;
        }
        // Unclosed code block: content runs to end of input.
        let code = if start < bytes.len() {
            // SAFETY: start is after a newline.
            unsafe { str_from_range(input, start, bytes.len()) }
        } else {
            ""
        };
        (code, bytes.len())
    }

    /// Lookup table: true for bytes that could start a block-level element
    /// (blockquote, list marker, HR character, or digit for ordered lists).
    const COULD_START_BLOCK: [bool; 256] = {
        let mut table = [false; 256];
        table[SpecialChar::GreaterThan.byte() as usize] = true;
        table[SpecialChar::Dash.byte() as usize] = true;
        table[SpecialChar::Asterisk.byte() as usize] = true;
        table[SpecialChar::Plus.byte() as usize] = true;
        table[SpecialChar::Underscore.byte() as usize] = true;
        let mut d = b'0';
        while d <= b'9' {
            table[d as usize] = true;
            d += 1;
        }
        table
    };

    // -----------------------------------------------------------------------
    // Byte-level classification helpers — no &str involved
    // -----------------------------------------------------------------------

    #[inline]
    /// Returns the fence length (number of backticks) if the line bytes are a
    /// valid code fence (`CommonMark` §4.5), or 0 if not. A valid fence has 3+
    /// backticks with no backticks in the info string.
    fn code_fence_len_bytes(line: &[u8]) -> usize {
        // Skip leading whitespace.
        let mut first = 0;
        while first < line.len() && line[first].is_ascii_whitespace() {
            first += 1;
        }
        // Quick reject: first non-whitespace byte must be a backtick.
        let backtick = SpecialChar::Backtick.byte();
        if first >= line.len() || line[first] != backtick {
            return 0;
        }
        // Count backticks directly from the offset we already found.
        let mut end = first + 1;
        while end < line.len() && line[end] == backtick {
            end += 1;
        }
        let len = end - first;
        if len >= 3 && !line[end..].contains(&backtick) {
            len
        } else {
            0
        }
    }

    /// Extract the language tag from a code fence line (bytes), returning
    /// a `&str` slice from `input`.
    fn extract_code_language_bytes(input: &'src str, line: &[u8]) -> Option<&'src str> {
        // Find the start of the info string: skip whitespace then backticks.
        let mut i = 0;
        while i < line.len() && line[i].is_ascii_whitespace() {
            i += 1;
        }
        let backtick = SpecialChar::Backtick.byte();
        while i < line.len() && line[i] == backtick {
            i += 1;
        }
        // Trim remaining whitespace around the info string.
        while i < line.len() && line[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut end = line.len();
        while end > i && line[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
        if i >= end {
            None
        } else {
            // Compute the absolute offset into input. line is a subslice of
            // input.as_bytes(), so pointer arithmetic gives us the offset.
            let line_offset = line.as_ptr() as usize - input.as_ptr() as usize;
            // SAFETY: i..end are within the ASCII prefix/suffix we just trimmed.
            Some(unsafe { str_from_range(input, line_offset + i, line_offset + end) })
        }
    }

    /// Check whether a line (bytes) is a thematic break / horizontal rule
    /// (`CommonMark` §4.1): three or more matching `-`, `*`, or `_` characters,
    /// optionally separated by spaces, with nothing else on the line.
    fn is_horizontal_rule_bytes(line: &[u8]) -> bool {
        let mut rule_byte = 0u8;
        let mut count = 0u32;
        for &b in line {
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

    /// Parse an ATX heading (`CommonMark` §4.2) from a byte slice.
    /// Strips optional closing `#` sequences when preceded by whitespace.
    fn try_parse_heading_bytes(
        input: &'src str,
        line: &[u8],
        line_offset: usize,
        pool: &mut Vec<Inline<'src>>,
    ) -> Option<Section<'src>> {
        let level = SpecialChar::Hash.count_leading_bytes(line);
        if !(1..=6).contains(&level) || line.get(level) != SpecialChar::Space {
            return None;
        }
        // Trim leading whitespace after '#'s.
        let mut start = level;
        while start < line.len() && line[start].is_ascii_whitespace() {
            start += 1;
        }
        let mut end = line.len();
        while end > start && line[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
        // Strip optional closing # sequence per CommonMark §4.2:
        // trailing #s are removed only if preceded by whitespace (or they
        // are the entire content after the opening).
        let mut stripped_end = end;
        while stripped_end > start && line[stripped_end - 1] == SpecialChar::Hash.byte() {
            stripped_end -= 1;
        }
        if stripped_end == start
            || line
                .get(stripped_end - 1)
                .is_some_and(|&b| b == SpecialChar::Space || b == SpecialChar::Tab)
        {
            // Trim whitespace before the closing hashes.
            end = stripped_end;
            while end > start && line[end - 1].is_ascii_whitespace() {
                end -= 1;
            }
        }
        // SAFETY: start and end are within the ASCII heading prefix/suffix.
        let text = unsafe { str_from_range(input, line_offset + start, line_offset + end) };
        Some(Section::Heading {
            level: u8::try_from(level).expect("heading level already validated 1..=6"),
            content: Inline::parse(text, pool),
        })
    }

    /// Try to parse an unordered list item from a byte slice.
    /// Returns `(marker, item_byte_offset)` where offset is relative to line start.
    fn try_parse_unordered_item_bytes(line: &[u8]) -> Option<(SpecialChar, usize)> {
        let &first = line.first()?;
        let marker = SpecialChar::from_byte(first)?;
        if !marker.is_list_char() {
            return None;
        }
        if line.get(1) == SpecialChar::Space {
            Some((marker, 2))
        } else {
            None
        }
    }

    /// Try to parse an ordered list item from a byte slice.
    /// Returns `(number, delimiter, item_byte_offset)` where offset is relative
    /// to line start.
    fn try_parse_ordered_item_bytes(line: &[u8]) -> Option<(u32, OrderedListDelimiter, usize)> {
        let mut num: u32 = 0;
        let mut digits = 0usize;
        for &b in line {
            if b.is_ascii_digit() {
                digits += 1;
                if digits > 9 {
                    return None;
                }
                num = num * 10 + u32::from(b - b'0');
            } else {
                break;
            }
        }
        if digits == 0 {
            return None;
        }
        let delimiter = OrderedListDelimiter::from_byte(line.get(digits).copied()?)?;
        if line.get(digits + 1) != SpecialChar::Space {
            return None;
        }
        let item_offset = digits + 2;
        if item_offset >= line.len() {
            return None;
        }
        Some((num, delimiter, item_offset))
    }

    /// Merge two subslices of `base` into one contiguous slice spanning from the
    /// start of `a` to the end of `b`.
    fn merge_slices(base: &'src str, a: &str, b: &str) -> Option<&'src str> {
        let base_start = base.as_ptr() as usize;
        let a_start = a.as_ptr() as usize;
        let b_end = b.as_ptr() as usize + b.len();

        if a_start < base_start || b_end > base_start + base.len() || b_end < a_start {
            return None;
        }

        // SAFETY: a and b are subslices of base (verified above), so the
        // range a_start..b_end is valid UTF-8 within base.
        Some(unsafe { str_from_range(base, a_start - base_start, b_end - base_start) })
    }
}

// ---------------------------------------------------------------------------
// ParseCtx methods — line-level fold logic
// ---------------------------------------------------------------------------

impl<'src> ParseCtx<'src> {
    /// Process one line given as byte range `[pos..line_end)`.
    /// Operates on `&[u8]` throughout; converts to `&str` only when storing.
    fn fold_line(
        &mut self,
        acc: Accumulator<'src>,
        pos: usize,
        line_end: usize,
    ) -> Accumulator<'src> {
        if let Accumulator::InCodeBlock {
            language,
            content,
            fence_len,
        } = acc
        {
            return self.fold_code_block(language, content, fence_len, pos, line_end);
        }

        let first = self.bytes.get(pos).copied().unwrap_or(SpecialChar::Space.byte());

        if first.is_ascii_whitespace() && is_blank_line(self.bytes, pos, line_end) {
            acc.flush_into(self);
            return Accumulator::Empty;
        }

        // Code fence opening is already handled by parse()'s fast-path before
        // fold_line is called, so no need to re-check here.

        // ATX headings (CommonMark §4.2): only if line starts with '#'.
        if first == SpecialChar::Hash.byte()
            && let Some(section) = MarkdownFile::try_parse_heading_bytes(
                self.input,
                &self.bytes[pos..line_end],
                pos,
                &mut self.pool,
            )
        {
            acc.flush_into(self);
            self.sections.push(section);
            return Accumulator::Empty;
        }

        self.fold_block_element(acc, pos, line_end)
    }

    #[inline]
    fn fold_code_block(
        &mut self,
        language: Option<&'src str>,
        content: Option<&'src str>,
        fence_len: usize,
        pos: usize,
        line_end: usize,
    ) -> Accumulator<'src> {
        // Guard: only check for closing fence if first byte could be a backtick
        // or whitespace (indented fence). Avoids calling code_fence_len on
        // every code block content line.
        let first = self.bytes.get(pos).copied().unwrap_or(0);
        if (first == SpecialChar::Backtick.byte() || first.is_ascii_whitespace())
            && MarkdownFile::code_fence_len_bytes(&self.bytes[pos..line_end]) >= fence_len
        {
            self.sections.push(Section::CodeBlock {
                language,
                code: content.unwrap_or(""),
            });
            return Accumulator::Empty;
        }
        // SAFETY: pos and line_end are at newline boundaries.
        let line_str = unsafe { str_from_range(self.input, pos, line_end) };
        let content = content.map_or(line_str, |existing| {
            MarkdownFile::merge_slices(self.input, existing, line_str)
                .expect("merge_slices failed in code block: slices not from same input")
        });
        Accumulator::InCodeBlock {
            language,
            content: Some(content),
            fence_len,
        }
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
            && !MarkdownFile::<'src>::COULD_START_BLOCK[line_bytes[0] as usize]
        {
            return self.fold_paragraph(acc, pos, line_end);
        }

        if line_bytes.first() == SpecialChar::GreaterThan {
            // SAFETY: pos+1 is after '>', which is ASCII.
            let content_start = pos + 1;
            let content = if self.bytes.get(content_start) == SpecialChar::Space {
                // SAFETY: pos+2 is after '> ', both ASCII.
                unsafe { str_from_range(self.input, content_start + 1, line_end) }
            } else {
                unsafe { str_from_range(self.input, content_start, line_end) }
            };
            if let Accumulator::InBlockquote { mut lines } = acc {
                lines.push(content);
                return Accumulator::InBlockquote { lines };
            }
            acc.flush_into(self);
            return Accumulator::InBlockquote {
                lines: vec_with_first(content),
            };
        }

        // Blockquote lazy continuation (CommonMark §5.1): a non-blank line
        // that doesn't start a new block-level construct continues the
        // current blockquote.
        let acc = if let Accumulator::InBlockquote { mut lines } = acc {
            if !MarkdownFile::is_horizontal_rule_bytes(line_bytes)
                && MarkdownFile::try_parse_heading_bytes(
                    self.input,
                    line_bytes,
                    pos,
                    &mut self.pool,
                )
                .is_none()
                && MarkdownFile::<'src>::code_fence_len_bytes(line_bytes) == 0
                && MarkdownFile::<'src>::try_parse_unordered_item_bytes(line_bytes).is_none()
                && MarkdownFile::<'src>::try_parse_ordered_item_bytes(line_bytes).is_none()
            {
                // SAFETY: pos and line_end are at newline boundaries.
                lines.push(unsafe { str_from_range(self.input, pos, line_end) });
                return Accumulator::InBlockquote { lines };
            }
            // Line starts a new block — flush the blockquote and fall through.
            Accumulator::InBlockquote { lines }.flush_into(self);
            Accumulator::Empty
        } else {
            acc
        };

        // Horizontal rules (CommonMark §4.1): three or more -, *, or _
        // characters (optionally with spaces) on a line by themselves.
        if MarkdownFile::is_horizontal_rule_bytes(line_bytes) {
            acc.flush_into(self);
            self.sections.push(Section::HorizontalRule);
            return Accumulator::Empty;
        }

        if let Some((marker, item_offset)) =
            MarkdownFile::<'src>::try_parse_unordered_item_bytes(line_bytes)
        {
            // SAFETY: item_offset is after ASCII marker + space.
            let item = unsafe { str_from_range(self.input, pos + item_offset, line_end) };
            return self.fold_unordered_list(acc, marker, item);
        }

        if let Some((num, delim, item_offset)) =
            MarkdownFile::<'src>::try_parse_ordered_item_bytes(line_bytes)
        {
            // SAFETY: item_offset is after ASCII digits + delimiter + space.
            let item = unsafe { str_from_range(self.input, pos + item_offset, line_end) };
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
            mut items,
        } = acc
        {
            if m == marker {
                items.push(item);
                return Accumulator::InUnorderedList { marker, items };
            }
            Accumulator::InUnorderedList { marker: m, items }.flush_into(self);
            return Accumulator::InUnorderedList {
                marker,
                items: vec_with_first(item),
            };
        }
        acc.flush_into(self);
        Accumulator::InUnorderedList {
            marker,
            items: vec_with_first(item),
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
            mut items,
        } = acc
        {
            if delimiter == delim {
                items.push(item);
                return Accumulator::InOrderedList {
                    start,
                    delimiter,
                    items,
                };
            }
            Accumulator::InOrderedList {
                start,
                delimiter,
                items,
            }
            .flush_into(self);
            return Accumulator::InOrderedList {
                start: num,
                delimiter: delim,
                items: vec_with_first(item),
            };
        }
        acc.flush_into(self);
        Accumulator::InOrderedList {
            start: num,
            delimiter: delim,
            items: vec_with_first(item),
        }
    }

    #[inline]
    fn fold_paragraph(
        &mut self,
        acc: Accumulator<'src>,
        pos: usize,
        line_end: usize,
    ) -> Accumulator<'src> {
        // SAFETY: pos and line_end are at newline boundaries.
        let line_str = unsafe { str_from_range(self.input, pos, line_end) };
        if let Accumulator::InParagraph { content } = acc {
            return MarkdownFile::merge_slices(self.input, content, line_str).map_or_else(
                || {
                    self.sections.push(Section::Paragraph {
                        content: Inline::parse(content, &mut self.pool),
                    });
                    Accumulator::InParagraph { content: line_str }
                },
                |merged| Accumulator::InParagraph { content: merged },
            );
        }
        acc.flush_into(self);
        Accumulator::InParagraph { content: line_str }
    }
}
