//! GFM extended autolinks (GFM §6.9), matching cmark-gfm. `www.` hosts and
//! `http://`, `https://`, `ftp://` URLs are found while the inline source is
//! scanned, before emphasis, so delimiter characters inside a URL stay part of
//! it. Email addresses are found afterwards, in the final text runs.

use crate::inline::{AutolinkKind, Inline};
use crate::simd::{ByteSet, ByteSliceExt};

/// Creates extended autolinks for one inline context. Inside link text and
/// image alt text it creates none.
#[derive(Clone, Copy)]
pub struct TextLinker {
    inside_link: bool,
}

impl TextLinker {
    pub const TOP_LEVEL: Self = Self { inside_link: false };

    #[allow(clippy::unused_self)]
    pub const fn inside_link(self) -> Self {
        Self { inside_link: true }
    }

    /// The first `www.` or URL autolink whose trigger byte lies in
    /// `ctx[from..to]`, as `(start, end, inline)`. A URL scheme may begin
    /// before `from` but not before `floor`. `brackets` counts the unclosed
    /// `[` before the scan position; each `]` scanned closes one, and no
    /// autolink is created while any is open.
    pub fn url_in<'src>(
        self,
        ctx: &'src str,
        from: usize,
        to: usize,
        floor: usize,
        brackets: &mut u32,
    ) -> Option<(usize, usize, Inline<'src>)> {
        if self.inside_link {
            return None;
        }
        let found = AutolinkScanner::new(ctx).url_in(from, to, floor, brackets)?;
        Some((found.start, found.end, found.inline(ctx)))
    }

    /// Whether `ctx` contains `www.` or `://`, without which no `www.` or URL
    /// autolink can exist. Inside link text this is always `false`.
    pub fn may_have_url(self, ctx: &str) -> bool {
        if self.inside_link {
            return false;
        }
        let bytes = ctx.as_bytes();
        let mut i = 0;
        while let Some(colon) = bytes.find_byte(i, b':') {
            if bytes[colon + 1..].starts_with(b"//") {
                return true;
            }
            i = colon + 1;
        }
        ctx.contains("www.")
    }

    /// Whether plain text with no inline syntax holds a `www.` or URL link.
    pub fn has_url(self, ctx: &str) -> bool {
        self.may_have_url(ctx) && self.url_in(ctx, 0, ctx.len(), 0, &mut 0).is_some()
    }

    #[inline]
    pub fn push<'src>(self, pool: &mut Vec<Inline<'src>>, item: Inline<'src>) {
        match item {
            Inline::Text(text) if !self.inside_link && text.as_bytes().contains(&b'@') => {
                Self::push_text(pool, text);
            }
            other => pool.push(other),
        }
    }

    #[inline]
    pub fn extend<'src>(
        self,
        pool: &mut Vec<Inline<'src>>,
        items: impl IntoIterator<Item = Inline<'src>>,
    ) {
        if self.inside_link {
            pool.extend(items);
        } else {
            for item in items {
                self.push(pool, item);
            }
        }
    }

    fn push_text<'src>(pool: &mut Vec<Inline<'src>>, text: &'src str) {
        let scanner = AutolinkScanner::new(text);
        let mut done = 0;
        while let Some(found) = scanner.email_from(done) {
            if found.start > done {
                pool.push(Inline::Text(&text[done..found.start]));
            }
            pool.push(found.inline(text));
            done = found.end;
        }
        if done < text.len() {
            pool.push(Inline::Text(&text[done..]));
        }
    }
}

/// One autolink: its byte range in the scanned text and how its destination is
/// formed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AutolinkMatch {
    start: usize,
    end: usize,
    kind: AutolinkKind,
}

impl AutolinkMatch {
    fn inline(self, text: &str) -> Inline<'_> {
        Inline::Autolink {
            target: &text[self.start..self.end],
            kind: self.kind,
        }
    }
}

