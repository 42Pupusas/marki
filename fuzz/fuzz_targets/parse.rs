#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &str| {
    let md = marki::MarkdownFile::parse(data);
    // Walk all sections to exercise index operations and catch panics.
    for section in &md.sections {
        match section {
            marki::Section::UnorderedList { items } => {
                for &span in md.item_spans(*items) {
                    let _ = md.inlines(span);
                }
            }
            marki::Section::OrderedList { items, .. } => {
                for &span in md.item_spans(*items) {
                    let _ = md.inlines(span);
                }
            }
            marki::Section::Heading { content, .. }
            | marki::Section::Paragraph { content }
            | marki::Section::Blockquote { content } => {
                let _ = md.inlines(*content);
            }
            marki::Section::CodeBlock { .. }
            | marki::Section::HorizontalRule => {}
        }
    }
});
