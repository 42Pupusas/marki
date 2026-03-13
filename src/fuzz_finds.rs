/// Regression tests for inputs discovered by fuzzing.
///
/// Each test simply parses the input and walks all sections/inlines,
/// asserting that the parser does not panic or crash.

fn parse_and_walk(input: &str) {
    let md: crate::MarkdownFile<'_> = crate::MarkdownFile::parse(input);
    for section in &md.sections {
        match section {
            crate::Section::UnorderedList { items } => {
                for &span in md.item_spans(*items) {
                    let _ = md.inlines(span);
                }
            }
            crate::Section::OrderedList { items, .. } => {
                for &span in md.item_spans(*items) {
                    let _ = md.inlines(span);
                }
            }
            crate::Section::Heading { content, .. }
            | crate::Section::Paragraph { content }
            | crate::Section::Blockquote { content } => {
                let _ = md.inlines(*content);
            }
            crate::Section::CodeBlock { .. } | crate::Section::HorizontalRule => {}
        }
    }
}

/// crash-eb78fafcb151602b035bad4c8b369aa8144f2608
/// Deeply nested `*` emphasis causing unbounded recursion (stack overflow).
#[test]
fn fuzz_deep_emphasis_recursion() {
    parse_and_walk("\n\n\n****************************************************\\*************b*****************************************************\\*************b*****************\\********************b*****************\\*********\\{***\\{***\\{***********\\********************b*****************\\*********\\{***\\{***\\{***\\{*********\\");
}
