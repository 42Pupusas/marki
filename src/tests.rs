use std::borrow::Cow;

use crate::section::{OrderedListDelimiter, SectionRange};
use crate::{Inline, InlineSpan, MarkdownFile, Section};

/// Assertion helper that carries a reference to the parsed document.
struct Checker<'md, 'src> {
    md: &'md MarkdownFile<'src>,
}

impl<'md, 'src> Checker<'md, 'src> {
    fn new(md: &'md MarkdownFile<'src>) -> Self {
        Self { md }
    }

    /// Assert that a blockquote contains exactly one paragraph child, and
    /// return that paragraph's inline span for further checking.
    fn quote_single_paragraph(&self, section: &Section<'src>) -> InlineSpan {
        match section {
            Section::Blockquote { children } => {
                let kids: Vec<_> = self.md.child_sections(*children).collect();
                assert_eq!(
                    kids.len(),
                    1,
                    "expected blockquote with one child, got {kids:?}"
                );
                match kids[0] {
                    Section::Paragraph { content } => *content,
                    other => panic!("expected paragraph in blockquote, got {other:?}"),
                }
            }
            other => panic!("expected blockquote, got {other:?}"),
        }
    }

    /// Collect the inline span of each list item's single paragraph child.
    /// Most list tests assume tight, single-paragraph items, so this flattens
    /// the `ListItem { Paragraph }` nesting back to one span per item.
    fn item_paragraphs(&self, items: SectionRange) -> Vec<InlineSpan> {
        self.md
            .child_sections(items)
            .map(|item| match item {
                Section::ListItem { children } => {
                    let kids: Vec<_> = self.md.child_sections(*children).collect();
                    assert_eq!(
                        kids.len(),
                        1,
                        "expected single-paragraph list item, got {kids:?}"
                    );
                    match kids[0] {
                        Section::Paragraph { content } => *content,
                        other => {
                            panic!("expected paragraph in list item, got {other:?}")
                        }
                    }
                }
                other => panic!("expected list item, got {other:?}"),
            })
            .collect()
    }

    /// Assert a span contains exactly `expected` inline elements.
    fn check_span(&self, span: InlineSpan, expected: &[Expect]) {
        let actual = self.md.inlines(span);
        assert_eq!(
            actual.len(),
            expected.len(),
            "span length mismatch: got {actual:?}, expected {expected:?}"
        );
        for (i, (a, e)) in actual.iter().zip(expected).enumerate() {
            self.check_inline(a, e, i);
        }
    }

    fn check_inline(&self, actual: &Inline, expected: &Expect, idx: usize) {
        match (actual, expected) {
            (Inline::Text(a), Expect::Text(e)) => {
                assert_eq!(a, e, "Text mismatch at index {idx}");
            }
            (Inline::Bold(span), Expect::Bold(children))
            | (Inline::Italic(span), Expect::Italic(children)) => {
                self.check_span(*span, children);
            }
            (Inline::Code(a), Expect::Code(e)) => {
                assert_eq!(a, e, "Code mismatch at index {idx}");
            }
            (Inline::SoftBreak, Expect::SoftBreak) | (Inline::HardBreak, Expect::HardBreak) => {}
            (
                Inline::Link { text, url, title },
                Expect::Link {
                    text: et,
                    url: eu,
                    title: eti,
                },
            ) => {
                assert_eq!(url, eu, "Link url mismatch at index {idx}");
                assert_eq!(title, eti, "Link title mismatch at index {idx}");
                self.check_span(*text, et);
            }
            (
                Inline::Image { alt, url, title },
                Expect::Image {
                    alt: ea,
                    url: eu,
                    title: eti,
                },
            ) => {
                assert_eq!(url, eu, "Image url mismatch at index {idx}");
                assert_eq!(title, eti, "Image title mismatch at index {idx}");
                self.check_span(*alt, ea);
            }
            (
                Inline::Autolink { target, is_email },
                Expect::Autolink {
                    target: et,
                    is_email: ee,
                },
            ) => {
                assert_eq!(target, et, "Autolink target mismatch at index {idx}");
                assert_eq!(is_email, ee, "Autolink is_email mismatch at index {idx}");
            }
            (Inline::RawHtml(a), Expect::RawHtml(e)) => {
                assert_eq!(a, e, "RawHtml mismatch at index {idx}");
            }
            _ => panic!("Inline mismatch at index {idx}: got {actual:?}, expected {expected:?}"),
        }
    }
}

/// Expected inline element for test assertions.
#[derive(Debug)]
#[allow(dead_code)]
enum Expect<'a> {
    Text(&'a str),
    Bold(Vec<Self>),
    Italic(Vec<Self>),
    Code(&'a str),
    SoftBreak,
    HardBreak,
    Link {
        text: Vec<Self>,
        url: &'a str,
        title: Option<&'a str>,
    },
    Image {
        alt: Vec<Self>,
        url: &'a str,
        title: Option<&'a str>,
    },
    Autolink {
        target: &'a str,
        is_email: bool,
    },
    RawHtml(&'a str),
}

impl<'a> Expect<'a> {
    /// Shorthand: a span containing a single [`Expect::Text`] node.
    fn text(s: &'a str) -> Vec<Self> {
        vec![Self::Text(s)]
    }
}

#[test]
fn test_heading() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("# Hello\n## World");
    assert_eq!(md.sections.len(), 2);
    match &md.sections[0] {
        Section::Heading { level: 1, content } => {
            Checker::new(&md).check_span(*content, &Expect::text("Hello"));
        }
        other => panic!("expected heading, got {other:?}"),
    }
    match &md.sections[1] {
        Section::Heading { level: 2, content } => {
            Checker::new(&md).check_span(*content, &Expect::text("World"));
        }
        other => panic!("expected heading, got {other:?}"),
    }
}

#[test]
fn test_paragraph() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("This is a paragraph.\nWith two lines.");
    assert_eq!(md.sections.len(), 1);
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[
                Expect::Text("This is a paragraph."),
                Expect::SoftBreak,
                Expect::Text("With two lines."),
            ],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_code_block() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("```rust\nfn main() {}\n```");
    assert_eq!(md.sections.len(), 1);
    assert_eq!(
        md.sections[0],
        Section::CodeBlock {
            language: Some("rust"),
            code: "fn main() {}",
        }
    );
}

