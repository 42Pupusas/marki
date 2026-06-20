//! Regression tests for inputs discovered by fuzzing.
//!
//! Each test parses the input and calls [`crate::MarkdownFile::walk_all_inlines`]
//! to assert the parser does not panic or crash.

/// crash-eb78fafcb151602b035bad4c8b369aa8144f2608
/// Deeply nested `*` emphasis causing unbounded recursion (stack overflow).
#[test]
fn fuzz_deep_emphasis_recursion() {
    let md: crate::MarkdownFile<'_> = crate::MarkdownFile::parse(
        "\n\n\n****************************************************\\*************b*****************************************************\\*************b*****************\\********************b*****************\\*********\\{***\\{***\\{***********\\********************b*****************\\*********\\{***\\{***\\{***\\{*********\\",
    );
    md.walk_all_inlines();
}
