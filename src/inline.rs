use crate::OffsetExt;
use crate::SpecialChar;
use crate::VecReuse;
use crate::link_def::{LinkDefs, LinkLabel};
use crate::raw_html::HtmlScan;
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
        /// The alt text parsed as inlines (`CommonMark` §6.4): the renderer
        /// flattens it to plain text, dropping emphasis/link markup but keeping
        /// the textual content (e.g. `![foo *bar*]` → alt `foo bar`).
        alt: InlineSpan,
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
    // Code spans, autolinks, and raw HTML bind tighter than link brackets, so
    // the closing-bracket scan must recognise and skip over them.
    SpecialChar::Backtick.byte(),
    SpecialChar::LessThan.byte(),
]);
static PAREN_CLOSE_SET: ByteSet = ByteSet::new(&[
    SpecialChar::OpenParen.byte(),
    SpecialChar::CloseParen.byte(),
    SpecialChar::Backslash.byte(),
]);

/// Emphasis delimiter characters (`*` and `_`). A single SIMD scan for this set
/// decides whether an inline context needs the full delimiter-stack algorithm
/// (below) or can take the existing allocation-free fast path.
static EMPH_ONLY_SET: ByteSet =
    ByteSet::new(&[SpecialChar::Asterisk.byte(), SpecialChar::Underscore.byte()]);

// ===========================================================================
// CommonMark emphasis: delimiter-stack algorithm (spec §6.2 / Appendix).
//
// marki stores inlines in a flat, post-order pool (`InlineSpan { start, len }`):
// a parent node's children occupy a contiguous range *earlier* in the pool, and
// the parent is appended after them. The reference algorithm works on a doubly
// linked list; we mirror it with an index-based arena (`EmphNode`) plus a
// parallel delimiter stack (`EmphDelim`), then emit the resolved tree into the
// pool in post-order so the contiguity invariant holds.
// ===========================================================================

/// Sentinel for "no node / no link" in the index-based linked lists.
const NIL: i32 = -1;

/// Covering-slice helpers on the inline context's source `str`.
trait SrcStr {
    fn contiguous_merge<'a>(&'a self, a: &'a str, b: &'a str) -> Option<&'a str>;
}

impl SrcStr for str {
    /// If `a` and `b` are adjacent slices that both lie within `self`, return
    /// the single sub-slice of `self` spanning both (`a` immediately followed
    /// by `b`). Used to merge split text fragments — e.g. an unmatched `*`
    /// delimiter and the word after it — back into one `Text` node so the
    /// inline pool stays compact.
    ///
    /// The merged slice is re-derived from `self`, which has provenance over
    /// the whole inline context, rather than from `a`'s pointer (whose borrow
    /// only covers `a`'s own bytes — reading past it into `b` is undefined
    /// behaviour under Stacked Borrows). All addresses are compared as
    /// integers, never dereferenced, so a stray slice from another allocation
    /// simply fails the containment check and is left unmerged.
    fn contiguous_merge<'a>(&'a self, a: &'a str, b: &'a str) -> Option<&'a str> {
        let base = self.as_ptr() as usize;
        let a_start = (a.as_ptr() as usize).checked_sub(base)?;
        // `a` and `b` must be immediately adjacent...
        if a.as_ptr() as usize + a.len() != b.as_ptr() as usize {
            return None;
        }
        // ...and the combined run must lie entirely within `self`.
        let total = a.len() + b.len();
        if a_start + total > self.len() {
            return None;
        }
        self.get(a_start..a_start + total)
    }
}

/// A node in the inline list processed by the emphasis resolver.
#[derive(Clone, Copy)]
enum EmphKind<'src> {
    /// Literal text. For a delimiter run this is the run's source slice, sliced
    /// down to the unconsumed length as delimiters are used.
    Text(&'src str),
    /// A fully-resolved inline whose children (if any) already live in the pool
    /// (code spans, links, images, autolinks, raw HTML, line breaks).
    Resolved(Inline<'src>),
    /// Emphasis (`Italic`) or strong (`Bold`) wrapping a child sub-list. The
    /// children are `head ..` following `next` links until `NIL`.
    Emph { strong: bool, head: i32 },
    /// An unlinked node; never emitted.
    Removed,
}

/// An arena slot: a node plus its neighbours in the (doubly linked) inline list.
struct EmphNode<'src> {
    kind: EmphKind<'src>,
    prev: i32,
    next: i32,
}

/// An entry on the delimiter stack: a run of `*` or `_` that may open and/or
/// close emphasis. Mirrors commonmark.js `delimiters`.
struct EmphDelim {
    /// Index of the backing text node in the arena.
    node: i32,
    ch: u8,
    /// Remaining (unconsumed) delimiter count.
    count: i32,
    /// Original count, for the rule-of-3 (`origdelims`).
    orig: i32,
    can_open: bool,
    can_close: bool,
    /// Previous/next on the delimiter stack (not the node list).
    prev: i32,
    next: i32,
}

/// Working state for one emphasis-resolution pass over a single inline context.
/// Holds the node arena and the delimiter stack; reused per parse call.
#[derive(Default)]
struct EmphArena<'src> {
    nodes: Vec<EmphNode<'src>>,
    delims: Vec<EmphDelim>,
    /// Backing inline-context string that every `Text` node slices from. Used
    /// to re-derive merged text runs with whole-context provenance (see
    /// [`contiguous_merge`]). Empty between parses so the `'static` pool never
    /// retains a borrow.
    src: &'src str,
    /// Reusable post-order emit stack (see [`EmphArena::emit_list`]).
    scratch: Vec<Inline<'src>>,
    /// Head/tail of the top-level node list.
    head: i32,
    tail: i32,
    /// Top of the delimiter stack (most recently pushed), or `NIL`.
    delim_top: i32,
}

