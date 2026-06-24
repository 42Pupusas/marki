//! Shared HTML tag recognition for `CommonMark` (`spec` §6.6 / §4.6).
//!
//! These scanners implement the HTML grammar subset `CommonMark` uses for raw
//! HTML — open tags, closing tags, comments, processing instructions,
//! declarations, and CDATA. They are reused by the block-level HTML block
//! parser and the inline raw-HTML parser.

/// Returns true for a byte that may start an HTML tag name (ASCII letter).
#[inline]
const fn is_tag_name_start(b: u8) -> bool {
    b.is_ascii_alphabetic()
}

/// Returns true for a byte that may continue an HTML tag name.
#[inline]
const fn is_tag_name_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-'
}

/// Scan an open tag beginning at index 0 of `b` (the leading `<` included).
/// Returns the number of bytes consumed through the closing `>`, or `None`.
///
/// Grammar (`CommonMark` §6.6): `<` tagname (whitespace attribute)\*
/// whitespace? `/`? `>`.
#[must_use]
pub fn scan_open_tag(b: &[u8]) -> Option<usize> {
    if b.first() != Some(&b'<') {
        return None;
    }
    let mut i = 1;
    // Tag name.
    if !b.get(i).copied().is_some_and(is_tag_name_start) {
        return None;
    }
    i += 1;
    while b.get(i).copied().is_some_and(is_tag_name_char) {
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
            i = scan_attribute_value(b, j)?;
        }
        // else: bare attribute, continue (i already past name).
    }
}

/// Scan an attribute value at index `i`. Returns the index past the value.
fn scan_attribute_value(b: &[u8], i: usize) -> Option<usize> {
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
#[must_use]
pub fn scan_closing_tag(b: &[u8]) -> Option<usize> {
    if b.first() != Some(&b'<') || b.get(1) != Some(&b'/') {
        return None;
    }
    let mut i = 2;
    if !b.get(i).copied().is_some_and(is_tag_name_start) {
        return None;
    }
    i += 1;
    while b.get(i).copied().is_some_and(is_tag_name_char) {
        i += 1;
    }
    while b.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    (b.get(i) == Some(&b'>')).then_some(i + 1)
}

/// Scan any inline raw-HTML construct at index 0: open/closing tag, comment,
/// processing instruction, declaration, or CDATA. Returns bytes consumed.
#[must_use]
pub fn scan_inline_html(b: &[u8]) -> Option<usize> {
    if b.first() != Some(&b'<') {
        return None;
    }
    match b.get(1) {
        Some(b'!') => scan_declarationish(b),
        Some(b'?') => scan_until(b, 2, b"?>"),
        Some(b'/') => scan_closing_tag(b),
        _ => scan_open_tag(b),
    }
}

/// Scan `<!` constructs: comment, CDATA, or declaration.
fn scan_declarationish(b: &[u8]) -> Option<usize> {
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
        return scan_until(b, 4, b"-->");
    }
    if b.starts_with(b"<![CDATA[") {
        return scan_until(b, 9, b"]]>");
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
fn scan_until(b: &[u8], start: usize, close: &[u8]) -> Option<usize> {
    let mut i = start;
    while i + close.len() <= b.len() {
        if &b[i..i + close.len()] == close {
            return Some(i + close.len());
        }
        i += 1;
    }
    None
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

/// Determine whether `line` (leading indentation already stripped) starts an
/// HTML block, returning the block kind. `can_interrupt_paragraph` is true when
/// the previous line was not part of an open paragraph; type 7 only starts a
/// block when it can interrupt (i.e. not mid-paragraph).
#[must_use]
pub fn html_block_start(line: &[u8], in_paragraph: bool) -> Option<HtmlBlockKind> {
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
        && RAW_TEXT_TAGS.iter().any(|t| eq_ignore_case(name, t))
        && line
            .get(end)
            .is_none_or(|&c| c.is_ascii_whitespace() || c == b'>')
    {
        return Some(HtmlBlockKind::Type1);
    }

    // Type 6: known block tags followed by ws, end-of-line, `>`, or `/>`.
    if BLOCK_TAGS.iter().any(|t| eq_ignore_case(name, t)) {
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
            scan_closing_tag(line)?
        } else {
            // Type 7 excludes the raw-text tags handled by type 1.
            if RAW_TEXT_TAGS.iter().any(|t| eq_ignore_case(name, t)) {
                return None;
            }
            scan_open_tag(line)?
        };
        // Only whitespace may follow the tag on the line.
        if line[consumed..].iter().all(u8::is_ascii_whitespace) {
            return Some(HtmlBlockKind::Type7);
        }
    }
    None
}

/// Case-insensitive ASCII byte-slice comparison.
fn eq_ignore_case(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.eq_ignore_ascii_case(y))
}

/// True if `line` contains any of the type-1 closing markers.
#[must_use]
pub fn type1_end(line: &[u8]) -> bool {
    const CLOSERS: &[&[u8]] = &[b"</script>", b"</pre>", b"</style>", b"</textarea>"];
    CLOSERS.iter().any(|c| contains_ci(line, c))
}

/// Case-insensitive substring search.
fn contains_ci(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.len() > haystack.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|w| eq_ignore_case(w, needle))
}
