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
pub(crate) fn scan_open_tag(b: &[u8]) -> Option<usize> {
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
        while b.get(i).is_some_and(|c| c.is_ascii_whitespace()) {
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
        while b.get(j).is_some_and(|c| c.is_ascii_whitespace()) {
            j += 1;
        }
        if b.get(j) == Some(&b'=') {
            j += 1;
            while b.get(j).is_some_and(|c| c.is_ascii_whitespace()) {
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
                !c.is_ascii_whitespace()
                    && !matches!(c, b'"' | b'\'' | b'=' | b'<' | b'>' | b'`')
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
pub(crate) fn scan_closing_tag(b: &[u8]) -> Option<usize> {
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
    while b.get(i).is_some_and(|c| c.is_ascii_whitespace()) {
        i += 1;
    }
    (b.get(i) == Some(&b'>')).then_some(i + 1)
}

/// Scan any inline raw-HTML construct at index 0: open/closing tag, comment,
/// processing instruction, declaration, or CDATA. Returns bytes consumed.
#[must_use]
pub(crate) fn scan_inline_html(b: &[u8]) -> Option<usize> {
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
        // Comment: text must not start with `>` or `->`, but CommonMark's
        // relaxed inline rule just scans to `-->`.
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