#[test]
fn test_code_block_no_language() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("```\nhello\n```");
    assert_eq!(
        md.sections[0],
        Section::CodeBlock {
            language: None,
            code: "hello",
        }
    );
}

#[test]
fn test_unordered_list() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("- one\n- two\n- three");
    assert_eq!(md.sections.len(), 1);
    match &md.sections[0] {
        Section::UnorderedList { items, .. } => {
            let checker = Checker::new(&md);
            let items = checker.item_paragraphs(*items);
            assert_eq!(items.len(), 3);
            checker.check_span(items[0], &Expect::text("one"));
            checker.check_span(items[1], &Expect::text("two"));
            checker.check_span(items[2], &Expect::text("three"));
        }
        other => panic!("expected list, got {other:?}"),
    }
}

#[test]
fn test_unordered_list_plus() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("+ one\n+ two\n+ three");
    match &md.sections[0] {
        Section::UnorderedList { items, .. } => {
            let checker = Checker::new(&md);
            let items = checker.item_paragraphs(*items);
            assert_eq!(items.len(), 3);
            checker.check_span(items[0], &Expect::text("one"));
        }
        other => panic!("expected list, got {other:?}"),
    }
}

#[test]
fn test_ordered_list() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("1. first\n2. second\n3. third");
    match &md.sections[0] {
        Section::OrderedList {
            start: 1,
            delimiter,
            items,
            ..
        } if *delimiter == OrderedListDelimiter::Dot => {
            let checker = Checker::new(&md);
            let items = checker.item_paragraphs(*items);
            assert_eq!(items.len(), 3);
            checker.check_span(items[0], &Expect::text("first"));
            checker.check_span(items[1], &Expect::text("second"));
            checker.check_span(items[2], &Expect::text("third"));
        }
        other => panic!("expected ordered list, got {other:?}"),
    }
}

#[test]
fn test_blockquote() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("> line one\n> line two");
    let checker = Checker::new(&md);
    let content = checker.quote_single_paragraph(&md.sections[0]);
    checker.check_span(
        content,
        &[
            Expect::Text("line one"),
            Expect::SoftBreak,
            Expect::Text("line two"),
        ],
    );
}

#[test]
fn test_blockquote_nested() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("> outer\n> > inner");
    let kids: Vec<_> = md
        .child_sections(match &md.sections[0] {
            Section::Blockquote { children } => *children,
            other => panic!("expected blockquote, got {other:?}"),
        })
        .collect();
    assert_eq!(kids.len(), 2, "got {kids:?}");
    assert!(matches!(kids[0], Section::Paragraph { .. }));
    assert!(
        matches!(kids[1], Section::Blockquote { .. }),
        "expected nested blockquote, got {:?}",
        kids[1]
    );
}

#[test]
fn test_blockquote_with_heading() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("> # Foo\n> bar");
    let kids: Vec<_> = md
        .child_sections(match &md.sections[0] {
            Section::Blockquote { children } => *children,
            other => panic!("expected blockquote, got {other:?}"),
        })
        .collect();
    assert_eq!(kids.len(), 2, "got {kids:?}");
    assert!(matches!(kids[0], Section::Heading { level: 1, .. }));
    assert!(matches!(kids[1], Section::Paragraph { .. }));
}

#[test]
fn test_horizontal_rule() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("---");
    assert_eq!(md.sections, vec![Section::HorizontalRule]);
}

