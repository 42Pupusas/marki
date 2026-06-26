use crate::OffsetExt;
use crate::inline::InlineParser;
use crate::link_def::{LinkDefs, normalize_label_cow, scan_link_def};
use crate::section::{LineRange, OrderedListDelimiter, PoolLine, Section, SectionRange};
use crate::small_bool::SmallBoolVec;
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
        /// Columns of indentation on the opening fence (0–3). `CommonMark`
        /// §4.5 strips up to this many leading spaces from each content line.
        /// When zero the content is emitted as one contiguous slice; otherwise
        /// pass 2 dedents line-by-line into the line pool.
        indent: usize,
    },
    IndentedCode {
        code: &'src str,
    },
    Blockquote {
        lines_start: u32,
        lines_len: u32,
    },
    /// A list (`CommonMark` §5.3) captured as a contiguous run of full source
    /// lines in the line pool. Pass 1 ([`scan_list`]) splits the top-level
    /// items, dedents each item's content into the line pool, and records one
    /// [`ItemMeta`] per item plus the loose/tight flag and ordered start, so
    /// pass 2 ([`assemble_list`]) iterates the items without re-deriving
    /// boundaries or content columns. Nested sublists are still derived lazily
    /// by [`build_list`] when [`resolve_blocks`] meets their marker.
    List {
        items_start: u32,
        items_len: u32,
        tight: bool,
        ordered: Option<(u32, OrderedListDelimiter)>,
    },
    HtmlBlock {
        html: &'src str,
    },
    HorizontalRule,
}

/// Pass-1 metadata for one top-level list item: the half-open range of
/// already-dedented content lines it owns in the line pool. Emitted by
/// [`scan_list`] and consumed by [`assemble_list`] in pass 2.
#[derive(Clone, Copy)]
struct ItemMeta {
    lines_start: u32,
    lines_len: u32,
}

/// Result of [`scan_list`]: the [`ItemMeta`] range for a top-level list, its
/// loose/tight flag and ordered start, plus the byte offset to resume at.
struct ListScan {
    items_start: u32,
    items_len: u32,
    tight: bool,
    ordered: Option<(u32, OrderedListDelimiter)>,
    resume: usize,
}

/// A parsed list marker at the start of a (de-indented) line.
#[derive(Clone, Copy)]
struct Marker {
    /// `Some((start, delim))` for an ordered item, `None` for a bullet.
    ordered: Option<(u32, OrderedListDelimiter)>,
    /// Width of the marker characters only (bullet = 1; ordered = digits + 1).
    width: usize,
    /// Bullet character for unordered markers (`-`, `+`, `*`), else 0.
    bullet: u8,
}

impl Marker {
    /// True if `other` belongs to the same list as `self` (`CommonMark` §5.3:
    /// a change of bullet character or ordered delimiter starts a new list).
    fn same_family(self, other: Self) -> bool {
        match (self.ordered, other.ordered) {
            (None, None) => self.bullet == other.bullet,
            (Some((_, d1)), Some((_, d2))) => d1 == d2,
            _ => false,
        }
    }
}

/// Recycler for the temporary line `Vec`s used while resolving container
/// blocks.
///
/// `resolve_blocks` and `build_list` each need a short-lived de-indented
/// content-line buffer; allocating one fresh per item made list-dense
/// documents malloc-bound. Buffers are checked out, used, and returned, so the
/// live allocation count tracks nesting depth rather than the number of items.
///
/// Child sections themselves are no longer buffered here: the section pool is a
/// pre-order arena, so each child block is appended in place (see
/// [`MarkdownFile::resolve_blocks`]).
#[derive(Default)]
struct Scratch<'src> {
    lines: Vec<Vec<&'src str>>,
}

impl<'src> Scratch<'src> {
    fn take_lines(&mut self) -> Vec<&'src str> {
        self.lines.pop().map_or_else(Vec::new, |mut v| {
            v.clear();
            v
        })
    }
    fn give_lines(&mut self, v: Vec<&'src str>) {
        self.lines.push(v);
    }
}

/// Mutable parsing context for pass 1. Only collects raw sections — no inline
/// pool or span pool needed.
/// Recyclable scratch buffers for one [`ParseCtx`]. These are all pass-1
/// intermediates — built while scanning blocks, consumed by pass-2 inline
/// resolution, then thrown away. Because they never escape into the returned
/// [`MarkdownFile`], they can be pooled across parses (see [`CTX_POOL`]):
/// `MarkdownFile::parse` checks a set out, hands it to [`block_pass`], and
/// checks it back in cleared. On a corpus of many small documents this turns
/// six heap allocate-and-free cycles *per document* into zero after the first.
#[derive(Default)]
struct ParseScratch<'src> {
    sections: Vec<RawSection<'src>>,
    lines: Vec<&'src str>,
    lazy: Vec<bool>,
    pad: Vec<u8>,
    list_items: Vec<ItemMeta>,
    defs: LinkDefs<'src>,
}

impl ParseScratch<'_> {
    /// Empty every buffer, dropping all borrowed `&'src` references so the set
    /// holds no lifetime-bound data and can be safely re-typed to `'static`
    /// when returned to the pool.
    fn reset(&mut self) {
        self.sections.clear();
        self.lines.clear();
        self.lazy.clear();
        self.pad.clear();
        self.list_items.clear();
        self.defs.clear();
    }
}

thread_local! {
    /// Free-list of [`ParseScratch`] buffer sets, reused across `parse` calls
    /// to amortize their `Vec`/`HashMap` allocations. A `Vec` (not a single
    /// slot) so re-entrant parsing — should it ever occur — borrows distinct
    /// sets rather than clobbering one. Mirrors `inline::ARENA_POOL`.
    static CTX_POOL: std::cell::RefCell<Vec<ParseScratch<'static>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Check a cleared [`ParseScratch`] out of the thread-local pool, allocating a
/// fresh one only when the pool is empty.
///
/// SAFETY: the pool stores `ParseScratch<'static>`; we hand back a
/// `ParseScratch<'src>`. The layout is identical for every lifetime (the
/// lifetime only constrains the borrowed `&str`s the buffers hold), and the
/// set is returned via [`checkin_scratch`] only after [`ParseScratch::reset`]
/// has dropped every `'src` reference, so the pool never observes a dangling
/// borrow.
fn checkout_scratch<'src>() -> ParseScratch<'src> {
    let scratch: ParseScratch<'static> =
        CTX_POOL.with(|p| p.borrow_mut().pop()).unwrap_or_default();
    unsafe { std::mem::transmute::<ParseScratch<'static>, ParseScratch<'src>>(scratch) }
}

/// Reset `scratch` (dropping all `'src` borrows) and return it to the pool as
/// `'static` for the next parse to reuse.
fn checkin_scratch(mut scratch: ParseScratch<'_>) {
    scratch.reset();
    // SAFETY: `reset` emptied every buffer, so no `'src` reference remains;
    // re-typing the now-borrow-free set to `'static` is sound.
    let scratch: ParseScratch<'static> =
        unsafe { std::mem::transmute::<ParseScratch<'_>, ParseScratch<'static>>(scratch) };
    CTX_POOL.with(|p| p.borrow_mut().push(scratch));
}

struct ParseCtx<'src> {
    input: &'src str,
    bytes: &'src [u8],
    sections: Vec<RawSection<'src>>,
    /// Shared pool for blockquote lines and list items, avoiding per-section
    /// `Vec<&str>` heap allocations.
    lines: Vec<&'src str>,
    /// Parallel to [`lines`](Self::lines): `true` where a line was collected as
    /// a *lazy paragraph continuation* (`CommonMark` §5.1). Pass 2 consults the
    /// slice for a container's lines so a lazy line is never re-promoted to a
    /// setext heading or a sublist marker (examples 93 and 312). Always kept
    /// the same length as `lines` via [`push_line`](Self::push_line).
    lazy: Vec<bool>,
    /// Parallel to [`lines`](Self::lines): synthetic leading-space columns that
    /// were consumed *mid-tab* when a container prefix (a blockquote `>`
    /// padding or a list content column) was stripped (`CommonMark` §2.2 tab
    /// expansion). The stored slice always begins on a tab stop (a column that
    /// is a multiple of 4), so tab expansion *within* the slice stays
    /// absolute-correct; this count restores the columns lost between the
    /// container's content origin and that tab stop. Almost always `0`.
    pad: Vec<u8>,
    /// Per-item metadata for top-level lists, referenced by
    /// [`RawSection::List`]; lets pass 2 skip re-scanning item boundaries.
    list_items: Vec<ItemMeta>,
    /// Link reference definitions collected during pass 1 (`CommonMark` §4.7).
    defs: LinkDefs<'src>,
}

