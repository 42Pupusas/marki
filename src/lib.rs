mod inline;
mod section;
mod special_char;

use std::borrow::Cow;

pub use inline::Inline;
pub use section::Section;
pub use special_char::SpecialChar;

/// Normalize line endings for parsing. Returns the input borrowed if it
/// contains no carriage returns, or an owned copy with `\r` stripped otherwise.
///
/// Use this before [`MarkdownFile::parse`] when the input may contain CRLF
/// line endings:
///
/// ```
/// let input = "# Hello\r\nWorld";
/// let normalized = marki::normalize(input);
/// let md = marki::MarkdownFile::parse(&normalized);
/// ```
#[must_use]
pub fn normalize(input: &str) -> Cow<'_, str> {
    if input.as_bytes().contains(&b'\r') {
        Cow::Owned(input.replace('\r', ""))
    } else {
        Cow::Borrowed(input)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownFile<'src> {
    pub sections: Vec<Section<'src>>,
}

enum Accumulator<'src> {
    Empty,
    InCodeBlock {
        language: Option<&'src str>,
        content: Option<&'src str>,
        fence_len: usize,
    },
    InBlockquote {
        lines: Vec<&'src str>,
    },
    InUnorderedList {
        marker: SpecialChar,
        items: Vec<&'src str>,
    },
    InOrderedList {
        start: u32,
        items: Vec<&'src str>,
    },
    InParagraph {
        content: &'src str,
    },
}

impl<'src> Accumulator<'src> {
    fn flush(self) -> Option<Section<'src>> {
        match self {
            Self::Empty => None,
            Self::InCodeBlock {
                language, content, ..
            } => Some(Section::CodeBlock {
                language,
                code: content.unwrap_or(""),
            }),
            Self::InBlockquote { lines } => {
                let mut content = Vec::with_capacity(lines.len() * 3);
                for (i, line) in lines.iter().enumerate() {
                    if i > 0 {
                        content.push(Inline::Text("\n"));
                    }
                    Inline::parse_into(line, &mut content);
                }
                Some(Section::Blockquote { content })
            }
            Self::InUnorderedList { items, .. } => Some(Section::UnorderedList {
                items: items.into_iter().map(Inline::parse).collect(),
            }),
            Self::InOrderedList { start, items } => Some(Section::OrderedList {
                start,
                items: items.into_iter().map(Inline::parse).collect(),
            }),
            Self::InParagraph { content } => Some(Section::Paragraph {
                content: Inline::parse(content),
            }),
        }
    }

    fn flush_into(self, sections: &mut Vec<Section<'src>>) {
        if let Some(section) = self.flush() {
            sections.push(section);
        }
    }
}

/// Create a `Vec` with pre-allocated capacity and a single initial element.
fn vec_with_first<T>(first: T) -> Vec<T> {
    let mut v = Vec::with_capacity(4);
    v.push(first);
    v
}

impl<'src> MarkdownFile<'src> {
    #[must_use]
    pub fn parse(input: &'src str) -> Self {
        // Rough heuristic: ~50 bytes per section on average.
        let mut sections = Vec::with_capacity(input.len() / 50 + 1);
        let mut acc = Accumulator::Empty;
        for line in input.lines() {
            acc = Self::fold_line(input, &mut sections, acc, line);
        }
        acc.flush_into(&mut sections);
        Self { sections }
    }

    fn fold_line(
        input: &'src str,
        sections: &mut Vec<Section<'src>>,
        acc: Accumulator<'src>,
        line: &'src str,
    ) -> Accumulator<'src> {
        if let Accumulator::InCodeBlock {
            language,
            content,
            fence_len,
        } = acc
        {
            return Self::fold_code_block(input, sections, language, content, fence_len, line);
        }

        if line.trim().is_empty() {
            acc.flush_into(sections);
            return Accumulator::Empty;
        }

        let fence_len = Self::code_fence_len(line);
        if fence_len > 0 {
            let language = Self::extract_code_language(line, fence_len);
            acc.flush_into(sections);
            return Accumulator::InCodeBlock {
                language,
                content: None,
                fence_len,
            };
        }

        if let Some(section) = Self::try_parse_heading(line) {
            acc.flush_into(sections);
            sections.push(section);
            return Accumulator::Empty;
        }