#[test]
fn test_mixed_document() {
    let md: MarkdownFile<'_> = MarkdownFile::parse(
        "# Title\n\nSome text.\n\n- a\n- b\n\n> quote\n\n---\n\n```\ncode\n```",
    );
    assert_eq!(md.sections.len(), 6);

    match &md.sections[0] {
        Section::Heading { level: 1, content } => {
            Checker::new(&md).check_span(*content, &Expect::text("Title"));
        }
        other => panic!("expected heading, got {other:?}"),
    }
    match &md.sections[1] {
        Section::Paragraph { content } => {
            Checker::new(&md).check_span(*content, &Expect::text("Some text."));
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
    match &md.sections[2] {
        Section::UnorderedList { items, .. } => {
            let checker = Checker::new(&md);
            let items = checker.item_paragraphs(*items);
            checker.check_span(items[0], &Expect::text("a"));
            checker.check_span(items[1], &Expect::text("b"));
        }
        other => panic!("expected list, got {other:?}"),
    }
    {
        let checker = Checker::new(&md);
        let content = checker.quote_single_paragraph(&md.sections[3]);
        checker.check_span(content, &Expect::text("quote"));
    }
    assert_eq!(md.sections[4], Section::HorizontalRule);
    assert_eq!(
        md.sections[5],
        Section::CodeBlock {
            language: None,
            code: "code",
        }
    );
}

#[test]
fn test_heading_without_blank_line() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("some text\n# Heading");
    assert_eq!(md.sections.len(), 2);
    match &md.sections[0] {
        Section::Paragraph { content } => {
            Checker::new(&md).check_span(*content, &Expect::text("some text"));
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
    match &md.sections[1] {
        Section::Heading { level: 1, content } => {
            Checker::new(&md).check_span(*content, &Expect::text("Heading"));
        }
        other => panic!("expected heading, got {other:?}"),
    }
}

#[test]
fn test_setext_heading_level_1() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("Foo\n=====");
    assert_eq!(md.sections.len(), 1);
    match &md.sections[0] {
        Section::Heading { level: 1, content } => {
            Checker::new(&md).check_span(*content, &Expect::text("Foo"));
        }
        other => panic!("expected h1, got {other:?}"),
    }
}

#[test]
fn test_setext_heading_level_2() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("Foo\n-----");
    assert_eq!(md.sections.len(), 1);
    match &md.sections[0] {
        Section::Heading { level: 2, content } => {
            Checker::new(&md).check_span(*content, &Expect::text("Foo"));
        }
        other => panic!("expected h2, got {other:?}"),
    }
}

#[test]
fn test_setext_heading_with_inline() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("Foo *bar*\n=========");
    match &md.sections[0] {
        Section::Heading { level: 1, content } => Checker::new(&md).check_span(
            *content,
            &[Expect::Text("Foo "), Expect::Italic(Expect::text("bar"))],
        ),
        other => panic!("expected h1, got {other:?}"),
    }
}

#[test]
fn test_setext_heading_multiline_paragraph() {
    // The whole preceding paragraph becomes the heading text.
    let md: MarkdownFile<'_> = MarkdownFile::parse("Foo\nBar\n---");
    match &md.sections[0] {
        Section::Heading { level: 2, content } => Checker::new(&md).check_span(
            *content,
            &[Expect::Text("Foo"), Expect::SoftBreak, Expect::Text("Bar")],
        ),
        other => panic!("expected h2, got {other:?}"),
    }
}

#[test]
fn test_link_ref_shortcut() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("[foo]\n\n[foo]: /url \"title\"");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[Expect::Link {
                text: Expect::text("foo"),
                url: "/url",
                title: Some("title"),
            }],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_link_ref_full() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("[text][bar]\n\n[bar]: /url");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[Expect::Link {
                text: Expect::text("text"),
                url: "/url",
                title: None,
            }],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_link_ref_collapsed_case_insensitive() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("[Foo][]\n\n[foo]: /url");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[Expect::Link {
                text: Expect::text("Foo"),
                url: "/url",
                title: None,
            }],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_link_ref_image() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("![alt][bar]\n\n[bar]: /img.png \"t\"");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[Expect::Image {
                alt: vec![Expect::Text("alt")],
                url: "/img.png",
                title: Some("t"),
            }],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_link_ref_undefined_is_text() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("[nope]");
    match &md.sections[0] {
        Section::Paragraph { content } => {
            Checker::new(&md).check_span(*content, &[Expect::Text("[nope]")]);
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_autolink_uri() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("<http://foo.bar>");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[Expect::Autolink {
                target: "http://foo.bar",
                is_email: false,
            }],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_autolink_email() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("<foo@bar.com>");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[Expect::Autolink {
                target: "foo@bar.com",
                is_email: true,
            }],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_raw_inline_html() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("a <b2/> c");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[
                Expect::Text("a "),
                Expect::RawHtml("<b2/>"),
                Expect::Text(" c"),
            ],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_html_block_div() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("<div>\nbar\n</div>\n");
    assert_eq!(md.sections.len(), 1);
    match &md.sections[0] {
        Section::HtmlBlock { html } => assert_eq!(*html, "<div>\nbar\n</div>"),
        other => panic!("expected html block, got {other:?}"),
    }
}

#[test]
fn test_html_block_comment() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("<!-- hi -->\n");
    match &md.sections[0] {
        Section::HtmlBlock { html } => assert_eq!(*html, "<!-- hi -->"),
        other => panic!("expected html block, got {other:?}"),
    }
}

#[test]
fn test_html_block_script_ends_at_close_tag() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("<script>\nfoo\n</script>\nokay\n");
    assert_eq!(md.sections.len(), 2);
    match &md.sections[0] {
        Section::HtmlBlock { html } => {
            assert_eq!(*html, "<script>\nfoo\n</script>");
        }
        other => panic!("expected html block, got {other:?}"),
    }
    assert!(matches!(md.sections[1], Section::Paragraph { .. }));
}

#[test]
fn test_setext_dash_without_paragraph_is_hr() {
    // A `---` with no preceding paragraph text is a thematic break, not a
    // setext underline.
    let md: MarkdownFile<'_> = MarkdownFile::parse("---");
    assert_eq!(md.sections.len(), 1);
    assert!(matches!(md.sections[0], Section::HorizontalRule));
}

