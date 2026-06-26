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

/// Pre-computed byte set for the bracket-matching pass (`build_bracket_table`).
/// Avoids rebuilding the 256-byte lookup table on every call.
static BRACKET_CLOSE_SET: ByteSet = ByteSet::new(&[
    SpecialChar::OpenBracket.byte(),
    SpecialChar::CloseBracket.byte(),
    SpecialChar::Backslash.byte(),
    // Code spans, autolinks, and raw HTML bind tighter than link brackets, so
    // the closing-bracket scan must recognise and skip over them.
    SpecialChar::Backtick.byte(),
    SpecialChar::LessThan.byte(),
]);

/// Emphasis delimiter characters (`*` and `_`). A single SIMD scan for this set
/// decides whether an inline context needs the full delimiter-stack algorithm
/// (below) or can take the existing allocation-free fast path.
static EMPH_ONLY_SET: ByteSet =
    ByteSet::new(&[SpecialChar::Asterisk.byte(), SpecialChar::Underscore.byte()]);



/// The only bytes that can begin a link/image inside link text, for the
/// "no links in links" pre-check ([`InlineParser::region_has_link`]): `[`
/// (link / reference), `!` (image), and `\` (escape, which neutralizes the
/// next byte). A SIMD scan for this set skips plain text — the overwhelmingly
/// common link-text content — in bulk.
static LINK_SCAN_SET: ByteSet = ByteSet::new(&[
    SpecialChar::OpenBracket.byte(),
    SpecialChar::ExclamationMark.byte(),
    SpecialChar::Backslash.byte(),
]);

/// Sentinel stored in `close_of` for a `[` that has been resolved and has *no*
/// matching `]` (distinct from "not yet resolved", which the generation stamp
/// tracks). Lets a failed match be memoized so it is never rescanned.
const NO_MATCH: u32 = u32::MAX;

/// Lazily-filled, memoized `[`->`]` match table for one inline context.
///
/// `close_of[p]` is the offset of the `]` closing the `[` at `p` (or [`NO_MATCH`]),
/// valid only when `gen_of[p] == generation`; `stack` is scratch for the scan.
///
/// Two design choices keep the happy path as cheap as a plain forward scan while
/// still making adversarial bracket nesting `O(n)`:
///
/// * **Lazy + memoized.** A `[`'s match is resolved on first query by a forward
///   stack-scan from that `[`, which records *every* pair nested inside it along
///   the way. Flat links (`[text](url)`) resolve in a short scan that never
///   touches the surrounding text; nested `[[[…]]]` is fully resolved by the
///   first outer query, leaving every inner lookup `O(1)`. Total work is `O(n)`
///   per context either way, but — unlike an eager whole-context pre-pass —
///   link-dense prose pays nothing for the long runs of text between links.
/// * **Generation stamp.** "Clearing" the table between contexts is a single
///   counter bump, not an `O(n)` memset; the buffers only ever grow (amortized
///   to nothing once pooled). All fields are plain `u32` (no borrowed data), so
///   pooling needs no lifetime juggling.
#[derive(Default)]
struct BracketScratch {
    close_of: Vec<u32>,
    gen_of: Vec<u32>,
    stack: Vec<u32>,
    generation: u32,
    /// Set by [`InlineParser::matching_close`] to report whether the just-
    /// resolved `[` contained any *inner* `[` in its interior. A nested link
    /// needs an inner `[`, so when this is `false` the "no links in links"
    /// (`CommonMark` §6.3) pre-check can be skipped entirely — the resolve scan
    /// already visited every bracket, so this reuses its work for free. A cache
    /// hit conservatively reports `true` (the cheap full check then runs).
    saw_inner_bracket: bool,
}