/// Byte-level matching rules for extended autolinks.
struct AutolinkScanner<'t> {
    src: &'t str,
    text: &'t [u8],
}

impl<'t> AutolinkScanner<'t> {
    const SCHEMES: [&'static [u8]; 3] = [b"http://", b"https://", b"ftp://"];
    const URL_TRIGGERS: ByteSet<3> = ByteSet::new(b"w:]");
    const EMAIL_TRIGGERS: ByteSet<3> = ByteSet::new(b"@[]");

    const fn new(src: &'t str) -> Self {
        Self {
            src,
            text: src.as_bytes(),
        }
    }

    fn url_in(
        &self,
        from: usize,
        to: usize,
        floor: usize,
        brackets: &mut u32,
    ) -> Option<AutolinkMatch> {
        let region = &self.text[..to.min(self.text.len())];
        let mut i = from;
        while let Some(pos) = region.find_byte_set(i, &Self::URL_TRIGGERS) {
            match self.text[pos] {
                b']' => *brackets = brackets.saturating_sub(1),
                _ if *brackets > 0 => {}
                b':' => {
                    if let Some(found) = self.url_at(pos, floor) {
                        return Some(found);
                    }
                }
                _ => {
                    if let Some(found) = self.www_at(pos) {
                        return Some(found);
                    }
                }
            }
            i = pos + 1;
        }
        None
    }

    /// The first email autolink at or after `from`, skipping any `@` inside
    /// an unclosed `[`.
    fn email_from(&self, from: usize) -> Option<AutolinkMatch> {
        let mut depth = 0i32;
        let mut i = from;
        while let Some(pos) = self.text.find_byte_set(i, &Self::EMAIL_TRIGGERS) {
            match self.text[pos] {
                b'[' => depth += 1,
                b']' => depth -= 1,
                _ if depth > 0 => {}
                _ => {
                    if let Some(found) = self.email_at(pos, from) {
                        return Some(found);
                    }
                }
            }
            i = pos + 1;
        }
        None
    }

    fn www_at(&self, i: usize) -> Option<AutolinkMatch> {
        let text = self.text;
        if i > 0 && !Self::www_may_follow(text[i - 1]) {
            return None;
        }
        if !text[i..].starts_with(b"www.") {
            return None;
        }
        let body_end = self.body_end(i + self.domain_len(i, false)?);
        let end = self.trim_trailing(i, body_end);
        (end > i).then_some(AutolinkMatch {
            start: i,
            end,
            kind: AutolinkKind::Www,
        })
    }

    fn url_at(&self, colon: usize, floor: usize) -> Option<AutolinkMatch> {
        let text = self.text;
        if text.len() - colon < 4 || text[colon + 1] != b'/' || text[colon + 2] != b'/' {
            return None;
        }
        let mut start = colon;
        while start > floor && text[start - 1].is_ascii_alphabetic() {
            start -= 1;
        }
        let candidate = &text[start..];
        let scheme_ok = Self::SCHEMES.iter().any(|scheme| {
            candidate.len() > scheme.len()
                && candidate[..scheme.len()].eq_ignore_ascii_case(scheme)
                && self.is_host_char_at(start + scheme.len())
        });
        if !scheme_ok {
            return None;
        }
        let body_end = self.body_end(colon + 3 + self.domain_len(colon + 3, true)?);
        let end = self.trim_trailing(colon, body_end);
        (end > colon).then_some(AutolinkMatch {
            start,
            end,
            kind: AutolinkKind::Uri,
        })
    }

    fn email_at(&self, at: usize, floor: usize) -> Option<AutolinkMatch> {
        let text = self.text;
        let mut start = at;
        let mut kind = AutolinkKind::Email;
        let mut xmpp = false;
        while start > floor {
            let c = text[start - 1];
            if c.is_ascii_alphanumeric() || matches!(c, b'.' | b'+' | b'-' | b'_') {
                start -= 1;
            } else if c == b':' && self.protocol_before(start - 1, floor, b"mailto") {
                kind = AutolinkKind::Uri;
                start -= 1;
            } else if c == b':' && self.protocol_before(start - 1, floor, b"xmpp") {
                kind = AutolinkKind::Uri;
                xmpp = true;
                start -= 1;
            } else {
                break;
            }
        }
        if start == at {
            return None;
        }
        let mut end = at + 1;
        let mut dots = 0;
        while let Some(&c) = text.get(end) {
            if c == b'@' {
                return None;
            }
            let dot = c == b'.' && text.get(end + 1).is_some_and(u8::is_ascii_alphanumeric);
            if dot {
                dots += 1;
            } else if !(c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || (xmpp && c == b'/'))
            {
                break;
            }
            end += 1;
        }
        let last = text[end - 1];
        if end - at < 2 || dots == 0 || !(last.is_ascii_alphabetic() || last == b'.') {
            return None;
        }
        let end = self.trim_trailing(at, end);
        (end > at).then_some(AutolinkMatch { start, end, kind })
    }

    fn protocol_before(&self, colon: usize, floor: usize, protocol: &[u8]) -> bool {
        let mut start = colon;
        while start > floor && self.text[start - 1].is_ascii_alphabetic() {
            start -= 1;
        }
        &self.text[start..colon] == protocol
    }

    /// Length of the domain at `from`, scanned byte by byte as cmark-gfm
    /// does: the first byte is always taken, the scan stops at the first byte
    /// that is neither a host character nor `-`, and the last byte is never
    /// examined. The last two labels may not contain `_`. Without
    /// `allow_short` the domain needs at least one `.`.
    fn domain_len(&self, from: usize, allow_short: bool) -> Option<usize> {
        let size = self.text.len() - from;
        let mut dots = 0;
        let mut underscores_prev = 0;
        let mut underscores_last = 0;
        let mut i = 1;
        while i + 1 < size {
            if self.text[from + i] == b'\\' && i + 2 < size {
                i += 1;
            }
            match self.text[from + i] {
                b'_' => underscores_last += 1,
                b'.' => {
                    underscores_prev = underscores_last;
                    underscores_last = 0;
                    dots += 1;
                }
                b'-' => {}
                _ if self.is_host_char_at(from + i) => {}
                _ => break,
            }
            i += 1;
        }
        if underscores_prev > 0 || underscores_last > 0 {
            return None;
        }
        (allow_short || dots > 0).then_some(i)
    }

    /// Whether the byte at `at` starts a host character: a decodable code
    /// point that is neither whitespace nor punctuation. A UTF-8 continuation
    /// byte never is.
    fn is_host_char_at(&self, at: usize) -> bool {
        let Some(&b) = self.text.get(at) else {
            return false;
        };
        if b.is_ascii() {
            return !(b.is_ascii_whitespace() || b.is_ascii_punctuation() || b == 0x0b);
        }
        self.src
            .get(at..)
            .and_then(|rest| rest.chars().next())
            .is_some_and(|c| !(c.is_whitespace() || Self::is_unicode_punctuation(c)))
    }

    const fn body_end(&self, mut end: usize) -> usize {
        while end < self.text.len() && !Self::is_space(self.text[end]) {
            end += 1;
        }
        end
    }

    /// GFM extended autolink path validation: cut at `<`, then drop trailing
    /// punctuation, an `&name;` entity-like tail, and unbalanced `)`. `start`
    /// is where parenthesis counting begins.
    fn trim_trailing(&self, start: usize, mut end: usize) -> usize {
        let text = self.text;
        if let Some(lt) = text[start..end].iter().position(|&b| b == b'<') {
            end = start + lt;
        }
        while end > start {
            match text[end - 1] {
                b'?' | b'!' | b'.' | b',' | b':' | b'*' | b'_' | b'~' | b'\'' | b'"' => end -= 1,
                b';' if end - start < 2 => end -= 1,
                b';' => {
                    let mut k = end - 2;
                    while k > start && text[k].is_ascii_alphabetic() {
                        k -= 1;
                    }
                    end = if k < end - 2 && text[k] == b'&' {
                        k
                    } else {
                        end - 1
                    };
                }
                b')' => {
                    let (open, close) =
                        text[start..end]
                            .iter()
                            .fold((0usize, 0usize), |(o, c), &b| match b {
                                b'(' => (o + 1, c),
                                b')' => (o, c + 1),
                                _ => (o, c),
                            });
                    if close <= open {
                        break;
                    }
                    end -= 1;
                }
                _ => break,
            }
        }
        end
    }

    const fn is_space(b: u8) -> bool {
        matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
    }

    const fn www_may_follow(b: u8) -> bool {
        Self::is_space(b) || matches!(b, b'*' | b'_' | b'~' | b'(')
    }

    /// Unicode general category P (Pc, Pd, Ps, Pe, Pi, Pf, Po) outside ASCII,
    /// covering the blocks where punctuation occurs in practice.
    const fn is_unicode_punctuation(c: char) -> bool {
        matches!(c,
            '\u{00A1}' | '\u{00A7}' | '\u{00AB}' | '\u{00B6}' | '\u{00B7}' | '\u{00BB}'
            | '\u{00BF}' | '\u{037E}' | '\u{0387}'
            | '\u{055A}'..='\u{055F}'
            | '\u{0589}' | '\u{058A}' | '\u{05BE}' | '\u{05C0}' | '\u{05C3}' | '\u{05C6}'
            | '\u{05F3}' | '\u{05F4}' | '\u{0609}' | '\u{060A}' | '\u{060C}' | '\u{060D}'
            | '\u{061B}' | '\u{061E}' | '\u{061F}'
            | '\u{066A}'..='\u{066D}'
            | '\u{06D4}' | '\u{0964}' | '\u{0965}' | '\u{0970}' | '\u{0E4F}' | '\u{0E5A}'
            | '\u{0E5B}'
            | '\u{2010}'..='\u{2027}'
            | '\u{2030}'..='\u{2043}'
            | '\u{2045}'..='\u{2051}'
            | '\u{2053}'..='\u{205E}'
            | '\u{207D}' | '\u{207E}' | '\u{208D}' | '\u{208E}'
            | '\u{2308}'..='\u{230B}'
            | '\u{2329}' | '\u{232A}'
            | '\u{2768}'..='\u{2775}'
            | '\u{27C5}' | '\u{27C6}'
            | '\u{27E6}'..='\u{27EF}'
            | '\u{2983}'..='\u{2998}'
            | '\u{29D8}'..='\u{29DB}'
            | '\u{29FC}' | '\u{29FD}'
            | '\u{2CF9}'..='\u{2CFC}'
            | '\u{2CFE}' | '\u{2CFF}'
            | '\u{2E00}'..='\u{2E4F}'
            | '\u{3001}'..='\u{3003}'
            | '\u{3008}'..='\u{3011}'
            | '\u{3014}'..='\u{301F}'
            | '\u{3030}' | '\u{303D}' | '\u{30A0}' | '\u{30FB}'
            | '\u{FE10}'..='\u{FE19}'
            | '\u{FE30}'..='\u{FE52}'
            | '\u{FE54}'..='\u{FE61}'
            | '\u{FE63}' | '\u{FE68}' | '\u{FE6A}' | '\u{FE6B}'
            | '\u{FF01}'..='\u{FF03}'
            | '\u{FF05}'..='\u{FF0A}'
            | '\u{FF0C}'..='\u{FF0F}'
            | '\u{FF1A}' | '\u{FF1B}' | '\u{FF1F}' | '\u{FF20}'
            | '\u{FF3B}'..='\u{FF3D}'
            | '\u{FF3F}' | '\u{FF5B}' | '\u{FF5D}'
            | '\u{FF5F}'..='\u{FF65}'
        )
    }
}

#[cfg(test)]
mod tests {
    use super::AutolinkScanner;
    use crate::inline::AutolinkKind;