thread_local! {
    /// Free-list of emphasis arenas, reused across parse calls to amortize the
    /// arena's `Vec` allocations. Parsing is re-entrant (a link's text is a
    /// nested inline context), so this is a stack: each [`with_arena`] call
    /// checks one out and returns it cleared.
    static ARENA_POOL: std::cell::RefCell<Vec<EmphArena<'static>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

impl EmphArena<'_> {
    /// Re-type an emptied arena to a different lifetime, recycling each buffer's
    /// heap allocation via [`crate::reuse_alloc`]. Every `Vec` is cleared first
    /// (dropping all borrowed elements), so no borrow of the old lifetime
    /// survives into the result. This is the safe replacement for the lifetime
    /// `transmute` the pooled arena used to require.
    fn relifetime<'dst>(self) -> EmphArena<'dst> {
        EmphArena {
            nodes: self.nodes.reuse_alloc(),
            delims: self.delims.reuse_alloc(),
            src: "",
            scratch: self.scratch.reuse_alloc(),
            head: NIL,
            tail: NIL,
            delim_top: NIL,
        }
    }
}

/// Run `f` with a recycled [`EmphArena`], returning it to the thread-local pool
/// afterwards. Buffers are recycled across the lifetime boundary by
/// [`EmphArena::relifetime`] (which clears them), so no `'src` reference ever
/// persists in the `'static` pool, and no `unsafe` is involved.
fn with_arena<'src, R>(f: impl FnOnce(&mut EmphArena<'src>) -> R) -> R {
    // Check out a pooled arena and re-type it to `'src`, reusing its
    // allocations. Both callers `reset()` the arena before filling it, so we
    // hand over the cleared buffers as-is.
    let mut arena: EmphArena<'src> = ARENA_POOL
        .with(|p| p.borrow_mut().pop())
        .map_or_else(EmphArena::default, EmphArena::relifetime);
    let out = f(&mut arena);
    // Return it to the pool as `'static`, again recycling the allocations and
    // dropping every `'src` borrow in the process.
    let pooled: EmphArena<'static> = arena.relifetime();
    ARENA_POOL.with(|p| p.borrow_mut().push(pooled));
    out
}

// The emphasis arena is an index-based linked list: node/delimiter handles are
// `i32` (sentinel `NIL = -1`) and index into `Vec`s whose length is bounded by
// the document's inline-pool cap (already `u32::MAX`). The `i32`<->`usize`
// conversions are therefore provably in range; suppress the cast lints here
// rather than threading `try_from` through every hot-loop access.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]
impl<'src> EmphArena<'src> {
    fn reset(&mut self) {
        self.nodes.clear();
        self.delims.clear();
        self.src = "";
        self.head = NIL;
        self.tail = NIL;
        self.delim_top = NIL;
    }

    /// Append a node to the end of the top-level list, returning its index.
    fn push_node(&mut self, kind: EmphKind<'src>) -> i32 {
        let idx = self.nodes.len() as i32;
        let prev = self.tail;
        self.nodes.push(EmphNode {
            kind,
            prev,
            next: NIL,
        });
        if prev == NIL {
            self.head = idx;
        } else {
            self.nodes[prev as usize].next = idx;
        }
        self.tail = idx;
        idx
    }

    /// Append plain text, merging into the previous node when it is also text
    /// (keeps the arena small and avoids adjacent `<text>` fragments).
    fn push_text(&mut self, s: &'src str) {
        if s.is_empty() {
            return;
        }
        self.push_node(EmphKind::Text(s));
    }

    /// Push a delimiter run (its text node and a delimiter-stack entry).
    fn push_delim(&mut self, src: &'src str, ch: u8, count: i32, can_open: bool, can_close: bool) {
        let node = self.push_node(EmphKind::Text(src));
        let idx = self.delims.len() as i32;
        let prev = self.delim_top;
        self.delims.push(EmphDelim {
            node,
            ch,
            count,
            orig: count,
            can_open,
            can_close,
            prev,
            next: NIL,
        });
        if prev != NIL {
            self.delims[prev as usize].next = idx;
        }
        self.delim_top = idx;
    }

    /// Unlink a node from the top-level list (used when a delimiter is fully
    /// consumed).
    fn unlink_node(&mut self, idx: i32) {
        let (prev, next) = {
            let n = &self.nodes[idx as usize];
            (n.prev, n.next)
        };
        if prev == NIL {
            self.head = next;
        } else {
            self.nodes[prev as usize].next = next;
        }
        if next == NIL {
            self.tail = prev;
        } else {
            self.nodes[next as usize].prev = prev;
        }
        self.nodes[idx as usize].kind = EmphKind::Removed;
    }

    /// Remove a delimiter from the delimiter stack (node list untouched).
    fn remove_delim(&mut self, idx: i32) {
        let (prev, next) = {
            let d = &self.delims[idx as usize];
            (d.prev, d.next)
        };
        if prev != NIL {
            self.delims[prev as usize].next = next;
        }
        if next == NIL {
            self.delim_top = prev;
        } else {
            self.delims[next as usize].prev = prev;
        }
    }

    /// commonmark.js `removeDelimitersBetween`: drop stack entries strictly
    /// between `bottom` and `top`.
    fn remove_delims_between(&mut self, bottom: i32, top: i32) {
        if self.delims[bottom as usize].next != top {
            self.delims[bottom as usize].next = top;
            self.delims[top as usize].prev = bottom;
        }
    }