thread_local! {
    /// Free-list of [`BracketScratch`] buffers, reused across inline contexts to
    /// amortize their allocations. A stack (not a single slot) because parsing
    /// is re-entrant: a link's text is a nested inline context that builds its
    /// own table while the outer one is still live. Mirrors `ARENA_POOL`.
    static BRACKET_POOL: std::cell::RefCell<Vec<BracketScratch>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

impl BracketScratch {
    /// Check a buffer set out of the pool, allocating only when it is empty.
    fn checkout() -> Self {
        BRACKET_POOL
            .with(|p| p.borrow_mut().pop())
            .unwrap_or_default()
    }

    /// Return the buffers to the pool for the next context to reuse. The
    /// generation-stamp scheme means `close_of`/`gen_of` need no clearing — the
    /// next build bumps `generation`, invalidating every existing entry — so
    /// only the transient `stack` is reset.
    fn checkin(mut self) {
        self.stack.clear();
        BRACKET_POOL.with(|p| p.borrow_mut().push(self));
    }
}

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
    /// nested inline context), so this is a stack: each [`EmphArena::with`] call
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
    /// Run `f` with a recycled [`EmphArena`], returning it to the thread-local
    /// pool afterwards. Buffers are recycled across the lifetime boundary by
    /// [`EmphArena::relifetime`] (which clears them), so no `'src` reference
    /// ever persists in the `'static` pool, and no `unsafe` is involved.
    fn with<R>(f: impl FnOnce(&mut Self) -> R) -> R {
        // Check out a pooled arena and re-type it to `'src`, reusing its
        // allocations. Both callers `reset()` the arena before filling it, so
        // we hand over the cleared buffers as-is.
        let mut arena: EmphArena<'src> = ARENA_POOL
            .with(|p| p.borrow_mut().pop())
            .map_or_else(EmphArena::default, EmphArena::relifetime);
        let out = f(&mut arena);
        // Return it to the pool as `'static`, again recycling the allocations
        // and dropping every `'src` borrow in the process.
        let pooled: EmphArena<'static> = arena.relifetime();
        ARENA_POOL.with(|p| p.borrow_mut().push(pooled));
        out
    }

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

/// Output sink for the shared inline scanner [`InlineParser::scan_inline`].
///
/// The two parse paths differ only in where scanned elements land and whether
/// `*`/`_` runs are emphasis delimiters: the allocation-free [`InlineBuf`] fast
/// path (no emphasis) and the [`EmphArena`] delimiter-stack path. Implementing
/// this trait for both lets one scanner drive both, with `HANDLES_DELIMS`
/// monomorphized to a constant so the delimiter branch is compiled out of the
/// fast path entirely.
trait InlineSink<'src> {
    /// Whether `*`/`_` runs should be handled as emphasis delimiters. `false`
    /// leaves them as ordinary text (the buffer fast path never sees emphasis).
    const HANDLES_DELIMS: bool;
    /// Ordinary plain text (may be merged with adjacent text downstream).
    fn text(&mut self, text: &'src str);
    /// Isolated text that must never merge with a neighbour — used for an
    /// escaped character, so e.g. `\&ouml;` cannot recombine into an entity.
    fn literal(&mut self, text: &'src str);
    /// A fully-resolved inline element (code, link, image, autolink, raw HTML).
    fn resolved(&mut self, el: Inline<'src>);
    /// A soft or hard line break.
    fn line_break(&mut self, is_hard: bool);
    /// An emphasis delimiter run (`count` copies of `ch`). Only ever called
    /// when `HANDLES_DELIMS` is `true`.
    fn delim_run(&mut self, src: &'src str, ch: u8, count: usize, can_open: bool, can_close: bool);
}

impl<'src, const CAP: usize> InlineSink<'src> for InlineBuf<'src, CAP> {
    const HANDLES_DELIMS: bool = false;
    #[inline]
    fn text(&mut self, text: &'src str) {
        self.push(Inline::Text(text));
    }
    #[inline]
    fn literal(&mut self, text: &'src str) {
        // The buffer never merges adjacent text, so an isolated literal is just
        // a plain text node.
        self.push(Inline::Text(text));
    }
    #[inline]
    fn resolved(&mut self, el: Inline<'src>) {
        self.push(el);
    }
    #[inline]
    fn line_break(&mut self, is_hard: bool) {
        self.push(if is_hard {
            Inline::HardBreak
        } else {
            Inline::SoftBreak
        });
    }
    fn delim_run(&mut self, _: &'src str, _: u8, _: usize, _: bool, _: bool) {
        unreachable!("buffer sink never handles emphasis delimiters")
    }
}

impl<'src> InlineSink<'src> for EmphArena<'src> {
    const HANDLES_DELIMS: bool = true;
    #[inline]
    fn text(&mut self, text: &'src str) {
        self.push_text(text);
    }
    #[inline]
    fn literal(&mut self, text: &'src str) {
        // `Resolved` is opaque to the delimiter resolver and to text merging,
        // keeping the escaped character isolated.
        self.push_node(EmphKind::Resolved(Inline::Text(text)));
    }
    #[inline]
    fn resolved(&mut self, el: Inline<'src>) {
        self.push_node(EmphKind::Resolved(el));
    }
    #[inline]
    fn line_break(&mut self, is_hard: bool) {
        self.push_node(EmphKind::Resolved(if is_hard {
            Inline::HardBreak
        } else {
            Inline::SoftBreak
        }));
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    fn delim_run(&mut self, src: &'src str, ch: u8, count: usize, can_open: bool, can_close: bool) {
        if can_open || can_close {
            self.push_delim(src, ch, count as i32, can_open, can_close);
        } else {
            self.push_text(src);
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
            return EmphArena::with(|arena| {
                Self::parse_lines_into_arena(input, lines, arena, pool, defs)
            });
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
            parser.scan_inline(bytes, &mut buf, 0);
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
            parser.scan_inline(line.as_bytes(), arena, 0);
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
            return EmphArena::with(|arena| self.parse_into_arena(bytes, arena, depth));
        }
        let mut buf = InlineBuf::<CAP>::new();
        self.scan_inline(bytes, &mut buf, depth);
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

    /// Cheap, **non-recursive** test for `CommonMark` §6.3 "no links in links":
    /// does the inline region `input[start..end)` contain any link (inline or
    /// reference), *not* counting links buried inside an image's alt text?
    ///
    /// This replaces the old approach of fully parsing the bracket text and
    /// then inspecting the resulting nodes. That was catastrophically
    /// exponential on nested link text like `[[[…[a](b)…](c)](c)`: each outer
    /// `[` parsed its entire text (recursively parsing every inner link) only
    /// to discover a link, revert, and re-parse the overlapping inner content
    /// — `T(d) = 2·T(d-1)`, so a ~120-byte input took seconds. Here we only
    /// need to know whether *some* link exists, which a single forward scan
    /// over the pre-computed bracket table answers without any recursion: once
    /// the outer text is known link-free, `parse_inner` runs on it exactly once
    /// and can never trigger a revert.
    ///
    /// Scanning the raw region for the *presence* of a link is equivalent to
    /// the old node inspection: emphasis markers do not hide brackets, links
    /// cannot nest inside code spans / autolinks (those have no entry in the
    /// bracket table and so are skipped), and a link existing at any nesting
    /// depth means the innermost one is a genuine link. Image extents are
    /// stepped over so that a link inside an image's alt does not count (it
    /// renders as plain text and so never disqualifies the enclosing link —
    /// matching the old `span_has_link`, which never descended into `Image`).
    fn region_has_link(
        &self,
        bytes: &[u8],
        scratch: &mut BracketScratch,
        start: usize,
        end: usize,
    ) -> bool {
        let mut j = start;
        // SIMD-skip plain text between the only bytes that can start a
        // link/image/escape; the common case (`[plain text](url)`) has none in
        // its text and exits after a single scan.
        while let Some(pos) = bytes.find_byte_set(j, &LINK_SCAN_SET) {
            if pos >= end {
                break;
            }
            j = pos;
            let b = bytes[j];
            // Backslash escape: the next punctuation byte is literal.
            if b == SpecialChar::Backslash {
                j = if bytes.get(j + 1).is_some_and(u8::is_ascii_punctuation) {
                    j + 2
                } else {
                    j + 1
                };
                continue;
            }
            // Image `![…](…)` / `![…][…]`: skip the whole construct so links in
            // its alt are not counted.
            if b == SpecialChar::ExclamationMark
                && bytes.get(j + 1) == SpecialChar::OpenBracket
            {
                if let Some((_, _, _, e)) =
                    Self::try_parse_bracket_paren(self.input, bytes, scratch, j + 1)
                {
                    j = e;
                    continue;
                }
                if let Some((_, _, _, e)) = self.try_parse_reference(bytes, scratch, j + 1) {
                    j = e;
                    continue;
                }
                // Not an image after all; the `!` is literal text.
                j += 1;
                continue;
            }
            if b == SpecialChar::OpenBracket {
                // A link that begins here means the region contains a link.
                if let Some((_, _, _, e)) =
                    Self::try_parse_bracket_paren(self.input, bytes, scratch, j)
                    && e <= end
                {
                    return true;
                }
                if let Some((_, _, _, e)) = self.try_parse_reference(bytes, scratch, j)
                    && e <= end
                {
                    return true;
                }
                // Not a link by itself, but one may be nested inside its
                // brackets (e.g. `[x [a](b) y]`); keep scanning inward.
                j += 1;
                continue;
            }
            // `b` is always a member of `LINK_SCAN_SET` (handled above), so this
            // is unreachable; advance defensively rather than risk a stall.
            j += 1;
        }
        false
    }

    fn parse_inner(&mut self, input: &'src str, depth: u8) -> InlineSpan {
        // Bound nested-context recursion. Each link/image/reference re-parses
        // its bracket text (and the "no links in links" revert re-parses
        // overlapping spans), so adversarial nesting like `[[[[...]]]]` would
        // otherwise be exponential — a ~4 KB input could take seconds (an
        // algorithmic-complexity DoS). At the limit, emit the inner text
        // verbatim as a single plain-text node instead of recursing further;
        // `MAX_DEPTH` (default 16) is far beyond any real document's nesting.
        if depth >= MAX_DEPTH {
            if input.is_empty() {
                return InlineSpan::EMPTY;
            }
            let start = self.pool.len().pool_offset();
            self.pool.push(Inline::Text(input));
            return InlineSpan::new(start, 1);
        }
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

    /// Shared inline scanner driving both parse paths.
    ///
    /// A single SIMD-accelerated loop over `bytes` recognises every inline
    /// construct (line breaks, backslash escapes, code spans, links, images,
    /// autolinks, raw HTML, and — when `S::HANDLES_DELIMS` — emphasis runs) and
    /// feeds them to the generic [`InlineSink`] `sink`. Monomorphization
    /// compiles the emphasis branch out of the buffer fast path entirely, so
    /// the two former copies (`parse_into_buf` / `scan_line_into_arena`) stay
    /// zero-cost while sharing one body.
    #[allow(
        clippy::too_many_lines,
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap
    )]
    fn scan_inline<S: InlineSink<'src>>(&mut self, bytes: &[u8], sink: &mut S, depth: u8) {
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
        // branches entirely, keeping them as plain bytes — and skip checking out
        // a scratch buffer at all, so link-free prose pays nothing.
        let links_possible = bytes
            .find_byte(0, SpecialChar::CloseBracket.byte())
            .is_some();

        // Bracket `[`->`]` matches are resolved lazily and memoized into this
        // scratch on first query (see `matching_close`): flat links cost a
        // short forward scan, nested brackets are resolved once then `O(1)`.
        let mut bracket_scratch = if links_possible {
            let mut s = BracketScratch::checkout();
            Self::bracket_reset(&mut s, bytes.len());
            Some(s)
        } else {
            None
        };

        // Flush pending plain text `[plain_start, upto)` as a text node.
        macro_rules! flush_text {
            ($upto:expr) => {
                if let Some(text) = self.input.get(plain_start..$upto)
                    && !text.is_empty()
                {
                    sink.text(text);
                }
            };
        }

        // SIMD-accelerated scan: find next special byte.
        while let Some(pos) = bytes.find_byte_set(i, &SPECIAL_SET) {
            i = pos;
            let b = bytes[i];

            if b == SpecialChar::Newline {
                let (trim_end, is_hard) = self.line_break_pieces(bytes, plain_start, i);
                flush_text!(trim_end);
                sink.line_break(is_hard);
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
                flush_text!(i);
                // Emit the escaped char as its own isolated node (dropping the
                // backslash). Isolating it prevents an escaped `&` from later
                // merging with a following `name;` and being decoded as an
                // entity at render time (e.g. `\&ouml;` must stay literal).
                if let Some(text) = self.input.get(i + 1..i + 2) {
                    sink.literal(text);
                }
                plain_start = i + 2;
                i += 2;
                continue;
            }

            // Inline code: `code` or ``code``
            if b == SpecialChar::Backtick {
                if let Some((code, end)) = Self::try_parse_inline_code(self.input, bytes, i) {
                    flush_text!(i);
                    sink.resolved(Inline::Code(code));
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
                && let Some((alt, url, title, end)) = Self::try_parse_bracket_paren(
                    self.input,
                    bytes,
                    bracket_scratch.as_mut().unwrap(),
                    i + 1,
                )
            {
                flush_text!(i);
                let alt = self.parse_inner(alt, depth.saturating_add(1));
                sink.resolved(Inline::Image { alt, url, title });
                plain_start = end;
                i = end;
                continue;
            }
            if links_possible
                && b == SpecialChar::ExclamationMark
                && bytes.get(i + 1) == SpecialChar::OpenBracket
                && let Some((text_str, url, title, end)) =
                    self.try_parse_reference(bytes, bracket_scratch.as_mut().unwrap(), i + 1)
            {
                flush_text!(i);
                let alt = self.parse_inner(text_str, depth.saturating_add(1));
                sink.resolved(Inline::Image { alt, url, title });
                plain_start = end;
                i = end;
                continue;
            }

            // Link: [text](url "title")
            if links_possible
                && b == SpecialChar::OpenBracket
                && let Some((text_str, url, title, end)) = Self::try_parse_bracket_paren(
                    self.input,
                    bytes,
                    bracket_scratch.as_mut().unwrap(),
                    i,
                )
            {
                // "No links in links" (CommonMark §6.3): if the bracket text
                // already contains a link, the outer `[` is literal and the
                // inner link wins. Test this *before* parsing the text — a
                // cheap non-recursive scan — so `parse_inner` only ever runs on
                // link-free text and never reverts (the former parse-then-
                // revert was exponential on nested link text).
                // The link text occupies `[i+1, i+1+text_str.len())` (the
                // bracket interior), not up to `end` (which is past the URL).
                //
                // Fast path: `try_parse_bracket_paren` just resolved this `[`
                // with a single `matching_close`, which recorded whether the
                // interior held any inner `[`. A nested link requires one, so
                // when there was none the §6.3 check is provably false and the
                // whole `region_has_link` scan is skipped (the resolve scan
                // already covered these bytes).
                let text_start = i + 1;
                let text_end = text_start + text_str.len();
                if bracket_scratch.as_ref().unwrap().saw_inner_bracket
                    && self.region_has_link(
                        bytes,
                        bracket_scratch.as_mut().unwrap(),
                        text_start,
                        text_end,
                    )
                {
                    i += 1;
                    continue;
                }
                flush_text!(i);
                let text_span = self.parse_inner(text_str, depth.saturating_add(1));
                sink.resolved(Inline::Link {
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
                && let Some((text_str, url, title, end)) =
                    self.try_parse_reference(bytes, bracket_scratch.as_mut().unwrap(), i)
            {
                // For full references the rendered text is the *first* bracket
                // (`[text][label]`); for shortcut/collapsed it is the label
                // itself. In every form the rendered text starts at `i+1` and
                // spans `text_str.len()` bytes.
                let text_start = i + 1;
                let text_end = text_start + text_str.len();
                if self.region_has_link(bytes, bracket_scratch.as_mut().unwrap(), text_start, text_end) {
                    i += 1;
                    continue;
                }
                flush_text!(i);
                let text_span = self.parse_inner(text_str, depth.saturating_add(1));
                sink.resolved(Inline::Link {
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
                flush_text!(i);
                sink.resolved(elem);
                plain_start = end;
                i = end;
                continue;
            }

            // Emphasis delimiter run (`*`/`_`). Only the arena path handles
            // these; the buffer fast path is only chosen when the context has
            // no emphasis, so this branch is compiled out there.
            if S::HANDLES_DELIMS
                && (b == SpecialChar::Asterisk || b == SpecialChar::Underscore)
            {
                let (count, can_open, can_close) = Self::scan_delims(bytes, i, b);
                debug_assert!(count > 0);
                flush_text!(i);
                let src = self.input.get(i..i + count).unwrap_or_default();
                sink.delim_run(src, b, count, can_open, can_close);
                plain_start = i + count;
                i += count;
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
            sink.text(text);
        }

        if let Some(scratch) = bracket_scratch {
            scratch.checkin();
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
        self.scan_inline(bytes, arena, depth);
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

    /// Build the bracket-match table for one inline context in a single
    /// left-to-right pass: for every `[` at offset `p` that has a matching `]`,
    /// stamp `scratch.close_of[p]` with that `]`'s offset and `scratch.gen_of[p]`
    /// Ready a [`BracketScratch`] for a new inline context spanning `len` bytes.
    /// Grows the parallel buffers (never shrinks) and bumps the generation so
    /// every prior entry is invalidated in `O(1)` — no memset. Slots need no
    /// initialization: a stale slot carries an old stamp and reads as "unresolved".
    fn bracket_reset(scratch: &mut BracketScratch, len: usize) {
        if scratch.close_of.len() < len {
            scratch.close_of.resize(len, 0);
            scratch.gen_of.resize(len, 0);
        }
        scratch.generation = scratch.generation.wrapping_add(1);
        if scratch.generation == 0 {
            // Astronomically rare wraparound: clear stamps once so no stale slot
            // can masquerade as belonging to the new generation.
            scratch.gen_of.iter_mut().for_each(|g| *g = u32::MAX);
            scratch.generation = 1;
        }
    }

    /// Look up the `]` matching the `[` at `open`, resolving it lazily and
    /// memoizing the result (and every pair nested inside it) into `scratch`.
    ///
    /// On a cache miss this runs one forward stack-scan from `open`: it walks
    /// brackets until `open`'s own `]` is found, stamping each inner `[`->`]`
    /// pair it closes along the way, then returns. Because a scan stops at
    /// `open`'s closer, flat links (`[text](url)`) scan only their own short
    /// interior and never the surrounding prose — matching the old per-bracket
    /// forward scan's cost on the common case. But unlike that design, the pairs
    /// discovered while resolving a deeply nested `[` are cached, so the whole
    /// `[[[…]]]` subtree is resolved in one scan and every later query is `O(1)`
    /// — keeping adversarial nesting `O(n)` overall instead of `O(n²)`.
    ///
    /// The skip rules match link structure exactly (`CommonMark` §6.3): backslash
    /// escapes and code spans / autolinks / raw HTML (which bind tighter than
    /// link brackets) are stepped over so a `]` inside them cannot close a link.
    fn matching_close(
        input: &'src str,
        bytes: &[u8],
        scratch: &mut BracketScratch,
        open: usize,
    ) -> Option<usize> {
        // Cached? (live stamp). `NO_MATCH` is a resolved "no closer" answer.
        // A cache hit didn't rescan, so conservatively assume inner brackets.
        if scratch.gen_of.get(open).copied() == Some(scratch.generation) {
            scratch.saw_inner_bracket = true;
            let c = scratch.close_of[open];
            return (c != NO_MATCH).then_some(c as usize);
        }
        let generation = scratch.generation;
        let stack_base = scratch.stack.len();
        #[allow(clippy::cast_possible_truncation)]
        scratch.stack.push(open as u32);
        let mut j = open + 1;
        let mut result = None;
        let mut saw_inner = false;
        while let Some(pos) = bytes.find_byte_set(j, &BRACKET_CLOSE_SET) {
            let b = bytes[pos];
            if b == SpecialChar::Backslash
                && bytes.get(pos + 1).is_some_and(u8::is_ascii_punctuation)
            {
                j = pos + 2;
                continue;
            }
            // Code spans, autolinks, and raw HTML take precedence over link
            // structure: skip past the whole construct (e.g. `[foo`](/uri)`` is
            // text + code span, not a link).
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
            if b == SpecialChar::Backtick || b == SpecialChar::LessThan {
                // A lone backtick/`<` that starts no construct is ordinary text.
                j = pos + 1;
                continue;
            }
            if b == SpecialChar::OpenBracket {
                saw_inner = true;
                #[allow(clippy::cast_possible_truncation)]
                scratch.stack.push(pos as u32);
            } else if b == SpecialChar::CloseBracket
                && let Some(inner) = scratch.stack.pop()
            {
                // Innermost open `[` matches this `]` (`CommonMark` nesting).
                #[allow(clippy::cast_possible_truncation)]
                {
                    scratch.close_of[inner as usize] = pos as u32;
                    scratch.gen_of[inner as usize] = generation;
                }
                if inner as usize == open {
                    // `open`'s own closer: done. Any `[` still on the stack
                    // above `stack_base` is left unresolved for a later query.
                    result = Some(pos);
                    break;
                }
            }
            j = pos + 1;
        }
        // Every `[` still open from this scan (including `open` itself on a
        // no-closer exit) has no matching `]`: memoize that so it is never
        // rescanned, then restore the stack to its caller-visible depth.
        #[allow(clippy::cast_possible_truncation)]
        for &unclosed in &scratch.stack[stack_base..] {
            scratch.close_of[unclosed as usize] = NO_MATCH;
            scratch.gen_of[unclosed as usize] = generation;
        }
        scratch.stack.truncate(stack_base);
        scratch.saw_inner_bracket = saw_inner;
        result
    }

    fn try_parse_bracket_paren(
        input: &'src str,
        bytes: &[u8],
        scratch: &mut BracketScratch,
        start: usize,
    ) -> Option<(&'src str, &'src str, Option<&'src str>, usize)> {
        if bytes.get(start) != SpecialChar::OpenBracket {
            return None;
        }

        let bracket_start = start + 1;
        let bracket_end = Self::matching_close(input, bytes, scratch, start)?;

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
        scratch: &mut BracketScratch,
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
        let first_end = Self::matching_close(self.input, bytes, scratch, start)?;
        let first_text = self.input.get(first_start..first_end)?;

        // Is there a second bracket pair `[...]` immediately after?
        let after_first = first_end + 1;
        if bytes.get(after_first) == SpecialChar::OpenBracket {
            let second_start = after_first + 1;
            let second_end = Self::matching_close(self.input, bytes, scratch, after_first)?;
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
