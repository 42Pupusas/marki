mod inline;
mod section;
mod special_char;

pub use inline::Inline;
pub use section::Section;
pub use special_char::SpecialChar;

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
                let mut content = Vec::new();
                for (i, line) in lines.iter().enumerate() {
                    if i > 0 {
                        content.push(Inline::Text("\n"));
                    }
                    content.extend(Inline::parse(line));
                }
                Some(Section::Blockquote { content })
            }
            Self::InUnorderedList { items, .. } => Some(Section::UnorderedList {
                items: items.into_iter().map(Inline::parse).collect(),
            }),
            Self::InOrderedList { items } => Some(Section::OrderedList {
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

impl<'src> MarkdownFile<'src> {
    #[must_use]
    pub fn parse(input: &'src str) -> Self {
        let mut sections = Vec::new();
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

        if Self::is_horizontal_rule(line) {
            acc.flush_into(sections);
            sections.push(Section::HorizontalRule);
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

    #[inline]
    fn fold_block_element(
        input: &'src str,
        sections: &mut Vec<Section<'src>>,
        acc: Accumulator<'src>,
        line: &'src str,
    ) -> Accumulator<'src> {
        if line.as_bytes().first() == Some(SpecialChar::GreaterThan.as_ref()) {
            let rest = &line[1..];
            let content = rest.strip_prefix(' ').unwrap_or(rest);
            if let Accumulator::InBlockquote { mut lines } = acc {
                lines.push(content);
                return Accumulator::InBlockquote { lines };
            }
            acc.flush_into(sections);
            return Accumulator::InBlockquote {
                lines: vec![content],
            };
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
                    items: vec![item],
                };
            }
            acc.flush_into(sections);
            return Accumulator::InUnorderedList {
                marker,
                items: vec![item],
            };
        }

        if let Some(item) = Self::try_parse_ordered_item(line) {
            if let Accumulator::InOrderedList { mut items } = acc {
                items.push(item);
                return Accumulator::InOrderedList { items };
            }
            acc.flush_into(sections);
            return Accumulator::InOrderedList { items: vec![item] };
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
        let trimmed = line.trim_start();
        let len = SpecialChar::Backtick.count_leading(trimmed);
        if len >= 3 && !trimmed.as_bytes()[len..].contains(&b'`') {
            len
        } else {
            0
        }
    }

    fn extract_code_language(line: &str, fence_len: usize) -> Option<&str> {
        let trimmed = line.trim_start();
        let after = trimmed[fence_len..].trim();
        if after.is_empty() {
            None
        } else {
            Some(after)
        }
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
        let trimmed = line.trim().as_bytes();
        if trimmed.len() < 3 {
            return false;
        }
        let first = trimmed[0];
        if !matches!(first, b'-' | b'*' | b'_') {
            return false;
        }
        let mut count = 0usize;
        for &b in trimmed {
            if b == first {
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

    fn try_parse_ordered_item(line: &str) -> Option<&str> {
        let (num_part, rest) = line.split_once(". ")?;
        if !num_part.is_empty()
            && num_part.len() <= 9
            && num_part.as_bytes().iter().all(u8::is_ascii_digit)
            && !rest.is_empty()
        {
            Some(rest)
        } else {
            None
        }
    }

    /// Merge two subslices of `base` into one contiguous slice spanning from the
    /// start of `a` to the end of `b`.
    fn merge_slices(base: &'src str, a: &str, b: &str) -> Option<&'src str> {
        let base_start = base.as_ptr() as usize;
        let base_end = base_start.checked_add(base.len())?;

        let a_start = a.as_ptr() as usize;
        let b_start = b.as_ptr() as usize;
        let b_end = b_start.checked_add(b.len())?;

        if a_start < base_start || a_start.checked_add(a.len())? > base_end {
            return None;
        }
        if b_start < base_start || b_end > base_end {
            return None;
        }
        if b_start < a_start {
            return None;
        }

        let start = a_start - base_start;
        let end = b_end - base_start;
        base.get(start..end)
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
    fn test_ordered_list() {
        let md = MarkdownFile::parse("1. first\n2. second\n3. third");
        assert_eq!(
            md.sections,
            vec![Section::OrderedList {
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
                    content: text(
                        "A simple Rust library for parsing markdown files into structured sections."
                    ),
                },
                Section::Heading {
                    level: 2,
                    content: text("Features")
                },
                Section::UnorderedList {
                    items: vec![
                        text("Parse markdown from strings or files"),
                        text("Type-safe representation of markdown elements"),
                        text("Zero-copy parsing with fold-based state machine"),
                    ],
                },
                Section::Heading {
                    level: 2,
                    content: text("Supported Sections")
                },
                Section::UnorderedList {
                    items: vec![
                        text("Headings (levels 1-6)"),
                        text("Paragraphs"),
                        text("Code blocks (with optional language)"),
                        text("Unordered lists"),
                        text("Ordered lists"),
                        text("Blockquotes"),
                        text("Horizontal rules"),
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
                            Inline::Text(" text"),
                        ],
                        vec![
                            Inline::Italic(vec![Inline::Text("Italic")]),
                            Inline::Text(" text"),
                        ],
                        vec![Inline::Link {
                            text: vec![Inline::Text("Links")],
                            url: "https://example.com",
                        }],
                        vec![Inline::Image {
                            alt: "Images",
                            url: "image.png",
                        }],
                    ],
                },
                Section::Heading {
                    level: 2,
                    content: text("Usage")
                },
                Section::CodeBlock {
                    language: Some("rust"),
                    code: "use marki::MarkdownFile;\n\nlet md: MarkdownFile = \"# Hello\\n\\nWorld\".parse().unwrap();",
                },
                Section::OrderedList {
                    items: vec![
                        text("Parse a string"),
                        text("Read a file"),
                        text("Inspect sections"),
                    ],
                },
            ]
        );
    }
}