    /// Shorten a delimiter run's backing text node by `used` characters from
    /// the *end* (closer) or *start* (opener). Emphasis consumes delimiters
    /// from the inner edge of each run.
    fn shorten_text(&mut self, node: i32, used: i32, from_start: bool) {
        if let EmphKind::Text(s) = self.nodes[node as usize].kind {
            let used = used as usize;
            let new = if from_start {
                &s[used.min(s.len())..]
            } else {
                &s[..s.len().saturating_sub(used)]
            };
            self.nodes[node as usize].kind = EmphKind::Text(new);
        }
    }

    /// Resolve emphasis/strong over the delimiter stack (commonmark.js
    /// `processEmphasis` with `stack_bottom = NULL`). After this returns, the
    /// top-level node list (`head`..) is the final inline sequence, with `Emph`
    /// nodes wrapping their resolved children.
    fn process_emphasis(&mut self) {
        // openers_bottom indexed by: 2 buckets (can_open) * 3 (origdelims%3),
        // separately for `*` and `_`. 12 slots; init to NIL (= stack_bottom).
        let mut openers_bottom = [NIL; 12];

        // First closer above stack_bottom = bottom of the stack.
        let mut closer = self.delim_top;
        if closer == NIL {
            return;
        }
        while self.delims[closer as usize].prev != NIL {
            closer = self.delims[closer as usize].prev;
        }

        while closer != NIL {
            if !self.delims[closer as usize].can_close {
                closer = self.delims[closer as usize].next;
                continue;
            }
            let cc = self.delims[closer as usize].ch;
            let closer_can_open = self.delims[closer as usize].can_open;
            let closer_orig = self.delims[closer as usize].orig;
            let base = if cc == SpecialChar::Underscore { 0 } else { 6 };
            let ob_index =
                base + (if closer_can_open { 3 } else { 0 }) + (closer_orig.rem_euclid(3)) as usize;

            // Look back for the first matching opener.
            let mut opener = self.delims[closer as usize].prev;
            let mut opener_found = false;
            while opener != NIL && opener != openers_bottom[ob_index] {
                let od = &self.delims[opener as usize];
                let odd_match = (closer_can_open || od.can_close)
                    && closer_orig.rem_euclid(3) != 0
                    && (od.orig + closer_orig).rem_euclid(3) == 0;
                if od.ch == cc && od.can_open && !odd_match {
                    opener_found = true;
                    break;
                }
                opener = od.prev;
            }
            let old_closer = closer;

            if opener_found {
                let use_delims = if self.delims[closer as usize].count >= 2
                    && self.delims[opener as usize].count >= 2
                {
                    2
                } else {
                    1
                };
                let opener_node = self.delims[opener as usize].node;
                let closer_node = self.delims[closer as usize].node;

                // Consume delimiters from the inner edges.
                self.delims[opener as usize].count -= use_delims;
                self.delims[closer as usize].count -= use_delims;
                self.shorten_text(opener_node, use_delims, false);
                self.shorten_text(closer_node, use_delims, true);

                // Gather nodes strictly between opener_node and closer_node
                // into a child sub-list under a new Emph node, inserted right
                // after opener_node.
                let head = self.nodes[opener_node as usize].next;
                // Detach [head .. closer_node) from the main list.
                let strong = use_delims == 2;
                let emph = self.push_node_detached(EmphKind::Emph { strong, head });
                // Re-link child range: terminate it before closer_node.
                let mut last_child = NIL;
                let mut t = head;
                while t != NIL && t != closer_node {
                    last_child = t;
                    t = self.nodes[t as usize].next;
                }
                if last_child != NIL {
                    self.nodes[last_child as usize].next = NIL;
                }
                // Splice emph between opener_node and closer_node.
                self.nodes[opener_node as usize].next = emph;
                self.nodes[emph as usize].prev = opener_node;
                self.nodes[emph as usize].next = closer_node;
                self.nodes[closer_node as usize].prev = emph;

                self.remove_delims_between(opener, closer);

                if self.delims[opener as usize].count == 0 {
                    self.unlink_node(opener_node);
                    self.remove_delim(opener);
                }
                if self.delims[closer as usize].count == 0 {
                    self.unlink_node(closer_node);
                    let next = self.delims[closer as usize].next;
                    self.remove_delim(closer);
                    closer = next;
                }
            } else {
                closer = self.delims[closer as usize].next;
            }

            if !opener_found {
                openers_bottom[ob_index] = self.delims[old_closer as usize].prev;
                if !self.delims[old_closer as usize].can_open {
                    self.remove_delim(old_closer);
                }
            }
        }
    }

    /// Allocate a node without linking it into the top-level list. Used for
    /// `Emph` wrappers, which are spliced in manually.
    fn push_node_detached(&mut self, kind: EmphKind<'src>) -> i32 {
        let idx = self.nodes.len() as i32;
        self.nodes.push(EmphNode {
            kind,
            prev: NIL,
            next: NIL,
        });
        idx
    }

