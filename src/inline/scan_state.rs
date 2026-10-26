use super::{Inline, InlineSink};

/// Position state of one inline scan pass.
pub(super) struct ScanState {
    /// Start of the pending plain-text run.
    pub plain_start: usize,
    /// Current scan position.
    pub i: usize,
    /// `[` that did not start a link and are not yet closed by a `]`. While
    /// any is open, no GFM autolink is created.
    pub open_brackets: u32,
}

impl ScanState {
    pub const fn new(start: usize) -> Self {
        Self {
            plain_start: start,
            i: start,
            open_brackets: 0,
        }
    }

    /// Emit pending plain text `input[plain_start..upto)` as a text node.
    #[inline]
    pub fn flush<'src, S: InlineSink<'src>>(&self, input: &'src str, sink: &mut S, upto: usize) {
        if let Some(text) = input.get(self.plain_start..upto)
            && !text.is_empty()
        {
            sink.text(text);
        }
    }

    /// Continue scanning at `end`, with a new plain-text run starting there.
    #[inline]
    pub const fn resume(&mut self, end: usize) {
        self.plain_start = end;
        self.i = end;
    }

    /// Emit `el` and resume scanning at `end`, where it stops.
    #[inline]
    pub fn resolve<'src, S: InlineSink<'src>>(
        &mut self,
        sink: &mut S,
        el: Inline<'src>,
        end: usize,
    ) {
        sink.resolved(el);
        self.resume(end);
    }

    /// Flush the text before `at`, then emit `el` spanning `at..end`.
    #[inline]
    pub fn emit<'src, S: InlineSink<'src>>(
        &mut self,
        input: &'src str,
        sink: &mut S,
        at: usize,
        el: Inline<'src>,
        end: usize,
    ) {
        self.flush(input, sink, at);
        self.resolve(sink, el, end);
    }
}
