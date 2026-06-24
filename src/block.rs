use crate::OffsetExt;
use crate::inline::InlineParser;
use crate::link_def::{LinkDefs, normalize_label, scan_link_def};
use crate::section::{InlineSpan, OrderedListDelimiter, Section, SectionRange};
use crate::simd::ByteSliceExt;
use crate::special_char::SpecialChar;
use crate::{Inline, MarkdownFile};

// ---------------------------------------------------------------------------
// Pass 1: block-level parsing into RawSection (no inline parsing)
// ---------------------------------------------------------------------------

/// Intermediate section representation produced by pass 1 (block parsing).
/// Stores raw `&str` text that will be inline-parsed in pass 2.
#[derive(Clone, Copy)]
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
    IndentedCode {
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
    HtmlBlock {
        html: &'src str,
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
    /// Link reference definitions collected during pass 1 (`CommonMark` §4.7).
    defs: LinkDefs<'src>,
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
    /// An indented code block (`CommonMark` §4.4). `start` is the byte offset
    /// of the first content line; `last_nonblank_end` is the end offset of the
    /// most recent non-blank line, so trailing blank lines are trimmed on
    /// flush.
    InIndentedCode {
        start: usize,
        last_nonblank_end: usize,
    },
}

impl<'src> Accumulator<'src> {
    const fn flush(self, lines_pool_len: u32) -> Option<RawSection<'src>> {
        match self {
            // Empty produces nothing; indented code needs `input` to slice its
            // span, so it is handled directly in `flush_acc` and never here.
            Self::Empty | Self::InIndentedCode { .. } => None,
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
    fn is_blank_line(&self, start: usize, end: usize) -> bool;
    fn strip_indent(&self) -> Option<usize>;
    fn code_fence_opening(&self) -> Option<(u8, usize)>;
    fn is_closing_fence(&self, fence_char: u8, min_len: usize) -> bool;
    fn is_horizontal_rule(&self) -> bool;
    fn try_parse_heading<'src>(
        &self,
        input: &'src str,
        line_offset: usize,
    ) -> Option<(u8, &'src str)>;
    fn try_parse_unordered_item(&self) -> Option<(SpecialChar, usize)>;
    fn try_parse_ordered_item(&self) -> Option<(u32, OrderedListDelimiter, usize)>;
    fn could_start_block(&self) -> bool;
    fn setext_heading_level(&self) -> Option<u8>;
}

impl BlockBytes for [u8] {
    fn is_blank_line(&self, start: usize, end: usize) -> bool {
        if start >= end {
            return true;
        }
        self[start..end].iter().all(u8::is_ascii_whitespace)
    }

    fn strip_indent(&self) -> Option<usize> {
        let mut n = 0;
        while n < self.len() && self[n] == SpecialChar::Space {
            n += 1;
            if n > 3 {
                return None;
            }
        }
        Some(n)
    }

    fn code_fence_opening(&self) -> Option<(u8, usize)> {
        let &first = self.first()?;
        let marker = SpecialChar::from_byte(first)?;
        if marker != SpecialChar::Backtick && marker != SpecialChar::Tilde {
            return None;
        }
        let len = marker.count_leading_bytes(self);
        if len < 3 {
            return None;
        }
        if first == SpecialChar::Backtick && self[len..].contains(&first) {
            return None;
        }
        Some((first, len))
    }

    fn is_closing_fence(&self, fence_char: u8, min_len: usize) -> bool {
        let len = SpecialChar::from_byte(fence_char)
            .expect("fence_char is backtick or tilde")
            .count_leading_bytes(self);
        len >= min_len && self[len..].iter().all(u8::is_ascii_whitespace)
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
    fn try_parse_heading<'src>(
        &self,
        input: &'src str,
        line_offset: usize,
    ) -> Option<(u8, &'src str)> {
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

    /// Check whether this line is a setext heading underline (`CommonMark`
    /// §4.3): one or more `=` (level 1) or `-` (level 2) characters, followed
    /// only by trailing whitespace. Leading 0-3 space indentation is assumed
    /// already stripped by the caller.
    fn setext_heading_level(&self) -> Option<u8> {
        let &first = self.first()?;
        let level = match first {
            b'=' => 1,
            b'-' => 2,
            _ => return None,
        };
        let mut run = 0;
        while run < self.len() && self[run] == first {
            run += 1;
        }
        if run == 0 || !self[run..].iter().all(u8::is_ascii_whitespace) {
            return None;
        }
        Some(level)
    }
}

impl<'src, const MAX_INLINE_DEPTH: u8, const INLINE_STACK_CAP: usize>
    MarkdownFile<'src, MAX_INLINE_DEPTH, INLINE_STACK_CAP>
{
    /// Convert the raw sections from pass 1 into final sections with inline
    /// parsing. Separating passes lets us pre-size the output pools from the
    /// raw section count and avoid interleaving block and inline allocation
    /// patterns.
    fn resolve_inlines(
        ctx: &ParseCtx<'src>,
        pool: &mut Vec<Inline<'src>>,
        section_pool: &mut Vec<Section<'src>>,
    ) -> Vec<Section<'src>> {
        let lines = &ctx.lines;
        let defs = &ctx.defs;
        let mut sections = Vec::with_capacity(ctx.sections.len());
        for raw_section in &ctx.sections {
            match *raw_section {
                RawSection::Heading { level, text } => {
                    sections.push(Section::Heading {
                        level,
                        content:
                            InlineParser::<MAX_INLINE_DEPTH, INLINE_STACK_CAP>::parse_configured(
                                text, pool, defs,
                            ),
                    });
                }
                RawSection::Paragraph { text } => {
                    sections.push(Section::Paragraph {
                        content:
                            InlineParser::<MAX_INLINE_DEPTH, INLINE_STACK_CAP>::parse_paragraph_configured(
                                text, pool, defs,
                            ),
                    });
                }
                RawSection::CodeBlock { language, code } => {
                    sections.push(Section::CodeBlock { language, code });
                }
                RawSection::IndentedCode { code } => {
                    sections.push(Section::IndentedCode { code });
                }
                RawSection::UnorderedList {
                    items_start,
                    items_len,
                } => {
                    let raw_items = lines
                        .get(items_start as usize..(items_start + items_len) as usize)
                        .unwrap_or(&[]);
                    let items = Self::resolve_list_items(raw_items, pool, section_pool, defs);
                    sections.push(Section::UnorderedList { tight: true, items });
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
                    let items = Self::resolve_list_items(raw_items, pool, section_pool, defs);
                    sections.push(Section::OrderedList {
                        start,
                        delimiter,
                        tight: true,
                        items,
                    });
                }
                RawSection::Blockquote {
                    lines_start,
                    lines_len,
                } => {
                    let raw_lines = lines
                        .get(lines_start as usize..(lines_start + lines_len) as usize)
                        .unwrap_or(&[]);
                    let children =
                        Self::resolve_quote(raw_lines, pool, section_pool, defs);
                    sections.push(Section::Blockquote { children });
                }
                RawSection::HtmlBlock { html } => {
                    sections.push(Section::HtmlBlock { html });
                }
                RawSection::HorizontalRule => {
                    sections.push(Section::HorizontalRule);
                }
            }
        }
        sections
    }

    /// Resolve a list's raw item texts (one `&str` per item, from pass 1) into
    /// a range of [`Section::ListItem`] sections in `section_pool`. Each item
    /// currently holds a single paragraph child; the upcoming continuation
    /// rewrite will let items carry arbitrary child blocks.
    fn resolve_list_items(
        raw_items: &[&'src str],
        pool: &mut Vec<Inline<'src>>,
        section_pool: &mut Vec<Section<'src>>,
        defs: &LinkDefs<'src>,
    ) -> SectionRange {
        // Build each item's children first, then append the ListItem sections
        // contiguously. Children of a single item are appended immediately so a
        // ListItem's range is stable before we collect the item-level range.
        let mut items: Vec<Section<'src>> = Vec::with_capacity(raw_items.len());
        for item in raw_items {
            let child_start = section_pool.len().pool_offset();
            let content =
                InlineParser::<MAX_INLINE_DEPTH, INLINE_STACK_CAP>::parse_configured(
                    item, pool, defs,
                );
            section_pool.push(Section::Paragraph { content });
            let child_len = section_pool.len().pool_offset() - child_start;
            items.push(Section::ListItem {
                children: SectionRange::new(child_start, child_len),
            });
        }
        let start = section_pool.len().pool_offset();
        section_pool.extend(items);
        let len = section_pool.len().pool_offset() - start;
        SectionRange::new(start, len)
    }

    /// Recursively resolve the dequoted content lines of a blockquote into a
    /// range of child sections, appended to `section_pool`. Handles the block
    /// constructs representable without owned storage: blank-line-separated
    /// paragraphs (multi-line, joined by soft breaks), ATX headings, thematic
    /// breaks, and nested blockquotes (via recursion). Other constructs fall
    /// back to paragraph text.
    fn resolve_quote(
        quote_lines: &[&'src str],
        pool: &mut Vec<Inline<'src>>,
        section_pool: &mut Vec<Section<'src>>,
        defs: &LinkDefs<'src>,
    ) -> SectionRange {
        // Build child sections in a local Vec first; the recursion may append
        // to `section_pool` for *its own* nested quotes, so we cannot hold a
        // stable range there until we finish. We reserve our slot at the end.
        let mut children: Vec<Section<'src>> = Vec::new();
        // Pending paragraph lines (still to be flushed as one paragraph).
        let mut para: Vec<&'src str> = Vec::new();
        // Pending nested-quote lines (dequoted one `>` level).
        let mut nested: Vec<&'src str> = Vec::new();

        let flush_para =
            |para: &mut Vec<&'src str>,
             children: &mut Vec<Section<'src>>,
             pool: &mut Vec<Inline<'src>>| {
                if para.is_empty() {
                    return;
                }
                let start = pool.len().pool_offset();
                for (i, line) in para.iter().enumerate() {
                    if i > 0 {
                        pool.push(Inline::SoftBreak);
                    }
                    InlineParser::<MAX_INLINE_DEPTH, INLINE_STACK_CAP>::parse_flat_into_configured(
                        line, pool, defs,
                    );
                }
                let len = pool.len().pool_offset() - start;
                children.push(Section::Paragraph {
                    content: InlineSpan::new(start, len),
                });
                para.clear();
            };

        let mut i = 0;
        while i < quote_lines.len() {
            let line = quote_lines[i];
            let bytes = line.as_bytes();
            let indent = bytes.strip_indent().unwrap_or(0);
            let body = &bytes[indent.min(bytes.len())..];

            // Nested blockquote line: collect consecutive `>`-prefixed lines and
            // recurse once the run ends.
            if body.first() == SpecialChar::GreaterThan {
                flush_para(&mut para, &mut children, pool);
                let after = indent + 1;
                let content = if bytes.get(after) == SpecialChar::Space {
                    line.get(after + 1..).unwrap_or("")
                } else {
                    line.get(after..).unwrap_or("")
                };
                nested.push(content);
                i += 1;
                continue;
            }
            // A non-`>` line ends any pending nested quote.
            if !nested.is_empty() {
                let range = Self::resolve_quote(&nested, pool, section_pool, defs);
                children.push(Section::Blockquote { children: range });
                nested.clear();
            }

            // Blank line: paragraph break.
            if body.is_empty() || body.iter().all(u8::is_ascii_whitespace) {
                flush_para(&mut para, &mut children, pool);
                i += 1;
                continue;
            }

            // ATX heading.
            if body.first() == SpecialChar::Hash
                && let Some((level, text)) = body.try_parse_heading(line, indent)
            {
                flush_para(&mut para, &mut children, pool);
                let content =
                    InlineParser::<MAX_INLINE_DEPTH, INLINE_STACK_CAP>::parse_configured(
                        text, pool, defs,
                    );
                children.push(Section::Heading { level, content });
                i += 1;
                continue;
            }

            // Thematic break.
            if body.is_horizontal_rule() {
                flush_para(&mut para, &mut children, pool);
                children.push(Section::HorizontalRule);
                i += 1;
                continue;
            }

            // Otherwise: paragraph text (trim the 0-3 space indent).
            para.push(line.get(indent..).unwrap_or(line));
            i += 1;
        }

        flush_para(&mut para, &mut children, pool);
        if !nested.is_empty() {
            let range = Self::resolve_quote(&nested, pool, section_pool, defs);
            children.push(Section::Blockquote { children: range });
        }

        // Append the collected children to the shared pool contiguously.
        let start = section_pool.len().pool_offset();
        section_pool.extend(children);
        let len = section_pool.len().pool_offset() - start;
        SectionRange::new(start, len)
    }

    #[must_use]
    pub fn parse(input: &'src str) -> Self {
        // --- Pass 1: block-level parsing (no inline work) ---
        let ctx = ParseCtx::block_pass(input);

        // --- Pass 2: inline parsing ---
        let mut pool = Vec::with_capacity(input.len() / 20);
        let mut section_pool = Vec::new();
        let sections = Self::resolve_inlines(&ctx, &mut pool, &mut section_pool);

        Self {
            sections,
            pool,
            section_pool,
        }
    }
}

// ---------------------------------------------------------------------------
// ParseCtx methods — pass 1 line-level fold logic
// ---------------------------------------------------------------------------

impl<'src> ParseCtx<'src> {
    /// Run pass 1: scan the input line by line, folding block-level constructs
    /// into [`RawSection`]s. Owns the entire block-parsing loop so the public
    /// [`MarkdownFile`] type never touches the `Accumulator`/`RawSection`
    /// intermediates.
    fn block_pass(input: &'src str) -> Self {
        let bytes = input.as_bytes();
        let mut ctx = ParseCtx {
            input,
            bytes,
            // Rough heuristic: ~50 bytes per section on average.
            sections: Vec::with_capacity(input.len() / 50 + 1),
            lines: Vec::with_capacity(input.len() / 80 + 1),
            defs: LinkDefs::new(),
        };
        let mut acc = Accumulator::Empty;
        let mut pos = 0;

        while pos < bytes.len() {
            let line_end = bytes
                .find_byte(pos, SpecialChar::Newline.byte())
                .unwrap_or(bytes.len());

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
                && let Some(indent) = bytes[pos..line_end].strip_indent()
                && let Some((fence_char, fence_len)) =
                    bytes[pos + indent..line_end].code_fence_opening()
            {
                let spos = pos + indent;
                let language = ctx.extract_language(&bytes[spos..line_end], fence_len);
                ctx.flush_acc(acc);
                let content_start = line_end + 1;
                let (code, resume) = ctx.scan_code_block_fast(content_start, fence_len, fence_char);
                ctx.sections.push(RawSection::CodeBlock { language, code });
                pos = resume;
                acc = Accumulator::Empty;
                continue;
            }

            // HTML blocks (CommonMark §4.6): detected by a `<` within the first
            // 4 bytes. Type 7 (a bare standalone tag) cannot interrupt an open
            // paragraph; the other six can.
            if (first == Some(b'<')
                || (first == SpecialChar::Space
                    && bytes[pos..line_end].get(..4).is_some_and(|w| w.contains(&b'<'))))
                && let Some(indent) = bytes[pos..line_end].strip_indent()
            {
                let spos = pos + indent;
                let in_paragraph = matches!(acc, Accumulator::InParagraph { .. });
                if let Some(kind) =
                    crate::raw_html::html_block_start(&bytes[spos..line_end], in_paragraph)
                {
                    ctx.flush_acc(acc);
                    let (html, resume) = ctx.scan_html_block(pos, line_end, kind);
                    ctx.sections.push(RawSection::HtmlBlock { html });
                    pos = resume;
                    acc = Accumulator::Empty;
                    continue;
                }
            }

            // Link reference definitions (CommonMark §4.7): a `[` within the
            // first 4 bytes, not interrupting a paragraph. They span up to
            // three lines and produce no output, only a registry entry.
            if !matches!(acc, Accumulator::InParagraph { .. })
                && (first == Some(b'[')
                    || (first == SpecialChar::Space
                        && bytes[pos..line_end].get(..4).is_some_and(|w| w.contains(&b'['))))
                && let Some(indent) = bytes[pos..line_end].strip_indent()
                && let Some((def, resume)) = scan_link_def(ctx.input, pos + indent)
            {
                ctx.flush_acc(acc);
                ctx.defs
                    .entry(normalize_label(def.label))
                    .or_insert((def.url, def.title));
                pos = resume;
                acc = Accumulator::Empty;
                continue;
            }

            acc = ctx.fold_line(acc, pos, line_end);
            pos = line_end + 1;
        }

        ctx.flush_acc(acc);
        ctx
    }
    /// Extract the language tag from a code fence opening line.
    /// `bytes` is the line with leading indentation already stripped.
    /// `fence_len` is the number of fence characters.
    fn extract_language(&self, bytes: &[u8], fence_len: usize) -> Option<&'src str> {
        debug_assert!(
            bytes.as_ptr() as usize >= self.input.as_ptr() as usize
                && bytes.as_ptr() as usize + bytes.len()
                    <= self.input.as_ptr() as usize + self.input.len(),
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
        let line_offset = bytes.as_ptr() as usize - self.input.as_ptr() as usize;
        self.input.get(line_offset + i..line_offset + end)
    }

    /// Scan forward from `start` to find a closing code fence of the same type
    /// (`fence_char`) and at least `fence_len` characters.
    /// Returns `(code_content, resume_position)`.
    fn scan_code_block_fast(
        &self,
        start: usize,
        fence_len: usize,
        fence_char: u8,
    ) -> (&'src str, usize) {
        let bytes = self.bytes;
        let mut pos = start;
        while pos < bytes.len() {
            let line_end = bytes
                .find_byte(pos, SpecialChar::Newline.byte())
                .unwrap_or(bytes.len());

            let first = bytes.get(pos).copied();
            if (first == Some(fence_char) || first == Some(SpecialChar::Space.byte()))
                && let Some(indent) = bytes[pos..line_end].strip_indent()
            {
                let spos = pos + indent;
                if bytes[spos..line_end].is_closing_fence(fence_char, fence_len) {
                    let code = if start < pos {
                        self.input.get(start..pos - 1).unwrap_or("")
                    } else {
                        ""
                    };
                    return (code, line_end + 1);
                }
            }
            pos = line_end + 1;
        }
        let code = self.input.get(start..).unwrap_or("");
        (code, bytes.len())
    }

    /// Scan an HTML block (`CommonMark` §4.6) starting at `start_pos`, whose
    /// first line ends at `first_line_end`. Returns `(html_slice, resume_pos)`.
    /// The returned slice spans whole lines verbatim (no trailing newline).
    fn scan_html_block(
        &self,
        start_pos: usize,
        first_line_end: usize,
        kind: crate::raw_html::HtmlBlockKind,
    ) -> (&'src str, usize) {
        use crate::raw_html::HtmlBlockKind;
        let bytes = self.bytes;

        // Helper: produce the slice [start_pos..end) trimmed of one trailing
        // newline, and the resume position after it.
        let finish = |content_end: usize, resume: usize| {
            let end = content_end.min(bytes.len());
            (self.input.get(start_pos..end).unwrap_or(""), resume)
        };

        // Types 1–5 end on the line containing their end marker.
        if matches!(
            kind,
            HtmlBlockKind::Type1
                | HtmlBlockKind::Type2
                | HtmlBlockKind::Type3
                | HtmlBlockKind::Type4
                | HtmlBlockKind::Type5
        ) {
            let mut pos = start_pos;
            let mut last_end_for_eof = start_pos;
            while pos < bytes.len() {
                let line_end = bytes
                    .find_byte(pos, SpecialChar::Newline.byte())
                    .unwrap_or(bytes.len());
                let line = &bytes[pos..line_end];
                let ends = match kind {
                    HtmlBlockKind::Type1 => crate::raw_html::type1_end(line),
                    other => other
                        .end_marker()
                        .is_some_and(|m| line.windows(m.len()).any(|w| w == m)),
                };
                if ends {
                    return finish(line_end, line_end + 1);
                }
                last_end_for_eof = line_end;
                pos = line_end + 1;
            }
            return finish(last_end_for_eof, bytes.len());
        }

        // Types 6 and 7 end at a blank line (the blank line is not included).
        let mut pos = first_line_end + 1;
        let mut last_end = first_line_end;
        while pos <= bytes.len() {
            if pos >= bytes.len() {
                break;
            }
            let line_end = bytes
                .find_byte(pos, SpecialChar::Newline.byte())
                .unwrap_or(bytes.len());
            if bytes.is_blank_line(pos, line_end) {
                return finish(last_end, line_end + 1);
            }
            last_end = line_end;
            pos = line_end + 1;
        }
        // Reached end of input: the block ends at the last non-blank line, not
        // the buffer end (which would wrongly include a trailing newline).
        finish(last_end, bytes.len())
    }

    /// Merge two subslices of the current input into one contiguous slice
    /// spanning from the start of `a` to the end of `b`.
    fn merge_slices(&self, a: &str, b: &str) -> Option<&'src str> {
        let base_start = self.input.as_ptr() as usize;
        let a_start = a.as_ptr() as usize;
        let b_end = b.as_ptr() as usize + b.len();

        if a_start < base_start || b_end > base_start + self.input.len() || b_end < a_start {
            return None;
        }

        self.input.get(a_start - base_start..b_end - base_start)
    }

    /// Flush an accumulator into this context's section list.
    fn flush_acc(&mut self, acc: Accumulator<'src>) {
        if let Accumulator::InIndentedCode {
            start,
            last_nonblank_end,
        } = acc
        {
            // The code content is the contiguous span from the first content
            // line to the end of the last non-blank line. Leading indentation
            // is preserved here and stripped (up to four spaces) at render
            // time.
            let code = self.input.get(start..last_nonblank_end).unwrap_or("");
            self.sections.push(RawSection::IndentedCode { code });
            return;
        }
        let pool_len = self.lines.len().lines_offset();
        if let Some(section) = acc.flush(pool_len) {
            self.sections.push(section);
        }
    }

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
        let blank = self.bytes.is_blank_line(pos, line_end);

        // Indented code blocks (CommonMark §4.4) own the blank-line handling:
        // interior blank lines stay part of the block (trimmed only at the
        // end), and a non-blank line with <4 spaces of indent closes it.
        if let Accumulator::InIndentedCode {
            start,
            last_nonblank_end,
        } = acc
        {
            if blank {
                return Accumulator::InIndentedCode {
                    start,
                    last_nonblank_end,
                };
            }
            if self.bytes[pos..line_end].strip_indent().is_none() {
                // Still indented ≥4 spaces: extend the block.
                return Accumulator::InIndentedCode {
                    start,
                    last_nonblank_end: line_end,
                };
            }
            // Dedented non-blank line: the code block ends here.
            self.flush_acc(Accumulator::InIndentedCode {
                start,
                last_nonblank_end,
            });
            return self.fold_block_element(Accumulator::Empty, pos, line_end);
        }

        if blank {
            self.flush_acc(acc);
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
        let Some(indent) = self.bytes[pos..line_end].strip_indent() else {
            // 4+ leading spaces.
            if let Accumulator::InBlockquote { lines_start } = acc {
                // Blockquote lazy continuation.
                self.lines.push(self.input.get(pos..line_end).unwrap_or(""));
                return Accumulator::InBlockquote { lines_start };
            }
            if matches!(acc, Accumulator::InParagraph { .. }) {
                // An indented line cannot interrupt a paragraph (CommonMark
                // §4.4): it is lazy paragraph continuation text.
                return self.fold_paragraph(acc, pos, line_end);
            }
            // Otherwise this opens an indented code block.
            self.flush_acc(acc);
            return Accumulator::InIndentedCode {
                start: pos,
                last_nonblank_end: line_end,
            };
        };
        let spos = pos + indent;
        let line_bytes = &self.bytes[spos..line_end];

        // Setext headings (CommonMark §4.3): an `=`/`-` underline directly
        // below a paragraph turns that paragraph's text into a heading. This
        // must precede both the paragraph fast-path (an `=` underline is not a
        // block-start byte, so the fast-path would otherwise swallow it) and
        // the thematic-break check (`---` after a paragraph is a level-2
        // setext underline, not an `<hr>`).
        if let Accumulator::InParagraph { content } = acc
            && let Some(level) = line_bytes.setext_heading_level()
        {
            self.sections.push(RawSection::Heading {
                level,
                text: content.trim(),
            });
            return Accumulator::Empty;
        }

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
            self.flush_acc(acc);
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
            self.flush_acc(acc);
            let lines_start = self.lines.len().lines_offset();
            self.lines.push(content);
            return Accumulator::InBlockquote { lines_start };
        }

        // Blockquote lazy continuation (CommonMark §5.1): a non-blank line
        // that doesn't start a new block-level construct continues the
        // current blockquote.
        let acc = if let Accumulator::InBlockquote { lines_start } = acc {
            if self.blockquote_continues(line_bytes, spos) {
                self.lines.push(self.input.get(pos..line_end).unwrap_or(""));
                return Accumulator::InBlockquote { lines_start };
            }
            // Line starts a new block — flush the blockquote and fall through.
            self.flush_acc(Accumulator::InBlockquote { lines_start });
            Accumulator::Empty
        } else {
            acc
        };

        // Horizontal rules (CommonMark §4.1): three or more -, *, or _
        // characters (optionally with spaces) on a line by themselves.
        if line_bytes.is_horizontal_rule() {
            self.flush_acc(acc);
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
            self.flush_acc(Accumulator::InUnorderedList {
                marker: m,
                items_start,
            });
        } else {
            self.flush_acc(acc);
        }
        let items_start = self.lines.len().lines_offset();
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
            self.flush_acc(Accumulator::InOrderedList {
                start,
                delimiter,
                items_start,
            });
        } else {
            self.flush_acc(acc);
        }
        let items_start = self.lines.len().lines_offset();
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
            return self.merge_slices(content, line_str).map_or_else(
                || {
                    self.sections.push(RawSection::Paragraph { text: content });
                    Accumulator::InParagraph { content: line_str }
                },
                |merged| Accumulator::InParagraph { content: merged },
            );
        }
        self.flush_acc(acc);
        Accumulator::InParagraph { content: line_str }
    }

    /// True if a non-blank line should continue the current blockquote rather
    /// than starting a new block-level construct.
    fn blockquote_continues(&self, line_bytes: &[u8], spos: usize) -> bool {
        if line_bytes.is_empty() || !line_bytes.could_start_block() {
            return true;
        }
        !line_bytes.is_horizontal_rule()
            && line_bytes.try_parse_heading(self.input, spos).is_none()
            && line_bytes.code_fence_opening().is_none()
            && line_bytes.try_parse_unordered_item().is_none()
            && line_bytes.try_parse_ordered_item().is_none()
    }
}