    struct Case;

    impl Case {
        fn first(text: &str) -> Option<(&str, AutolinkKind)> {
            let scanner = AutolinkScanner::new(text);
            scanner
                .url_in(0, text.len(), 0, &mut 0)
                .or_else(|| scanner.email_from(0))
                .map(|m| (&text[m.start..m.end], m.kind))
        }
    }

    #[test]
    fn www_host() {
        assert_eq!(
            Case::first("Visit www.commonmark.org/help for more."),
            Some(("www.commonmark.org/help", AutolinkKind::Www))
        );
    }

    #[test]
    fn trailing_punctuation_dropped() {
        assert_eq!(
            Case::first("www.commonmark.org/a.b."),
            Some(("www.commonmark.org/a.b", AutolinkKind::Www))
        );
    }

    #[test]
    fn unbalanced_paren_dropped() {
        assert_eq!(
            Case::first("(www.google.com/search?q=Markup+(business))"),
            Some((
                "www.google.com/search?q=Markup+(business)",
                AutolinkKind::Www
            ))
        );
    }

    #[test]
    fn entity_tail_dropped() {
        assert_eq!(
            Case::first("www.google.com/search?q=commonmark&hl;"),
            Some(("www.google.com/search?q=commonmark", AutolinkKind::Www))
        );
    }

