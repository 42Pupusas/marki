//! Pass-1 scan of a whole top-level list (`CommonMark` §5.2/§5.3).

use super::{BlockBytes, BlockStr, ListScan, Marker, ParseCtx};
use crate::OffsetExt;
use crate::simd::ByteSliceExt;
use crate::special_char::SpecialChar;

/// State of the list being scanned and of its current item.
struct ListWalk {
    loose: bool,
    /// Trailing blanks of closed items not yet attributed.
    pending_blanks: usize,
    /// Line-pool offset of the current item's first content line.
    item_start: u32,
    /// Content column of the current item.
    col: usize,
    /// Blanks seen inside the current item.
    item_blanks: usize,
    last_para: bool,
    /// `Some((char,len))` while the current item's content is inside a fenced
    /// code block; blank lines there must not loosen the list.
    fence: Option<(u8, usize)>,
    /// Set when the current item is empty (marker only) and a blank line has
    /// followed: a later indented line cannot then join this item.
    empty_then_blank: bool,
    /// Looseness must only count a blank that separates two *direct* children
    /// of the item (`CommonMark` §5.3: "directly contain"). A blank *inside* a
    /// sub-container (a nested list or blockquote) does not loosen the outer
    /// list. This is the dedented-indent threshold of the outermost open
    /// sub-container: a post-blank line at or beyond it continues that
    /// sub-container (no loosening); a line below it is a direct child.
    sub_col: Option<usize>,
}

impl ListWalk {
    /// Record a content line `content` joining the current item. A preceding
    /// blank loosens the list when `loosens`.
    fn take_content(&mut self, content: &[u8], loosens: bool) {
        if self.item_blanks > 0 && loosens {
            self.loose = true;
        }
        self.item_blanks = 0;
        self.fence = content.update_fence(self.fence);
        self.last_para = true;
    }
}