enum Accumulator<'src> {
    Empty,
    InBlockquote {
        lines_start: u32,
        /// True when the blockquote's inner content currently leaves an open
        /// paragraph, the only state in which a marker-less line may lazily
        /// continue the blockquote (`CommonMark` §5.1). False after a blank
        /// line, an indented code block, a code fence, or any other block.
        para_open: bool,
        /// `Some((char,len))` while the inner content is inside a fenced code
        /// block, whose lines must not be treated as lazy paragraph text.
        fence: Option<(u8, usize)>,
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
            Self::InBlockquote { lines_start, .. } => Some(RawSection::Blockquote {
                lines_start,
                lines_len: lines_pool_len - lines_start,
            }),
            Self::InParagraph { content } => Some(RawSection::Paragraph { text: content }),
        }
    }
}

// ---------------------------------------------------------------------------
// BlockBytes trait — block-level helpers on byte slices.
// ---------------------------------------------------------------------------

/// True if `line` ends in open paragraph text that a following marker-less
/// line could lazily continue (`CommonMark` §5.1). Descends through any nested
/// blockquote `>` markers, list markers, and 0-3 spaces of indentation so a
/// line like `> > foo` or `1. > foo` is judged by its innermost content
/// (`foo`), which is paragraph text even though the outer line begins a block.
fn is_lazy_paragraph_tail(mut line: &[u8]) -> bool {
    loop {
        let ind = line.leading_spaces().min(line.len());
        let body = &line[ind..];
        if body.first() == Some(&SpecialChar::GreaterThan.byte()) {
            let after = if body.get(1) == Some(&SpecialChar::Space.byte()) {
                2
            } else {
                1
            };
            line = &body[after.min(body.len())..];
            continue;
        }
        // Descend through a list marker into the item's content (e.g. the
        // `> foo` inside `1. > foo`), but not a thematic break that merely
        // looks like a bullet.
        if !body.is_horizontal_rule()
            && let Some(m) = body.list_marker()
        {
            let after = m.width;
            let rest = &body[after.min(body.len())..];
            // Require the conventional single space after the marker so we land
            // on the content column; an empty item has no open paragraph.
            if rest.first() == Some(&SpecialChar::Space.byte()) {
                line = &rest[1..];
                continue;
            }
            return false;
        }
        return !body.is_blank_line(0, body.len()) && !body.begins_block();
    }
}

/// Count leading indentation of `line` in *columns* (a tab advances to the
/// next 4-column stop, `CommonMark` §2.2). Unlike `leading_spaces` this counts
/// a leading tab as the columns it expands to.
fn leading_columns(line: &[u8]) -> usize {
    let mut col = 0;
    for &b in line {
        match b {
            b' ' => col += 1,
            b'\t' => col += 4 - (col % 4),
            _ => break,
        }
    }
    col
}

/// Strip exactly `cols` columns of leading indentation from `line`, returning
/// the remaining slice **only** when the strip lands on a byte boundary (each
/// consumed byte is a whole space, or a tab whose expansion ends at or before
/// `cols`). Returns `None` when `cols` falls in the middle of a tab — the
/// caller then needs partial-tab expansion, which this borrow-only helper
/// cannot produce, and should fall back to its existing handling.
fn strip_columns(line: &str, cols: usize) -> Option<&str> {
    let bytes = line.as_bytes();
    let mut col = 0;
    let mut i = 0;
    while col < cols {
        match bytes.get(i) {
            Some(b' ') => col += 1,
            Some(b'\t') => col += 4 - (col % 4),
            _ => return None,
        }
        i += 1;
    }
    if col == cols { line.get(i..) } else { None }
}

/// Strip `cols` columns of leading indentation from `line`, returning the
/// synthetic-space `pad` plus the remaining slice.
///
/// Unlike [`strip_columns`], this never fails on a mid-tab boundary: when the
/// requested column count lands partway through a tab, the tab's *remaining*
/// columns are returned as `pad` (synthetic spaces) and the slice begins at the
/// byte just past that tab — which sits on an absolute tab stop (a multiple of
/// 4), so any tabs *inside* the returned slice still expand correctly from a
/// local origin of 0 (`CommonMark` §2.2). If the line is shorter than `cols`,
/// the whole line is consumed and an empty slice with `pad == 0` is returned.
fn strip_columns_padded(line: &str, cols: usize) -> (u8, &str) {
    let bytes = line.as_bytes();
    let mut col = 0;
    let mut i = 0;
    while col < cols {
        match bytes.get(i) {
            Some(b' ') => col += 1,
            Some(b'\t') => col += 4 - (col % 4),
            // Ran out of indentation before reaching `cols`: nothing to pad.
            _ => return (0, line.get(i..).unwrap_or("")),
        }
        i += 1;
    }
    // `col >= cols`; any overshoot is the leftover columns of a straddled tab.
    let pad = u8::try_from(col - cols).unwrap_or(0);
    (pad, line.get(i..).unwrap_or(""))
}

/// Find the byte index in `line` at which the cumulative column count first
/// reaches `target`, expanding tabs to 4-column stops (`CommonMark` §2.2).
/// Unlike [`strip_columns_padded`] this walks *every* byte (including a list
/// marker or other non-whitespace), so it can locate a content column that
/// lies past a marker. Returns the byte index plus the synthetic-space `pad`:
/// the leftover columns when `target` lands partway through a tab. The byte at
/// the returned index sits on an absolute tab stop iff `pad > 0`.
fn byte_at_column(line: &[u8], target: usize) -> (usize, u8) {
    let mut col = 0;
    let mut i = 0;
    while col < target && i < line.len() {
        if line[i] == b'\t' {
            col += 4 - (col % 4);
        } else {
            col += 1;
        }
        i += 1;
    }
    (i, u8::try_from(col.saturating_sub(target)).unwrap_or(0))
}

/// Strip `n` columns of indentation from a padded line `(pad, slice)` — where
/// `slice` begins on an absolute tab stop and `pad` is its synthetic leading
/// spaces — returning the resulting `(pad, slice)`. Used to dedent the four
/// columns of an indented code block (`CommonMark` §4.4) while preserving any
/// straddled-tab padding. Consumes the synthetic `pad` first, then strips the
/// remainder from the slice (which may straddle a tab and produce fresh pad).
fn strip_cols_from_padded(pad: u8, slice: &str, n: usize) -> (u8, &str) {
    let pad = pad as usize;
    if n <= pad {
        (u8::try_from(pad - n).unwrap_or(0), slice)
    } else {
        strip_columns_padded(slice, n - pad)
    }
}


/// Update fenced-code-block state for one dedented list-item content line.
/// `state` is `Some((fence_char, fence_len))` while inside a fence. A line that
/// opens a fence enters the state; the matching closing fence leaves it. Used by
/// [`scan_list`] so blank lines *inside* a fenced code block are not mistaken
/// for the blank-line separators that make a list loose (`CommonMark` §5.3).
fn update_fence(state: Option<(u8, usize)>, content: &[u8]) -> Option<(u8, usize)> {
    let cind = content.leading_spaces().min(3);
    let body = &content[cind.min(content.len())..];
    match state {
        Some((fc, flen)) => {
            if body.is_closing_fence(fc, flen) {
                None
            } else {
                Some((fc, flen))
            }
        }
        None => body.code_fence_opening(),
    }
}

/// Peel leading 0-3 space indentation plus any nested blockquote (`>`) and
/// list markers from `line`, returning the byte offset of the innermost
/// content. Mirrors [`is_lazy_paragraph_tail`]'s descent so a container line
/// like `> [foo]: /url` is judged by its innermost content.
fn peel_content_offset(line: &[u8]) -> usize {
    let mut off = 0;
    loop {
        let rel_ind = line.get(off..).map_or(0, <[u8]>::leading_spaces).min(3);
        let i = off + rel_ind;
        let body = &line[i.min(line.len())..];
        if body.first() == Some(&SpecialChar::GreaterThan.byte()) {
            let after = if body.get(1) == Some(&SpecialChar::Space.byte()) {
                2
            } else {
                1
            };
            off = i + after.min(body.len());
            continue;
        }
        if !body.is_horizontal_rule()
            && let Some(m) = body.list_marker()
            && body.get(m.width) == Some(&SpecialChar::Space.byte())
        {
            off = i + m.width + 1;
            continue;
        }
        return i;
    }
}

