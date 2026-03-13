use crate::inline::pool_offset;
use crate::section::{InlineSpan, OrderedListDelimiter, Section, SpanSlice};
use crate::simd::find_byte;
use crate::special_char::SpecialChar;
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
            Self::InUnorderedList { items_start, .. } => Some(RawSection::UnorderedList {
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

/// Return the number of leading spaces (0–3) if valid `CommonMark` indentation.
/// Returns `None` if 4+ leading spaces (too much indentation for block elements).
#[allow(clippy::inline_always)]
#[inline(always)]
fn strip_indent(bytes: &[u8]) -> Option<usize> {
    let mut n = 0;
    while n < bytes.len() && bytes[n] == SpecialChar::Space {
        n += 1;
        if n > 3 {
            return None;
        }
    }
    Some(n)
}

/// Count consecutive occurrences of `needle` at the start of `bytes`.
#[inline]
fn count_leading_byte(bytes: &[u8], needle: u8) -> usize {
    let mut n = 0;
    while n < bytes.len() && bytes[n] == needle {
        n += 1;
    }
    n
}

/// Check if `bytes` starts with a valid code fence opening (3+ backticks or tildes).
/// Returns `(fence_char, fence_len)` if valid.
/// Expects leading indentation to already be stripped by the caller.
fn code_fence_opening(bytes: &[u8]) -> Option<(u8, usize)> {
    let &first = bytes.first()?;
    if first != SpecialChar::Backtick && first != SpecialChar::Tilde {
        return None;
    }
    let len = count_leading_byte(bytes, first);
    if len < 3 {
        return None;
    }
    // Backtick fences: info string must not contain backticks (CommonMark §4.5).
    // Tilde fences have no such restriction.
    if first == SpecialChar::Backtick && bytes[len..].contains(&first) {
        return None;
    }
    Some((first, len))
}

/// Check if `bytes` is a valid closing fence for the given character and minimum length.
/// Expects leading indentation to already be stripped by the caller.
fn is_closing_fence(bytes: &[u8], fence_char: u8, min_len: usize) -> bool {
    let len = count_leading_byte(bytes, fence_char);
    // Must have at least as many fence chars as the opening.
    len >= min_len
    // Rest must be whitespace only (no info string on closing fences).
        && bytes[len..].iter().all(u8::is_ascii_whitespace)
}

/// Extract the language tag from a code fence opening line.
/// `bytes` is the line with leading indentation already stripped.
/// `fence_len` is the number of fence characters.
fn extract_language<'src>(input: &'src str, bytes: &[u8], fence_len: usize) -> Option<&'src str> {
    debug_assert!(
        bytes.as_ptr() as usize >= input.as_ptr() as usize
            && bytes.as_ptr() as usize + bytes.len() <= input.as_ptr() as usize + input.len(),
        "bytes must be a subslice of input"
    );
    let mut i = fence_len;
    while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    let mut end = bytes.len();
    while end > i && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    if i >= end {
        return None;
    }
    // Compute the absolute offset into input. `bytes` is a subslice of
    // `input.as_bytes()`, so pointer arithmetic gives us the offset.
    let line_offset = bytes.as_ptr() as usize - input.as_ptr() as usize;
    input.get(line_offset + i..line_offset + end)
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
                let raw_items = lines
                    .get(items_start as usize..(items_start + items_len) as usize)
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
                let raw_items = lines
                    .get(items_start as usize..(items_start + items_len) as usize)
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
/// (heading, blockquote, list marker, HR character, or digit for ordered lists).
const COULD_START_BLOCK: [bool; 256] = {
    let mut table = [false; 256];
    table[SpecialChar::Hash.byte() as usize] = true;
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
    fn is_horizontal_rule(&self) -> bool;
    fn try_parse_heading<'src>(
        &self,
        input: &'src str,
        line_offset: usize,
    ) -> Option<(u8, &'src str)>;
    fn try_parse_unordered_item(&self) -> Option<(SpecialChar, usize)>;
    fn try_parse_ordered_item(&self) -> Option<(u32, OrderedListDelimiter, usize)>;
    fn could_start_block(&self) -> bool;
}

impl BlockBytes for [u8] {
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
    fn try_parse_heading<'src>(
        &self,
        input: &'src str,
        line_offset: usize,
    ) -> Option<(u8, &'src str)> {
        let level = count_leading_byte(self, SpecialChar::Hash.byte());
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
        Some((num, delimiter, item_offset))
    }

    /// Check whether the first byte of this line could start a block-level element.
    #[inline]
    fn could_start_block(&self) -> bool {
        self.first().is_some_and(|&b| COULD_START_BLOCK[b as usize])
    }
}

