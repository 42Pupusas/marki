use std::borrow::Cow;

use crate::section::OrderedListDelimiter;
use crate::{normalize, Inline, MarkdownFile, Section};

fn text(s: &str) -> Vec<Inline<'_>> {
    vec![Inline::Text(s)]
}

#[test]
fn test_heading() {
    let md = MarkdownFile::parse("# Hello\n## World");
    assert_eq!(
        md.sections,
        vec![
            Section::Heading {
                level: 1,
                content: text("Hello")
            },
            Section::Heading {
                level: 2,
                content: text("World")
            },
        ]
    );
}

#[test]
fn test_paragraph() {
    let md = MarkdownFile::parse("This is a paragraph.\nWith two lines.");
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: vec![
                Inline::Text("This is a paragraph."),
                Inline::SoftBreak,
                Inline::Text("With two lines."),
            ]
        }]
    );
}

#[test]
fn test_code_block() {
    let md = MarkdownFile::parse("```rust\nfn main() {}\n```");
    assert_eq!(
        md.sections,
        vec![Section::CodeBlock {
            language: Some("rust"),
            code: "fn main() {}",
        }]
    );
}

#[test]
fn test_code_block_no_language() {
    let md = MarkdownFile::parse("```\nhello\n```");
    assert_eq!(
        md.sections,
        vec![Section::CodeBlock {
            language: None,
            code: "hello",
        }]
    );
}

#[test]
fn test_unordered_list() {
    let md = MarkdownFile::parse("- one\n- two\n- three");
    assert_eq!(
        md.sections,
        vec![Section::UnorderedList {
            items: vec![text("one"), text("two"), text("three")],
        }]
    );
}

#[test]
fn test_unordered_list_plus() {
    let md = MarkdownFile::parse("+ one\n+ two\n+ three");
    assert_eq!(
        md.sections,
        vec![Section::UnorderedList {
            items: vec![text("one"), text("two"), text("three")],
        }]
    );
}

#[test]
fn test_ordered_list() {
    let md = MarkdownFile::parse("1. first\n2. second\n3. third");
    assert_eq!(
        md.sections,
        vec![Section::OrderedList {
            start: 1,
            delimiter: OrderedListDelimiter::Dot,
            items: vec![text("first"), text("second"), text("third")],
        }]
    );
}

#[test]
fn test_blockquote() {
    let md = MarkdownFile::parse("> line one\n> line two");
    assert_eq!(
        md.sections,
        vec![Section::Blockquote {
            content: vec![
                Inline::Text("line one"),
                Inline::Text("\n"),
                Inline::Text("line two"),
            ],
        }]
    );
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
    assert_eq!(
        md.sections,
        vec![
            Section::Heading {
                level: 1,
                content: text("Title")
            },
            Section::Paragraph {
                content: text("Some text.")
            },
            Section::UnorderedList {
                items: vec![text("a"), text("b")]
            },
            Section::Blockquote {
                content: text("quote")
            },
            Section::HorizontalRule,
            Section::CodeBlock {
                language: None,
                code: "code"
            },
        ]
    );
}

#[test]
fn test_heading_without_blank_line() {
    let md = MarkdownFile::parse("some text\n# Heading");
    assert_eq!(
        md.sections,
        vec![
            Section::Paragraph {
                content: text("some text")
            },
            Section::Heading {
                level: 1,
                content: text("Heading")
            },
        ]
    );
}

#[test]
fn test_bold() {
    let md = MarkdownFile::parse("This is **bold** text");
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: vec![
                Inline::Text("This is "),
                Inline::Bold(vec![Inline::Text("bold")]),
                Inline::Text(" text"),
            ],
        }]
    );
}

#[test]
fn test_italic() {
    let md = MarkdownFile::parse("This is *italic* text");
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: vec![
                Inline::Text("This is "),
                Inline::Italic(vec![Inline::Text("italic")]),
                Inline::Text(" text"),
            ],
        }]
    );
}