    #[test]
    fn underscore_in_last_labels_rejected() {
        assert_eq!(Case::first("www.xxx.yyy._zzz"), None);
    }

    #[test]
    fn http_scheme() {
        assert_eq!(
            Case::first("see http://example.com/x now"),
            Some(("http://example.com/x", AutolinkKind::Uri))
        );
    }

    #[test]
    fn scheme_rules() {
        assert_eq!(Case::first("gopher://example.com"), None);
        assert_eq!(Case::first("xhttp://example.com"), None);
        assert_eq!(
            Case::first("HTTP://example.com"),
            Some(("HTTP://example.com", AutolinkKind::Uri))
        );
        assert_eq!(Case::first("WWW.example.com"), None);
    }

    #[test]
    fn url_allows_short_domain() {
        assert_eq!(
            Case::first("http://localhost now"),
            Some(("http://localhost", AutolinkKind::Uri))
        );
    }

    #[test]
    fn email() {
        assert_eq!(
            Case::first("mail foo@bar.baz now"),
            Some(("foo@bar.baz", AutolinkKind::Email))
        );
    }

    #[test]
    fn mailto_prefix_kept_in_target() {
        assert_eq!(
            Case::first("mailto:foo@bar.baz"),
            Some(("mailto:foo@bar.baz", AutolinkKind::Uri))
        );
    }

    #[test]
    fn email_trailing_dash_rejected() {
        assert_eq!(Case::first("a.b-c_d@a.b-"), None);
        assert_eq!(Case::first("a.b-c_d@a.b_"), None);
    }

    #[test]
    fn email_trailing_dot_dropped() {
        assert_eq!(
            Case::first("a.b-c_d@a.b."),
            Some(("a.b-c_d@a.b", AutolinkKind::Email))
        );
    }

    #[test]
    fn email_inside_open_bracket_ignored() {
        assert_eq!(Case::first("see [a@b.com"), None);
    }

    #[test]
    fn www_inside_word_ignored() {
        assert_eq!(Case::first("awww.example.com"), None);
    }

    #[test]
    fn open_brackets_block_urls_until_closed() {
        let text = "[[ www.a.com ] www.b.com ] www.c.com";
        let scanner = AutolinkScanner::new(text);
        let mut brackets = 2;
        let found = scanner.url_in(2, text.len(), 0, &mut brackets).unwrap();
        assert_eq!(&text[found.start..found.end], "www.c.com");
    }
}
