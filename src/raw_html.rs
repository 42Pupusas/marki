//! Shared HTML tag recognition for `CommonMark` (`spec` §6.6 / §4.6).
//!
//! These scanners implement the HTML grammar subset `CommonMark` uses for raw
//! HTML — open tags, closing tags, comments, processing instructions,
//! declarations, and CDATA. They are reused by the block-level HTML block
//! parser and the inline raw-HTML parser.
//!
//! Per the crate's no-free-functions rule, every scanner is a method: byte
//! classifiers hang off [`HtmlByte`] (`u8`) and the slice scanners off
//! [`HtmlScan`] (`[u8]`).

/// Byte classifiers for HTML tag names.
pub trait HtmlByte {
    fn tag_name_start(self) -> bool;
    fn tag_name_char(self) -> bool;
}

impl HtmlByte for u8 {
    /// Returns true for a byte that may start an HTML tag name (ASCII letter).
    #[inline]
    fn tag_name_start(self) -> bool {
        self.is_ascii_alphabetic()
    }

    /// Returns true for a byte that may continue an HTML tag name.
    #[inline]
    fn tag_name_char(self) -> bool {
        self.is_ascii_alphanumeric() || self == b'-'
    }
}

/// HTML tag/construct scanners over a byte slice. Each scanner operates at a
/// fixed offset (index 0 unless noted) and returns the number of bytes consumed.
pub trait HtmlScan {
    fn scan_open_tag(&self) -> Option<usize>;
    fn scan_attribute_value(&self, i: usize) -> Option<usize>;
    fn scan_closing_tag(&self) -> Option<usize>;
    fn scan_inline_html(&self) -> Option<usize>;
    fn scan_declarationish(&self) -> Option<usize>;
    fn scan_until(&self, start: usize, close: &[u8]) -> Option<usize>;
    fn html_block_start(&self, in_paragraph: bool) -> Option<HtmlBlockKind>;
    fn type1_end(&self) -> bool;
    fn eq_ignore_case(&self, other: &[u8]) -> bool;
    fn contains_ci(&self, needle: &[u8]) -> bool;
}

impl HtmlScan for [u8] {
    /// Scan an open tag beginning at index 0 (the leading `<` included).
    /// Returns the number of bytes consumed through the closing `>`, or `None`.
    ///
    /// Grammar (`CommonMark` §6.6): `<` tagname (whitespace attribute)\*
    /// whitespace? `/`? `>`.
    fn scan_open_tag(&self) -> Option<usize> {
        let b = self;
        if b.first() != Some(&b'<') {
            return None;
        }
        let mut i = 1;
        // Tag name.
        if !b.get(i).copied().is_some_and(u8::tag_name_start) {
            return None;
        }
        i += 1;
        while b.get(i).copied().is_some_and(u8::tag_name_char) {
            i += 1;
        }
        // Attributes.
        loop {
            let ws_start = i;
            while b.get(i).is_some_and(u8::is_ascii_whitespace) {
                i += 1;
            }
            let had_ws = i > ws_start;
            // Optional `/` then `>` ends the tag.
            if b.get(i) == Some(&b'/') && b.get(i + 1) == Some(&b'>') {
                return Some(i + 2);
            }
            if b.get(i) == Some(&b'>') {
                return Some(i + 1);
            }
            // An attribute must be preceded by whitespace.
            if !had_ws {
                return None;
            }
            // Attribute name: [A-Za-z_:][A-Za-z0-9_.:-]*
            let name_start = i;
            if !b
                .get(i)
                .is_some_and(|&c| c.is_ascii_alphabetic() || c == b'_' || c == b':')
            {
                return None;
            }
            i += 1;
            while b.get(i).is_some_and(|&c| {
                c.is_ascii_alphanumeric() || c == b'_' || c == b'.' || c == b':' || c == b'-'
            }) {
                i += 1;
            }
            if i == name_start {
                return None;
            }
            // Optional value spec: whitespace? `=` whitespace? value.
            let mut j = i;
            while b.get(j).is_some_and(u8::is_ascii_whitespace) {
                j += 1;
            }
            if b.get(j) == Some(&b'=') {
                j += 1;
                while b.get(j).is_some_and(u8::is_ascii_whitespace) {
                    j += 1;
                }
                i = b.scan_attribute_value(j)?;
            }
            // else: bare attribute, continue (i already past name).
        }
    }

