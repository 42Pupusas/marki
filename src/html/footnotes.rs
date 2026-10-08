//! HTML for GFM footnotes: the `<sup>` reference marks and the footnote list
//! rendered after the document body, in the markup cmark-gfm produces.

use std::fmt::{self, Write as _};

use super::HtmlSink;
use crate::{MarkdownFile, Section};

/// Writes `-n` for the second and later references to a footnote, and nothing
/// for the first.
struct RefSuffix(u32);

impl fmt::Display for RefSuffix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 > 1 {
            write!(f, "-{}", self.0)
        } else {
            Ok(())
        }
    }
}

impl<const MAX_INLINE_DEPTH: u8, const INLINE_STACK_CAP: usize, const MAX_BLOCK_DEPTH: u16>
    MarkdownFile<'_, MAX_INLINE_DEPTH, INLINE_STACK_CAP, MAX_BLOCK_DEPTH>
{
    pub(super) fn render_footnote_ref(label: &str, number: u32, occurrence: u32, out: &mut String) {
        out.push_str("<sup class=\"footnote-ref\"><a href=\"#fn-");
        out.escape_footnote_id(label);
        out.push_str("\" id=\"fnref-");
        out.escape_footnote_id(label);
        let _ = write!(
            out,
            "{}\" data-footnote-ref>{number}</a></sup>",
            RefSuffix(occurrence)
        );
    }

    /// Render every referenced footnote definition, ordered by footnote
    /// number, as the closing `<section class="footnotes">`.
    pub(super) fn render_footnotes(&self, out: &mut String) {
        let mut notes: Vec<&Section<'_>> = self
            .sections
            .iter()
            .filter(|s| matches!(s, Section::FootnoteDefinition { number, .. } if *number > 0))
            .collect();
        if notes.is_empty() {
            return;
        }
        notes.sort_by_key(|s| match s {
            Section::FootnoteDefinition { number, .. } => *number,
            _ => 0,
        });
        out.push_str("<section class=\"footnotes\" data-footnotes>\n<ol>\n");
        for note in notes {
            self.render_footnote(note, out);
        }
        out.push_str("</ol>\n</section>\n");
    }

    /// One `<li>` of the footnote list. The back-references go inside the last
    /// paragraph when the definition ends with one, otherwise on their own
    /// line after the content.
    fn render_footnote(&self, note: &Section<'_>, out: &mut String) {
        let &Section::FootnoteDefinition {
            label,
            number,
            references,
            children,
        } = note
        else {
            return;
        };
        out.push_str("<li id=\"fn-");
        out.escape_footnote_id(label);
        out.push_str("\">");
        let mut kids = self.child_sections(children).peekable();
        let mut backrefs_written = false;
        while let Some(kid) = kids.next() {
            out.push_cr();
            if kids.peek().is_none()
                && let Section::Paragraph { content } = kid
            {
                out.push_str("<p>");
                self.render_inlines(*content, out);
                out.push(' ');
                Self::render_backrefs(label, number, references, out);
                out.push_str("</p>\n");
                backrefs_written = true;
            } else {
                self.render_section(kid, out);
            }
        }
        if !backrefs_written {
            out.push_cr();
            Self::render_backrefs(label, number, references, out);
            out.push('\n');
        }
        out.push_str("</li>\n");
    }

    fn render_backrefs(label: &str, number: u32, references: u32, out: &mut String) {
        for occurrence in 1..=references {
            let suffix = RefSuffix(occurrence);
            if occurrence > 1 {
                out.push(' ');
            }
            out.push_str("<a href=\"#fnref-");
            out.escape_footnote_id(label);
            let _ = write!(
                out,
                "{suffix}\" class=\"footnote-backref\" data-footnote-backref \
                 data-footnote-backref-idx=\"{number}{suffix}\" \
                 aria-label=\"Back to reference {number}{suffix}\">\u{21a9}"
            );
            if occurrence > 1 {
                let _ = write!(out, "<sup class=\"footnote-ref\">{occurrence}</sup>");
            }
            out.push_str("</a>");
        }
    }
}