#[test]
fn test_bold() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("This is **bold** text");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[
                Expect::Text("This is "),
                Expect::Bold(Expect::text("bold")),
                Expect::Text(" text"),
            ],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_italic() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("This is *italic* text");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[
                Expect::Text("This is "),
                Expect::Italic(Expect::text("italic")),
                Expect::Text(" text"),
            ],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_bold_underscore() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("__bold__");
    match &md.sections[0] {
        Section::Paragraph { content } => {
            Checker::new(&md).check_span(*content, &[Expect::Bold(Expect::text("bold"))]);
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_italic_underscore() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("_italic_");
    match &md.sections[0] {
        Section::Paragraph { content } => {
            Checker::new(&md).check_span(*content, &[Expect::Italic(Expect::text("italic"))]);
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_link() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("Click [here](https://example.com) now");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[
                Expect::Text("Click "),
                Expect::Link {
                    text: Expect::text("here"),
                    url: "https://example.com",
                    title: None,
                },
                Expect::Text(" now"),
            ],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_image() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("![alt text](image.png)");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[Expect::Image {
                alt: vec![Expect::Text("alt text")],
                url: "image.png",
                title: None,
            }],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_bold_inside_link() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("[**bold link**](url)");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[Expect::Link {
                text: vec![Expect::Bold(Expect::text("bold link"))],
                url: "url",
                title: None,
            }],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_inline_in_heading() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("# A **bold** heading");
    match &md.sections[0] {
        Section::Heading { level: 1, content } => Checker::new(&md).check_span(
            *content,
            &[
                Expect::Text("A "),
                Expect::Bold(Expect::text("bold")),
                Expect::Text(" heading"),
            ],
        ),
        other => panic!("expected heading, got {other:?}"),
    }
}

#[test]
fn test_inline_in_list() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("- *italic item*\n- **bold item**");
    match &md.sections[0] {
        Section::UnorderedList { items, .. } => {
            let checker = Checker::new(&md);
            let items = checker.item_paragraphs(*items);
            checker.check_span(items[0], &[Expect::Italic(Expect::text("italic item"))]);
            checker.check_span(items[1], &[Expect::Bold(Expect::text("bold item"))]);
        }
        other => panic!("expected list, got {other:?}"),
    }
}

#[test]
fn test_parse_readme_file() {
    let content = std::fs::read_to_string("README.md").unwrap();
    let md: MarkdownFile<'_> = MarkdownFile::parse(&content);
    // Verify structure at a high level — detailed span content checked by other tests.
    assert!(md.sections.len() > 10, "README should have many sections");
    match &md.sections[0] {
        Section::Heading { level: 1, content } => {
            Checker::new(&md).check_span(*content, &Expect::text("marki"));
        }
        other => panic!("expected h1, got {other:?}"),
    }
}

#[test]
fn test_normalize_lf_is_borrowed() {
    let input = "hello\nworld";
    let normalized = MarkdownFile::normalize(input);
    assert!(matches!(normalized, Cow::Borrowed(_)));
    assert_eq!(&*normalized, input);
}

#[test]
fn test_normalize_crlf_strips_cr() {
    let input = "hello\r\nworld\r\n";
    let normalized = MarkdownFile::normalize(input);
    assert!(matches!(normalized, Cow::Owned(_)));
    assert_eq!(&*normalized, "hello\nworld\n");
}

#[test]
fn test_crlf_paragraph() {
    let input = MarkdownFile::normalize("line one\r\nline two\r\n");
    let md: MarkdownFile<'_> = MarkdownFile::parse(&input);
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[
                Expect::Text("line one"),
                Expect::SoftBreak,
                Expect::Text("line two"),
            ],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_crlf_code_block() {
    let input = MarkdownFile::normalize("```rust\r\nfn main() {}\r\nlet x = 1;\r\n```\r\n");
    let md: MarkdownFile<'_> = MarkdownFile::parse(&input);
    assert_eq!(
        md.sections[0],
        Section::CodeBlock {
            language: Some("rust"),
            code: "fn main() {}\nlet x = 1;",
        }
    );
}

#[test]
fn test_crlf_mixed_document() {
    let input = MarkdownFile::normalize("# Title\r\n\r\nSome text.\r\n\r\n- a\r\n- b\r\n");
    let md: MarkdownFile<'_> = MarkdownFile::parse(&input);
    assert_eq!(md.sections.len(), 3);
    match &md.sections[0] {
        Section::Heading { level: 1, content } => {
            Checker::new(&md).check_span(*content, &Expect::text("Title"));
        }
        other => panic!("expected heading, got {other:?}"),
    }
}

#[test]
fn test_emphasis_backslash_space_no_close() {
    let md: MarkdownFile<'_> = MarkdownFile::parse(r"*test\ *");
    match &md.sections[0] {
        Section::Paragraph { content } => {
            Checker::new(&md).check_span(*content, &[Expect::Text(r"*test\ *")]);
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_backslash_escape_punctuation() {
    let md: MarkdownFile<'_> = MarkdownFile::parse(r"hello \*world\*");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[
                Expect::Text("hello "),
                Expect::Text("*world"),
                Expect::Text("*"),
            ],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_backslash_escape_non_punctuation() {
    let md: MarkdownFile<'_> = MarkdownFile::parse(r"hello \n world");
    match &md.sections[0] {
        Section::Paragraph { content } => {
            Checker::new(&md).check_span(*content, &[Expect::Text(r"hello \n world")]);
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_ordered_list_paren_delimiter() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("1) first\n2) second");
    match &md.sections[0] {
        Section::OrderedList {
            start: 1,
            delimiter,
            items,
            ..
        } if *delimiter == OrderedListDelimiter::Paren => {
            let checker = Checker::new(&md);
            let items = checker.item_paragraphs(*items);
            checker.check_span(items[0], &Expect::text("first"));
            checker.check_span(items[1], &Expect::text("second"));
        }
        other => panic!("expected ordered list, got {other:?}"),
    }
}

#[test]
fn test_ordered_list_different_delimiters_split() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("1. dot\n2) paren");
    assert_eq!(md.sections.len(), 2);
    match &md.sections[0] {
        Section::OrderedList {
            start: 1,
            delimiter,
            items,
            ..
        } if *delimiter == OrderedListDelimiter::Dot => {
            let checker = Checker::new(&md);
            let items = checker.item_paragraphs(*items);
            checker.check_span(items[0], &Expect::text("dot"));
        }
        other => panic!("expected ordered list dot, got {other:?}"),
    }
    match &md.sections[1] {
        Section::OrderedList {
            start: 2,
            delimiter,
            items,
            ..
        } if *delimiter == OrderedListDelimiter::Paren => {
            let checker = Checker::new(&md);
            let items = checker.item_paragraphs(*items);
            checker.check_span(items[0], &Expect::text("paren"));
        }
        other => panic!("expected ordered list paren, got {other:?}"),
    }
}

#[test]
fn test_ordered_list_paren_custom_start() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("5) fifth\n6) sixth");
    match &md.sections[0] {
        Section::OrderedList {
            start: 5,
            delimiter,
            items,
            ..
        } if *delimiter == OrderedListDelimiter::Paren => {
            let checker = Checker::new(&md);
            let items = checker.item_paragraphs(*items);
            checker.check_span(items[0], &Expect::text("fifth"));
            checker.check_span(items[1], &Expect::text("sixth"));
        }
        other => panic!("expected ordered list, got {other:?}"),
    }
}

#[test]
fn test_heading_closing_hashes() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("# Heading #");
    match &md.sections[0] {
        Section::Heading { level: 1, content } => {
            Checker::new(&md).check_span(*content, &Expect::text("Heading"));
        }
        other => panic!("expected heading, got {other:?}"),
    }
}

#[test]
fn test_heading_closing_multiple_hashes() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("## Heading ##");
    match &md.sections[0] {
        Section::Heading { level: 2, content } => {
            Checker::new(&md).check_span(*content, &Expect::text("Heading"));
        }
        other => panic!("expected heading, got {other:?}"),
    }
}

#[test]
fn test_heading_closing_mismatched_hashes() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("# Heading ####");
    match &md.sections[0] {
        Section::Heading { level: 1, content } => {
            Checker::new(&md).check_span(*content, &Expect::text("Heading"));
        }
        other => panic!("expected heading, got {other:?}"),
    }
}

#[test]
fn test_heading_hash_no_space_not_stripped() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("# Heading#");
    match &md.sections[0] {
        Section::Heading { level: 1, content } => {
            Checker::new(&md).check_span(*content, &Expect::text("Heading#"));
        }
        other => panic!("expected heading, got {other:?}"),
    }
}

#[test]
fn test_heading_only_hashes() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("# ###");
    match &md.sections[0] {
        Section::Heading { level: 1, content } => {
            Checker::new(&md).check_span(*content, &[]);
        }
        other => panic!("expected heading, got {other:?}"),
    }
}

#[test]
fn test_soft_break_in_paragraph() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("line one\nline two\nline three");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[
                Expect::Text("line one"),
                Expect::SoftBreak,
                Expect::Text("line two"),
                Expect::SoftBreak,
                Expect::Text("line three"),
            ],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_soft_break_single_trailing_space() {
    // A single trailing space before a soft break is stripped (CommonMark
    // reflows paragraph lines): the space is too few for a hard break and is
    // removed as trailing line whitespace.
    let md: MarkdownFile<'_> = MarkdownFile::parse("line one \nline two");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[
                Expect::Text("line one"),
                Expect::SoftBreak,
                Expect::Text("line two"),
            ],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_paragraph_strips_leading_indentation_each_line() {
    // CommonMark reflows paragraph lines: leading indentation on every line
    // (including the first) is removed.
    let md: MarkdownFile<'_> = MarkdownFile::parse("  aaa\n bbb");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[Expect::Text("aaa"), Expect::SoftBreak, Expect::Text("bbb")],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_paragraph_strips_trailing_whitespace_final_line() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("aaa   ");
    match &md.sections[0] {
        Section::Paragraph { content } => {
            Checker::new(&md).check_span(*content, &[Expect::Text("aaa")]);
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_hard_break_two_trailing_spaces() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("line one  \nline two");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[
                Expect::Text("line one"),
                Expect::HardBreak,
                Expect::Text("line two"),
            ],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_hard_break_many_trailing_spaces() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("line one     \nline two");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[
                Expect::Text("line one"),
                Expect::HardBreak,
                Expect::Text("line two"),
            ],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_hard_break_trailing_backslash() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("line one\\\nline two");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[
                Expect::Text("line one"),
                Expect::HardBreak,
                Expect::Text("line two"),
            ],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_hard_break_with_inline() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("**bold**  \nnext line");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[
                Expect::Bold(Expect::text("bold")),
                Expect::HardBreak,
                Expect::Text("next line"),
            ],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_blockquote_lazy_continuation() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("> line one\ncontinuation");
    let checker = Checker::new(&md);
    let content = checker.quote_single_paragraph(&md.sections[0]);
    checker.check_span(
        content,
        &[
            Expect::Text("line one"),
            Expect::SoftBreak,
            Expect::Text("continuation"),
        ],
    );
}

#[test]
fn test_blockquote_lazy_multiple_lines() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("> first\nsecond\nthird");
    let checker = Checker::new(&md);
    let content = checker.quote_single_paragraph(&md.sections[0]);
    checker.check_span(
        content,
        &[
            Expect::Text("first"),
            Expect::SoftBreak,
            Expect::Text("second"),
            Expect::SoftBreak,
            Expect::Text("third"),
        ],
    );
}

#[test]
fn test_blockquote_lazy_stops_at_heading() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("> quoted\n# Heading");
    assert_eq!(md.sections.len(), 2);
    {
        let checker = Checker::new(&md);
        let content = checker.quote_single_paragraph(&md.sections[0]);
        checker.check_span(content, &Expect::text("quoted"));
    }
    match &md.sections[1] {
        Section::Heading { level: 1, content } => {
            Checker::new(&md).check_span(*content, &Expect::text("Heading"));
        }
        other => panic!("expected heading, got {other:?}"),
    }
}

#[test]
fn test_blockquote_lazy_stops_at_hr() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("> quoted\n---");
    assert_eq!(md.sections.len(), 2);
    {
        let checker = Checker::new(&md);
        let content = checker.quote_single_paragraph(&md.sections[0]);
        checker.check_span(content, &Expect::text("quoted"));
    }
    assert_eq!(md.sections[1], Section::HorizontalRule);
}