#[test]
fn test_bold_underscore() {
    let md = MarkdownFile::parse("__bold__");
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: vec![Inline::Bold(vec![Inline::Text("bold")])],
        }]
    );
}

#[test]
fn test_italic_underscore() {
    let md = MarkdownFile::parse("_italic_");
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: vec![Inline::Italic(vec![Inline::Text("italic")])],
        }]
    );
}

#[test]
fn test_link() {
    let md = MarkdownFile::parse("Click [here](https://example.com) now");
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: vec![
                Inline::Text("Click "),
                Inline::Link {
                    text: vec![Inline::Text("here")],
                    url: "https://example.com",
                    title: None,
                },
                Inline::Text(" now"),
            ],
        }]
    );
}

#[test]
fn test_image() {
    let md = MarkdownFile::parse("![alt text](image.png)");
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: vec![Inline::Image {
                alt: "alt text",
                url: "image.png",
                title: None,
            }],
        }]
    );
}

#[test]
fn test_bold_inside_link() {
    let md = MarkdownFile::parse("[**bold link**](url)");
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: vec![Inline::Link {
                text: vec![Inline::Bold(vec![Inline::Text("bold link")])],
                url: "url",
                title: None,
            }],
        }]
    );
}

#[test]
fn test_inline_in_heading() {
    let md = MarkdownFile::parse("# A **bold** heading");
    assert_eq!(
        md.sections,
        vec![Section::Heading {
            level: 1,
            content: vec![
                Inline::Text("A "),
                Inline::Bold(vec![Inline::Text("bold")]),
                Inline::Text(" heading"),
            ],
        }]
    );
}

#[test]
fn test_inline_in_list() {
    let md = MarkdownFile::parse("- *italic item*\n- **bold item**");
    assert_eq!(
        md.sections,
        vec![Section::UnorderedList {
            items: vec![
                vec![Inline::Italic(vec![Inline::Text("italic item")])],
                vec![Inline::Bold(vec![Inline::Text("bold item")])],
            ],
        }]
    );
}