impl ParseCtx<'_> {
    /// Scan a whole top-level list starting at `start`. Splits the list into
    /// items in a single byte walk, pushes each item's *dedented* content
    /// lines onto the line pool, and records an [`ItemMeta`](super::ItemMeta)
    /// per item plus the loose/tight flag and ordered start.
    ///
    /// Nested sublists are derived lazily in pass 2 (the dedented item lines
    /// are re-scanned only when their own marker is met), so this walk only
    /// tracks the *outermost* item structure.
    pub(super) fn scan_list(&mut self, start: usize) -> ListScan {
        let items_start = self.list_items.len().lines_offset();
        let le = self.line_end_of(start);
        let ind0 = self.bytes[start..le].leading_spaces();
        let family = self.bytes[start + ind0..le]
            .list_marker()
            .expect("caller verified a marker");
        let mut walk = ListWalk {
            loose: false,
            pending_blanks: 0,
            item_start: 0,
            col: 0,
            item_blanks: 0,
            last_para: true,
            fence: None,
            empty_then_blank: false,
            sub_col: None,
        };
        self.open_item(&mut walk, start, le, ind0, family);
        let mut pos = le + 1;
        while pos < self.bytes.len() {
            let le = self.line_end_of(pos);
            if !self.list_line(&mut walk, family, pos, le) {
                break;
            }
            pos = le + 1;
        }
        self.close_item(walk.item_start, &mut walk.pending_blanks);

        ListScan {
            items_start,
            items_len: self.list_items.len().lines_offset() - items_start,
            tight: !walk.loose,
            ordered: family.ordered,
            resume: pos,
        }
    }

    fn line_end_of(&self, pos: usize) -> usize {
        self.bytes
            .find_byte(pos, SpecialChar::Newline.byte())
            .unwrap_or(self.bytes.len())
    }

    /// Open a new item whose marker `m` sits `ind` spaces into `pos..le`.
    fn open_item(&mut self, walk: &mut ListWalk, pos: usize, le: usize, ind: usize, m: Marker) {
        let after_marker = pos + ind + m.width;
        walk.item_start = self.lines.len().lines_offset();
        walk.col = self.bytes[after_marker..le].item_content_indent(ind, m);
        walk.item_blanks = 0;
        walk.last_para = true;
        walk.empty_then_blank = false;
        walk.sub_col = None;
        walk.fence = self.bytes[after_marker.min(le)..le].update_fence(None);
        self.push_marker_content(pos, le, walk.col);
    }

    /// Fold the line `pos..le` into the list. Returns `false` when the line
    /// ends the list (and is not consumed).
    fn list_line(&mut self, walk: &mut ListWalk, family: Marker, pos: usize, le: usize) -> bool {
        let line = &self.bytes[pos..le];
        if line.iter().all(u8::is_ascii_whitespace) {
            self.list_blank(walk);
            return true;
        }
        let ind = line.leading_spaces();
        if ind < walk.col
            && !walk.empty_then_blank
            && line.leading_columns() >= walk.col
            && self.list_tab_continuation(walk, pos, le)
        {
            return true;
        }
        if ind >= walk.col {
            // An empty item followed by a blank line cannot absorb later
            // content: end the list here so it parses outside.
            if walk.empty_then_blank {
                return false;
            }
            self.list_continuation(walk, pos, le, ind);
            return true;
        }
        let body = &self.bytes[pos + ind..le];
        // A thematic break (`* * *`) ends the list rather than opening a new
        // item, even though its first run looks like a bullet marker.
        if ind <= 3 && body.is_horizontal_rule() {
            return false;
        }
        // Dedented sibling marker: close this item, open the next.
        if ind <= 3
            && let Some(m) = body.list_marker()
        {
            if !family.same_family(m) {
                return false;
            }
            self.close_item(walk.item_start, &mut walk.pending_blanks);
            if walk.pending_blanks > 0 {
                walk.loose = true;
                walk.pending_blanks = 0;
            }
            self.open_item(walk, pos, le, ind, m);
            return true;
        }
        self.list_lazy(walk, pos, le, ind)
    }

    fn list_blank(&mut self, walk: &mut ListWalk) {
        // An empty list item (marker with no content) followed by a blank line
        // begins with a blank line, so it cannot contain a following
        // continuation block (`CommonMark` §5.2). A sibling marker can still
        // extend the list.
        if walk.fence.is_none() {
            if self.lines.len().lines_offset() == walk.item_start {
                walk.empty_then_blank = true;
            }
            // A blank line inside a fenced code block stays part of the block
            // and does not make the list loose.
            walk.item_blanks += 1;
        }
        self.push_line("", false);
        walk.last_para = false;
    }

    /// A continuation line that reaches the content column only via a tab
    /// (e.g. `\tbar` is 4 columns). Returns whether the line was consumed.
    fn list_tab_continuation(&mut self, walk: &mut ListWalk, pos: usize, le: usize) -> bool {
        let src = self.input.get(pos..le).unwrap_or("");
        if let Some(content) = src.strip_columns(walk.col) {
            // Clean byte boundary (pure-space prefix, or a tab landing exactly
            // on the content column): borrow the slice as-is.
            walk.take_content(content.as_bytes(), true);
            self.push_line(content, false);
            return true;
        }
        if self.bytes[pos..le].leading_columns() - walk.col >= 4 {
            // The content column lands mid-tab *and* the remainder is indented
            // code (≥4 columns past the column). Emit the straddled tab's
            // leftover columns as synthetic `pad`; the slice resumes on a tab
            // stop so it still renders with the correct leading spaces
            // (`CommonMark` §2.2, example 5). When the remainder is shallower
            // it is a nested list / paragraph, handled byte-wise by the other
            // paths — feeding pad there would confuse the byte-based sublist
            // parser.
            let (cpad, content) = src.strip_columns_padded(walk.col);
            walk.take_content(content.as_bytes(), true);
            self.push_line_padded(content, false, cpad);
            return true;
        }
        false
    }

    /// A continuation line indented `ind` spaces, at or past the item's
    /// content column.
    fn list_continuation(&mut self, walk: &mut ListWalk, pos: usize, le: usize, ind: usize) {
        // Dedented indentation of this line within the item frame, and its
        // content past that indentation.
        let dind = ind - walk.col;
        let content = self.input.get(pos + walk.col..le).unwrap_or("");
        let inner = &content.as_bytes()[dind.min(content.len())..];
        // A preceding blank loosens the list only when this line is a *direct*
        // child of the item, not inside an open sub-container.
        let in_sub = walk.sub_col.is_some_and(|th| dind >= th);
        // A line that does not continue the current sub-container becomes a
        // new direct child: a nested list/blockquote (re)opens a sub-container
        // at its marker column; anything else closes it.
        if !in_sub {
            walk.sub_col = ((!inner.is_horizontal_rule() && inner.list_marker().is_some())
                || inner.first() == Some(&SpecialChar::GreaterThan.byte()))
            .then_some(dind + 1);
        }
        walk.take_content(content.as_bytes(), !in_sub);
        self.push_dedented(pos, le, walk.col);
    }

    /// Lazy paragraph continuation. A marker in the "dead zone" — indented
    /// past the 3-space sibling threshold but short of the item content
    /// column — can neither open a sub-item (too shallow) nor a sibling (too
    /// deep), so it is ordinary lazy paragraph text and must stay text in
    /// pass 2 (flagged lazy; `CommonMark` example 312). Returns whether the
    /// line was consumed.
    fn list_lazy(&mut self, walk: &ListWalk, pos: usize, le: usize, ind: usize) -> bool {
        let body = &self.bytes[pos + ind..le];
        let dead_zone_marker =
            ind > 3 && ind < walk.col && !body.is_horizontal_rule() && body.list_marker().is_some();
        if walk.item_blanks > 0 || !walk.last_para || (body.begins_block() && !dead_zone_marker) {
            return false;
        }
        // Lazy lines are dedented by trimming (they sit left of `col`).
        self.push_line(
            self.input.get(pos + ind..le).unwrap_or(""),
            dead_zone_marker,
        );
        true
    }
}
