use std::borrow::Cow;

use crate::section::OrderedListDelimiter;
use crate::{Inline, InlineSpan, MarkdownFile, Section, normalize};

/// Assert that a span in the pool contains exactly the given inline elements.
/// For nested spans (Bold, Italic, Link), recursively checks children.
fn assert_inlines(md: &MarkdownFile, span: InlineSpan, expected: &[Expect]) {
    let actual = md.inlines(span);
    assert_eq!(
        actual.len(),
        expected.len(),
        "span length mismatch: got {actual:?}, expected {expected:?}"
    );
    for (i, (a, e)) in actual.iter().zip(expected).enumerate() {
        assert_inline_eq(md, a, e, i);
    }
}

fn assert_inline_eq(md: &MarkdownFile, actual: &Inline, expected: &Expect, idx: usize) {
    match (actual, expected) {
        (Inline::Text(a), Expect::Text(e)) => {
            assert_eq!(a, e, "Text mismatch at index {idx}");
        }
        (Inline::Bold(span), Expect::Bold(children)) => {
            assert_inlines(md, *span, children);
        }
        (Inline::Italic(span), Expect::Italic(children)) => {
            assert_inlines(md, *span, children);
        }
        (Inline::Code(a), Expect::Code(e)) => {
            assert_eq!(a, e, "Code mismatch at index {idx}");
        }
        (Inline::SoftBreak, Expect::SoftBreak) => {}
        (Inline::HardBreak, Expect::HardBreak) => {}
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
            assert_inlines(md, *text, et);
        }
        (
            Inline::Image { alt, url, title },
            Expect::Image {
                alt: ea,
                url: eu,
                title: eti,
            },
        ) => {
            assert_eq!(alt, ea, "Image alt mismatch at index {idx}");
            assert_eq!(url, eu, "Image url mismatch at index {idx}");
            assert_eq!(title, eti, "Image title mismatch at index {idx}");
        }
        _ => panic!("Inline mismatch at index {idx}: got {actual:?}, expected {expected:?}"),
    }
}

