//! A minimal `CommonMark`-style HTML renderer for [`MarkdownFile`].
//!
//! This is intentionally the *simplest thing that can produce spec-shaped
//! output*: it walks the parsed [`Section`]/[`Inline`] tree and emits HTML
//! matching the structure the `CommonMark` reference renderer uses. It is the
//! comparison target for the conformance test suite in `tests/commonmark.rs`.

use std::fmt::Write as _;

use crate::{Inline, InlineSpan, MarkdownFile, Section};

/// Render a code span's content per `CommonMark` §6.1: first convert interior
/// line endings (`\n`, with any preceding `\r` already normalized away) to
/// single spaces, then — if the result contains at least one non-space — strip a
/// single leading and trailing space. Finally HTML-escape. Entities and
/// backslashes are *not* interpreted inside a code span.
fn escape_code_span(s: &str, out: &mut String) {
    // Step 1: collapse line endings to spaces into a scratch buffer.
    let mut buf = String::with_capacity(s.len());
    for ch in s.chars() {
        buf.push(if ch == '\n' { ' ' } else { ch });
    }

    // Step 2: strip one leading + trailing space, but only when the content is
    // not made up entirely of spaces (`` `  ` `` keeps both spaces).
    let trimmed = if buf.len() >= 2
        && buf.starts_with(' ')
        && buf.ends_with(' ')
        && buf.bytes().any(|b| b != b' ')
    {
        &buf[1..buf.len() - 1]
    } else {
        &buf[..]
    };

    escape_html(trimmed, out);
}

/// Escape the four HTML-significant characters in text content
/// (`CommonMark` renders these in body text).
fn escape_html(s: &str, out: &mut String) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
}

/// Like [`escape_html`], but first resolves numeric character references
/// (`CommonMark` §2.5). Entities are decoded *late*, at render time, because
/// they never affect document structure. Used only for `Inline::Text`; code
/// spans and code blocks keep entities verbatim.
fn escape_text(s: &str, out: &mut String) {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'&'
            && let Some((ch, consumed)) = crate::entity::decode_numeric(&bytes[i..])
        {
            // A decoded character is emitted as literal text, so it must still
            // be HTML-escaped (e.g. `&#34;` -> `"` -> `&quot;`).
            push_escaped_char(ch, out);
            i += consumed;
            continue;
        }
        match b {
            b'&' => out.push_str("&amp;"),
            b'<' => out.push_str("&lt;"),
            b'>' => out.push_str("&gt;"),
            b'"' => out.push_str("&quot;"),
            // ASCII fast path; multi-byte UTF-8 falls through to a char decode.
            _ if b < 0x80 => out.push(b as char),
            _ => {
                let ch = s[i..].chars().next().unwrap_or('\u{FFFD}');
                out.push(ch);
                i += ch.len_utf8();
                continue;
            }
        }
        i += 1;
    }
}

/// Push a single already-decoded character, HTML-escaping the four significant
/// metacharacters.
fn push_escaped_char(ch: char, out: &mut String) {
    match ch {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '"' => out.push_str("&quot;"),
        _ => out.push(ch),
    }
}

/// Per-byte URL safety table mirroring the `CommonMark` reference renderer
/// (`houdini_escape_href` / JavaScript `encodeURI`). `true` means the byte is
/// emitted verbatim; `false` means it is percent-encoded as `%XX`. All bytes
/// ≥ 0x80 are encoded (their UTF-8 representation). `&` is handled specially
/// (→ `&amp;`) before this table is consulted.
static HREF_SAFE: [bool; 256] = {
    let mut t = [false; 256];
    let mut b = b'!';
    while b <= b'~' {
        t[b as usize] = true;
        b += 1;
    }
    // Carve out the unsafe ASCII punctuation that `encodeURI` percent-encodes.
    let unsafe_bytes = [
        b'"', b'<', b'>', b'[', b'\\', b']', b'^', b'`', b'{', b'|', b'}',
    ];
    let mut i = 0;
    while i < unsafe_bytes.len() {
        t[unsafe_bytes[i] as usize] = false;
        i += 1;
    }
    t
};

