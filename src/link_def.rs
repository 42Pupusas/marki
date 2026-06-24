//! Link reference definitions (`CommonMark` §4.7).
//!
//! A definition has the form
//!
//! ```text
//! [label]: destination "optional title"
//! ```
//!
//! and may span up to three lines (label/destination on one line, the title on
//! the next). Definitions produce no output; instead they populate a registry
//! that the inline parser consults to resolve reference links and images
//! (`[text][label]`, `[label][]`, `[label]`).

use std::collections::HashMap;

/// Registry mapping a normalized link label to its `(url, title)` pair.
pub type LinkDefs<'src> = HashMap<String, (&'src str, Option<&'src str>)>;

/// A parsed link reference definition, borrowing label/url/title from the
/// source. The label is still raw here; it is normalized before insertion.
pub struct LinkDef<'src> {
    pub label: &'src str,
    pub url: &'src str,
    pub title: Option<&'src str>,
}

/// Normalize a link label for matching (`CommonMark` §4.7): trim surrounding
/// whitespace, collapse internal whitespace runs to a single space, and apply
/// Unicode case folding (approximated by `to_lowercase`).
#[must_use]
pub fn normalize_label(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    let mut prev_ws = false;
    for ch in label.trim().chars() {
        if ch.is_whitespace() {
            if !prev_ws {
                out.push(' ');
                prev_ws = true;
            }
        } else {
            prev_ws = false;
            for lc in ch.to_lowercase() {
                out.push(lc);
            }
        }
    }
    out
}

#[inline]
fn is_escape(bytes: &[u8], i: usize) -> bool {
    bytes.get(i) == Some(&b'\\') && bytes.get(i + 1).is_some_and(u8::is_ascii_punctuation)
}

/// Skip spaces/tabs, then optionally a single line ending followed by more
/// spaces/tabs. Returns `None` if a blank line is encountered (two line
/// endings), which terminates a definition.
fn skip_ws_one_nl(bytes: &[u8], mut i: usize) -> Option<usize> {
    while matches!(bytes.get(i), Some(b' ' | b'\t')) {
        i += 1;
    }
    if bytes.get(i) == Some(&b'\n') {
        i += 1;
        while matches!(bytes.get(i), Some(b' ' | b'\t')) {
            i += 1;
        }
        if bytes.get(i) == Some(&b'\n') {
            return None;
        }
    }
    Some(i)
}

/// If the remainder of the current line (from `i`) is only whitespace, return
/// the offset at which the next line begins (or the input length at EOF).
/// Otherwise return `None` — the line has trailing non-whitespace content.
fn line_end_resume(bytes: &[u8], mut i: usize) -> Option<usize> {
    while matches!(bytes.get(i), Some(b' ' | b'\t')) {
        i += 1;
    }
    match bytes.get(i) {
        None => Some(bytes.len()),
        Some(&b'\n') => Some(i + 1),
        _ => None,
    }
}

/// Scan a link destination at `i`. Returns `(url_start, url_end, after)`.
/// Supports the `<...>` bracketed form and the bare form (terminated by
/// whitespace or a control character, with balanced parentheses).
fn scan_destination(bytes: &[u8], i: usize) -> Option<(usize, usize, usize)> {
    if bytes.get(i) == Some(&b'<') {
        let mut j = i + 1;
        loop {
            match bytes.get(j) {
                None | Some(b'\n' | b'<') => return None,
                _ if is_escape(bytes, j) => j += 2,
                Some(b'>') => return Some((i + 1, j, j + 1)),
                _ => j += 1,
            }
        }
    } else {
        let start = i;
        let mut j = i;
        let mut depth: i32 = 0;
        loop {
            match bytes.get(j) {
                None => break,
                Some(&b) if b.is_ascii_whitespace() => break,
                _ if is_escape(bytes, j) => {
                    j += 2;
                    continue;
                }
                Some(b'(') => depth += 1,
                Some(b')') => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                Some(&b) if b < 0x20 => break,
                _ => {}
            }
            j += 1;
        }
        if j == start || depth != 0 {
            return None;
        }
        Some((start, j, j))
    }
}

/// Scan a title at `i` delimited by `"`, `'`, or `(...)`.
/// Returns `(title_start, title_end, after)`.
fn scan_title(bytes: &[u8], i: usize) -> Option<(usize, usize, usize)> {
    let close = match bytes.get(i) {
        Some(b'"') => b'"',
        Some(b'\'') => b'\'',
        Some(b'(') => b')',
        _ => return None,
    };
    let mut j = i + 1;
    loop {
        match bytes.get(j) {
            None => return None,
            _ if is_escape(bytes, j) => j += 2,
            Some(&b) if b == close => return Some((i + 1, j, j + 1)),
            Some(b'\n') => {
                if bytes.get(j + 1) == Some(&b'\n') {
                    return None; // blank line inside a title is not allowed
                }
                j += 1;
            }
            _ => j += 1,
        }
    }
}

/// After a destination at `i`, try to parse a title that is separated from the
/// destination by whitespace (possibly crossing one line ending).
fn title_after(bytes: &[u8], i: usize) -> Option<(usize, usize, usize)> {
    let after_ws = skip_ws_one_nl(bytes, i)?;
    if after_ws == i {
        return None; // a title must be separated from the destination
    }
    scan_title(bytes, after_ws)
}

/// Try to parse a link reference definition beginning at `start` (the `[`),
/// with leading block indentation already stripped by the caller. Returns the
/// definition and the byte offset at which parsing should resume.
#[must_use]
pub fn scan_link_def(input: &str, start: usize) -> Option<(LinkDef<'_>, usize)> {
    let bytes = input.as_bytes();
    if bytes.get(start) != Some(&b'[') {
        return None;
    }
    let label_start = start + 1;
    let mut i = label_start;
    let label_end = loop {
        match bytes.get(i) {
            None => return None,
            _ if is_escape(bytes, i) => i += 2,
            Some(b'[') => return None,
            Some(b']') => break i,
            Some(b'\n') => {
                if bytes.get(i + 1) == Some(&b'\n') {
                    return None;
                }
                i += 1;
            }
            _ => i += 1,
        }
    };
    let label = input.get(label_start..label_end)?;
    if label.len() > 999 || label.trim().is_empty() {
        return None;
    }
    i = label_end + 1;
    if bytes.get(i) != Some(&b':') {
        return None;
    }
    i += 1;
    i = skip_ws_one_nl(bytes, i)?;

    let (url_start, url_end, after_dest) = scan_destination(bytes, i)?;
    let url = input.get(url_start..url_end)?;

    // A definition with no title requires the destination line to end cleanly.
    let no_title_resume = line_end_resume(bytes, after_dest);

    if let Some((t_start, t_end, t_after)) = title_after(bytes, after_dest)
        && let Some(resume) = line_end_resume(bytes, t_after)
    {
        return Some((
            LinkDef {
                label,
                url,
                title: input.get(t_start..t_end),
            },
            resume,
        ));
    }

    let resume = no_title_resume?;
    Some((
        LinkDef {
            label,
            url,
            title: None,
        },
        resume,
    ))
}