#[test]
fn test_parse_readme_file() {
    let content = std::fs::read_to_string("README.md").unwrap();
    let md = MarkdownFile::parse(&content);
    assert_eq!(
        md.sections,
        vec![
            Section::Heading {
                level: 1,
                content: text("marki")
            },
            Section::Paragraph {
                content: vec![Inline::Text(
                    "A zero-copy Markdown parser for Rust. Parses markdown strings into structured sections and inline elements, borrowing directly from the input with no intermediate allocations for text content."
                ),],
            },
            Section::Heading {
                level: 2,
                content: text("Features")
            },
            Section::UnorderedList {
                items: vec![
                    text("Zero-copy parsing \u{2014} all text slices borrow from the input"),
                    vec![
                        Inline::Text("Type-safe representation of markdown elements via "),
                        Inline::Code("SpecialChar"),
                        Inline::Text(", "),
                        Inline::Code("Section"),
                        Inline::Text(", and "),
                        Inline::Code("Inline"),
                    ],
                    text("Fold-based state machine for single-pass block-level parsing"),
                ],
            },
            Section::Heading {
                level: 2,
                content: text("Supported Sections")
            },
            Section::UnorderedList {
                items: vec![
                    text("Headings (levels 1-6)"),
                    text("Paragraphs (with multi-line continuation)"),
                    text("Code blocks (fenced with backticks, optional language)"),
                    vec![
                        Inline::Text("Unordered lists ("),
                        Inline::Code("-"),
                        Inline::Text(", "),
                        Inline::Code("*"),
                        Inline::Text(", or "),
                        Inline::Code("+"),
                        Inline::Text(" markers)"),
                    ],
                    text("Ordered lists (with preserved start number)"),
                    text("Blockquotes"),
                    vec![
                        Inline::Text("Horizontal rules ("),
                        Inline::Code("---"),
                        Inline::Text(", "),
                        Inline::Code("***"),
                        Inline::Text(", "),
                        Inline::Code("___"),
                        Inline::Text(")"),
                    ],
                ],
            },
            Section::Heading {
                level: 2,
                content: text("Inline Formatting")
            },
            Section::UnorderedList {
                items: vec![
                    vec![
                        Inline::Bold(vec![Inline::Text("Bold")]),
                        Inline::Text(" text ("),
                        Inline::Code("**"),
                        Inline::Text(" or "),
                        Inline::Code("__"),
                        Inline::Text(")"),
                    ],
                    vec![
                        Inline::Italic(vec![Inline::Text("Italic")]),
                        Inline::Text(" text ("),
                        Inline::Code("*"),
                        Inline::Text(" or "),
                        Inline::Code("_"),
                        Inline::Text(")"),
                    ],
                    vec![
                        Inline::Code("Code"),
                        Inline::Text(" spans (backtick-delimited, CommonMark space-stripping)"),
                    ],
                    vec![Inline::Link {
                        text: vec![Inline::Text("Links")],
                        url: "https://example.com",
                        title: None,
                    }],
                    vec![Inline::Image {
                        alt: "Images",
                        url: "image.png",
                        title: None,
                    }],
                    text("Backslash escapes"),
                ],
            },
            Section::Heading {
                level: 2,
                content: text("Usage")
            },
            Section::CodeBlock {
                language: Some("rust"),
                code: "use marki::MarkdownFile;\n\nlet md = MarkdownFile::parse(\"# Hello\\n\\nWorld\");\nfor section in &md.sections {\n    println!(\"{section:?}\");\n}",
            },
            Section::Heading {
                level: 2,
                content: text("CRLF Support")
            },
            Section::Paragraph {
                content: vec![
                    Inline::Text("The parser operates on LF ("),
                    Inline::Code("\\n"),
                    Inline::Text(") line endings. For CRLF ("),
                    Inline::Code("\\r\\n"),
                    Inline::Text(") input, call "),
                    Inline::Code("normalize"),
                    Inline::Text(
                        " before parsing \u{2014} it returns the input borrowed when no "
                    ),
                    Inline::Code("\\r"),
                    Inline::Text(" is present (zero-cost), or an owned copy with "),
                    Inline::Code("\\r"),
                    Inline::Text(" stripped:"),
                ],
            },
            Section::CodeBlock {
                language: Some("rust"),
                code: "use marki::{normalize, MarkdownFile};\n\nlet normalized = normalize(input);\nlet md = MarkdownFile::parse(&normalized);",
            },
            Section::Heading {
                level: 2,
                content: text("Known Limitations")
            },
            Section::UnorderedList {
                items: vec![
                    text("List items are single-line only (no continuation with indentation)"),
                    vec![
                        Inline::Text("Emphasis cannot span across blockquote lines ("),
                        Inline::Code("> **bold\\n> continues**"),
                        Inline::Text(" is not recognized)"),
                    ],
                    vec![
                        Inline::Text("For CRLF ("),
                        Inline::Code("\\r\\n"),
                        Inline::Text(") input, call "),
                        Inline::Code("marki::normalize"),
                        Inline::Text(" before parsing"),
                    ],
                ],
            },
        ]
    );
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
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: vec![
                Inline::Text("line one"),
                Inline::SoftBreak,
                Inline::Text("line two"),
            ],
        }]
    );
}

#[test]
fn test_crlf_code_block() {
    let input = normalize("```rust\r\nfn main() {}\r\nlet x = 1;\r\n```\r\n");
    let md = MarkdownFile::parse(&input);
    assert_eq!(
        md.sections,
        vec![Section::CodeBlock {
            language: Some("rust"),
            code: "fn main() {}\nlet x = 1;",
        }]
    );
}

#[test]
fn test_crlf_mixed_document() {
    let input = normalize("# Title\r\n\r\nSome text.\r\n\r\n- a\r\n- b\r\n");
    let md = MarkdownFile::parse(&input);
    assert_eq!(
        md.sections,
        vec![
            Section::Heading {
                level: 1,
                content: text("Title")
            },
            Section::Paragraph {
                content: text("Some text.")
            },
            Section::UnorderedList {
                items: vec![text("a"), text("b")]
            },
        ]
    );
}