#[test]
fn test_blockquote_lazy_stops_at_list() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("> quoted\n- item");
    assert_eq!(md.sections.len(), 2);
    {
        let checker = Checker::new(&md);
        let content = checker.quote_single_paragraph(&md.sections[0]);
        checker.check_span(content, &Expect::text("quoted"));
    }
    match &md.sections[1] {
        Section::UnorderedList { items, .. } => {
            let checker = Checker::new(&md);
            let items = checker.item_paragraphs(*items);
            checker.check_span(items[0], &Expect::text("item"));
        }
        other => panic!("expected list, got {other:?}"),
    }
}

#[test]
fn test_blockquote_lazy_stops_at_code_fence() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("> quoted\n```\ncode\n```");
    assert_eq!(md.sections.len(), 2);
    {
        let checker = Checker::new(&md);
        let content = checker.quote_single_paragraph(&md.sections[0]);
        checker.check_span(content, &Expect::text("quoted"));
    }
    assert_eq!(
        md.sections[1],
        Section::CodeBlock {
            language: None,
            code: "code",
        }
    );
}

#[test]
fn test_link_with_double_quote_title() {
    let md: MarkdownFile<'_> = MarkdownFile::parse(r#"[text](url "a title")"#);
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[Expect::Link {
                text: Expect::text("text"),
                url: "url",
                title: Some("a title"),
            }],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_link_with_single_quote_title() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("[text](url 'a title')");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[Expect::Link {
                text: Expect::text("text"),
                url: "url",
                title: Some("a title"),
            }],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_link_with_paren_title() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("[text](url (a title))");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[Expect::Link {
                text: Expect::text("text"),
                url: "url",
                title: Some("a title"),
            }],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_link_no_title() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("[text](url)");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[Expect::Link {
                text: Expect::text("text"),
                url: "url",
                title: None,
            }],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_image_with_title() {
    let md: MarkdownFile<'_> = MarkdownFile::parse(r#"![alt](img.png "photo")"#);
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[Expect::Image {
                alt: vec![Expect::Text("alt")],
                url: "img.png",
                title: Some("photo"),
            }],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_emphasis_star_intraword() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("foo*bar*baz");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[
                Expect::Text("foo"),
                Expect::Italic(Expect::text("bar")),
                Expect::Text("baz"),
            ],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_emphasis_underscore_no_intraword() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("foo_bar_baz");
    match &md.sections[0] {
        Section::Paragraph { content } => {
            Checker::new(&md).check_span(*content, &Expect::text("foo_bar_baz"));
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_emphasis_underscore_word_boundaries() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("_foo_ bar _baz_");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[
                Expect::Italic(Expect::text("foo")),
                Expect::Text(" bar "),
                Expect::Italic(Expect::text("baz")),
            ],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_bold_underscore_no_intraword() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("foo__bar__baz");
    match &md.sections[0] {
        Section::Paragraph { content } => {
            Checker::new(&md).check_span(*content, &Expect::text("foo__bar__baz"));
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_emphasis_star_after_punctuation() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("(*foo*)");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[
                Expect::Text("("),
                Expect::Italic(Expect::text("foo")),
                Expect::Text(")"),
            ],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_emphasis_underscore_after_punctuation() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("(_foo_)");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[
                Expect::Text("("),
                Expect::Italic(Expect::text("foo")),
                Expect::Text(")"),
            ],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_emphasis_not_opened_by_whitespace_after() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("a * not emphasis * b");
    match &md.sections[0] {
        Section::Paragraph { content } => {
            Checker::new(&md).check_span(*content, &Expect::text("a * not emphasis * b"));
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_bold_star_intraword() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("foo**bar**baz");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[
                Expect::Text("foo"),
                Expect::Bold(Expect::text("bar")),
                Expect::Text("baz"),
            ],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_triple_star_bold_italic() {
    // A symmetric triple-star run should produce strong emphasis nested
    // inside emphasis (bold+italic), not a swallowed trailing star.
    let md: MarkdownFile<'_> = MarkdownFile::parse("***bold italic***");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[Expect::Italic(vec![Expect::Bold(Expect::text(
                "bold italic",
            ))])],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_triple_star_bold_italic_intraword() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("foo***bar***baz");
    match &md.sections[0] {
        Section::Paragraph { content } => Checker::new(&md).check_span(
            *content,
            &[
                Expect::Text("foo"),
                Expect::Italic(vec![Expect::Bold(Expect::text("bar"))]),
                Expect::Text("baz"),
            ],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

// -----------------------------------------------------------------------
// Indentation (0-3 spaces) — CommonMark §4
// -----------------------------------------------------------------------

#[test]
fn test_heading_indented_1_space() {
    let md: MarkdownFile<'_> = MarkdownFile::parse(" # Hello");
    match &md.sections[0] {
        Section::Heading { level: 1, content } => {
            Checker::new(&md).check_span(*content, &Expect::text("Hello"));
        }
        other => panic!("expected heading, got {other:?}"),
    }
}

#[test]
fn test_heading_indented_3_spaces() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("   ## World");
    match &md.sections[0] {
        Section::Heading { level: 2, content } => {
            Checker::new(&md).check_span(*content, &Expect::text("World"));
        }
        other => panic!("expected heading, got {other:?}"),
    }
}

#[test]
fn test_heading_indented_4_spaces_is_indented_code() {
    // CommonMark §4.4 example 69: four leading spaces makes an indented code
    // block, not a heading.
    let md: MarkdownFile<'_> = MarkdownFile::parse("    # Not a heading");
    match &md.sections[0] {
        Section::IndentedCode { .. } => {}
        other => panic!("expected indented code, got {other:?}"),
    }
}

#[test]
fn test_hr_indented_3_spaces() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("   ---");
    assert_eq!(md.sections, vec![Section::HorizontalRule]);
}

#[test]
fn test_indented_code_block_basic() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("    a simple\n      indented code block");
    assert_eq!(
        md.sections,
        vec![Section::IndentedCode {
            code: "    a simple\n      indented code block",
        }]
    );
    assert_eq!(
        md.to_html(),
        "<pre><code>a simple\n  indented code block\n</code></pre>\n"
    );
}

#[test]
fn test_indented_code_block_interior_blanks() {
    // Interior blank lines are kept; trailing blanks are trimmed.
    let md: MarkdownFile<'_> =
        MarkdownFile::parse("    chunk1\n\n    chunk2\n  \n \n \n    chunk3\n");
    assert_eq!(
        md.to_html(),
        "<pre><code>chunk1\n\nchunk2\n\n\n\nchunk3\n</code></pre>\n"
    );
}

#[test]
fn test_indented_code_does_not_interrupt_paragraph() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("Foo\n    bar");
    assert_eq!(md.sections.len(), 1);
    assert!(matches!(md.sections[0], Section::Paragraph { .. }));
}

#[test]
fn test_hr_indented_4_spaces_is_indented_code() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("    ---");
    match &md.sections[0] {
        Section::IndentedCode { .. } => {}
        other => panic!("expected indented code, got {other:?}"),
    }
}

#[test]
fn test_unordered_list_indented_2_spaces() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("  - one\n  - two");
    match &md.sections[0] {
        Section::UnorderedList { items, .. } => {
            let checker = Checker::new(&md);
            let items = checker.item_paragraphs(*items);
            assert_eq!(items.len(), 2);
            checker.check_span(items[0], &Expect::text("one"));
            checker.check_span(items[1], &Expect::text("two"));
        }
        other => panic!("expected list, got {other:?}"),
    }
}