    /// Scan an attribute value at index `i`. Returns the index past the value.
    fn scan_attribute_value(&self, i: usize) -> Option<usize> {
        let b = self;
        match b.get(i) {
            Some(b'\'') => {
                let mut j = i + 1;
                while b.get(j).is_some_and(|&c| c != b'\'') {
                    j += 1;
                }
                (b.get(j) == Some(&b'\'')).then_some(j + 1)
            }
            Some(b'"') => {
                let mut j = i + 1;
                while b.get(j).is_some_and(|&c| c != b'"') {
                    j += 1;
                }
                (b.get(j) == Some(&b'"')).then_some(j + 1)
            }
            // Unquoted: one or more chars excluding whitespace and " ' = < > ` .
            _ => {
                let start = i;
                let mut j = i;
                while b.get(j).is_some_and(|&c| {
                    !c.is_ascii_whitespace() && !matches!(c, b'"' | b'\'' | b'=' | b'<' | b'>' | b'`')
                }) {
                    j += 1;
                }
                (j > start).then_some(j)
            }
        }
    }

    /// Scan a closing tag beginning at index 0 (the leading `</` included).
    /// Returns the bytes consumed through the closing `>`, or `None`.
    fn scan_closing_tag(&self) -> Option<usize> {
        let b = self;
        if b.first() != Some(&b'<') || b.get(1) != Some(&b'/') {
            return None;
        }
        let mut i = 2;
        if !b.get(i).copied().is_some_and(u8::tag_name_start) {
            return None;
        }
        i += 1;
        while b.get(i).copied().is_some_and(u8::tag_name_char) {
            i += 1;
        }
        while b.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        (b.get(i) == Some(&b'>')).then_some(i + 1)
    }

    /// Scan any inline raw-HTML construct at index 0: open/closing tag, comment,
    /// processing instruction, declaration, or CDATA. Returns bytes consumed.
    fn scan_inline_html(&self) -> Option<usize> {
        let b = self;
        if b.first() != Some(&b'<') {
            return None;
        }
        match b.get(1) {
            Some(b'!') => b.scan_declarationish(),
            Some(b'?') => b.scan_until(2, b"?>"),
            Some(b'/') => b.scan_closing_tag(),
            _ => b.scan_open_tag(),
        }
    }

    /// Scan `<!` constructs: comment, CDATA, or declaration.
    fn scan_declarationish(&self) -> Option<usize> {
        let b = self;
        if b.starts_with(b"<!--") {
            // Comment (CommonMark 0.30 §6.6): `<!-->` and `<!--->` are complete
            // comments. Otherwise the text after `<!--` must not start with `>`
            // or `->`, then runs up to the closing `-->`.
            if b.starts_with(b"<!-->") {
                return Some(5);
            }
            if b.starts_with(b"<!--->") {
                return Some(6);
            }
            if matches!(b.get(4), Some(&b'>')) || b[4..].starts_with(b"->") {
                return None;
            }
            return b.scan_until(4, b"-->");
        }
        if b.starts_with(b"<![CDATA[") {
            return b.scan_until(9, b"]]>");
        }
        // Declaration: <! + ASCII letter ... >
        if b.get(2).is_some_and(u8::is_ascii_alphabetic) {
            let mut i = 3;
            while b.get(i).is_some_and(|&c| c != b'>') {
                i += 1;
            }
            return (b.get(i) == Some(&b'>')).then_some(i + 1);
        }
        None
    }

    /// Scan from `start` to just past the first occurrence of `close`.
    fn scan_until(&self, start: usize, close: &[u8]) -> Option<usize> {
        let b = self;
        let mut i = start;
        while i + close.len() <= b.len() {
            if &b[i..i + close.len()] == close {
                return Some(i + close.len());
            }
            i += 1;
        }
        None
    }

    /// Determine whether `self` (one line, leading indentation already stripped)
    /// starts an HTML block, returning the block kind. `in_paragraph` is true
    /// when the previous line was part of an open paragraph; type 7 only starts
    /// a block when it can interrupt (i.e. not mid-paragraph).
    fn html_block_start(&self, in_paragraph: bool) -> Option<HtmlBlockKind> {
        let line = self;
        if line.first() != Some(&b'<') {
            return None;
        }
        // Type 2: <!--
        if line.starts_with(b"<!--") {
            return Some(HtmlBlockKind::Type2);
        }
        // Type 3: <?
        if line.starts_with(b"<?") {
            return Some(HtmlBlockKind::Type3);
        }
        // Type 5: <![CDATA[
        if line.starts_with(b"<![CDATA[") {
            return Some(HtmlBlockKind::Type5);
        }
        // Type 4: <! + ASCII letter
        if line.starts_with(b"<!") && line.get(2).is_some_and(u8::is_ascii_alphabetic) {
            return Some(HtmlBlockKind::Type4);
        }
        // Extract the tag name (after optional `/`).
        let name_start = if line.get(1) == Some(&b'/') { 2 } else { 1 };
        let mut end = name_start;
        while line.get(end).is_some_and(u8::is_ascii_alphanumeric) {
            end += 1;
        }
        let name = &line[name_start..end];
        if name.is_empty() {
            return None;
        }

        // Type 1: raw-text tags, only as open tags, followed by ws / > / EOL.
        if name_start == 1
            && RAW_TEXT_TAGS.iter().any(|t| name.eq_ignore_case(t))
            && line
                .get(end)
                .is_none_or(|&c| c.is_ascii_whitespace() || c == b'>')
        {
            return Some(HtmlBlockKind::Type1);
        }

        // Type 6: known block tags followed by ws, end-of-line, `>`, or `/>`.
        if BLOCK_TAGS.iter().any(|t| name.eq_ignore_case(t)) {
            let ok = match line.get(end) {
                None => true,
                Some(&c) if c.is_ascii_whitespace() || c == b'>' => true,
                Some(&b'/') if line.get(end + 1) == Some(&b'>') => true,
                _ => false,
            };
            if ok {
                return Some(HtmlBlockKind::Type6);
            }
        }

        // Type 7: a complete standalone tag filling the whole line. Cannot
        // interrupt a paragraph.
        if !in_paragraph {
            let consumed = if name_start == 2 {
                line.scan_closing_tag()?
            } else {
                // Type 7 excludes the raw-text tags handled by type 1.
                if RAW_TEXT_TAGS.iter().any(|t| name.eq_ignore_case(t)) {
                    return None;
                }
                line.scan_open_tag()?
            };
            // Only whitespace may follow the tag on the line.
            if line[consumed..].iter().all(u8::is_ascii_whitespace) {
                return Some(HtmlBlockKind::Type7);
            }
        }
        None
    }