/// Percent-encode a single byte as `%XX` (uppercase hex), or emit it verbatim
/// when [`HREF_SAFE`] permits.
fn push_href_byte(b: u8, out: &mut String) {
    if HREF_SAFE[b as usize] {
        out.push(b as char);
    } else {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        out.push('%');
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
}

/// Encode one already-decoded character into an `href`: `&` becomes `&amp;`,
/// everything else is percent-encoded byte-by-byte (safe bytes pass through).
fn push_href_char(ch: char, out: &mut String) {
    if ch == '&' {
        out.push_str("&amp;");
        return;
    }
    let mut buf = [0u8; 4];
    for &b in ch.encode_utf8(&mut buf).as_bytes() {
        push_href_byte(b, out);
    }
}

/// Escape a link/image **destination** for an `href`/`src` attribute, matching
/// the `CommonMark` reference renderer: resolve backslash escapes, decode
/// numeric character references, preserve existing `%XX` sequences, then
/// percent-encode unsafe bytes. `&` becomes `&amp;` (or a decoded entity).
fn escape_href(s: &str, out: &mut String) {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' && bytes.get(i + 1).is_some_and(u8::is_ascii_punctuation) {
            push_href_char(bytes[i + 1] as char, out);
            i += 2;
            continue;
        }
        if b == b'&' {
            if let Some((ch, consumed)) = crate::entity::decode_numeric(&bytes[i..]) {
                push_href_char(ch, out);
                i += consumed;
            } else {
                // Named entities need the full HTML5 table (deferred); emit the
                // literal ampersand HTML-escaped.
                out.push_str("&amp;");
                i += 1;
            }
            continue;
        }
        push_href_byte(b, out);
        i += 1;
    }
}

/// Escape an **autolink** target. Like [`escape_href`] but backslash escapes
/// are *not* resolved (autolink content is literal per `CommonMark` §6.5), so a
/// `\` is percent-encoded like any other unsafe byte.
fn escape_href_autolink(s: &str, out: &mut String) {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'&' {
            out.push_str("&amp;");
            i += 1;
            continue;
        }
        push_href_byte(b, out);
        i += 1;
    }
}

/// Escape a link/image **title**: resolve backslash escapes and numeric
/// character references, then HTML-escape the result.
fn escape_link_title(s: &str, out: &mut String) {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' && bytes.get(i + 1).is_some_and(u8::is_ascii_punctuation) {
            push_escaped_char(bytes[i + 1] as char, out);
            i += 2;
            continue;
        }
        if b == b'&'
            && let Some((ch, consumed)) = crate::entity::decode_numeric(&bytes[i..])
        {
            push_escaped_char(ch, out);
            i += consumed;
            continue;
        }
        match b {
            b'&' => out.push_str("&amp;"),
            b'<' => out.push_str("&lt;"),
            b'>' => out.push_str("&gt;"),
            b'"' => out.push_str("&quot;"),
            _ if b < 0x80 => out.push(b as char),
            _ => {
                let ch = s[i..].chars().next().unwrap_or('\u{FFFD}');
                out.push(ch);
                i += ch.len_utf8();
                continue;
            }
        }
        i += 1;
    }
}

/// Emit a newline only if `out` is non-empty and doesn't already end with one.
/// Mirrors the `CommonMark` reference renderer's `cr()`, which keeps block
/// elements on their own lines without doubling up newlines.
fn cr(out: &mut String) {
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
}

/// Strip the four-space (or single-tab) indentation prefix from one line of an
/// indented code block (`CommonMark` §4.4).
fn strip_code_indent(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut col = 0;
    let mut i = 0;
    while i < bytes.len() && col < 4 {
        match bytes[i] {
            b' ' => col += 1,
            b'\t' => col += 4 - (col % 4),
            _ => break,
        }
        i += 1;
    }
    &line[i..]
}