#[test]
fn test_unordered_list_indented_4_spaces_is_indented_code() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("    - not a list");
    match &md.sections[0] {
        Section::IndentedCode { .. } => {}
        other => panic!("expected indented code, got {other:?}"),
    }
}

#[test]
fn test_ordered_list_indented_1_space() {
    let md: MarkdownFile<'_> = MarkdownFile::parse(" 1. first\n 2. second");
    match &md.sections[0] {
        Section::OrderedList {
            start: 1, items, ..
        } => {
            let checker = Checker::new(&md);
            let items = checker.item_paragraphs(*items);
            assert_eq!(items.len(), 2);
            checker.check_span(items[0], &Expect::text("first"));
        }
        other => panic!("expected ordered list, got {other:?}"),
    }
}

#[test]
fn test_blockquote_indented_3_spaces() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("   > quoted");
    let checker = Checker::new(&md);
    let content = checker.quote_single_paragraph(&md.sections[0]);
    checker.check_span(content, &Expect::text("quoted"));
}

#[test]
fn test_blockquote_indented_4_spaces_is_indented_code() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("    > not a quote");
    match &md.sections[0] {
        Section::IndentedCode { .. } => {}
        other => panic!("expected indented code, got {other:?}"),
    }
}

#[test]
fn test_code_fence_indented_3_spaces() {
    // A fence indented 1-3 columns strips up to that many leading spaces from
    // each content line (CommonMark §4.5). The dedent routes the content
    // through the line pool, so assert on rendered HTML rather than the
    // section shape.
    let md: MarkdownFile<'_> = MarkdownFile::parse("   ```\n   hello\n   ```");
    assert_eq!(md.to_html(), "<pre><code>hello\n</code></pre>\n");
}

