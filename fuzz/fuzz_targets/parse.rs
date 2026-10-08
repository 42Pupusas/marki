#![no_main]

use libfuzzer_sys::fuzz_target;

use marki::{MarkdownFile, Section};

/// Walks every section, dereferencing each inline span and child-section range
/// to exercise index operations and catch panics.
struct Walker<'md, 'src> {
    md: &'md MarkdownFile<'src>,
}

impl Walker<'_, '_> {
    fn walk<'s>(&self, sections: impl Iterator<Item = &'s Section<'s>>)
    where
        Self: 's,
    {
        for section in sections {
            match section {
                Section::UnorderedList { items, .. } | Section::OrderedList { items, .. } => {
                    self.walk(self.md.child_sections(*items));
                }
                Section::ListItem { children, .. }
                | Section::Blockquote { children }
                | Section::Alert { children, .. }
                | Section::FootnoteDefinition { children, .. } => {
                    self.walk(self.md.child_sections(*children));
                }
                Section::Table { rows, .. } => self.walk(self.md.child_sections(*rows)),
                Section::TableRow { cells, .. } => self.walk(self.md.child_sections(*cells)),
                Section::Heading { content, .. }
                | Section::Paragraph { content }
                | Section::TableCell { content, .. } => {
                    let _ = self.md.inlines(*content);
                }
                Section::CodeLines { lines, .. } | Section::HtmlLines { lines } => {
                    let _ = self.md.code_lines(*lines);
                }
                Section::CodeBlock { .. }
                | Section::IndentedCode { .. }
                | Section::HtmlBlock { .. }
                | Section::HorizontalRule => {}
            }
        }
    }
}

fuzz_target!(|data: &str| {
    let md: MarkdownFile<'_> = MarkdownFile::parse(data);
    Walker { md: &md }.walk(md.sections.iter());
    let _ = md.to_html();
});
