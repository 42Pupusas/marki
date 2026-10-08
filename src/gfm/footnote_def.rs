//! Pass-1 detection of GFM footnote definitions (`[^label]: text`). The first
//! line's text and every following line indented four or more columns (or a
//! lazy paragraph continuation, or a blank line between such lines) belong to
//! the definition.

use crate::OffsetExt;
use crate::block::{BlockBytes, BlockStr, ParseCtx};
use crate::simd::ByteSliceExt;
use crate::special_char::SpecialChar;

/// Where footnote detection records what it finds: the definition's label,
/// and its dedented content lines in the pass-1 line pool.
pub trait FootnoteSink<'src> {
    fn register_footnote(&mut self, label: &'src str);
    fn pooled_lines(&self) -> usize;
    fn append_line(&mut self, line: &'src str, lazy: bool, pad: u8);
    fn drop_last_line(&mut self);
}

impl<'src> FootnoteSink<'src> for ParseCtx<'src> {
    fn register_footnote(&mut self, label: &'src str) {
        self.refs.footnotes.register(label);
    }

    fn pooled_lines(&self) -> usize {
        self.lines.len()
    }

    fn append_line(&mut self, line: &'src str, lazy: bool, pad: u8) {
        self.push_line_padded(line, lazy, pad);
    }

    fn drop_last_line(&mut self) {
        self.pop_line();
    }
}

/// A footnote definition found in pass 1: its label, the range of its
/// content lines in the line pool, and the byte offset to resume at.
pub struct FootnoteScan<'src> {
    pub label: &'src str,
    pub lines_start: u32,
    pub lines_len: u32,
    pub resume: usize,
}

/// The `[^label]:` prefix of a footnote definition line.
pub struct FootnoteStart<'src> {
    label: &'src str,
    content_start: usize,
}

impl<'src> FootnoteStart<'src> {
    /// Match `[^label]:` after at most three spaces on `input[pos..line_end]`.
    fn match_line(input: &'src str, pos: usize, line_end: usize) -> Option<Self> {
        let bytes = input.as_bytes();
        let indent = bytes[pos..line_end].strip_indent()?;
        let open = pos + indent;
        if bytes.get(open) != Some(&b'[') || bytes.get(open + 1) != Some(&b'^') {
            return None;
        }
        let label_start = open + 2;
        let mut close = label_start;
        while close < line_end && !matches!(bytes[close], b']' | b' ' | b'\t' | b'\r') {
            close += 1;
        }
        if close == label_start
            || bytes.get(close) != Some(&b']')
            || bytes.get(close + 1) != Some(&b':')
        {
            return None;
        }
        let mut content_start = close + 2;
        while content_start < line_end && matches!(bytes[content_start], b' ' | b'\t') {
            content_start += 1;
        }
        Some(Self {
            label: input.get(label_start..close)?,
            content_start,
        })
    }

    /// Recognise a definition starting on the line `[pos..line_end)`, register
    /// its label, and append its content lines to `sink`.
    pub fn scan(
        sink: &mut impl FootnoteSink<'src>,
        input: &'src str,
        pos: usize,
        line_end: usize,
    ) -> Option<FootnoteScan<'src>> {
        let bytes = input.as_bytes();
        let start = Self::match_line(input, pos, line_end)?;
        sink.register_footnote(start.label);

        let lines_start = sink.pooled_lines();
        let first = input.get(start.content_start..line_end).unwrap_or("");
        let mut para_open = !first.is_empty() && first.as_bytes().is_lazy_paragraph_tail();
        if !first.is_empty() {
            sink.append_line(first, false, 0);
        }
        let mut committed = sink.pooled_lines();
        let mut resume = line_end + 1;
        let mut p = line_end + 1;
        while p < bytes.len() {
            let le = bytes
                .find_byte(p, SpecialChar::Newline.byte())
                .unwrap_or(bytes.len());
            let line = &bytes[p..le];
            let text = input.get(p..le).unwrap_or("");
            if bytes.is_blank_line(p, le) {
                sink.append_line("", false, 0);
                para_open = false;
            } else if line.leading_columns() >= 4 {
                let (pad, rest) = text.strip_columns_padded(4);
                sink.append_line(rest, false, pad);
                para_open = rest.as_bytes().is_lazy_paragraph_tail();
                committed = sink.pooled_lines();
                resume = le + 1;
            } else if para_open
                && Self::match_line(input, p, le).is_none()
                && !line[line.leading_spaces().min(line.len())..].begins_block()
            {
                sink.append_line(text.trim(), true, 0);
                committed = sink.pooled_lines();
                resume = le + 1;
            } else {
                break;
            }
            p = le + 1;
        }
        while sink.pooled_lines() > committed {
            sink.drop_last_line();
        }

        Some(FootnoteScan {
            label: start.label,
            lines_start: lines_start.lines_offset(),
            lines_len: (sink.pooled_lines() - lines_start).lines_offset(),
            resume: resume.min(bytes.len()),
        })
    }
}