        Self::fold_block_element(input, sections, acc, line)
    }

    #[inline]
    fn fold_code_block(
        input: &'src str,
        sections: &mut Vec<Section<'src>>,
        language: Option<&'src str>,
        content: Option<&'src str>,
        fence_len: usize,
        line: &'src str,
    ) -> Accumulator<'src> {
        if Self::code_fence_len(line) >= fence_len {
            sections.push(Section::CodeBlock {
                language,
                code: content.unwrap_or(""),
            });
            return Accumulator::Empty;
        }
        let content = content.map_or(line, |existing| {
            Self::merge_slices(input, existing, line).unwrap_or_else(|| {
                debug_assert!(
                    false,
                    "merge_slices failed in code block: slices not from same base"
                );
                existing
            })
        });
        Accumulator::InCodeBlock {
            language,
            content: Some(content),
            fence_len,
        }
    }

    /// Returns true if the first byte of `line` could start a block-level
    /// element (blockquote, list marker, or HR character). Used to fast-path
    /// paragraph continuations.
    const fn could_start_block(first: u8) -> bool {
        matches!(
            SpecialChar::from_byte(first),
            Some(
                SpecialChar::GreaterThan
                    | SpecialChar::Dash
                    | SpecialChar::Asterisk
                    | SpecialChar::Plus
                    | SpecialChar::Underscore
            )
        ) || first.is_ascii_digit()
    }

    #[inline]
    fn fold_block_element(
        input: &'src str,
        sections: &mut Vec<Section<'src>>,
        acc: Accumulator<'src>,
        line: &'src str,
    ) -> Accumulator<'src> {
        // Fast-path: if we're in a paragraph and the line can't start a block
        // element, skip all the block-level checks and extend the paragraph.
        if let Accumulator::InParagraph { .. } = acc
            && let Some(&first) = line.as_bytes().first()
            && !Self::could_start_block(first)
        {
            return Self::fold_paragraph(input, sections, acc, line);
        }

        if line.as_bytes().first().copied() == Some(SpecialChar::GreaterThan as u8) {
            let rest = &line[1..];
            let content = rest.strip_prefix(' ').unwrap_or(rest);
            if let Accumulator::InBlockquote { mut lines } = acc {
                lines.push(content);
                return Accumulator::InBlockquote { lines };
            }
            acc.flush_into(sections);
            return Accumulator::InBlockquote {
                lines: vec_with_first(content),
            };
        }

        if Self::is_horizontal_rule(line) {
            acc.flush_into(sections);
            sections.push(Section::HorizontalRule);
            return Accumulator::Empty;
        }

        if let Some((marker, item)) = Self::try_parse_unordered_item(line) {
            if let Accumulator::InUnorderedList {
                marker: m,
                mut items,
            } = acc
            {
                if m == marker {
                    items.push(item);
                    return Accumulator::InUnorderedList { marker, items };
                }
                Accumulator::InUnorderedList { marker: m, items }.flush_into(sections);
                return Accumulator::InUnorderedList {
                    marker,
                    items: vec_with_first(item),
                };
            }
            acc.flush_into(sections);
            return Accumulator::InUnorderedList {
                marker,
                items: vec_with_first(item),
            };
        }

        if let Some((num, item)) = Self::try_parse_ordered_item(line) {
            if let Accumulator::InOrderedList { start, mut items } = acc {
                items.push(item);
                return Accumulator::InOrderedList { start, items };
            }
            acc.flush_into(sections);
            return Accumulator::InOrderedList {
                start: num,
                items: vec_with_first(item),
            };
        }

        Self::fold_paragraph(input, sections, acc, line)
    }

    #[inline]
    fn fold_paragraph(
        input: &'src str,
        sections: &mut Vec<Section<'src>>,
        acc: Accumulator<'src>,
        line: &'src str,
    ) -> Accumulator<'src> {
        if let Accumulator::InParagraph { content } = acc {
            return Self::merge_slices(input, content, line).map_or_else(
                || {
                    sections.push(Section::Paragraph {
                        content: Inline::parse(content),
                    });
                    Accumulator::InParagraph { content: line }
                },
                |merged| Accumulator::InParagraph { content: merged },
            );
        }
        acc.flush_into(sections);
        Accumulator::InParagraph { content: line }
    }

    /// Returns the fence length (number of backticks) if the line is a valid
    /// code fence, or 0 if it is not. A valid fence has 3+ backticks with no
    /// backticks in the info string.
    fn code_fence_len(line: &str) -> usize {
        let bytes = line.as_bytes();
        // Quick reject: first non-whitespace byte must be a backtick.
        let first = bytes
            .iter()
            .position(|b| !b.is_ascii_whitespace())
            .unwrap_or(0);
        if bytes.get(first).copied() != Some(SpecialChar::Backtick as u8) {
            return 0;
        }
        // Count backticks directly from the offset we already found.
        let len = bytes[first..]
            .iter()
            .take_while(|&&b| b == SpecialChar::Backtick)
            .count();
        if len >= 3 && !bytes[first + len..].contains(&(SpecialChar::Backtick as u8)) {
            len
        } else {
            0
        }
    }

    fn extract_code_language(line: &str, fence_len: usize) -> Option<&str> {
        let trimmed = line.trim_start();
        let after = trimmed[fence_len..].trim();
        if after.is_empty() { None } else { Some(after) }
    }

    #[allow(clippy::cast_possible_truncation)]
    fn try_parse_heading(line: &str) -> Option<Section<'_>> {
        let level = SpecialChar::Hash.count_leading(line);
        if (1..=6).contains(&level) && line.as_bytes().get(level) == Some(&b' ') {
            Some(Section::Heading {
                level: level as u8,
                content: Inline::parse(line[level..].trim()),
            })
        } else {
            None
        }
    }

    fn is_horizontal_rule(line: &str) -> bool {
        let bytes = line.as_bytes();
        let first = bytes.iter().find(|b| !b.is_ascii_whitespace());
        let Some(&first_byte) = first else {
            return false;
        };
        let Some(rule_char) = SpecialChar::from_byte(first_byte).filter(|sc| sc.is_rule_char())
        else {
            return false;
        };
        let mut count = 0u32;
        for &b in bytes {
            if b == rule_char {
                count += 1;
            } else if !b.is_ascii_whitespace() {
                return false;
            }
        }
        count >= 3
    }

    fn try_parse_unordered_item(line: &str) -> Option<(SpecialChar, &str)> {
        let first = SpecialChar::from_byte(*line.as_bytes().first()?)?;
        if !first.is_list_char() {
            return None;
        }
        let rest = line.get(1..)?;
        rest.strip_prefix(' ').map(|item| (first, item))
    }

    fn try_parse_ordered_item(line: &str) -> Option<(u32, &str)> {
        let bytes = line.as_bytes();
        let mut num: u32 = 0;
        let mut digits = 0usize;
        for &b in bytes {
            if b.is_ascii_digit() {
                digits += 1;
                if digits > 9 {
                    return None;
                }
                num = num * 10 + u32::from(b - b'0');
            } else {
                break;
            }
        }
        if digits == 0 {
            return None;
        }
        // Expect ". " after the digits, then non-empty item text.
        if bytes.get(digits).copied() != Some(b'.') || bytes.get(digits + 1).copied() != Some(b' ')
        {
            return None;
        }
        let rest = line.get(digits + 2..)?;
        if rest.is_empty() {
            return None;
        }
        Some((num, rest))
    }

    /// Merge two subslices of `base` into one contiguous slice spanning from the
    /// start of `a` to the end of `b`.
    fn merge_slices(base: &'src str, a: &str, b: &str) -> Option<&'src str> {
        let base_start = base.as_ptr() as usize;
        let a_start = a.as_ptr() as usize;
        let b_end = b.as_ptr() as usize + b.len();

        if a_start < base_start || b_end > base_start + base.len() || b_end < a_start {
            return None;
        }

        base.get((a_start - base_start)..(b_end - base_start))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                content: text("This is a paragraph.\nWith two lines.")
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
                        }],
                        vec![Inline::Image {
                            alt: "Images",
                            url: "image.png",
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
                content: text("line one\nline two"),
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
    fn test_emphasis_close_after_escaped_space() {
        // Backslash-escaped space before closing delimiter should still close
        let md = MarkdownFile::parse(r"*test\ *");
        assert_eq!(
            md.sections,
            vec![Section::Paragraph {
                content: vec![Inline::Italic(vec![
                    Inline::Text("test"),
                    Inline::Text(" "),
                ])],
            }]
        );
    }
}