    /// Emit the resolved node list starting at `head` (following `next` links)
    /// into `pool` in post-order, returning the span of the top-level run.
    ///
    /// `scratch` is used as an explicit stack: each frame appends its own
    /// top-level markers to `scratch[mark..]`, recursing into `Emph` children
    /// first (which flush *their* content to `pool` and return a span). When
    /// the frame finishes it bulk-copies its markers from `scratch` to `pool`,
    /// keeping every level's run contiguous — the pool's post-order invariant.
    fn emit_list(&mut self, head: i32, pool: &mut Vec<Inline<'src>>) -> InlineSpan {
        let src = self.src;
        let mark = self.scratch.len();
        let mut t = head;
        // Tracks whether the last pushed scratch entry is a backslash-escaped
        // ampersand (`Resolved(Inline::Text("&"))`). Such a node must not absorb
        // a following contiguous text run: `\&ouml;` splits into `&` and
        // `ouml;`, and merging them back would let the renderer decode the
        // (escaped, hence literal) `&` as an entity. Other escaped punctuation
        // is harmless to merge, so we keep that compaction.
        let mut last_is_escaped = false;
        while t != NIL {
            let node = &self.nodes[t as usize];
            let next = node.next;
            match node.kind {
                EmphKind::Text(s) => {
                    if !s.is_empty() {
                        // Merge with the previous emitted text node when the two
                        // source slices are contiguous (e.g. unmatched `*` plus
                        // the following word), keeping the pool compact.
                        if !last_is_escaped
                            && let Some(Inline::Text(prev)) = self.scratch.last_mut()
                            && let Some(merged) = src.contiguous_merge(prev, s)
                        {
                            *prev = merged;
                        } else {
                            self.scratch.push(Inline::Text(s));
                        }
                        last_is_escaped = false;
                    }
                }
                EmphKind::Resolved(inl) => {
                    self.scratch.push(inl);
                    last_is_escaped = matches!(inl, Inline::Text("&"));
                }
                EmphKind::Emph { strong, head } => {
                    let span = self.emit_list(head, pool);
                    self.scratch.push(if strong {
                        Inline::Bold(span)
                    } else {
                        Inline::Italic(span)
                    });
                    last_is_escaped = false;
                }
                EmphKind::Removed => {}
            }
            t = next;
        }
        let start = pool.len().pool_offset();
        let len = (self.scratch.len() - mark).pool_offset();
        pool.extend(self.scratch.drain(mark..));
        InlineSpan::new(start, len)
    }
}

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
            | '\u{20A0}'..='\u{20CF}' // Currency symbols (Sc), e.g. U+20AC EURO
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

/// Stack-allocated buffer for collecting inline elements without heap allocation.
/// The capacity `CAP` is configurable via `MarkdownFile`'s `INLINE_STACK_CAP`
/// const generic — falls back to heap if exceeded.
///
/// `Inline` is `Copy` with a cheap unit variant ([`Inline::SoftBreak`]), so the
/// inline storage is a plain initialized array seeded with that filler rather
/// than `MaybeUninit`. This keeps the whole type safe — no `assume_init` /
/// `from_raw_parts` — at the cost of writing `CAP` filler values once in
/// [`new`](Self::new); only `0..len` are ever read back.
struct InlineBuf<'src, const CAP: usize> {
    stack: [Inline<'src>; CAP],
    len: usize,
    overflow: Vec<Inline<'src>>,
}

impl<'src, const CAP: usize> InlineBuf<'src, CAP> {
    #[inline]
    const fn new() -> Self {
        Self {
            // Filler the array with a cheap unit variant; only `0..len` is read.
            stack: [Inline::SoftBreak; CAP],
            len: 0,
            overflow: Vec::new(),
        }
    }

    #[allow(clippy::inline_always)]
    #[inline(always)]
    fn push(&mut self, item: Inline<'src>) {
        if self.len < CAP {
            self.stack[self.len] = item;
            self.len += 1;
        } else {
            self.push_slow(item);
        }
    }

    #[cold]
    fn push_slow(&mut self, item: Inline<'src>) {
        if self.overflow.is_empty() {
            // Spill stack to heap.
            self.overflow = Vec::with_capacity(CAP * 2);
            self.overflow.extend_from_slice(&self.stack[..self.len]);
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
        } else if self.len > 0
            && matches!(
                self.stack[self.len - 1],
                Inline::SoftBreak | Inline::HardBreak
            )
        {
            self.len -= 1;
        }
    }

    /// Get initialized stack elements as a slice.
    #[inline]
    fn initialized_stack(&self) -> &[Inline<'src>] {
        &self.stack[..self.len]
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
        input: &'src str,
        lines: &[&'src str],
        pool: &'pool mut Vec<Inline<'src>>,
        defs: &'pool LinkDefs<'src>,
    ) -> InlineSpan {
        // Join the lines with soft breaks and parse as one context so emphasis
        // can span line boundaries (`CommonMark` reflows joined paragraph
        // lines). When any line carries emphasis, route the whole run through
        // the arena path; otherwise keep the allocation-free buffer path.
        let any_emph = lines
            .iter()
            .any(|l| l.as_bytes().find_byte_set(0, &EMPH_ONLY_SET).is_some());
        if any_emph {
            return with_arena(|arena| Self::parse_lines_into_arena(input, lines, arena, pool, defs));
        }
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
            // Reborrow the pool for just this line so the next iteration (and
            // the final flush) can borrow it again.
            let mut parser: InlineParser<'src, '_, MAX_DEPTH, CAP> =
                InlineParser::new(line, &mut *pool, defs);
            parser.parse_into_buf(bytes, &mut buf, 0);
        }
        buf.flush_to_pool(pool)
    }