impl<const MAX_INLINE_DEPTH: u8, const INLINE_STACK_CAP: usize>
    MarkdownFile<'_, MAX_INLINE_DEPTH, INLINE_STACK_CAP>
{
    /// Render the document to an HTML string.
    ///
    /// The output mirrors the structure produced by the `CommonMark` reference
    /// implementation (block elements each followed by a newline) so it can be
    /// compared directly against the spec test vectors.
    #[must_use]
    pub fn to_html(&self) -> String {
        let mut out = String::new();
        for section in &self.sections {
            self.render_section(section, &mut out);
        }
        out
    }

    fn render_section(&self, section: &Section<'_>, out: &mut String) {
        match section {
            Section::Heading { level, content } => {
                let _ = write!(out, "<h{level}>");
                self.render_inlines(*content, out);
                let _ = writeln!(out, "</h{level}>");
            }
            Section::Paragraph { content } => {
                out.push_str("<p>");
                self.render_inlines(*content, out);
                out.push_str("</p>\n");
            }
            Section::CodeBlock { language, code } => {
                out.push_str("<pre><code");
                if let Some(lang) = language {
                    // CommonMark uses the first word of the info string as the
                    // language class.
                    let first = lang.split_whitespace().next().unwrap_or("");
                    if !first.is_empty() {
                        out.push_str(" class=\"language-");
                        escape_html(first, out);
                        out.push('"');
                    }
                }
                out.push('>');
                escape_html(code, out);
                // CommonMark code blocks always end their final line with a
                // single newline. The parser strips the newline before a
                // closing fence but keeps it for an unclosed block, so only
                // add one when it isn't already present.
                if !code.is_empty() && !code.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str("</code></pre>\n");
            }
            Section::CodeLines { language, lines } => {
                out.push_str("<pre><code");
                if let Some(lang) = language {
                    let first = lang.split_whitespace().next().unwrap_or("");
                    if !first.is_empty() {
                        out.push_str(" class=\"language-");
                        escape_html(first, out);
                        out.push('"');
                    }
                }
                out.push('>');
                for line in self.code_lines(*lines) {
                    escape_html(line, out);
                    out.push('\n');
                }
                out.push_str("</code></pre>\n");
            }
            Section::IndentedCode { code } => {
                out.push_str("<pre><code>");
                // Strip up to four leading spaces (or one leading tab) from
                // each line; the span is verbatim source otherwise.
                let mut first = true;
                for line in code.split('\n') {
                    if !first {
                        out.push('\n');
                    }
                    first = false;
                    escape_html(strip_code_indent(line), out);
                }
                out.push_str("\n</code></pre>\n");
            }
            Section::UnorderedList { tight, items } => {
                out.push_str("<ul>\n");
                for item in self.child_sections(*items) {
                    self.render_list_item(item, *tight, out);
                }
                out.push_str("</ul>\n");
            }
            Section::OrderedList {
                start,
                tight,
                items,
                ..
            } => {
                if *start == 1 {
                    out.push_str("<ol>\n");
                } else {
                    let _ = writeln!(out, "<ol start=\"{start}\">");
                }
                for item in self.child_sections(*items) {
                    self.render_list_item(item, *tight, out);
                }
                out.push_str("</ol>\n");
            }
            Section::ListItem { children } => {
                // A bare ListItem (not reached via a list) renders loose.
                self.render_list_item(
                    &Section::ListItem {
                        children: *children,
                    },
                    false,
                    out,
                );
            }
            Section::Blockquote { children } => {
                out.push_str("<blockquote>\n");
                for child in self.child_sections(*children) {
                    self.render_section(child, out);
                }
                out.push_str("</blockquote>\n");
            }
            Section::HtmlBlock { html } => {
                // Emitted verbatim, with the trailing newline CommonMark adds.
                out.push_str(html);
                out.push('\n');
            }
            Section::HorizontalRule => out.push_str("<hr />\n"),
        }
    }

    /// Render one `<li>` element following `CommonMark`'s reference model: a
    /// tight list's paragraph children render their inlines bare (no `<p>`),
    /// while every other block is preceded by a soft line break (emitted only
    /// when the output isn't already at the start of a line).
    fn render_list_item(&self, item: &Section<'_>, tight: bool, out: &mut String) {
        let Section::ListItem { children } = item else {
            self.render_section(item, out);
            return;
        };
        let kids = self.child_sections(*children);
        out.push_str("<li>");
        for kid in kids {
            if tight && let Section::Paragraph { content } = kid {
                // Tight paragraph: bare inlines, no wrapper, no leading break.
                self.render_inlines(*content, out);
            } else {
                cr(out);
                self.render_section(kid, out);
            }
        }
        out.push_str("</li>\n");
    }

    fn render_inlines(&self, span: InlineSpan, out: &mut String) {
        for inline in self.inlines(span) {
            self.render_inline(inline, out);
        }
    }

    fn render_inline(&self, inline: &Inline<'_>, out: &mut String) {
        match inline {
            Inline::Text(t) => escape_text(t, out),
            Inline::Bold(span) => {
                out.push_str("<strong>");
                self.render_inlines(*span, out);
                out.push_str("</strong>");
            }
            Inline::Italic(span) => {
                out.push_str("<em>");
                self.render_inlines(*span, out);
                out.push_str("</em>");
            }
            Inline::Code(c) => {
                out.push_str("<code>");
                escape_code_span(c, out);
                out.push_str("</code>");
            }
            Inline::Autolink { target, is_email } => {
                out.push_str("<a href=\"");
                if *is_email {
                    out.push_str("mailto:");
                }
                escape_href_autolink(target, out);
                out.push_str("\">");
                escape_html(target, out);
                out.push_str("</a>");
            }
            Inline::RawHtml(html) => out.push_str(html),
            Inline::Link { text, url, title } => {
                out.push_str("<a href=\"");
                escape_href(url, out);
                out.push('"');
                if let Some(title) = title {
                    out.push_str(" title=\"");
                    escape_link_title(title, out);
                    out.push('"');
                }
                out.push('>');
                self.render_inlines(*text, out);
                out.push_str("</a>");
            }
            Inline::Image { alt, url, title } => {
                out.push_str("<img src=\"");
                escape_href(url, out);
                out.push_str("\" alt=\"");
                escape_html(alt, out);
                out.push('"');
                if let Some(title) = title {
                    out.push_str(" title=\"");
                    escape_link_title(title, out);
                    out.push('"');
                }
                out.push_str(" />");
            }
            Inline::SoftBreak => out.push('\n'),
            Inline::HardBreak => out.push_str("<br />\n"),
        }
    }
}