/// Scan a container's already-dedented content `lines` for link reference
/// definitions (`CommonMark` §4.7), pushing each `(normalized label, (url,
/// title))` onto `found`. Tracks paragraph-open and fenced-code state so a
/// def-looking line inside code, or one that would lazily continue an open
/// paragraph, is not mistaken for a definition. Descends through nested
/// blockquote/list markers via [`peel_content_offset`].
fn collect_container_defs_in<'src>(
    lines: &[&'src str],
    found: &mut Vec<(std::borrow::Cow<'src, str>, (&'src str, Option<&'src str>))>,
) {
    let mut para_open = false;
    let mut fence: Option<(u8, usize)> = None;
    for &line in lines {
        let b = line.as_bytes();
        if b.is_blank_line(0, b.len()) {
            para_open = false;
            continue;
        }
        let off = peel_content_offset(b);
        let body = &b[off.min(b.len())..];
        if let Some((fc, flen)) = fence {
            if body.is_closing_fence(fc, flen) {
                fence = None;
            }
            continue;
        }
        let ind_body = body.leading_spaces();
        if ind_body >= 4 && !para_open {
            continue; // indented code block line
        }
        let inner = &body[ind_body.min(body.len())..];
        if let Some(f) = inner.code_fence_opening() {
            fence = Some(f);
            para_open = false;
            continue;
        }
        if !para_open
            && inner.first() == Some(&b'[')
            && let Some((def, _)) = scan_link_def(line, off + ind_body)
        {
            found.push((
                normalize_label_cow(def.label),
                (def.url, def.title),
            ));
            para_open = false;
            continue;
        }
        para_open = !inner.begins_block();
    }
}

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
    fn could_start_block(&self) -> bool;
    fn setext_heading_level(&self) -> Option<u8>;
    /// Count leading ASCII spaces (no tab expansion).
    fn leading_spaces(&self) -> usize;
    /// Parse a list marker at the very start of the slice (no leading indent).
    fn list_marker(&self) -> Option<Marker>;
    /// Content-indent column for a freshly-opened item whose marker starts at
    /// `marker_col` and is described by `m`; `self` is the bytes after the
    /// marker characters.
    fn item_content_indent(&self, marker_col: usize, m: Marker) -> usize;
    /// True if this de-indented slice begins a block a paragraph cannot lazily
    /// continue across.
    fn begins_block(&self) -> bool;
}

impl BlockBytes for [u8] {
    fn is_blank_line(&self, start: usize, end: usize) -> bool {
        if start >= end {
            return true;
        }
        self[start..end].iter().all(u8::is_ascii_whitespace)
    }

