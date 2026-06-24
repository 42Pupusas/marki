#![no_main]

use libfuzzer_sys::fuzz_target;

use marki::{MarkdownFile, Section};

/// Recursively walk sections, dereferencing every inline span and child-section
/// range to exercise index operations and catch panics.
fn walk(md: &MarkdownFile<'_>, sections: &[Section<'_>]) {
    for section in sections {
        match section {
            Section::UnorderedList { items, .. } | Section::OrderedList { items, .. } => {
                walk(md, md.child_sections(*items));
            }
            Section::ListItem { children } | Section::Blockquote { children } => {
                walk(md, md.child_sections(*children));
            }
            Section::Heading { content, .. } | Section::Paragraph { content } => {
                let _ = md.inlines(*content);
            }
            Section::CodeBlock { .. }
            | Section::IndentedCode { .. }
            | Section::HtmlBlock { .. }
            | Section::HorizontalRule => {}
        }
    }
}

fuzz_target!(|data: &str| {
    let md: MarkdownFile<'_> = MarkdownFile::parse(data);
    walk(&md, &md.sections);
});