#[test]
fn test_code_fence_indented_4_spaces_is_indented_code() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("    ```\nnot code\n    ```");
    // 4-space indent: not a code fence; the whole thing is an indented code
    // block (CommonMark §4.4 example 134).
    match &md.sections[0] {
        Section::IndentedCode { .. } => {}
        other => panic!("expected indented code, got {other:?}"),
    }
}

// -----------------------------------------------------------------------
// Tilde code fences — CommonMark §4.5
// -----------------------------------------------------------------------

#[test]
fn test_tilde_code_fence() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("~~~\nhello\n~~~");
    assert_eq!(
        md.sections[0],
        Section::CodeBlock {
            language: None,
            code: "hello",
        }
    );
}

#[test]
fn test_tilde_code_fence_with_language() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("~~~python\nprint('hi')\n~~~");
    assert_eq!(
        md.sections[0],
        Section::CodeBlock {
            language: Some("python"),
            code: "print('hi')",
        }
    );
}

#[test]
fn test_tilde_code_fence_longer_close() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("~~~\nhello\n~~~~~");
    assert_eq!(
        md.sections[0],
        Section::CodeBlock {
            language: None,
            code: "hello",
        }
    );
}

#[test]
fn test_tilde_fence_not_closed_by_backticks() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("~~~\nhello\n```\n~~~");
    assert_eq!(
        md.sections[0],
        Section::CodeBlock {
            language: None,
            code: "hello\n```",
        }
    );
}

