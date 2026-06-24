//! A minimal `CommonMark`-style HTML renderer for [`MarkdownFile`].
//!
//! This is intentionally the *simplest thing that can produce spec-shaped
//! output*: it walks the parsed [`Section`]/[`Inline`] tree and emits HTML
//! matching the structure the `CommonMark` reference renderer uses. It is the
//! comparison target for the conformance test suite in `tests/commonmark.rs`.

use std::fmt::Write as _;

use crate::{Inline, InlineSpan, MarkdownFile, Section};

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

/// Escape a URL for use in an `href`/`src` attribute. The reference renderer
/// percent-encodes a handful of bytes; for our simple corpus only the HTML
/// metacharacters matter.
fn escape_href(s: &str, out: &mut String) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
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
            Section::UnorderedList { items } => {
                out.push_str("<ul>\n");
                for &span in self.item_spans(*items) {
                    out.push_str("<li>");
                    self.render_inlines(span, out);
                    out.push_str("</li>\n");
                }
                out.push_str("</ul>\n");
            }
            Section::OrderedList {
                start, items, ..
            } => {
                if *start == 1 {
                    out.push_str("<ol>\n");
                } else {
                    let _ = writeln!(out, "<ol start=\"{start}\">");
                }
                for &span in self.item_spans(*items) {
                    out.push_str("<li>");
                    self.render_inlines(span, out);
                    out.push_str("</li>\n");
                }
                out.push_str("</ol>\n");
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
                escape_html(c, out);
                out.push_str("</code>");
            }
            Inline::Autolink { target, is_email } => {
                out.push_str("<a href=\"");
                if *is_email {
                    out.push_str("mailto:");
                }
                escape_href(target, out);
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
                    escape_html(title, out);
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
                    escape_html(title, out);
                    out.push('"');
                }
                out.push_str(" />");
            }
            Inline::SoftBreak => out.push('\n'),
            Inline::HardBreak => out.push_str("<br />\n"),
        }
    }
}