/// Expected inline element for test assertions.
#[derive(Debug)]
#[allow(dead_code)]
enum Expect<'a> {
    Text(&'a str),
    Bold(Vec<Expect<'a>>),
    Italic(Vec<Expect<'a>>),
    Code(&'a str),
    SoftBreak,
    HardBreak,
    Link {
        text: Vec<Expect<'a>>,
        url: &'a str,
        title: Option<&'a str>,
    },
    Image {
        alt: &'a str,
        url: &'a str,
        title: Option<&'a str>,
    },
}

/// Shorthand for expected text-only content.
fn text(s: &str) -> Vec<Expect<'_>> {
    vec![Expect::Text(s)]
}

/// Assert a section's inline content matches expected.
fn assert_content(md: &MarkdownFile, span: InlineSpan, expected: &[Expect]) {
    assert_inlines(md, span, expected);
}

#[test]
fn test_heading() {
    let md = MarkdownFile::parse("# Hello\n## World");
    assert_eq!(md.sections.len(), 2);
    match &md.sections[0] {
        Section::Heading { level: 1, content } => assert_content(&md, *content, &text("Hello")),
        other => panic!("expected heading, got {other:?}"),
    }
    match &md.sections[1] {
        Section::Heading { level: 2, content } => assert_content(&md, *content, &text("World")),
        other => panic!("expected heading, got {other:?}"),
    }
}

#[test]
fn test_paragraph() {
    let md = MarkdownFile::parse("This is a paragraph.\nWith two lines.");
    assert_eq!(md.sections.len(), 1);
    match &md.sections[0] {
        Section::Paragraph { content } => assert_content(
            &md,
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
    let md = MarkdownFile::parse("```rust\nfn main() {}\n```");
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
    let md = MarkdownFile::parse("```\nhello\n```");
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
    let md = MarkdownFile::parse("- one\n- two\n- three");
    assert_eq!(md.sections.len(), 1);
    match &md.sections[0] {
        Section::UnorderedList { items } => {
            assert_eq!(items.len(), 3);
            assert_content(&md, items[0], &text("one"));
            assert_content(&md, items[1], &text("two"));
            assert_content(&md, items[2], &text("three"));
        }
        other => panic!("expected list, got {other:?}"),
    }
}

#[test]
fn test_unordered_list_plus() {
    let md = MarkdownFile::parse("+ one\n+ two\n+ three");
    match &md.sections[0] {
        Section::UnorderedList { items } => {
            assert_eq!(items.len(), 3);
            assert_content(&md, items[0], &text("one"));
        }
        other => panic!("expected list, got {other:?}"),
    }
}

#[test]
fn test_ordered_list() {
    let md = MarkdownFile::parse("1. first\n2. second\n3. third");
    match &md.sections[0] {
        Section::OrderedList {
            start: 1,
            delimiter,
            items,
        } if *delimiter == OrderedListDelimiter::Dot => {
            assert_eq!(items.len(), 3);
            assert_content(&md, items[0], &text("first"));
            assert_content(&md, items[1], &text("second"));
            assert_content(&md, items[2], &text("third"));
        }
        other => panic!("expected ordered list, got {other:?}"),
    }
}

#[test]
fn test_blockquote() {
    let md = MarkdownFile::parse("> line one\n> line two");
    match &md.sections[0] {
        Section::Blockquote { content } => assert_content(
            &md,
            *content,
            &[
                Expect::Text("line one"),
                Expect::Text("\n"),
                Expect::Text("line two"),
            ],
        ),
        other => panic!("expected blockquote, got {other:?}"),
    }
}

#[test]
fn test_horizontal_rule() {
    let md = MarkdownFile::parse("---");
    assert_eq!(md.sections, vec![Section::HorizontalRule]);
}

#[test]
fn test_mixed_document() {
    let md = MarkdownFile::parse(
        "# Title\n\nSome text.\n\n- a\n- b\n\n> quote\n\n---\n\n```\ncode\n```",
    );
    assert_eq!(md.sections.len(), 6);

    match &md.sections[0] {
        Section::Heading { level: 1, content } => assert_content(&md, *content, &text("Title")),
        other => panic!("expected heading, got {other:?}"),
    }
    match &md.sections[1] {
        Section::Paragraph { content } => {
            assert_content(&md, *content, &text("Some text."));
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
    match &md.sections[2] {
        Section::UnorderedList { items } => {
            assert_content(&md, items[0], &text("a"));
            assert_content(&md, items[1], &text("b"));
        }
        other => panic!("expected list, got {other:?}"),
    }
    match &md.sections[3] {
        Section::Blockquote { content } => {
            assert_content(&md, *content, &text("quote"));
        }
        other => panic!("expected blockquote, got {other:?}"),
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
    let md = MarkdownFile::parse("some text\n# Heading");
    assert_eq!(md.sections.len(), 2);
    match &md.sections[0] {
        Section::Paragraph { content } => assert_content(&md, *content, &text("some text")),
        other => panic!("expected paragraph, got {other:?}"),
    }
    match &md.sections[1] {
        Section::Heading { level: 1, content } => {
            assert_content(&md, *content, &text("Heading"));
        }
        other => panic!("expected heading, got {other:?}"),
    }
}

#[test]
fn test_bold() {
    let md = MarkdownFile::parse("This is **bold** text");
    match &md.sections[0] {
        Section::Paragraph { content } => assert_content(
            &md,
            *content,
            &[
                Expect::Text("This is "),
                Expect::Bold(text("bold")),
                Expect::Text(" text"),
            ],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_italic() {
    let md = MarkdownFile::parse("This is *italic* text");
    match &md.sections[0] {
        Section::Paragraph { content } => assert_content(
            &md,
            *content,
            &[
                Expect::Text("This is "),
                Expect::Italic(text("italic")),
                Expect::Text(" text"),
            ],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_bold_underscore() {
    let md = MarkdownFile::parse("__bold__");
    match &md.sections[0] {
        Section::Paragraph { content } => {
            assert_content(&md, *content, &[Expect::Bold(text("bold"))]);
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_italic_underscore() {
    let md = MarkdownFile::parse("_italic_");
    match &md.sections[0] {
        Section::Paragraph { content } => {
            assert_content(&md, *content, &[Expect::Italic(text("italic"))]);
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_link() {
    let md = MarkdownFile::parse("Click [here](https://example.com) now");
    match &md.sections[0] {
        Section::Paragraph { content } => assert_content(
            &md,
            *content,
            &[
                Expect::Text("Click "),
                Expect::Link {
                    text: text("here"),
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
    let md = MarkdownFile::parse("![alt text](image.png)");
    match &md.sections[0] {
        Section::Paragraph { content } => assert_content(
            &md,
            *content,
            &[Expect::Image {
                alt: "alt text",
                url: "image.png",
                title: None,
            }],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_bold_inside_link() {
    let md = MarkdownFile::parse("[**bold link**](url)");
    match &md.sections[0] {
        Section::Paragraph { content } => assert_content(
            &md,
            *content,
            &[Expect::Link {
                text: vec![Expect::Bold(text("bold link"))],
                url: "url",
                title: None,
            }],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_inline_in_heading() {
    let md = MarkdownFile::parse("# A **bold** heading");
    match &md.sections[0] {
        Section::Heading { level: 1, content } => assert_content(
            &md,
            *content,
            &[
                Expect::Text("A "),
                Expect::Bold(text("bold")),
                Expect::Text(" heading"),
            ],
        ),
        other => panic!("expected heading, got {other:?}"),
    }
}

#[test]
fn test_inline_in_list() {
    let md = MarkdownFile::parse("- *italic item*\n- **bold item**");
    match &md.sections[0] {
        Section::UnorderedList { items } => {
            assert_content(&md, items[0], &[Expect::Italic(text("italic item"))]);
            assert_content(&md, items[1], &[Expect::Bold(text("bold item"))]);
        }
        other => panic!("expected list, got {other:?}"),
    }
}

#[test]
fn test_parse_readme_file() {
    let content = std::fs::read_to_string("README.md").unwrap();
    let md = MarkdownFile::parse(&content);
    // Verify structure at a high level — detailed span content checked by other tests.
    assert!(md.sections.len() > 10, "README should have many sections");
    match &md.sections[0] {
        Section::Heading { level: 1, content } => assert_content(&md, *content, &text("marki")),
        other => panic!("expected h1, got {other:?}"),
    }
}

#[test]
fn test_normalize_lf_is_borrowed() {
    let input = "hello\nworld";
    let normalized = normalize(input);
    assert!(matches!(normalized, Cow::Borrowed(_)));
    assert_eq!(&*normalized, input);
}

#[test]
fn test_normalize_crlf_strips_cr() {
    let input = "hello\r\nworld\r\n";
    let normalized = normalize(input);
    assert!(matches!(normalized, Cow::Owned(_)));
    assert_eq!(&*normalized, "hello\nworld\n");
}

#[test]
fn test_crlf_paragraph() {
    let input = normalize("line one\r\nline two\r\n");
    let md = MarkdownFile::parse(&input);
    match &md.sections[0] {
        Section::Paragraph { content } => assert_content(
            &md,
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
    let input = normalize("```rust\r\nfn main() {}\r\nlet x = 1;\r\n```\r\n");
    let md = MarkdownFile::parse(&input);
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
    let input = normalize("# Title\r\n\r\nSome text.\r\n\r\n- a\r\n- b\r\n");
    let md = MarkdownFile::parse(&input);
    assert_eq!(md.sections.len(), 3);
    match &md.sections[0] {
        Section::Heading { level: 1, content } => assert_content(&md, *content, &text("Title")),
        other => panic!("expected heading, got {other:?}"),
    }
}

#[test]
fn test_emphasis_backslash_space_no_close() {
    let md = MarkdownFile::parse(r"*test\ *");
    match &md.sections[0] {
        Section::Paragraph { content } => {
            assert_content(&md, *content, &[Expect::Text(r"*test\ *")]);
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_backslash_escape_punctuation() {
    let md = MarkdownFile::parse(r"hello \*world\*");
    match &md.sections[0] {
        Section::Paragraph { content } => assert_content(
            &md,
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
    let md = MarkdownFile::parse(r"hello \n world");
    match &md.sections[0] {
        Section::Paragraph { content } => {
            assert_content(&md, *content, &[Expect::Text(r"hello \n world")]);
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_ordered_list_paren_delimiter() {
    let md = MarkdownFile::parse("1) first\n2) second");
    match &md.sections[0] {
        Section::OrderedList {
            start: 1,
            delimiter,
            items,
        } if *delimiter == OrderedListDelimiter::Paren => {
            assert_content(&md, items[0], &text("first"));
            assert_content(&md, items[1], &text("second"));
        }
        other => panic!("expected ordered list, got {other:?}"),
    }
}

#[test]
fn test_ordered_list_different_delimiters_split() {
    let md = MarkdownFile::parse("1. dot\n2) paren");
    assert_eq!(md.sections.len(), 2);
    match &md.sections[0] {
        Section::OrderedList {
            start: 1,
            delimiter,
            items,
        } if *delimiter == OrderedListDelimiter::Dot => {
            assert_content(&md, items[0], &text("dot"));
        }
        other => panic!("expected ordered list dot, got {other:?}"),
    }
    match &md.sections[1] {
        Section::OrderedList {
            start: 2,
            delimiter,
            items,
        } if *delimiter == OrderedListDelimiter::Paren => {
            assert_content(&md, items[0], &text("paren"));
        }
        other => panic!("expected ordered list paren, got {other:?}"),
    }
}

#[test]
fn test_ordered_list_paren_custom_start() {
    let md = MarkdownFile::parse("5) fifth\n6) sixth");
    match &md.sections[0] {
        Section::OrderedList {
            start: 5,
            delimiter,
            items,
        } if *delimiter == OrderedListDelimiter::Paren => {
            assert_content(&md, items[0], &text("fifth"));
            assert_content(&md, items[1], &text("sixth"));
        }
        other => panic!("expected ordered list, got {other:?}"),
    }
}

#[test]
fn test_heading_closing_hashes() {
    let md = MarkdownFile::parse("# Heading #");
    match &md.sections[0] {
        Section::Heading { level: 1, content } => {
            assert_content(&md, *content, &text("Heading"));
        }
        other => panic!("expected heading, got {other:?}"),
    }
}

#[test]
fn test_heading_closing_multiple_hashes() {
    let md = MarkdownFile::parse("## Heading ##");
    match &md.sections[0] {
        Section::Heading { level: 2, content } => {
            assert_content(&md, *content, &text("Heading"));
        }
        other => panic!("expected heading, got {other:?}"),
    }
}

#[test]
fn test_heading_closing_mismatched_hashes() {
    let md = MarkdownFile::parse("# Heading ####");
    match &md.sections[0] {
        Section::Heading { level: 1, content } => {
            assert_content(&md, *content, &text("Heading"));
        }
        other => panic!("expected heading, got {other:?}"),
    }
}

#[test]
fn test_heading_hash_no_space_not_stripped() {
    let md = MarkdownFile::parse("# Heading#");
    match &md.sections[0] {
        Section::Heading { level: 1, content } => {
            assert_content(&md, *content, &text("Heading#"));
        }
        other => panic!("expected heading, got {other:?}"),
    }
}

#[test]
fn test_heading_only_hashes() {
    let md = MarkdownFile::parse("# ###");
    match &md.sections[0] {
        Section::Heading { level: 1, content } => {
            assert_content(&md, *content, &[]);
        }
        other => panic!("expected heading, got {other:?}"),
    }
}

#[test]
fn test_soft_break_in_paragraph() {
    let md = MarkdownFile::parse("line one\nline two\nline three");
    match &md.sections[0] {
        Section::Paragraph { content } => assert_content(
            &md,
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
    let md = MarkdownFile::parse("line one \nline two");
    match &md.sections[0] {
        Section::Paragraph { content } => assert_content(
            &md,
            *content,
            &[
                Expect::Text("line one "),
                Expect::SoftBreak,
                Expect::Text("line two"),
            ],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_hard_break_two_trailing_spaces() {
    let md = MarkdownFile::parse("line one  \nline two");
    match &md.sections[0] {
        Section::Paragraph { content } => assert_content(
            &md,
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
    let md = MarkdownFile::parse("line one     \nline two");
    match &md.sections[0] {
        Section::Paragraph { content } => assert_content(
            &md,
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
    let md = MarkdownFile::parse("line one\\\nline two");
    match &md.sections[0] {
        Section::Paragraph { content } => assert_content(
            &md,
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
    let md = MarkdownFile::parse("**bold**  \nnext line");
    match &md.sections[0] {
        Section::Paragraph { content } => assert_content(
            &md,
            *content,
            &[
                Expect::Bold(text("bold")),
                Expect::HardBreak,
                Expect::Text("next line"),
            ],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_blockquote_lazy_continuation() {
    let md = MarkdownFile::parse("> line one\ncontinuation");
    match &md.sections[0] {
        Section::Blockquote { content } => assert_content(
            &md,
            *content,
            &[
                Expect::Text("line one"),
                Expect::Text("\n"),
                Expect::Text("continuation"),
            ],
        ),
        other => panic!("expected blockquote, got {other:?}"),
    }
}

#[test]
fn test_blockquote_lazy_multiple_lines() {
    let md = MarkdownFile::parse("> first\nsecond\nthird");
    match &md.sections[0] {
        Section::Blockquote { content } => assert_content(
            &md,
            *content,
            &[
                Expect::Text("first"),
                Expect::Text("\n"),
                Expect::Text("second"),
                Expect::Text("\n"),
                Expect::Text("third"),
            ],
        ),
        other => panic!("expected blockquote, got {other:?}"),
    }
}

#[test]
fn test_blockquote_lazy_stops_at_heading() {
    let md = MarkdownFile::parse("> quoted\n# Heading");
    assert_eq!(md.sections.len(), 2);
    match &md.sections[0] {
        Section::Blockquote { content } => assert_content(&md, *content, &text("quoted")),
        other => panic!("expected blockquote, got {other:?}"),
    }
    match &md.sections[1] {
        Section::Heading { level: 1, content } => {
            assert_content(&md, *content, &text("Heading"));
        }
        other => panic!("expected heading, got {other:?}"),
    }
}

#[test]
fn test_blockquote_lazy_stops_at_hr() {
    let md = MarkdownFile::parse("> quoted\n---");
    assert_eq!(md.sections.len(), 2);
    match &md.sections[0] {
        Section::Blockquote { content } => assert_content(&md, *content, &text("quoted")),
        other => panic!("expected blockquote, got {other:?}"),
    }
    assert_eq!(md.sections[1], Section::HorizontalRule);
}

#[test]
fn test_blockquote_lazy_stops_at_list() {
    let md = MarkdownFile::parse("> quoted\n- item");
    assert_eq!(md.sections.len(), 2);
    match &md.sections[0] {
        Section::Blockquote { content } => assert_content(&md, *content, &text("quoted")),
        other => panic!("expected blockquote, got {other:?}"),
    }
    match &md.sections[1] {
        Section::UnorderedList { items } => {
            assert_content(&md, items[0], &text("item"));
        }
        other => panic!("expected list, got {other:?}"),
    }
}

#[test]
fn test_blockquote_lazy_stops_at_code_fence() {
    let md = MarkdownFile::parse("> quoted\n```\ncode\n```");
    assert_eq!(md.sections.len(), 2);
    match &md.sections[0] {
        Section::Blockquote { content } => assert_content(&md, *content, &text("quoted")),
        other => panic!("expected blockquote, got {other:?}"),
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
    let md = MarkdownFile::parse(r#"[text](url "a title")"#);
    match &md.sections[0] {
        Section::Paragraph { content } => assert_content(
            &md,
            *content,
            &[Expect::Link {
                text: text("text"),
                url: "url",
                title: Some("a title"),
            }],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_link_with_single_quote_title() {
    let md = MarkdownFile::parse("[text](url 'a title')");
    match &md.sections[0] {
        Section::Paragraph { content } => assert_content(
            &md,
            *content,
            &[Expect::Link {
                text: text("text"),
                url: "url",
                title: Some("a title"),
            }],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_link_with_paren_title() {
    let md = MarkdownFile::parse("[text](url (a title))");
    match &md.sections[0] {
        Section::Paragraph { content } => assert_content(
            &md,
            *content,
            &[Expect::Link {
                text: text("text"),
                url: "url",
                title: Some("a title"),
            }],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_link_no_title() {
    let md = MarkdownFile::parse("[text](url)");
    match &md.sections[0] {
        Section::Paragraph { content } => assert_content(
            &md,
            *content,
            &[Expect::Link {
                text: text("text"),
                url: "url",
                title: None,
            }],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_image_with_title() {
    let md = MarkdownFile::parse(r#"![alt](img.png "photo")"#);
    match &md.sections[0] {
        Section::Paragraph { content } => assert_content(
            &md,
            *content,
            &[Expect::Image {
                alt: "alt",
                url: "img.png",
                title: Some("photo"),
            }],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_emphasis_star_intraword() {
    let md = MarkdownFile::parse("foo*bar*baz");
    match &md.sections[0] {
        Section::Paragraph { content } => assert_content(
            &md,
            *content,
            &[
                Expect::Text("foo"),
                Expect::Italic(text("bar")),
                Expect::Text("baz"),
            ],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_emphasis_underscore_no_intraword() {
    let md = MarkdownFile::parse("foo_bar_baz");
    match &md.sections[0] {
        Section::Paragraph { content } => {
            assert_content(&md, *content, &text("foo_bar_baz"));
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_emphasis_underscore_word_boundaries() {
    let md = MarkdownFile::parse("_foo_ bar _baz_");
    match &md.sections[0] {
        Section::Paragraph { content } => assert_content(
            &md,
            *content,
            &[
                Expect::Italic(text("foo")),
                Expect::Text(" bar "),
                Expect::Italic(text("baz")),
            ],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_bold_underscore_no_intraword() {
    let md = MarkdownFile::parse("foo__bar__baz");
    match &md.sections[0] {
        Section::Paragraph { content } => {
            assert_content(&md, *content, &text("foo__bar__baz"));
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_emphasis_star_after_punctuation() {
    let md = MarkdownFile::parse("(*foo*)");
    match &md.sections[0] {
        Section::Paragraph { content } => assert_content(
            &md,
            *content,
            &[
                Expect::Text("("),
                Expect::Italic(text("foo")),
                Expect::Text(")"),
            ],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_emphasis_underscore_after_punctuation() {
    let md = MarkdownFile::parse("(_foo_)");
    match &md.sections[0] {
        Section::Paragraph { content } => assert_content(
            &md,
            *content,
            &[
                Expect::Text("("),
                Expect::Italic(text("foo")),
                Expect::Text(")"),
            ],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_emphasis_not_opened_by_whitespace_after() {
    let md = MarkdownFile::parse("a * not emphasis * b");
    match &md.sections[0] {
        Section::Paragraph { content } => {
            assert_content(&md, *content, &text("a * not emphasis * b"));
        }
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn test_bold_star_intraword() {
    let md = MarkdownFile::parse("foo**bar**baz");
    match &md.sections[0] {
        Section::Paragraph { content } => assert_content(
            &md,
            *content,
            &[
                Expect::Text("foo"),
                Expect::Bold(text("bar")),
                Expect::Text("baz"),
            ],
        ),
        other => panic!("expected paragraph, got {other:?}"),
    }
}