    /// Parse multiple soft-break-joined lines into one [`EmphArena`], so a
    /// single emphasis run can span line boundaries, then emit to the pool.
    fn parse_lines_into_arena(
        input: &'src str,
        lines: &[&'src str],
        arena: &mut EmphArena<'src>,
        pool: &'pool mut Vec<Inline<'src>>,
        defs: &'pool LinkDefs<'src>,
    ) -> InlineSpan {
        arena.reset();
        // Every collected line is a sub-slice of the document `input`, so it is
        // the covering slice with provenance over all of them — the slice a
        // within-line text merge re-derives its combined run from. (A merge
        // never crosses a line boundary: consecutive lines are separated by a
        // `SoftBreak`, which is `Resolved` and never a mergeable `Text`.)
        arena.src = input;
        for (idx, line) in lines.iter().enumerate() {
            if idx > 0 {
                arena.push_node(EmphKind::Resolved(Inline::SoftBreak));
            }
            let mut parser: InlineParser<'src, '_, MAX_DEPTH, CAP> =
                InlineParser::new(line, &mut *pool, defs);
            parser.scan_line_into_arena(line.as_bytes(), arena, 0);
        }
        arena.process_emphasis();
        arena.emit_list(arena.head, pool)
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
        // Emphasis present? Route through the delimiter-stack algorithm.
        // Otherwise keep the allocation-free `InlineBuf` fast path.
        if bytes.find_byte_set(0, &EMPH_ONLY_SET).is_some() {
            return with_arena(|arena| self.parse_into_arena(bytes, arena, depth));
        }
        let mut buf = InlineBuf::<CAP>::new();
        self.parse_into_buf(bytes, &mut buf, depth);
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

    /// True if any node in `span` is a link (`CommonMark` §6.3 "no links in
    /// links"). The pool is post-order, so an emphasis node's children live at
    /// indices *below* `span.start`; recurse through `Bold`/`Italic` child
    /// spans to catch a link nested inside emphasis (e.g. `[foo *[bar](/u)*]`).
    fn span_has_link(&self, span: InlineSpan) -> bool {
        let s = span.start as usize;
        let e = s + span.len as usize;
        self.pool[s..e].iter().any(|n| match *n {
            Inline::Link { .. } => true,
            Inline::Bold(child) | Inline::Italic(child) => self.span_has_link(child),
            _ => false,
        })
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

    /// Fast path for inline content with **no** `*`/`_` emphasis delimiters.
    /// Collects elements straight into the stack-allocated [`InlineBuf`].
    #[allow(clippy::too_many_lines)]
    fn parse_into_buf(&mut self, bytes: &[u8], buf: &mut InlineBuf<'src, CAP>, depth: u8) {
        // Paragraph reflow strips indentation at the start of the first line.
        let mut plain_start = if self.strip_lines {
            Self::skip_line_leading_ws(bytes, 0)
        } else {
            0
        };
        let mut i = plain_start;

        // A link, image, or reference can only ever close on a `]`. One SIMD
        // scan up front: when the whole context has no `]`, every `[`/`!`
        // becomes literal text and we skip the (forward-scanning) bracket
        // branches entirely, keeping them as plain bytes.
        let links_possible = bytes
            .find_byte(0, SpecialChar::CloseBracket.byte())
            .is_some();

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
                // Emit the escaped char as its own text node (dropping the
                // backslash). Isolating it prevents an escaped `&` from later
                // merging with a following `name;` and being decoded as an
                // entity at render time (e.g. `\&ouml;` must stay literal).
                if let Some(text) = self.input.get(i + 1..i + 2) {
                    buf.push(Inline::Text(text));
                }
                plain_start = i + 2;
                i += 2;
                continue;
            }

            // Inline code: `code` or ``code``
            if b == SpecialChar::Backtick {
                if let Some((code, end)) = Self::try_parse_inline_code(self.input, bytes, i) {
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
                // No matching closing run: the entire opening run of backticks
                // is literal text (CommonMark §6.1, the opening run is
                // maximal). Skip past it so a shorter sub-run can't re-open.
                i += SpecialChar::Backtick.count_leading_bytes(&bytes[i..]);
                continue;
            }

            // Image: ![alt](url "title") or reference ![alt][label]
            if links_possible
                && b == SpecialChar::ExclamationMark
                && bytes.get(i + 1) == SpecialChar::OpenBracket
                && let Some((alt, url, title, end)) =
                    Self::try_parse_bracket_paren(self.input, bytes, i + 1)
            {
                if let Some(text) = self.input.get(plain_start..i)
                    && !text.is_empty()
                {
                    buf.push(Inline::Text(text));
                }
                let alt = self.parse_inner(alt, depth.saturating_add(1));
                buf.push(Inline::Image { alt, url, title });
                plain_start = end;
                i = end;
                continue;
            }
            if links_possible
                && b == SpecialChar::ExclamationMark
                && bytes.get(i + 1) == SpecialChar::OpenBracket
                && let Some((text_str, url, title, end)) = self.try_parse_reference(bytes, i + 1)
            {
                if let Some(text) = self.input.get(plain_start..i)
                    && !text.is_empty()
                {
                    buf.push(Inline::Text(text));
                }
                let alt = self.parse_inner(text_str, depth.saturating_add(1));
                buf.push(Inline::Image { alt, url, title });
                plain_start = end;
                i = end;
                continue;
            }

            // Link: [text](url "title")
            if links_possible
                && b == SpecialChar::OpenBracket
                && let Some((text_str, url, title, end)) =
                    Self::try_parse_bracket_paren(self.input, bytes, i)
            {
                let saved = self.pool.len();
                let text_span = self.parse_inner(text_str, depth.saturating_add(1));
                if self.span_has_link(text_span) {
                    // "No links in links" (CommonMark §6.3): the outer `[` is
                    // literal and the inner link wins. Revert the text we just
                    // parsed and let the scanner rediscover it from i+1.
                    self.pool.truncate(saved);
                    i += 1;
                    continue;
                }
                if let Some(text) = self.input.get(plain_start..i)
                    && !text.is_empty()
                {
                    buf.push(Inline::Text(text));
                }
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
            if links_possible
                && b == SpecialChar::OpenBracket
                && let Some((text_str, url, title, end)) = self.try_parse_reference(bytes, i)
            {
                let saved = self.pool.len();
                let text_span = self.parse_inner(text_str, depth.saturating_add(1));
                if self.span_has_link(text_span) {
                    self.pool.truncate(saved);
                    i += 1;
                    continue;
                }
                if let Some(text) = self.input.get(plain_start..i)
                    && !text.is_empty()
                {
                    buf.push(Inline::Text(text));
                }
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

            i += 1;
        }

        // Trailing segment: paragraph reflow strips whitespace at the end of
        // the final line.
        let tail = if self.strip_lines {
            self.input
                .get(plain_start..)
                .map(|t| t.trim_end_matches([' ', '\t']))
        } else {
            self.input.get(plain_start..)
        };
        if let Some(text) = tail
            && !text.is_empty()
        {
            buf.push(Inline::Text(text));
        }
    }

    /// Scan a run of identical `*`/`_` delimiters at `i`, returning
    /// `(count, can_open, can_close)` per `CommonMark` flanking rules.
    fn scan_delims(bytes: &[u8], i: usize, ch: u8) -> (usize, bool, bool) {
        let mut n = 0;
        while bytes.get(i + n) == Some(&ch) {
            n += 1;
        }
        let before = Self::char_class_before(bytes, i);
        let after = Self::char_class_after(bytes, i + n);
        let before_ws = before == CharClass::Whitespace;
        let after_ws = after == CharClass::Whitespace;
        let before_punct = before == CharClass::Punctuation;
        let after_punct = after == CharClass::Punctuation;
        let left_flanking = !after_ws && (!after_punct || before_ws || before_punct);
        let right_flanking = !before_ws && (!before_punct || after_ws || after_punct);
        let (can_open, can_close) = if ch == SpecialChar::Underscore {
            (
                left_flanking && (!right_flanking || before_punct),
                right_flanking && (!left_flanking || after_punct),
            )
        } else {
            (left_flanking, right_flanking)
        };
        (n, can_open, can_close)
    }

    /// Emphasis-aware parse path (`CommonMark` §6.2). Mirrors
    /// [`parse_into_buf`](Self::parse_into_buf) but feeds elements into an
    /// [`EmphArena`]: `*`/`_` runs become delimiter-stack entries, everything
    /// else becomes a resolved node. After scanning, [`process_emphasis`] pairs
    /// the delimiters and [`emit_list`] flushes the result to the pool.
    #[allow(clippy::cast_sign_loss)]
    fn parse_into_arena(
        &mut self,
        bytes: &[u8],
        arena: &mut EmphArena<'src>,
        depth: u8,
    ) -> InlineSpan {
        arena.reset();
        // Single inline context: every `Text` node slices from `self.input`,
        // so it is the covering slice for contiguous-text merging.
        arena.src = self.input;
        self.scan_line_into_arena(bytes, arena, depth);
        // Drop a dangling trailing break for paragraph reflow.
        if self.strip_lines
            && arena.tail != NIL
            && matches!(
                arena.nodes[arena.tail as usize].kind,
                EmphKind::Resolved(Inline::SoftBreak | Inline::HardBreak)
            )
        {
            let tail = arena.tail;
            arena.unlink_node(tail);
        }
        arena.process_emphasis();
        arena.emit_list(arena.head, self.pool)
    }

    /// Scan one inline context into `arena` without resetting, resolving, or
    /// emitting. Shared by single-context and multi-line paths.
    #[allow(
        clippy::too_many_lines,
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap
    )]
    fn scan_line_into_arena(&mut self, bytes: &[u8], arena: &mut EmphArena<'src>, depth: u8) {
        let mut plain_start = if self.strip_lines {
            Self::skip_line_leading_ws(bytes, 0)
        } else {
            0
        };
        let mut i = plain_start;

        // A link, image, or reference can only ever close on a `]`. One SIMD
        // scan up front: when the whole context has no `]`, every `[`/`!`
        // becomes literal text and we skip the (forward-scanning) bracket
        // branches entirely.
        let links_possible = bytes
            .find_byte(0, SpecialChar::CloseBracket.byte())
            .is_some();

        // Flush pending plain text `[plain_start, upto)` as a text node.
        macro_rules! flush_text {
            ($upto:expr) => {
                if let Some(text) = self.input.get(plain_start..$upto)
                    && !text.is_empty()
                {
                    arena.push_text(text);
                }
            };
        }

        while let Some(pos) = bytes.find_byte_set(i, &SPECIAL_SET) {
            i = pos;
            let b = bytes[i];

            if b == SpecialChar::Newline {
                let (trim_end, is_hard) = self.line_break_pieces(bytes, plain_start, i);
                flush_text!(trim_end);
                arena.push_node(EmphKind::Resolved(if is_hard {
                    Inline::HardBreak
                } else {
                    Inline::SoftBreak
                }));
                plain_start = i + 1;
                if self.strip_lines {
                    plain_start = Self::skip_line_leading_ws(bytes, plain_start);
                }
                i = plain_start;
                continue;
            }

            if b == SpecialChar::Backslash
                && let Some(&next) = bytes.get(i + 1)
                && next.is_ascii_punctuation()
            {
                flush_text!(i);
                // Emit the escaped char (drop the backslash).
                if let Some(text) = self.input.get(i + 1..i + 2) {
                    arena.push_node(EmphKind::Resolved(Inline::Text(text)));
                }
                plain_start = i + 2;
                i += 2;
                continue;
            }

            if b == SpecialChar::Backtick {
                if let Some((code, end)) = Self::try_parse_inline_code(self.input, bytes, i) {
                    flush_text!(i);
                    arena.push_node(EmphKind::Resolved(Inline::Code(code)));
                    plain_start = end;
                    i = end;
                    continue;
                }
                // Unmatched opening run: keep the whole run as literal text
                // and skip it so a shorter sub-run cannot re-open a code span.
                i += SpecialChar::Backtick.count_leading_bytes(&bytes[i..]);
                continue;
            }

            // Image (inline then reference).
            if links_possible
                && b == SpecialChar::ExclamationMark
                && bytes.get(i + 1) == SpecialChar::OpenBracket
                && let Some((alt, url, title, end)) =
                    Self::try_parse_bracket_paren(self.input, bytes, i + 1)
            {
                flush_text!(i);
                let alt = self.parse_inner(alt, depth.saturating_add(1));
                arena.push_node(EmphKind::Resolved(Inline::Image { alt, url, title }));
                plain_start = end;
                i = end;
                continue;
            }
            if links_possible
                && b == SpecialChar::ExclamationMark
                && bytes.get(i + 1) == SpecialChar::OpenBracket
                && let Some((alt, url, title, end)) = self.try_parse_reference(bytes, i + 1)
            {
                flush_text!(i);
                let alt = self.parse_inner(alt, depth.saturating_add(1));
                arena.push_node(EmphKind::Resolved(Inline::Image { alt, url, title }));
                plain_start = end;
                i = end;
                continue;
            }

            // Inline link.
            if links_possible
                && b == SpecialChar::OpenBracket
                && let Some((text_str, url, title, end)) =
                    Self::try_parse_bracket_paren(self.input, bytes, i)
            {
                let saved = self.pool.len();
                let text_span = self.parse_inner(text_str, depth.saturating_add(1));
                if self.span_has_link(text_span) {
                    // "No links in links" (CommonMark §6.3): outer `[` is
                    // literal, the inner link wins. Revert and rescan from i+1.
                    self.pool.truncate(saved);
                    i += 1;
                    continue;
                }
                flush_text!(i);
                arena.push_node(EmphKind::Resolved(Inline::Link {
                    text: text_span,
                    url,
                    title,
                }));
                plain_start = end;
                i = end;
                continue;
            }
            // Reference link.
            if links_possible
                && b == SpecialChar::OpenBracket
                && let Some((text_str, url, title, end)) = self.try_parse_reference(bytes, i)
            {
                let saved = self.pool.len();
                let text_span = self.parse_inner(text_str, depth.saturating_add(1));
                if self.span_has_link(text_span) {
                    self.pool.truncate(saved);
                    i += 1;
                    continue;
                }
                flush_text!(i);
                arena.push_node(EmphKind::Resolved(Inline::Link {
                    text: text_span,
                    url,
                    title,
                }));
                plain_start = end;
                i = end;
                continue;
            }

            // Autolink / raw HTML.
            if b == SpecialChar::LessThan
                && let Some((elem, end)) = Self::try_parse_angle(self.input, bytes, i)
            {
                flush_text!(i);
                arena.push_node(EmphKind::Resolved(elem));
                plain_start = end;
                i = end;
                continue;
            }

            // Emphasis delimiter run.
            if b == SpecialChar::Asterisk || b == SpecialChar::Underscore {
                let (count, can_open, can_close) = Self::scan_delims(bytes, i, b);
                debug_assert!(count > 0);
                flush_text!(i);
                let src = self.input.get(i..i + count).unwrap_or_default();
                if can_open || can_close {
                    arena.push_delim(src, b, count as i32, can_open, can_close);
                } else {
                    arena.push_text(src);
                }
                plain_start = i + count;
                i += count;
                continue;
            }

            i += 1;
        }

        let tail = if self.strip_lines {
            self.input
                .get(plain_start..)
                .map(|t| t.trim_end_matches([' ', '\t']))
        } else {
            self.input.get(plain_start..)
        };
        if let Some(text) = tail
            && !text.is_empty()
        {
            arena.push_text(text);
        }
    }

    /// Compute the text-slice end and break kind at a newline position.
    /// Hard break if preceded by trailing `\` or 2+ spaces; soft otherwise.
    /// Returns `(trim_end, is_hard)` where `plain_start..trim_end` is the text
    /// to emit before the break.
    #[inline]
    fn line_break_pieces(
        &self,
        bytes: &[u8],
        plain_start: usize,
        newline_pos: usize,
    ) -> (usize, bool) {
        let preceding = bytes.get(plain_start..newline_pos).unwrap_or_default();
        let (mut trim_end, is_hard) = if preceding.last() == SpecialChar::Backslash {
            (newline_pos - 1, true)
        } else {
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
        if self.strip_lines {
            while trim_end > plain_start && matches!(bytes.get(trim_end - 1), Some(b' ' | b'\t')) {
                trim_end -= 1;
            }
        }
        (trim_end, is_hard)
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
            while trim_end > plain_start && matches!(bytes.get(trim_end - 1), Some(b' ' | b'\t')) {
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

    /// Find the position of a matching closing delimiter, handling backslash
    /// escapes and nested pairs.
    fn find_matching_close(
        input: &'src str,
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
            // Inline code, autolinks, and raw HTML take precedence over link
            // structure (CommonMark §6.3): when one begins inside the brackets,
            // skip past its whole extent so a `]` it contains cannot close the
            // link (e.g. `[foo`](/uri)`` is text + code span, not a link).
            if open == SpecialChar::OpenBracket {
                if b == SpecialChar::Backtick
                    && let Some((_, end)) = Self::try_parse_inline_code(input, bytes, pos)
                {
                    j = end;
                    continue;
                }
                if b == SpecialChar::LessThan
                    && let Some((_, end)) = Self::try_parse_angle(input, bytes, pos)
                {
                    j = end;
                    continue;
                }
            }
            if b == SpecialChar::Backtick || b == SpecialChar::LessThan {
                // A lone backtick/`<` that starts no construct is ordinary text.
                j = pos + 1;
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
            input,
            bytes,
            bracket_start,
            SpecialChar::OpenBracket,
            SpecialChar::CloseBracket,
        )?;

        let paren_pos = bracket_end + 1;
        if bytes.get(paren_pos) != SpecialChar::OpenParen {
            return None;
        }

        let (url, title, end) = Self::scan_link_tail(input, bytes, paren_pos)?;
        Some((input.get(bracket_start..bracket_end)?, url, title, end))
    }

    /// Parse a link/image tail `(destination "title")` beginning at `paren`
    /// (the `(` byte), per `CommonMark` §6.3. Handles angle-bracket and
    /// balanced bare destinations, the three title quote forms, and optional
    /// surrounding whitespace. Returns the raw source slices for url and title
    /// (backslash/entity decoding and percent-encoding happen later, at render
    /// time) plus the index just past the closing `)`.
    fn scan_link_tail(
        input: &'src str,
        bytes: &[u8],
        paren: usize,
    ) -> Option<(&'src str, Option<&'src str>, usize)> {
        let mut i = Self::skip_link_ws(bytes, paren + 1);
        let (url, after_dest) = Self::scan_link_destination(input, bytes, i)?;
        i = after_dest;

        // A title, if present, must be separated from the destination by
        // whitespace; otherwise only trailing whitespace before `)` is allowed.
        let ws_end = Self::skip_link_ws(bytes, i);
        let mut title = None;
        if ws_end > i
            && let Some((t, after_title)) = Self::scan_link_title(input, bytes, ws_end)
        {
            title = Some(t);
            i = Self::skip_link_ws(bytes, after_title);
        } else {
            i = ws_end;
        }

        if bytes.get(i).copied() != Some(SpecialChar::CloseParen.byte()) {
            return None;
        }
        Some((url, title, i + 1))
    }

    /// Scan a link destination at `start`. Returns the destination slice (inner
    /// content for the `<...>` form, without the angle brackets) and the index
    /// just past it. The bare form requires balanced parentheses and forbids
    /// ASCII spaces and control characters; the angle form forbids line breaks
    /// and unescaped `<`/`>`.
    fn scan_link_destination(
        input: &'src str,
        bytes: &[u8],
        start: usize,
    ) -> Option<(&'src str, usize)> {
        if bytes.get(start).copied() == Some(SpecialChar::LessThan.byte()) {
            let mut j = start + 1;
            loop {
                match bytes.get(j).copied() {
                    None | Some(b'\n' | b'<') => return None,
                    Some(b'>') => return Some((input.get(start + 1..j)?, j + 1)),
                    Some(b'\\') if bytes.get(j + 1).is_some_and(u8::is_ascii_punctuation) => {
                        j += 2;
                    }
                    Some(_) => j += 1,
                }
            }
        } else {
            let mut j = start;
            let mut depth = 0u32;
            loop {
                match bytes.get(j).copied() {
                    None => break,
                    Some(b'\\') if bytes.get(j + 1).is_some_and(u8::is_ascii_punctuation) => {
                        j += 2;
                    }
                    Some(b'(') => {
                        depth += 1;
                        j += 1;
                    }
                    Some(b')') => {
                        if depth == 0 {
                            break;
                        }
                        depth -= 1;
                        j += 1;
                    }
                    // ASCII space or any control character ends the destination.
                    Some(b) if b <= b' ' || b == 0x7f => break,
                    Some(_) => j += 1,
                }
            }
            if depth != 0 {
                return None;
            }
            Some((input.get(start..j)?, j))
        }
    }

    /// Scan an optional link title at `start` (`"..."`, `'...'`, or `(...)`),
    /// returning the inner slice and the index just past the closing delimiter.
    /// In the `(...)` form an unescaped `(` is disallowed.
    fn scan_link_title(input: &'src str, bytes: &[u8], start: usize) -> Option<(&'src str, usize)> {
        let (open, close) = match bytes.get(start).copied()? {
            b'"' => (b'"', b'"'),
            b'\'' => (b'\'', b'\''),
            b'(' => (b'(', b')'),
            _ => return None,
        };
        let mut j = start + 1;
        loop {
            match bytes.get(j).copied() {
                None => return None,
                Some(b'\\') if bytes.get(j + 1).is_some_and(u8::is_ascii_punctuation) => {
                    j += 2;
                }
                Some(b) if b == open && open != close => return None,
                Some(b) if b == close => return Some((input.get(start + 1..j)?, j + 1)),
                Some(_) => j += 1,
            }
        }
    }

    /// Skip spaces, tabs, and line endings around link destinations/titles.
    fn skip_link_ws(bytes: &[u8], mut i: usize) -> usize {
        while matches!(bytes.get(i).copied(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            i += 1;
        }
        i
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
            self.input,
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
                self.input,
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
        // `normalize_label_cow` borrows the label when it is already in
        // normalized form (the common case), so the hash lookup allocates only
        // for labels that genuinely need case folding or whitespace collapse.
        self.defs.get(label.normalize_label_cow().as_ref()).copied()
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
        if let Some(len) = bytes[start..].scan_inline_html() {
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
        while bytes
            .get(i)
            .is_some_and(|&b| b.is_ascii_alphanumeric() || b == b'+' || b == b'.' || b == b'-')
        {
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
                    b'.' | b'!'
                        | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'/'
                        | b'='
                        | b'?'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'{'
                        | b'|'
                        | b'}'
                        | b'~'
                        | b'-'
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
                // Return the raw inter-backtick slice. CommonMark §6.1
                // normalization (line endings -> spaces, then a single
                // leading/trailing space strip when the content is not all
                // spaces) happens at render time in `escape_code_span`,
                // because line-ending collapse requires allocation while this
                // parser only borrows from the source.
                return Some((input.get(content_start..i)?, i + close_count));
            }
            i += close_count;
        }

        None
    }
}