    /// True if `self` contains any of the type-1 closing markers.
    fn type1_end(&self) -> bool {
        const CLOSERS: &[&[u8]] = &[b"</script>", b"</pre>", b"</style>", b"</textarea>"];
        CLOSERS.iter().any(|c| self.contains_ci(c))
    }

    /// Case-insensitive ASCII byte-slice comparison.
    fn eq_ignore_case(&self, other: &[u8]) -> bool {
        self.len() == other.len() && self.iter().zip(other).all(|(x, y)| x.eq_ignore_ascii_case(y))
    }

    /// Case-insensitive substring search.
    fn contains_ci(&self, needle: &[u8]) -> bool {
        if needle.len() > self.len() {
            return false;
        }
        self.windows(needle.len()).any(|w| w.eq_ignore_case(needle))
    }
}

// ---------------------------------------------------------------------------
// Block-level HTML start conditions (CommonMark §4.6).
// ---------------------------------------------------------------------------

/// The seven kinds of HTML block, distinguished by start/end conditions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HtmlBlockKind {
    /// `<script`, `<pre`, `<style`, `<textarea` — ends at matching close tag.
    Type1,
    /// `<!--` — ends at `-->`.
    Type2,
    /// `<?` — ends at `?>`.
    Type3,
    /// `<!` + letter — ends at `>`.
    Type4,
    /// `<![CDATA[` — ends at `]]>`.
    Type5,
    /// A known block-level tag name — ends at a blank line.
    Type6,
    /// A complete standalone open/closing tag — ends at a blank line.
    Type7,
}

impl HtmlBlockKind {
    /// The literal end marker for types 1–5 whose block ends on the line that
    /// contains it. Types 6 and 7 end at a blank line and return `None`.
    pub const fn end_marker(self) -> Option<&'static [u8]> {
        match self {
            Self::Type2 => Some(b"-->"),
            Self::Type3 => Some(b"?>"),
            Self::Type4 => Some(b">"),
            Self::Type5 => Some(b"]]>"),
            // Type1 uses multiple markers (handled specially); Type6/Type7 end
            // at a blank line.
            Self::Type1 | Self::Type6 | Self::Type7 => None,
        }
    }
}

/// HTML block tag names for start condition 6 (`CommonMark` §4.6).
const BLOCK_TAGS: &[&[u8]] = &[
    b"address",
    b"article",
    b"aside",
    b"base",
    b"basefont",
    b"blockquote",
    b"body",
    b"caption",
    b"center",
    b"col",
    b"colgroup",
    b"dd",
    b"details",
    b"dialog",
    b"dir",
    b"div",
    b"dl",
    b"dt",
    b"fieldset",
    b"figcaption",
    b"figure",
    b"footer",
    b"form",
    b"frame",
    b"frameset",
    b"h1",
    b"h2",
    b"h3",
    b"h4",
    b"h5",
    b"h6",
    b"head",
    b"header",
    b"hr",
    b"html",
    b"iframe",
    b"legend",
    b"li",
    b"link",
    b"main",
    b"menu",
    b"menuitem",
    b"nav",
    b"noframes",
    b"ol",
    b"optgroup",
    b"option",
    b"p",
    b"param",
    b"search",
    b"section",
    b"summary",
    b"table",
    b"tbody",
    b"td",
    b"tfoot",
    b"th",
    b"thead",
    b"title",
    b"tr",
    b"track",
    b"ul",
];

/// Tag names for start condition 1 (raw text elements).
const RAW_TEXT_TAGS: &[&[u8]] = &[b"script", b"pre", b"style", b"textarea"];