#[test]
fn test_emphasis_backslash_space_no_close() {
    // Backslash before space is NOT an escape (space isn't ASCII punctuation).
    // The space before closing `*` prevents it from being a valid closer,
    // so no emphasis is produced — the whole thing is literal text.
    let md = MarkdownFile::parse(r"*test\ *");
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: vec![Inline::Text(r"*test\ *")],
        }]
    );
}

#[test]
fn test_backslash_escape_punctuation() {
    // Backslash before punctuation: backslash consumed, punctuation is literal
    let md = MarkdownFile::parse(r"hello \*world\*");
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: vec![
                Inline::Text("hello "),
                Inline::Text("*world"),
                Inline::Text("*"),
            ],
        }]
    );
}

#[test]
fn test_backslash_escape_non_punctuation() {
    // Backslash before non-punctuation: backslash kept as literal text
    let md = MarkdownFile::parse(r"hello \n world");
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: vec![Inline::Text(r"hello \n world")],
        }]
    );
}

// ── Ordered list `)` delimiter ──────────────────────────────────

#[test]
fn test_ordered_list_paren_delimiter() {
    let md = MarkdownFile::parse("1) first\n2) second");
    assert_eq!(
        md.sections,
        vec![Section::OrderedList {
            start: 1,
            delimiter: OrderedListDelimiter::Paren,
            items: vec![text("first"), text("second")],
        }]
    );
}

#[test]
fn test_ordered_list_different_delimiters_split() {
    // Dot and paren delimiters produce separate lists.
    let md = MarkdownFile::parse("1. dot\n2) paren");
    assert_eq!(
        md.sections,
        vec![
            Section::OrderedList {
                start: 1,
                delimiter: OrderedListDelimiter::Dot,
                items: vec![text("dot")],
            },
            Section::OrderedList {
                start: 2,
                delimiter: OrderedListDelimiter::Paren,
                items: vec![text("paren")],
            },
        ]
    );
}

#[test]
fn test_ordered_list_paren_custom_start() {
    let md = MarkdownFile::parse("5) fifth\n6) sixth");
    assert_eq!(
        md.sections,
        vec![Section::OrderedList {
            start: 5,
            delimiter: OrderedListDelimiter::Paren,
            items: vec![text("fifth"), text("sixth")],
        }]
    );
}

// ── ATX heading closing # ───────────────────────────────────────

#[test]
fn test_heading_closing_hashes() {
    let md = MarkdownFile::parse("# Heading #");
    assert_eq!(
        md.sections,
        vec![Section::Heading {
            level: 1,
            content: text("Heading"),
        }]
    );
}

#[test]
fn test_heading_closing_multiple_hashes() {
    let md = MarkdownFile::parse("## Heading ##");
    assert_eq!(
        md.sections,
        vec![Section::Heading {
            level: 2,
            content: text("Heading"),
        }]
    );
}

#[test]
fn test_heading_closing_mismatched_hashes() {
    // Closing count doesn't need to match opening — still stripped.
    let md = MarkdownFile::parse("# Heading ####");
    assert_eq!(
        md.sections,
        vec![Section::Heading {
            level: 1,
            content: text("Heading"),
        }]
    );
}

#[test]
fn test_heading_hash_no_space_not_stripped() {
    // No space before trailing #, so it's part of the content.
    let md = MarkdownFile::parse("# Heading#");
    assert_eq!(
        md.sections,
        vec![Section::Heading {
            level: 1,
            content: text("Heading#"),
        }]
    );
}

#[test]
fn test_heading_only_hashes() {
    // `# ###` → empty heading (all trailing # stripped).
    let md = MarkdownFile::parse("# ###");
    assert_eq!(
        md.sections,
        vec![Section::Heading {
            level: 1,
            content: vec![],
        }]
    );
}