/// Scan forward from `start` to find a closing code fence of the same type
/// (`fence_char`) and at least `fence_len` characters.
/// Returns `(code_content, resume_position)`.
fn scan_code_block_fast<'src>(
    input: &'src str,
    bytes: &[u8],
    start: usize,
    fence_len: usize,
    fence_char: u8,
) -> (&'src str, usize) {
    let mut pos = start;
    while pos < bytes.len() {
        let line_end = find_byte(bytes, pos, SpecialChar::Newline.byte()).unwrap_or(bytes.len());

        // Fast reject: a closing fence must start with the fence char or a
        // space (for 0-3 indentation). Skip lines that start with anything else.
        let first = bytes.get(pos).copied();
        if (first == Some(fence_char) || first == Some(SpecialChar::Space.byte()))
            && let Some(indent) = strip_indent(&bytes[pos..line_end])
        {
            let spos = pos + indent;
            if is_closing_fence(&bytes[spos..line_end], fence_char, fence_len) {
                // Content is everything between opening and closing fence.
                let code = if start < pos {
                    input.get(start..pos - 1).unwrap_or("")
                } else {
                    ""
                };
                return (code, line_end + 1);
            }
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
            let line_end =
                find_byte(bytes, pos, SpecialChar::Newline.byte()).unwrap_or(bytes.len());

            // Fast-path: when we detect a code fence opening, scan ahead for
            // the closing fence in one shot instead of processing line-by-line.
            // CommonMark §4.5: a code fence can be indented 0-3 spaces, so we
            // check if a backtick or tilde appears within the first 4 bytes.
            let first = bytes.get(pos).copied();
            if (first == SpecialChar::Backtick
                || first == SpecialChar::Tilde
                || (first == SpecialChar::Space
                    && bytes[pos..line_end].get(..4).is_some_and(|w| {
                        w.contains(&SpecialChar::Backtick.byte())
                            || w.contains(&SpecialChar::Tilde.byte())
                    })))
                && let Some(indent) = strip_indent(&bytes[pos..line_end])
                && let Some((fence_char, fence_len)) =
                    code_fence_opening(&bytes[pos + indent..line_end])
            {
                let spos = pos + indent;
                let language = extract_language(input, &bytes[spos..line_end], fence_len);
                acc.flush_into(&mut ctx);
                let content_start = line_end + 1;
                let (code, resume) =
                    scan_code_block_fast(input, bytes, content_start, fence_len, fence_char);
                ctx.sections.push(RawSection::CodeBlock { language, code });
                pos = resume;
                acc = Accumulator::Empty;
                continue;
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
    #[inline]
    fn fold_line(
        &mut self,
        acc: Accumulator<'src>,
        pos: usize,
        line_end: usize,
    ) -> Accumulator<'src> {
        let first = self.bytes.get(pos).copied();

        if first.is_some_and(|b| b.is_ascii_whitespace())
            && is_blank_line(self.bytes, pos, line_end)
        {
            acc.flush_into(self);
            return Accumulator::Empty;
        }

        self.fold_block_element(acc, pos, line_end)
    }

    /// Detect and fold all block-level constructs. Computes `CommonMark` 0-3
    /// space indentation internally.
    #[inline]
    fn fold_block_element(
        &mut self,
        acc: Accumulator<'src>,
        pos: usize,
        line_end: usize,
    ) -> Accumulator<'src> {
        // Strip 0-3 spaces of optional indentation (CommonMark §4).
        // Lines with 4+ leading spaces cannot start a block-level construct.
        let Some(indent) = strip_indent(&self.bytes[pos..line_end]) else {
            // 4+ leading spaces: only valid as paragraph text or
            // blockquote lazy continuation.
            if let Accumulator::InBlockquote { lines_start } = acc {
                self.lines.push(self.input.get(pos..line_end).unwrap_or(""));
                return Accumulator::InBlockquote { lines_start };
            }
            return self.fold_paragraph(acc, pos, line_end);
        };
        let spos = pos + indent;
        let line_bytes = &self.bytes[spos..line_end];

        // Fast-path: if we're in a paragraph and the line can't start a block
        // element, skip all the block-level checks and extend the paragraph.
        if let Accumulator::InParagraph { .. } = acc
            && !line_bytes.is_empty()
            && !line_bytes.could_start_block()
        {
            return self.fold_paragraph(acc, pos, line_end);
        }

        // ATX headings (CommonMark §4.2).
        if line_bytes.first() == SpecialChar::Hash
            && let Some((level, text)) = line_bytes.try_parse_heading(self.input, spos)
        {
            acc.flush_into(self);
            self.sections.push(RawSection::Heading { level, text });
            return Accumulator::Empty;
        }

        if line_bytes.first() == SpecialChar::GreaterThan {
            let content_start = spos + 1;
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
            let continues = if !line_bytes.is_empty() && !line_bytes.could_start_block() {
                true
            } else {
                !line_bytes.is_horizontal_rule()
                    && line_bytes.try_parse_heading(self.input, spos).is_none()
                    && code_fence_opening(line_bytes).is_none()
                    && line_bytes.try_parse_unordered_item().is_none()
                    && line_bytes.try_parse_ordered_item().is_none()
            };
            if continues {
                self.lines.push(self.input.get(pos..line_end).unwrap_or(""));
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

        if let Some((marker, item_offset)) = line_bytes.try_parse_unordered_item() {
            let item = self.input.get(spos + item_offset..line_end).unwrap_or("");
            return self.fold_unordered_list(acc, marker, item);
        }

        if let Some((num, delim, item_offset)) = line_bytes.try_parse_ordered_item() {
            let item = self.input.get(spos + item_offset..line_end).unwrap_or("");
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
                return Accumulator::InUnorderedList {
                    marker,
                    items_start,
                };
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