#[test]
fn test_backtick_fence_not_closed_by_tildes() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("```\nhello\n~~~\n```");
    assert_eq!(
        md.sections[0],
        Section::CodeBlock {
            language: None,
            code: "hello\n~~~",
        }
    );
}

#[test]
fn test_tilde_fence_backticks_in_info_string() {
    // Tilde fences allow backticks in the info string (CommonMark §4.5)
    let md: MarkdownFile<'_> = MarkdownFile::parse("~~~ aa ```\nhello\n~~~");
    assert_eq!(
        md.sections[0],
        Section::CodeBlock {
            language: Some("aa ```"),
            code: "hello",
        }
    );
}

#[test]
fn test_tilde_fence_indented() {
    // See `test_code_fence_indented_3_spaces`: a 2-column indent dedents the
    // content (here a less-indented line keeps only its surplus spaces).
    let md: MarkdownFile<'_> = MarkdownFile::parse("  ~~~\n hello\n  ~~~");
    assert_eq!(md.to_html(), "<pre><code>hello\n</code></pre>\n");
}

// -----------------------------------------------------------------------
// Empty ordered list items
// -----------------------------------------------------------------------

#[test]
fn test_ordered_list_empty_item() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("1. \n2. second");
    match &md.sections[0] {
        Section::OrderedList {
            start: 1, items, ..
        } => {
            // The first item is empty: CommonMark gives it no child blocks at
            // all (an empty `<li></li>`), while the second holds one paragraph.
            let item_sections: Vec<_> = md.child_sections(*items).collect();
            assert_eq!(item_sections.len(), 2);
            match item_sections[0] {
                Section::ListItem { children } => {
                    assert_eq!(md.child_sections(*children).count(), 0);
                }
                other => panic!("expected list item, got {other:?}"),
            }
            let checker = Checker::new(&md);
            match item_sections[1] {
                Section::ListItem { children } => {
                    let kids: Vec<_> = md.child_sections(*children).collect();
                    assert_eq!(kids.len(), 1);
                    match kids[0] {
                        Section::Paragraph { content } => {
                            checker.check_span(*content, &Expect::text("second"));
                        }
                        other => panic!("expected paragraph, got {other:?}"),
                    }
                }
                other => panic!("expected list item, got {other:?}"),
            }
        }
        other => panic!("expected ordered list, got {other:?}"),
    }
}

// -----------------------------------------------------------------------
// normalize: bare \r handling
// -----------------------------------------------------------------------

#[test]
fn test_normalize_bare_cr_becomes_newline() {
    let input = "hello\rworld";
    let normalized = MarkdownFile::normalize(input);
    assert_eq!(&*normalized, "hello\nworld");
}

#[test]
fn test_normalize_mixed_cr_crlf() {
    let input = "a\rb\r\nc\r";
    let normalized = MarkdownFile::normalize(input);
    assert_eq!(&*normalized, "a\nb\nc\n");
}

// -----------------------------------------------------------------------
// Closing fence indentation
// -----------------------------------------------------------------------

#[test]
fn test_closing_fence_indented_3_spaces() {
    let md: MarkdownFile<'_> = MarkdownFile::parse("```\nhello\n   ```");
    assert_eq!(
        md.sections[0],
        Section::CodeBlock {
            language: None,
            code: "hello",
        }
    );
}

#[test]
fn test_closing_fence_indented_4_spaces_not_closing() {
    // 4-space indent on closing fence means it's content, not a close.
    let md: MarkdownFile<'_> = MarkdownFile::parse("```\nhello\n    ```");
    assert_eq!(
        md.sections[0],
        Section::CodeBlock {
            language: None,
            code: "hello\n    ```",
        }
    );
}