// ── Soft line breaks ────────────────────────────────────────────

#[test]
fn test_soft_break_in_paragraph() {
    let md = MarkdownFile::parse("line one\nline two\nline three");
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: vec![
                Inline::Text("line one"),
                Inline::SoftBreak,
                Inline::Text("line two"),
                Inline::SoftBreak,
                Inline::Text("line three"),
            ],
        }]
    );
}

#[test]
fn test_soft_break_single_trailing_space() {
    // One trailing space is NOT a hard break, still a soft break.
    let md = MarkdownFile::parse("line one \nline two");
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: vec![
                Inline::Text("line one "),
                Inline::SoftBreak,
                Inline::Text("line two"),
            ],
        }]
    );
}

// ── Hard line breaks ────────────────────────────────────────────

#[test]
fn test_hard_break_two_trailing_spaces() {
    let md = MarkdownFile::parse("line one  \nline two");
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: vec![
                Inline::Text("line one"),
                Inline::HardBreak,
                Inline::Text("line two"),
            ],
        }]
    );
}

#[test]
fn test_hard_break_many_trailing_spaces() {
    let md = MarkdownFile::parse("line one     \nline two");
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: vec![
                Inline::Text("line one"),
                Inline::HardBreak,
                Inline::Text("line two"),
            ],
        }]
    );
}

#[test]
fn test_hard_break_trailing_backslash() {
    let md = MarkdownFile::parse("line one\\\nline two");
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: vec![
                Inline::Text("line one"),
                Inline::HardBreak,
                Inline::Text("line two"),
            ],
        }]
    );
}

#[test]
fn test_hard_break_with_inline() {
    let md = MarkdownFile::parse("**bold**  \nnext line");
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: vec![
                Inline::Bold(vec![Inline::Text("bold")]),
                Inline::HardBreak,
                Inline::Text("next line"),
            ],
        }]
    );
}

// ── Blockquote lazy continuation ────────────────────────────────

#[test]
fn test_blockquote_lazy_continuation() {
    let md = MarkdownFile::parse("> line one\ncontinuation");
    assert_eq!(
        md.sections,
        vec![Section::Blockquote {
            content: vec![
                Inline::Text("line one"),
                Inline::Text("\n"),
                Inline::Text("continuation"),
            ],
        }]
    );
}

#[test]
fn test_blockquote_lazy_multiple_lines() {
    let md = MarkdownFile::parse("> first\nsecond\nthird");
    assert_eq!(
        md.sections,
        vec![Section::Blockquote {
            content: vec![
                Inline::Text("first"),
                Inline::Text("\n"),
                Inline::Text("second"),
                Inline::Text("\n"),
                Inline::Text("third"),
            ],
        }]
    );
}

#[test]
fn test_blockquote_lazy_stops_at_heading() {
    let md = MarkdownFile::parse("> quoted\n# Heading");
    assert_eq!(
        md.sections,
        vec![
            Section::Blockquote {
                content: text("quoted"),
            },
            Section::Heading {
                level: 1,
                content: text("Heading"),
            },
        ]
    );
}

#[test]
fn test_blockquote_lazy_stops_at_hr() {
    let md = MarkdownFile::parse("> quoted\n---");
    assert_eq!(
        md.sections,
        vec![
            Section::Blockquote {
                content: text("quoted"),
            },
            Section::HorizontalRule,
        ]
    );
}

#[test]
fn test_blockquote_lazy_stops_at_list() {
    let md = MarkdownFile::parse("> quoted\n- item");
    assert_eq!(
        md.sections,
        vec![
            Section::Blockquote {
                content: text("quoted"),
            },
            Section::UnorderedList {
                items: vec![text("item")],
            },
        ]
    );
}

#[test]
fn test_blockquote_lazy_stops_at_code_fence() {
    let md = MarkdownFile::parse("> quoted\n```\ncode\n```");
    assert_eq!(
        md.sections,
        vec![
            Section::Blockquote {
                content: text("quoted"),
            },
            Section::CodeBlock {
                language: None,
                code: "code",
            },
        ]
    );
}