    fn strip_indent(&self) -> Option<usize> {
        // Measure leading whitespace in *columns* (a tab advances to the next
        // 4-column stop, CommonMark §2.2): return the byte length of 0-3
        // columns of indentation, or None once it reaches ≥4 columns (the line
        // can then only open indented code or continue it). Because a tab from
        // column 0-3 always lands on column 4, a sub-4-column result is always
        // pure spaces, so the byte length still equals the column count for
        // every caller that uses it as a strip offset.
        let mut col = 0;
        let mut n = 0;
        while let Some(&b) = self.get(n) {
            match b {
                b' ' => col += 1,
                b'\t' => col += 4 - (col % 4),
                _ => break,
            }
            if col >= 4 {
                return None;
            }
            n += 1;
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
        // The opening hash sequence must be followed by a space, a tab, or the
        // end of the line (CommonMark §4.2).
        if !(1..=6).contains(&level)
            || !matches!(self.get(level), None | Some(&b' ' | &b'\t'))
        {
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

    fn leading_spaces(&self) -> usize {
        let mut n = 0;
        while self.get(n) == Some(&SpecialChar::Space.byte()) {
            n += 1;
        }
        n
    }

    fn list_marker(&self) -> Option<Marker> {
        let &first = self.first()?;
        // Unordered bullet: `-`, `+`, or `*` followed by space/tab/EOL.
        if first == SpecialChar::Dash.byte()
            || first == SpecialChar::Plus.byte()
            || first == SpecialChar::Asterisk.byte()
        {
            return match self.get(1) {
                None | Some(b' ' | b'\t') => Some(Marker {
                    ordered: None,
                    width: 1,
                    bullet: first,
                }),
                _ => None,
            };
        }
        // Ordered marker: 1-9 digits, then `.` or `)`, then space/tab/EOL.
        if first.is_ascii_digit() {
            let mut num: u32 = 0;
            let mut digits = 0usize;
            for &d in self {
                if d.is_ascii_digit() {
                    digits += 1;
                    if digits > 9 {
                        return None;
                    }
                    num = num * 10 + u32::from(d - SpecialChar::Zero.byte());
                } else {
                    break;
                }
            }
            let delim = OrderedListDelimiter::from_byte(*self.get(digits)?)?;
            return match self.get(digits + 1) {
                None | Some(b' ' | b'\t') => Some(Marker {
                    ordered: Some((num, delim)),
                    width: digits + 1,
                    bullet: 0,
                }),
                _ => None,
            };
        }
        None
    }

    fn item_content_indent(&self, marker_col: usize, m: Marker) -> usize {
        // `self` is the bytes after the marker characters (spaces + content).
        let spaces = self.leading_spaces();
        let after_marker = marker_col + m.width;
        if spaces == 0 || spaces > 4 || spaces == self.len() {
            // Empty item or an over-indented first line: one-space indent (the
            // surplus becomes indented-code content).
            after_marker + 1
        } else {
            after_marker + spaces
        }
    }

    fn begins_block(&self) -> bool {
        if self.is_empty() || self.first() == SpecialChar::GreaterThan {
            return true;
        }
        if self.list_marker().is_some()
            || self.is_horizontal_rule()
            || self.code_fence_opening().is_some()
        {
            return true;
        }
        // ATX heading.
        let hashes = SpecialChar::Hash.count_leading_bytes(self);
        (1..=6).contains(&hashes) && matches!(self.get(hashes), None | Some(b' ' | b'\t'))
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
    #[allow(clippy::too_many_lines)]
    fn resolve_inlines(
        ctx: &ParseCtx<'src>,
        pool: &mut Vec<Inline<'src>>,
        section_pool: &mut Vec<Section<'src>>,
        line_pool: &mut Vec<PoolLine<'src>>,
    ) -> Vec<Section<'src>> {
        let lines = &ctx.lines;
        // Link reference definitions may also live inside containers
        // (blockquotes / list items), and a reference can appear *earlier* in
        // the document than its container-nested definition. So sweep every
        // container's collected lines for defs up front, merging them with the
        // top-level ones (top-level / earlier defs win via `or_insert`).
        let combined_defs: LinkDefs<'src>;
        let defs: &LinkDefs<'src> = if ctx
            .sections
            .iter()
            .any(|s| matches!(s, RawSection::Blockquote { .. } | RawSection::List { .. }))
        {
            let mut found = Vec::new();
            for raw in &ctx.sections {
                match *raw {
                    RawSection::Blockquote {
                        lines_start,
                        lines_len,
                    } => {
                        let raw_lines = lines
                            .get(lines_start as usize..(lines_start + lines_len) as usize)
                            .unwrap_or(&[]);
                        collect_container_defs_in(raw_lines, &mut found);
                    }
                    RawSection::List {
                        items_start,
                        items_len,
                        ..
                    } => {
                        let metas = ctx
                            .list_items
                            .get(items_start as usize..(items_start + items_len) as usize)
                            .unwrap_or(&[]);
                        for meta in metas {
                            let item_lines = lines
                                .get(
                                    meta.lines_start as usize
                                        ..(meta.lines_start + meta.lines_len) as usize,
                                )
                                .unwrap_or(&[]);
                            collect_container_defs_in(item_lines, &mut found);
                        }
                    }
                    _ => {}
                }
            }
            if found.is_empty() {
                &ctx.defs
            } else {
                let mut map = ctx.defs.clone();
                for (label, val) in found {
                    map.entry(label).or_insert(val);
                }
                combined_defs = map;
                &combined_defs
            }
        } else {
            &ctx.defs
        };
        let mut scratch = Scratch::default();
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
                RawSection::CodeBlock {
                    language,
                    code,
                    indent,
                } => {
                    if indent == 0 {
                        // Common case: no dedent needed, keep the zero-copy slice.
                        sections.push(Section::CodeBlock { language, code });
                    } else {
                        // CommonMark §4.5: strip up to `indent` leading spaces
                        // from each content line. Removing interior bytes breaks
                        // contiguity, so dedent into the line pool and emit the
                        // line-backed `CodeLines` variant instead.
                        let code_start = line_pool.len().pool_offset();
                        if !code.is_empty() {
                            for line in code.split('\n') {
                                let strip =
                                    line.bytes().take(indent).take_while(|&b| b == b' ').count();
                                line_pool.push(PoolLine::plain(line.get(strip..).unwrap_or("")));
                            }
                        }
                        let len = line_pool.len().pool_offset() - code_start;
                        sections.push(Section::CodeLines {
                            language,
                            lines: LineRange::new(code_start, len),
                        });
                    }
                }
                RawSection::IndentedCode { code } => {
                    sections.push(Section::IndentedCode { code });
                }
                RawSection::List {
                    items_start,
                    items_len,
                    tight,
                    ordered,
                } => {
                    let metas = ctx
                        .list_items
                        .get(items_start as usize..(items_start + items_len) as usize)
                        .unwrap_or(&[]);
                    let list = Self::assemble_list(
                        metas,
                        lines,
                        &ctx.lazy,
                        &ctx.pad,
                        tight,
                        ordered,
                        pool,
                        section_pool,
                        line_pool,
                        &mut scratch,
                        defs,
                    );
                    sections.push(list);
                }
                RawSection::Blockquote {
                    lines_start,
                    lines_len,
                } => {
                    let span = lines_start as usize..(lines_start + lines_len) as usize;
                    let raw_lines = lines.get(span.clone()).unwrap_or(&[]);
                    let raw_lazy = ctx.lazy.get(span.clone()).unwrap_or(&[]);
                    let raw_pad = ctx.pad.get(span).unwrap_or(&[]);
                    let children = Self::resolve_blocks(
                        raw_lines,
                        raw_lazy,
                        raw_pad,
                        pool,
                        section_pool,
                        line_pool,
                        &mut scratch,
                        defs,
                    );
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

    /// Assemble a top-level list [`Section`] from the per-item metadata that
    /// [`scan_list`] produced in pass 1. Each [`ItemMeta`] already points at
    /// its item's dedented content lines, so this just resolves each item's
    /// child blocks via [`resolve_blocks`] and stitches the `ListItem`s
    /// together — no item-boundary or content-column work is repeated.
    #[allow(clippy::too_many_arguments)]
    fn assemble_list(
        metas: &[ItemMeta],
        lines: &[&'src str],
        lazy: &[bool],
        pad: &[u8],
        tight: bool,
        ordered: Option<(u32, OrderedListDelimiter)>,
        pool: &mut Vec<Inline<'src>>,
        section_pool: &mut Vec<Section<'src>>,
        line_pool: &mut Vec<PoolLine<'src>>,
        scratch: &mut Scratch<'src>,
        defs: &LinkDefs<'src>,
    ) -> Section<'src> {
        // Pre-order: each item is a ListItem container followed immediately by
        // its child-block subtree.
        let items_start = section_pool.len().pool_offset();
        for meta in metas {
            let span = meta.lines_start as usize..(meta.lines_start + meta.lines_len) as usize;
            let item_lines = lines.get(span.clone()).unwrap_or(&[]);
            let item_lazy = lazy.get(span.clone()).unwrap_or(&[]);
            let item_pad = pad.get(span).unwrap_or(&[]);
            let item_at = section_pool.len();
            section_pool.push(Section::ListItem {
                children: SectionRange::EMPTY,
            });
            let range = Self::resolve_blocks(
                item_lines, item_lazy, item_pad, pool, section_pool, line_pool, scratch, defs,
            );
            section_pool[item_at] = Section::ListItem { children: range };
        }
        let items_len = section_pool.len().pool_offset() - items_start;
        let items = SectionRange::new(items_start, items_len);

        if let Some((start, delimiter)) = ordered {
            Section::OrderedList {
                start,
                delimiter,
                tight,
                items,
            }
        } else {
            Section::UnorderedList { tight, items }
        }
    }

    /// Recursively resolve a sequence of container content lines into a range
    /// of child block [`Section`]s appended to `section_pool`. This is the
    /// shared line-based block engine used by both blockquote interiors and
    /// list items. Handles paragraphs (with soft-break reflow and setext
    /// promotion), ATX headings, thematic breaks, fenced and indented code,
    /// nested blockquotes, and nested lists.
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    fn resolve_blocks(
        lines: &[&'src str],
        lazy: &[bool],
        pad: &[u8],
        pool: &mut Vec<Inline<'src>>,
        section_pool: &mut Vec<Section<'src>>,
        line_pool: &mut Vec<PoolLine<'src>>,
        scratch: &mut Scratch<'src>,
        defs: &LinkDefs<'src>,
    ) -> SectionRange {
        let pad_at = |k: usize| pad.get(k).copied().unwrap_or(0);
        // Leading indentation of pooled line `k` in *columns*. When the line
        // carries straddled-tab pad (`pad > 0`) it begins on an absolute tab
        // stop, so its own tabs expand correctly from a local origin and the
        // total indent is `pad + leading_columns`. When `pad == 0` we keep the
        // historical `leading_spaces` measure exactly (a bare leading tab in a
        // container is *not* promoted to indented code here), so every
        // pre-existing path is byte-for-byte unchanged.
        let indent_cols = |k: usize| {
            let b = lines[k].as_bytes();
            let p = pad_at(k);
            if p > 0 {
                p as usize + leading_columns(b)
            } else {
                b.leading_spaces()
            }
        };
        // Pre-order arena: append this forest's nodes directly to the section
        // pool. Containers push a placeholder, recurse to append their subtree
        // immediately after, then backpatch their child span. The returned
        // range covers everything appended (children plus descendants).
        let start = section_pool.len().pool_offset();
        let mut para = scratch.take_lines();
        let mut i = 0;

        while i < lines.len() {
            let line = lines[i];
            let b = line.as_bytes();

            if b.iter().all(u8::is_ascii_whitespace) {
                Self::flush_para(&mut para, section_pool, pool, defs);
                i += 1;
                continue;
            }

            // Block-level indentation gates are column-based (≥4 = code,
            // ≤3 = ordinary block); the byte offset for slicing the body is
            // computed separately. `indent_cols` reduces to the historical
            // `leading_spaces` whenever `pad == 0`, so existing paths are
            // unchanged.
            let ind_cols = indent_cols(i);
            let ind = b.leading_spaces();

            // Indented code block (≥4 columns), but only when not continuing an
            // open paragraph (an indented line is lazy paragraph text there).
            if ind_cols >= 4 && para.is_empty() {
                let code_start = line_pool.len().pool_offset();
                while i < lines.len() {
                    let l = lines[i];
                    let lb = l.as_bytes();
                    if lb.iter().all(u8::is_ascii_whitespace) {
                        // Interior blank: keep, but only if a later indented
                        // line follows (trailing blanks are trimmed below).
                        line_pool.push(PoolLine::plain(""));
                        i += 1;
                        continue;
                    }
                    if indent_cols(i) < 4 {
                        break;
                    }
                    // Strip the four indent columns, preserving any straddled-
                    // tab pad so the rendered code keeps its leading spaces.
                    let (cpad, ctext) = strip_cols_from_padded(pad_at(i), l, 4);
                    line_pool.push(PoolLine { pad: cpad, text: ctext });
                    i += 1;
                }
                // Trim trailing blank lines out of the code block.
                while line_pool.len().pool_offset() > code_start
                    && line_pool.last().is_some_and(|l| l.text.is_empty() && l.pad == 0)
                {
                    line_pool.pop();
                }
                let len = line_pool.len().pool_offset() - code_start;
                section_pool.push(Section::CodeLines {
                    language: None,
                    lines: LineRange::new(code_start, len),
                });
                continue;
            }

            let body = &b[ind.min(b.len())..];

            // Link reference definition inside this container: it produces no
            // output (its label was already collected into `defs` by the
            // pre-pass in `resolve_inlines`), so skip the line when it isn't
            // lazily continuing an open paragraph.
            if para.is_empty()
                && body.first() == Some(&b'[')
                && scan_link_def(line, ind).is_some()
            {
                i += 1;
                continue;
            }

            if ind_cols <= 3 {
                // Nested blockquote: collect the run and recurse.
                if body.first() == SpecialChar::GreaterThan {
                    Self::flush_para(&mut para, section_pool, pool, defs);
                    let mut nested = scratch.take_lines();
                    // `last_para` tracks whether the previous collected line is
                    // paragraph content that a following marker-less line may
                    // lazily continue (CommonMark §5.1). A blank `>` line or a
                    // line that begins a block closes that paragraph.
                    let mut last_para = false;
                    // Parallel lazy flags for the collected nested lines.
                    // Inline storage spills to the heap only for long runs.
                    let mut nested_lazy = SmallBoolVec::new();
                    while i < lines.len() {
                        let l = lines[i];
                        let lb = l.as_bytes();
                        let qind = lb.leading_spaces().min(3);
                        let lbody = &lb[qind.min(lb.len())..];
                        if lbody.first() == SpecialChar::GreaterThan {
                            let after = qind + 1;
                            let content = if lb.get(after) == Some(&SpecialChar::Space.byte()) {
                                l.get(after + 1..).unwrap_or("")
                            } else {
                                l.get(after..).unwrap_or("")
                            };
                            last_para = is_lazy_paragraph_tail(content.as_bytes());
                            nested.push(content);
                            // A `>`-marked line carries its own lazy status down.
                            nested_lazy.push(lazy.get(i).copied().unwrap_or(false));
                            i += 1;
                            continue;
                        }
                        // Lazy paragraph continuation: a marker-less, non-blank
                        // line that doesn't begin a block extends the paragraph
                        // still open inside the blockquote. Recursing on the
                        // collected lines carries it down to the innermost one.
                        let indent = lb.leading_spaces().min(lb.len());
                        if last_para
                            && !lb.is_blank_line(0, lb.len())
                            && !lb[indent..].begins_block()
                        {
                            nested.push(l.trim());
                            // This line lazily continues the inner paragraph.
                            nested_lazy.push(true);
                            i += 1;
                            continue;
                        }
                        break;
                    }
                    // Placeholder, recurse to append the subtree, then backpatch.
                    let at = section_pool.len();
                    section_pool.push(Section::Blockquote {
                        children: SectionRange::EMPTY,
                    });
                    // Nested-container content does not carry straddled-tab
                    // pad (that lives only on the top-level container's pooled
                    // lines); an empty pad slice reads as all-zero.
                    let range = Self::resolve_blocks(
                        &nested,
                        &nested_lazy,
                        &[],
                        pool,
                        section_pool,
                        line_pool,
                        scratch,
                        defs,
                    );
                    scratch.give_lines(nested);
                    section_pool[at] = Section::Blockquote { children: range };
                    continue;
                }

                // ATX heading.
                if body.first() == SpecialChar::Hash
                    && let Some((level, text)) = body.try_parse_heading(line, ind)
                {
                    Self::flush_para(&mut para, section_pool, pool, defs);
                    let content =
                        InlineParser::<MAX_INLINE_DEPTH, INLINE_STACK_CAP>::parse_configured(
                            text, pool, defs,
                        );
                    section_pool.push(Section::Heading { level, content });
                    i += 1;
                    continue;
                }

                // Fenced code block.
                if let Some((fence_char, fence_len)) = body.code_fence_opening() {
                    Self::flush_para(&mut para, section_pool, pool, defs);
                    let language = Self::fence_language(line, ind, fence_len);
                    let code_start = line_pool.len().pool_offset();
                    i += 1;
                    while i < lines.len() {
                        let l = lines[i];
                        let lb = l.as_bytes();
                        let cind = lb.leading_spaces().min(3);
                        if lb[cind.min(lb.len())..].is_closing_fence(fence_char, fence_len) {
                            i += 1;
                            break;
                        }
                        // Strip up to the opening fence's indentation.
                        line_pool.push(PoolLine::plain(l.get(ind.min(l.len())..).unwrap_or("")));
                        i += 1;
                    }
                    let len = line_pool.len().pool_offset() - code_start;
                    section_pool.push(Section::CodeLines {
                        language,
                        lines: LineRange::new(code_start, len),
                    });
                    continue;
                }

                // HTML block (CommonMark §4.6). Detect a start condition on the
                // de-indented line; type 7 cannot interrupt an open paragraph.
                if body.first() == SpecialChar::LessThan
                    && let Some(kind) =
                        crate::raw_html::html_block_start(body, !para.is_empty())
                {
                    Self::flush_para(&mut para, section_pool, pool, defs);
                    let html_start = line_pool.len().pool_offset();
                    i += Self::collect_html_block(&lines[i..], kind, line_pool);
                    let len = line_pool.len().pool_offset() - html_start;
                    section_pool.push(Section::HtmlLines {
                        lines: LineRange::new(html_start, len),
                    });
                    continue;
                }

                // Setext heading underline promotes the open paragraph. The
                // promoted lines keep soft breaks between them, matching how a
                // paragraph would have rendered. A line that arrived as a lazy
                // paragraph continuation (CommonMark §5.1) is paragraph text,
                // never an underline (example 93).
                if !para.is_empty()
                    && !lazy.get(i).copied().unwrap_or(false)
                    && let Some(level) = body.setext_heading_level()
                {
                    // `para` already holds trimmed lines, so no further copy.
                    let content =
                        InlineParser::<MAX_INLINE_DEPTH, INLINE_STACK_CAP>::parse_lines_configured(
                            &para, pool, defs,
                        );
                    section_pool.push(Section::Heading { level, content });
                    para.clear();
                    i += 1;
                    continue;
                }

                // Thematic break.
                if body.is_horizontal_rule() {
                    Self::flush_para(&mut para, section_pool, pool, defs);
                    section_pool.push(Section::HorizontalRule);
                    i += 1;
                    continue;
                }

                // Nested list (a thematic break takes precedence over a bullet).
                // A lazy continuation line stays paragraph text rather than
                // opening a sublist (example 312).
                if !body.is_horizontal_rule()
                    && body.list_marker().is_some()
                    && !lazy.get(i).copied().unwrap_or(false)
                {
                    Self::flush_para(&mut para, section_pool, pool, defs);
                    let consumed = Self::build_list(
                        &lines[i..],
                        &lazy[i..],
                        pad.get(i..).unwrap_or(&[]),
                        pool,
                        section_pool,
                        line_pool,
                        scratch,
                        defs,
                    );
                    i += consumed;
                    continue;
                }
            }

            // Otherwise: paragraph text. Leading 0-3 indent is stripped; deeper
            // indentation on a continuation line is lazy paragraph text.
            para.push(line.trim());
            i += 1;
        }

        Self::flush_para(&mut para, section_pool, pool, defs);

        scratch.give_lines(para);
        let len = section_pool.len().pool_offset() - start;
        SectionRange::new(start, len)
    }

    /// Collect the lines of an HTML block (`CommonMark` §4.6) from a container's
    /// already-dedented `lines`, pushing each verbatim onto the line pool.
    /// Returns the number of lines consumed. Types 1–5 end on the line holding
    /// their end marker; types 6–7 end at (but exclude) a blank line.
    fn collect_html_block(
        lines: &[&'src str],
        kind: crate::raw_html::HtmlBlockKind,
        line_pool: &mut Vec<PoolLine<'src>>,
    ) -> usize {
        use crate::raw_html::HtmlBlockKind;
        let mut n = 0;
        while n < lines.len() {
            let l = lines[n];
            let lb = l.as_bytes();
            // Types 6 and 7 terminate before a blank line.
            if matches!(kind, HtmlBlockKind::Type6 | HtmlBlockKind::Type7)
                && lb.is_blank_line(0, lb.len())
            {
                break;
            }
            line_pool.push(PoolLine::plain(l));
            n += 1;
            // Types 1–5 terminate on the line containing their end marker.
            let ends = match kind {
                HtmlBlockKind::Type1 => crate::raw_html::type1_end(lb),
                HtmlBlockKind::Type6 | HtmlBlockKind::Type7 => false,
                other => other
                    .end_marker()
                    .is_some_and(|m| lb.windows(m.len()).any(|w| w == m)),
            };
            if ends {
                break;
            }
        }
        n
    }

    /// Flush pending paragraph lines into one [`Section::Paragraph`] appended
    /// to the section pool, joining multiple lines with soft breaks.
    fn flush_para(
        para: &mut Vec<&'src str>,
        section_pool: &mut Vec<Section<'src>>,
        pool: &mut Vec<Inline<'src>>,
        defs: &LinkDefs<'src>,
    ) {
        if para.is_empty() {
            return;
        }
        let content = InlineParser::<MAX_INLINE_DEPTH, INLINE_STACK_CAP>::parse_lines_configured(
            para, pool, defs,
        );
        section_pool.push(Section::Paragraph { content });
        para.clear();
    }

    /// Extract the language from a fenced-code opening line, given the line's
    /// 0-3 space indent and the fence character count.
    fn fence_language(line: &'src str, ind: usize, fence_len: usize) -> Option<&'src str> {
        let b = line.as_bytes();
        let mut i = ind + fence_len;
        while b.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        let mut end = b.len();
        while end > i && b[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
        if i >= end {
            return None;
        }
        line.get(i..end)
    }

    /// Group a run of lines beginning with a list marker into items, resolve
    /// each item's child blocks, and append the list [`Section`] (plus its
    /// pre-order subtree) to the section pool. Returns the number of input
    /// lines consumed.
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    fn build_list(
        lines: &[&'src str],
        lazy: &[bool],
        pad: &[u8],
        pool: &mut Vec<Inline<'src>>,
        section_pool: &mut Vec<Section<'src>>,
        line_pool: &mut Vec<PoolLine<'src>>,
        scratch: &mut Scratch<'src>,
        defs: &LinkDefs<'src>,
    ) -> usize {
        // A nested sublist's content is dedented by a pure-space content
        // column (a straddled container tab only ever appears on the *outer*
        // container's pooled lines), so collected item lines carry no
        // synthetic pad; `item_pad` below is all zeros. The parameter exists so
        // `pad` stays aligned with `lines` across the recursion.
        let _ = pad;
        let first_ind = lines[0].as_bytes().leading_spaces().min(3);
        let first_marker = lines[0].as_bytes()[first_ind..]
            .list_marker()
            .expect("build_list called on a marker line");
        let ordered = first_marker.ordered;

        // Pre-order layout: push the list node placeholder, then append each
        // item directly after it. Each item is a ListItem container whose own
        // subtree (its child blocks) follows it immediately. A single scratch
        // line buffer is reused for every item's de-indented content (one heap
        // allocation per list level, not per item).
        let list_at = section_pool.len();
        section_pool.push(Section::HorizontalRule); // placeholder, backpatched below
        let items_start = section_pool.len().pool_offset();
        let mut item = scratch.take_lines();
        let mut loose = false;
        let mut i = 0;
        let mut pending_blanks = 0usize;
        let mut first = true;

        while i < lines.len() {
            let head = lines[i].as_bytes();
            let ind = head.leading_spaces().min(head.len());
            let body = &head[ind..];

            // A sibling marker at indent <= 3 opens a new item. A thematic
            // break (`* * *`) takes precedence over a bullet and ends the list.
            if ind > 3 || body.is_horizontal_rule() {
                break;
            }
            let Some(marker) = body.list_marker() else {
                break;
            };
            if !first && !first_marker.same_family(marker) {
                break;
            }
            first = false;

            if pending_blanks > 0 {
                loose = true;
            }
            pending_blanks = 0;
            // Content column: every continuation line of this item must be
            // indented at least this far to belong to it.
            let col = head[ind + marker.width..].item_content_indent(ind, marker);
            let first_content = lines[i].get(col.min(lines[i].len())..).unwrap_or("");
            item.clear();
            // Parallel lazy flags for this item's collected lines. No pad
            // buffer: collected item lines are dedented by a pure-space
            // content column, so their pad is uniformly zero — and
            // `resolve_blocks` reads a missing pad entry as 0, so an empty
            // slice is byte-for-byte equivalent (see the `&[]` pad argument).
            let mut item_lazy = SmallBoolVec::new();
            if !first_content.is_empty() {
                item.push(first_content);
                item_lazy.push(lazy.get(i).copied().unwrap_or(false));
            }
            i += 1;
            // Gather continuation lines for this item.
            let mut item_blanks = 0usize;
            // Threshold of the outermost open sub-container (see `scan_list`):
            // a blank interior to a nested list/blockquote must not loosen this
            // list.
            let mut sub_col: Option<usize> = None;
            while i < lines.len() {
                let cont = lines[i];
                let cb = cont.as_bytes();
                if cb.iter().all(u8::is_ascii_whitespace) {
                    item_blanks += 1;
                    item.push("");
                    item_lazy.push(false);
                    i += 1;
                    continue;
                }
                let cind = cb.leading_spaces();
                if cind >= col {
                    let dind = cind - col;
                    let inner = &cb[cind.min(cb.len())..];
                    // Does this line continue the currently-open sub-container?
                    let in_sub = sub_col.is_some_and(|th| dind >= th);
                    if item_blanks > 0 && !in_sub {
                        loose = true;
                    }
                    if !in_sub {
                        sub_col = if (!inner.is_horizontal_rule() && inner.list_marker().is_some())
                            || inner.first() == Some(&SpecialChar::GreaterThan.byte())
                        {
                            Some(dind + 1)
                        } else {
                            None
                        };
                    }
                    item_blanks = 0;
                    item.push(cont.get(col..).unwrap_or(""));
                    item_lazy.push(lazy.get(i).copied().unwrap_or(false));
                    i += 1;
                    continue;
                }
                // Dedented sibling marker or lazy paragraph continuation?
                if cind <= 3 && cb[cind.min(cb.len())..].list_marker().is_some() {
                    break;
                }
                if item_blanks == 0 && !cb[cind.min(cb.len())..].begins_block() {
                    item.push(cont.trim());
                    item_lazy.push(false);
                    i += 1;
                    continue;
                }
                break;
            }
            // Trailing blank lines belong to the list, not the item.
            while item.last().is_some_and(|l| l.is_empty()) {
                item.pop();
                item_lazy.pop();
                pending_blanks += 1;
            }
            // ListItem placeholder, then append its subtree and backpatch.
            let item_at = section_pool.len();
            section_pool.push(Section::ListItem {
                children: SectionRange::EMPTY,
            });
            let range = Self::resolve_blocks(
                &item, &item_lazy, &[], pool, section_pool, line_pool, scratch, defs,
            );
            section_pool[item_at] = Section::ListItem { children: range };
        }
        scratch.give_lines(item);

        let items_len = section_pool.len().pool_offset() - items_start;
        let items_range = SectionRange::new(items_start, items_len);
        let tight = !loose;

        section_pool[list_at] = if let Some((start, delimiter)) = ordered {
            Section::OrderedList {
                start,
                delimiter,
                tight,
                items: items_range,
            }
        } else {
            Section::UnorderedList {
                tight,
                items: items_range,
            }
        };
        i
    }

    #[must_use]
    pub fn parse(input: &'src str) -> Self {
        // --- Pass 1: block-level parsing (no inline work) ---
        // Borrow recycled scratch buffers from the thread-local pool; pass 1
        // fills them, pass 2 reads them, then they go back cleared.
        let ctx = ParseCtx::block_pass(input, checkout_scratch());

        // --- Pass 2: inline parsing ---
        // Pre-size the pools from pass-1 counts so large container-heavy
        // documents allocate once instead of doubling from zero. Pass 1
        // already knows the exact list-item count; each item yields one
        // `ListItem` section plus at least one child block, so `2 * items` is
        // a tight lower bound. Flat documents have zero list items, so this
        // reserves nothing and the common path is unchanged.
        let mut pool = Vec::with_capacity(input.len() / 20);
        let mut section_pool = Vec::with_capacity(ctx.list_items.len().saturating_mul(2));
        let mut line_pool = Vec::new();
        let sections = Self::resolve_inlines(&ctx, &mut pool, &mut section_pool, &mut line_pool);

        // Return the scratch buffers to the thread-local pool (reset clears the
        // `'src` borrows) so the next parse on this thread reuses their
        // capacity instead of re-allocating.
        let ParseCtx {
            sections: ctx_sections,
            lines,
            lazy,
            pad,
            list_items,
            defs,
            ..
        } = ctx;
        checkin_scratch(ParseScratch {
            sections: ctx_sections,
            lines,
            lazy,
            pad,
            list_items,
            defs,
        });

        Self {
            sections,
            pool,
            section_pool,
            line_pool,
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
    #[allow(clippy::too_many_lines)]
    fn block_pass(input: &'src str, scratch: ParseScratch<'src>) -> Self {
        let bytes = input.as_bytes();
        // Adopt the recycled (already-cleared) buffers from the thread-local
        // pool. After the first parse on a thread these arrive with capacity
        // intact, so pass 1 fills them without touching the allocator.
        let ParseScratch {
            sections,
            lines,
            lazy,
            pad,
            list_items,
            defs,
        } = scratch;
        let mut ctx = ParseCtx {
            input,
            bytes,
            sections,
            lines,
            lazy,
            pad,
            list_items,
            defs,
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
                ctx.sections.push(RawSection::CodeBlock {
                    language,
                    code,
                    indent,
                });
                pos = resume;
                acc = Accumulator::Empty;
                continue;
            }

            // HTML blocks (CommonMark §4.6): detected by a `<` within the first
            // 4 bytes. Type 7 (a bare standalone tag) cannot interrupt an open
            // paragraph; the other six can.
            if (first == Some(b'<')
                || (first == SpecialChar::Space
                    && bytes[pos..line_end]
                        .get(..4)
                        .is_some_and(|w| w.contains(&b'<'))))
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
                        && bytes[pos..line_end]
                            .get(..4)
                            .is_some_and(|w| w.contains(&b'['))))
                && let Some(indent) = bytes[pos..line_end].strip_indent()
                && let Some((def, resume)) = scan_link_def(ctx.input, pos + indent)
            {
                ctx.flush_acc(acc);
                ctx.defs
                    .entry(normalize_label_cow(def.label))
                    .or_insert((def.url, def.title));
                pos = resume;
                acc = Accumulator::Empty;
                continue;
            }

            // Lists (CommonMark §5.2/§5.3): when a line opens a list item, scan
            // the whole list run (all items + their continuation lines) in one
            // shot and let pass 2 resolve item structure. A list can interrupt
            // a paragraph only with a non-empty first item, and an ordered list
            // only when it starts at 1.
            if let Some(indent) = bytes[pos..line_end].strip_indent()
                && !bytes[pos + indent..line_end].is_horizontal_rule()
                && let Some(m) = bytes[pos + indent..line_end].list_marker()
            {
                let in_para = matches!(acc, Accumulator::InParagraph { .. });
                let rest = &bytes[pos + indent + m.width..line_end];
                let empty_item = rest.iter().all(u8::is_ascii_whitespace);
                let interrupts_ok = !in_para
                    || (!empty_item
                        && match m.ordered {
                            Some((n, _)) => n == 1,
                            None => true,
                        });
                if interrupts_ok {
                    ctx.flush_acc(acc);
                    let scan = ctx.scan_list(pos);
                    ctx.sections.push(RawSection::List {
                        items_start: scan.items_start,
                        items_len: scan.items_len,
                        tight: scan.tight,
                        ordered: scan.ordered,
                    });
                    pos = scan.resume;
                    acc = Accumulator::Empty;
                    continue;
                }
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

    /// Scan a whole top-level list (`CommonMark` §5.2/§5.3) starting at
    /// `start`. Splits the list into items in a single byte walk, pushes each
    /// item's *dedented* content lines onto the line pool, and records an
    /// [`ItemMeta`] per item plus the loose/tight flag and ordered start.
    ///
    /// This subsumes the item-boundary and content-column work that pass 2
    /// used to redo: [`assemble_list`] now just maps each [`ItemMeta`] through
    /// [`resolve_blocks`]. Nested sublists are still derived lazily in pass 2
    /// (the dedented item lines are re-scanned only when their own marker is
    /// met), so this walk only tracks the *outermost* item structure.
    #[allow(clippy::too_many_lines)]
    fn scan_list(&mut self, start: usize) -> ListScan {
        let bytes = self.bytes;
        let line_end_of = |p: usize| {
            bytes
                .find_byte(p, SpecialChar::Newline.byte())
                .unwrap_or(bytes.len())
        };

        let items_start = self.list_items.len().lines_offset();
        let ind0 = bytes[start..line_end_of(start)].leading_spaces();
        let family = bytes[start + ind0..line_end_of(start)]
            .list_marker()
            .expect("caller verified a marker");
        let ordered = family.ordered;

        let mut pos = start;
        let mut loose = false;
        // Per-item state, (re)initialised at each marker line.
        let mut item_start = self.lines.len().lines_offset();
        let mut col; // content column of the current item
        let mut item_blanks = 0usize; // blanks seen inside the current item
        let mut pending_blanks = 0usize; // trailing blanks not yet attributed
        let mut last_para = true;
        // `Some((char,len))` while the current item's content is inside a fenced
        // code block; blank lines there must not loosen the list.
        let mut fence: Option<(u8, usize)> = None;
        // Set when the current item is empty (marker only) and a blank line has
        // followed: a later indented line cannot then join this item.
        let mut empty_then_blank = false;
        // Looseness must only count a blank that separates two *direct* children
        // of the item (CommonMark §5.3: "directly contain"). A blank *inside* a
        // sub-container (a nested list or blockquote) does not loosen the outer
        // list. `sub_col` is the dedented-indent threshold of the outermost open
        // sub-container: a post-blank line at or beyond it continues that
        // sub-container (no loosening); a line below it is a direct child.
        let mut sub_col: Option<usize> = None;

        // Open the first item.
        {
            let le = line_end_of(pos);
            col = bytes[start + ind0 + family.width..le].item_content_indent(ind0, family);
            fence = update_fence(fence, &bytes[(start + ind0 + family.width).min(le)..le]);
            self.push_marker_content(pos, le, col);
            pos = le + 1;
        }

        while pos < bytes.len() {
            let le = line_end_of(pos);
            let line = &bytes[pos..le];

            if line.iter().all(u8::is_ascii_whitespace) {
                // An empty list item (marker with no content) followed by a
                // blank line begins with a blank line, so it cannot contain a
                // following continuation block (CommonMark §5.2). Record this so
                // the continuation branch below won't absorb the next indented
                // line into the empty item; a sibling marker can still extend
                // the list.
                if self.lines.len().lines_offset() == item_start && fence.is_none() {
                    empty_then_blank = true;
                }
                // A blank line inside a fenced code block stays part of the
                // block and does not make the list loose.
                if fence.is_none() {
                    item_blanks += 1;
                }
                self.push_line("", false);
                pos = le + 1;
                last_para = false;
                continue;
            }

            let ind = line.leading_spaces();

            // A continuation line may reach the content column via a tab even
            // when its leading *spaces* fall short (e.g. `\tbar` is 4 columns).
            if ind < col && !empty_then_blank && leading_columns(line) >= col {
                let src = self.input.get(pos..le).unwrap_or("");
                if let Some(content) = strip_columns(src, col) {
                    // Clean byte boundary (pure-space prefix, or a tab landing
                    // exactly on the content column): borrow the slice as-is.
                    if item_blanks > 0 {
                        loose = true;
                    }
                    item_blanks = 0;
                    fence = update_fence(fence, content.as_bytes());
                    self.push_line(content, false);
                    pos = le + 1;
                    last_para = true;
                    continue;
                } else if leading_columns(line) - col >= 4 {
                    // The content column lands mid-tab *and* the remainder is
                    // indented code (≥4 columns past the column). Emit the
                    // straddled tab's leftover columns as synthetic `pad`; the
                    // slice resumes on a tab stop so it still renders with the
                    // correct leading spaces (`CommonMark` §2.2, example 5).
                    // When the remainder is shallower it is a nested list /
                    // paragraph, handled byte-wise by the paths below — feeding
                    // pad there would confuse the byte-based sublist parser.
                    let (cpad, content) = strip_columns_padded(src, col);
                    if item_blanks > 0 {
                        loose = true;
                    }
                    item_blanks = 0;
                    fence = update_fence(fence, content.as_bytes());
                    self.push_line_padded(content, false, cpad);
                    pos = le + 1;
                    last_para = true;
                    continue;
                }
            }

            // Continuation indented to the item's content column.
            if ind >= col {
                // An empty item followed by a blank line cannot absorb later
                // content: end the list here so it parses outside.
                if empty_then_blank {
                    break;
                }
                // Dedented indentation of this line within the item frame, and
                // its content past that indentation.
                let dind = ind - col;
                let content = self.input.get(pos + col..le).unwrap_or("");
                let inner = &content.as_bytes()[dind.min(content.len())..];
                // A preceding blank loosens the list only when this line is a
                // *direct* child of the item. If an inner sub-container (nested
                // list / blockquote) was open and this line lies within it
                // (indented past its marker), the blank was interior to that
                // sub-container and must not loosen the outer list.
                let in_sub = sub_col.is_some_and(|th| dind >= th);
                if item_blanks > 0 && !in_sub {
                    loose = true;
                }
                item_blanks = 0;
                // Update the open-sub-container threshold. A line that does not
                // continue the current sub-container becomes a new direct child:
                // a nested list/blockquote (re)opens a sub-container at its
                // marker column; anything else closes it.
                if !in_sub {
                    sub_col = if (!inner.is_horizontal_rule() && inner.list_marker().is_some())
                        || inner.first() == Some(&SpecialChar::GreaterThan.byte())
                    {
                        Some(dind + 1)
                    } else {
                        None
                    };
                }
                fence = update_fence(fence, content.as_bytes());
                self.push_dedented(pos, le, col);
                pos = le + 1;
                last_para = true;
                continue;
            }

            // A thematic break (`* * *`) ends the list rather than opening a
            // new item, even though its first run looks like a bullet marker.
            if ind <= 3 && bytes[pos + ind..le].is_horizontal_rule() {
                break;
            }

            // Dedented sibling marker: close this item, open the next.
            if ind <= 3
                && let Some(m) = bytes[pos + ind..le].list_marker()
            {
                if !family.same_family(m) {
                    break;
                }
                self.close_item(item_start, &mut pending_blanks);
                if pending_blanks > 0 {
                    loose = true;
                    pending_blanks = 0;
                }
                item_start = self.lines.len().lines_offset();
                col = bytes[pos + ind + m.width..le].item_content_indent(ind, m);
                item_blanks = 0;
                empty_then_blank = false;
                sub_col = None;
                fence = update_fence(None, &bytes[(pos + ind + m.width).min(le)..le]);
                self.push_marker_content(pos, le, col);
                pos = le + 1;
                last_para = true;
                continue;
            }

            // Lazy paragraph continuation. A marker in the "dead zone" —
            // indented past the 3-space sibling threshold but short of the item
            // content column — can neither open a sub-item (too shallow) nor a
            // sibling (too deep), so it is ordinary lazy paragraph text and must
            // stay text in pass 2 (flagged lazy; CommonMark example 312).
            let body = &bytes[pos + ind..le];
            let dead_zone_marker =
                ind > 3 && ind < col && !body.is_horizontal_rule() && body.list_marker().is_some();
            if item_blanks == 0 && last_para && (!body.begins_block() || dead_zone_marker) {
                // Lazy lines are dedented by trimming (they sit left of `col`).
                self.push_line(self.input.get(pos + ind..le).unwrap_or(""), dead_zone_marker);
                pos = le + 1;
                continue;
            }
            break;
        }

        // Close the final item.
        self.close_item(item_start, &mut pending_blanks);

        let items_len = self.list_items.len().lines_offset() - items_start;
        ListScan {
            items_start,
            items_len,
            tight: !loose,
            ordered,
            resume: pos,
        }
    }

    /// Push one line onto the pool, recording whether it arrived as a lazy
    /// paragraph continuation. Keeps `lines` and `lazy` the same length.
    fn push_line(&mut self, line: &'src str, lazy: bool) {
        self.push_line_padded(line, lazy, 0);
    }

    /// Like [`push_line`](Self::push_line) but records `pad` synthetic leading
    /// columns consumed mid-tab when the container prefix was stripped (see
    /// [`ParseCtx::pad`]). The stored `line` begins on an absolute tab stop.
    fn push_line_padded(&mut self, line: &'src str, lazy: bool, pad: u8) {
        self.lines.push(line);
        self.lazy.push(lazy);
        self.pad.push(pad);
    }

    /// Pop the last pooled line (and its lazy/pad metadata), returning the line.
    fn pop_line(&mut self) -> Option<&'src str> {
        self.lazy.pop();
        self.pad.pop();
        self.lines.pop()
    }

    /// Push one source line `[pos..le)` onto the line pool with its first `col`
    /// columns of indentation removed (the item's content column). Lines
    /// shorter than `col` (e.g. blanks) collapse to `""`. When `col` lands
    /// mid-tab the straddled tab's leftover columns are recorded as synthetic
    /// `pad` (see [`ParseCtx::pad`]); for a pure-space prefix this is identical
    /// to a plain byte strip with `pad == 0`.
    fn push_dedented(&mut self, pos: usize, le: usize, col: usize) {
        let line = self.input.get(pos..le).unwrap_or("");
        let (byte, pad) = byte_at_column(line.as_bytes(), col);
        self.push_line_padded(line.get(byte..).unwrap_or(""), false, pad);
    }

    /// Like [`push_dedented`] but skips a marker line whose content is empty,
    /// so an empty item (`-` alone) contributes zero lines and does not look
    /// like a trailing blank that would mark the list loose.
    fn push_marker_content(&mut self, pos: usize, le: usize, col: usize) {
        let line = self.input.get(pos..le).unwrap_or("");
        let (byte, pad) = byte_at_column(line.as_bytes(), col);
        let content = line.get(byte..).unwrap_or("");
        if !content.is_empty() {
            self.push_line_padded(content, false, pad);
        }
    }

    /// Finish the item whose dedented lines start at `item_start`: trim its
    /// trailing blank lines out of the line pool (they belong to the list, not
    /// the item), counting them into `pending_blanks`, and record an
    /// [`ItemMeta`] spanning the remaining lines.
    fn close_item(&mut self, item_start: u32, pending_blanks: &mut usize) {
        while self.lines.len().lines_offset() > item_start
            && self.lines.last().is_some_and(|l| l.is_empty())
        {
            self.pop_line();
            *pending_blanks += 1;
        }
        let lines_len = self.lines.len().lines_offset() - item_start;
        self.list_items.push(ItemMeta {
            lines_start: item_start,
            lines_len,
        });
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
    #[allow(clippy::too_many_lines)]
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
            if let Accumulator::InBlockquote {
                lines_start,
                para_open,
                fence,
            } = acc
            {
                // Lazy continuation only when an inner paragraph is open and we
                // are not inside a fenced code block.
                if para_open && fence.is_none() {
                    self.push_line(self.input.get(pos..line_end).unwrap_or(""), true);
                    return Accumulator::InBlockquote {
                        lines_start,
                        para_open: true,
                        fence: None,
                    };
                }
                self.flush_acc(Accumulator::InBlockquote {
                    lines_start,
                    para_open,
                    fence,
                });
                return self.fold_block_element(Accumulator::Empty, pos, line_end);
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
            // Strip the `>` marker plus up to one column of following
            // whitespace (`CommonMark` §5.1). When that one column lands inside
            // a tab, the tab's remaining columns survive as synthetic `pad`
            // (§2.2): the content slice resumes on the next tab stop, so its own
            // tabs still expand from a local origin of 0. `>` sits at column
            // `indent` (the 0-3 space prefix is pure spaces).
            let content_start = spos + 1;
            let (bq_pad, content) = match self.bytes.get(content_start) {
                Some(&b' ') => (0, self.input.get(content_start + 1..line_end).unwrap_or("")),
                Some(&b'\t') => {
                    // Tab begins at column `indent + 1`; consuming one column
                    // leaves `(tab_width - 1)` columns of pad.
                    let tab_col = indent + 1;
                    let pad = u8::try_from((4 - (tab_col % 4)).saturating_sub(1)).unwrap_or(0);
                    (pad, self.input.get(content_start + 1..line_end).unwrap_or(""))
                }
                _ => (0, self.input.get(content_start..line_end).unwrap_or("")),
            };
            let cb = content.as_bytes();
            let (prev_para, prev_fence) = match acc {
                Accumulator::InBlockquote {
                    para_open, fence, ..
                } => (para_open, fence),
                _ => (false, None),
            };
            // Track inner paragraph / fenced-code state so a later lazy line is
            // only absorbed when an inner paragraph is genuinely open.
            let (para_open, fence) =
                Self::blockquote_inner_state(cb, bq_pad, prev_para, prev_fence);
            if let Accumulator::InBlockquote { lines_start, .. } = acc {
                self.push_line_padded(content, false, bq_pad);
                return Accumulator::InBlockquote {
                    lines_start,
                    para_open,
                    fence,
                };
            }
            self.flush_acc(acc);
            let lines_start = self.lines.len().lines_offset();
            self.push_line_padded(content, false, bq_pad);
            return Accumulator::InBlockquote {
                lines_start,
                para_open,
                fence,
            };
        }

        // Blockquote lazy continuation (CommonMark §5.1): a non-blank line
        // that doesn't start a new block-level construct continues the
        // current blockquote — but only when the blockquote's last line was
        // not blank (a blank line closes the inner paragraph, ending laziness).
        let acc = if let Accumulator::InBlockquote {
            lines_start,
            para_open,
            fence,
        } = acc
        {
            // Lazy continuation requires an open inner paragraph (not blank,
            // not inside indented/fenced code).
            if para_open && fence.is_none() && self.blockquote_continues(line_bytes, spos) {
                self.push_line(self.input.get(pos..line_end).unwrap_or(""), true);
                return Accumulator::InBlockquote {
                    lines_start,
                    para_open: true,
                    fence: None,
                };
            }
            // Line starts a new block — flush the blockquote and fall through.
            self.flush_acc(Accumulator::InBlockquote {
                lines_start,
                para_open,
                fence,
            });
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

        self.fold_paragraph(acc, pos, line_end)
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

    /// Compute the blockquote's inner `(para_open, fence)` state after one
    /// quoted content line `cb` (the bytes after the `>` marker), given the
    /// previous state. This decides whether a following marker-less line may
    /// lazily continue the quote: only when an inner paragraph is open and no
    /// fenced code block is active.
    fn blockquote_inner_state(
        cb: &[u8],
        bq_pad: u8,
        prev_para: bool,
        prev_fence: Option<(u8, usize)>,
    ) -> (bool, Option<(u8, usize)>) {
        // Inside a fenced code block: the only thing that matters is whether
        // this line closes it. No paragraph is open either way.
        if let Some((fc, flen)) = prev_fence {
            let cind = cb.leading_spaces().min(3);
            let body = &cb[cind.min(cb.len())..];
            if body.is_closing_fence(fc, flen) {
                return (false, None);
            }
            return (false, Some((fc, flen)));
        }
        // A blank line closes any open paragraph.
        if cb.is_blank_line(0, cb.len()) {
            return (false, None);
        }
        // Indent in *columns*: any synthetic pad from a straddled `>`-tab plus
        // the content's own leading whitespace (the slice starts on a tab
        // stop, so its tabs expand from a local 0). With `pad == 0` this is the
        // historical `leading_spaces` measure, so existing paths are unchanged.
        let cols = bq_pad as usize + leading_columns(cb);
        let ind = cb.leading_spaces();
        let body = &cb[ind.min(cb.len())..];
        // An opening code fence (0-3 indent) starts a fenced block.
        if cols <= 3
            && let Some(fence) = body.code_fence_opening()
        {
            return (false, Some(fence));
        }
        // 4+ columns of indent with no open paragraph is indented code, which a
        // lazy line cannot continue.
        if cols >= 4 && !prev_para {
            return (false, None);
        }
        // A nested blockquote (or list item) can itself leave an inner
        // paragraph open, which a lazy line continues at the deepest level;
        // judge laziness by the innermost content via is_lazy_paragraph_tail.
        if ind <= 3 && body.begins_block() {
            return (is_lazy_paragraph_tail(body), None);
        }
        (true, None)
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
            && line_bytes.list_marker().is_none()
    }
}