// ── Link titles ─────────────────────────────────────────────────

#[test]
fn test_link_with_double_quote_title() {
    let md = MarkdownFile::parse(r#"[text](url "a title")"#);
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: vec![Inline::Link {
                text: vec![Inline::Text("text")],
                url: "url",
                title: Some("a title"),
            }],
        }]
    );
}

#[test]
fn test_link_with_single_quote_title() {
    let md = MarkdownFile::parse("[text](url 'a title')");
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: vec![Inline::Link {
                text: vec![Inline::Text("text")],
                url: "url",
                title: Some("a title"),
            }],
        }]
    );
}

#[test]
fn test_link_with_paren_title() {
    let md = MarkdownFile::parse("[text](url (a title))");
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: vec![Inline::Link {
                text: vec![Inline::Text("text")],
                url: "url",
                title: Some("a title"),
            }],
        }]
    );
}

#[test]
fn test_link_no_title() {
    let md = MarkdownFile::parse("[text](url)");
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: vec![Inline::Link {
                text: vec![Inline::Text("text")],
                url: "url",
                title: None,
            }],
        }]
    );
}

#[test]
fn test_image_with_title() {
    let md = MarkdownFile::parse(r#"![alt](img.png "photo")"#);
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: vec![Inline::Image {
                alt: "alt",
                url: "img.png",
                title: Some("photo"),
            }],
        }]
    );
}

// ── Emphasis flanking rules ─────────────────────────────────────

#[test]
fn test_emphasis_star_intraword() {
    // * can open/close intraword.
    let md = MarkdownFile::parse("foo*bar*baz");
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: vec![
                Inline::Text("foo"),
                Inline::Italic(vec![Inline::Text("bar")]),
                Inline::Text("baz"),
            ],
        }]
    );
}

#[test]
fn test_emphasis_underscore_no_intraword() {
    // _ cannot open/close intraword emphasis.
    let md = MarkdownFile::parse("foo_bar_baz");
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: text("foo_bar_baz"),
        }]
    );
}

#[test]
fn test_emphasis_underscore_word_boundaries() {
    // _ works at word boundaries.
    let md = MarkdownFile::parse("_foo_ bar _baz_");
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: vec![
                Inline::Italic(vec![Inline::Text("foo")]),
                Inline::Text(" bar "),
                Inline::Italic(vec![Inline::Text("baz")]),
            ],
        }]
    );
}

#[test]
fn test_bold_underscore_no_intraword() {
    let md = MarkdownFile::parse("foo__bar__baz");
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: text("foo__bar__baz"),
        }]
    );
}

#[test]
fn test_emphasis_star_after_punctuation() {
    // * can open after punctuation.
    let md = MarkdownFile::parse("(*foo*)");
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: vec![
                Inline::Text("("),
                Inline::Italic(vec![Inline::Text("foo")]),
                Inline::Text(")"),
            ],
        }]
    );
}

#[test]
fn test_emphasis_underscore_after_punctuation() {
    // _ can open after punctuation.
    let md = MarkdownFile::parse("(_foo_)");
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: vec![
                Inline::Text("("),
                Inline::Italic(vec![Inline::Text("foo")]),
                Inline::Text(")"),
            ],
        }]
    );
}

#[test]
fn test_emphasis_not_opened_by_whitespace_after() {
    // Delimiter followed by whitespace cannot open.
    let md = MarkdownFile::parse("a * not emphasis * b");
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: text("a * not emphasis * b"),
        }]
    );
}

#[test]
fn test_bold_star_intraword() {
    let md = MarkdownFile::parse("foo**bar**baz");
    assert_eq!(
        md.sections,
        vec![Section::Paragraph {
            content: vec![
                Inline::Text("foo"),
                Inline::Bold(vec![Inline::Text("bar")]),
                Inline::Text("baz"),
            ],
        }]
    );
}
