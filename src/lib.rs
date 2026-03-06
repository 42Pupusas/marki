mod inline;
mod section;
mod special_char;

pub use inline::Inline;
pub use section::Section;
pub use special_char::SpecialChar;

use std::fs;
use std::path::Path;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownFile {
    pub sections: Vec<Section>,
}

enum Accumulator<'a> {
    Empty,
    InCodeBlock {
        language: Option<&'a str>,
        lines: Vec<&'a str>,
    },
    InBlockquote {
        lines: Vec<&'a str>,
    },
    InUnorderedList {
        marker: SpecialChar,
        items: Vec<&'a str>,
    },
    InOrderedList {
        items: Vec<&'a str>,
    },
    InParagraph {
        lines: Vec<&'a str>,
    },
}

impl Accumulator<'_> {
    fn flush(self) -> Option<Section> {
        match self {
            Self::Empty => None,
            Self::InCodeBlock { language, lines } => Some(Section::CodeBlock {
                language: language.map(String::from),
                code: lines.join("\n"),
            }),
            Self::InBlockquote { lines } => Some(Section::Blockquote {
                content: Inline::parse(&lines.join("\n")),
            }),
            Self::InUnorderedList { items, .. } => Some(Section::UnorderedList {
                items: items.into_iter().map(Inline::parse).collect(),
            }),
            Self::InOrderedList { items } => Some(Section::OrderedList {
                items: items.into_iter().map(Inline::parse).collect(),
            }),
            Self::InParagraph { lines } => Some(Section::Paragraph {
                content: Inline::parse(&lines.join("\n")),
            }),
        }
    }
}

struct FoldResult<'a> {
    acc: Accumulator<'a>,
    emitted: Vec<Section>,
}

impl<'a> FoldResult<'a> {
    const fn new(acc: Accumulator<'a>) -> Self {
        Self {
            acc,
            emitted: Vec::new(),
        }
    }

    fn emit(mut self, section: Section) -> Self {
        self.emitted.push(section);
        self
    }

    fn flush_prior(mut self, prior: Accumulator<'a>) -> Self {
        if let Some(section) = prior.flush() {
            self.emitted.push(section);
        }
        self
    }
}

impl FromStr for MarkdownFile {
    type Err = std::convert::Infallible;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let (mut sections, final_acc) = input.lines().fold(
            (Vec::new(), Accumulator::Empty),
            |(mut sections, acc), line| {
                let result = Self::fold_line(acc, line);
                sections.extend(result.emitted);
                (sections, result.acc)
            },
        );

        if let Some(section) = final_acc.flush() {
            sections.push(section);
        }

        Ok(Self { sections })
    }
}

impl MarkdownFile {
    /// # Errors
    ///
    /// Returns an error if the file cannot be read.
    pub fn from_file(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let content = fs::read_to_string(path)?;
        Ok(match content.parse() {
            Ok(md) => md,
            Err(e) => match e {},
        })
    }

    fn fold_line<'a>(acc: Accumulator<'a>, line: &'a str) -> FoldResult<'a> {
        if let Accumulator::InCodeBlock { language, mut lines } = acc {
            if Self::is_code_fence(line) {
                let section = Section::CodeBlock {
                    language: language.map(String::from),
                    code: lines.join("\n"),
                };
                return FoldResult::new(Accumulator::Empty).emit(section);
            }
            lines.push(line);
            return FoldResult::new(Accumulator::InCodeBlock { language, lines });
        }

        if line.trim().is_empty() {
            return FoldResult::new(Accumulator::Empty).flush_prior(acc);
        }

        if Self::is_code_fence(line) {
            let language = Self::extract_code_language(line);
            return FoldResult::new(Accumulator::InCodeBlock {
                language,
                lines: Vec::new(),
            })
            .flush_prior(acc);
        }

        if let Some(section) = Self::try_parse_heading(line) {
            return FoldResult::new(Accumulator::Empty)
                .flush_prior(acc)
                .emit(section);
        }

        if Self::is_horizontal_rule(line) {
            return FoldResult::new(Accumulator::Empty)
                .flush_prior(acc)
                .emit(Section::HorizontalRule);
        }

        if line.starts_with(SpecialChar::GreaterThan.as_char()) {
            let content = line[1..].strip_prefix(' ').unwrap_or_else(|| &line[1..]);
            if let Accumulator::InBlockquote { mut lines } = acc {
                lines.push(content);
                return FoldResult::new(Accumulator::InBlockquote { lines });
            }
            return FoldResult::new(Accumulator::InBlockquote {
                lines: vec![content],
            })
            .flush_prior(acc);
        }

        if let Some((marker, item)) = Self::try_parse_unordered_item(line) {
            if let Accumulator::InUnorderedList {
                marker: m,
                mut items,
            } = acc
            {
                if m == marker {
                    items.push(item);
                    return FoldResult::new(Accumulator::InUnorderedList { marker, items });
                }
                return FoldResult::new(Accumulator::InUnorderedList {
                    marker,
                    items: vec![item],
                })
                .flush_prior(Accumulator::InUnorderedList { marker: m, items });
            }
            return FoldResult::new(Accumulator::InUnorderedList {
                marker,
                items: vec![item],
            })
            .flush_prior(acc);
        }

        if let Some(item) = Self::try_parse_ordered_item(line) {
            if let Accumulator::InOrderedList { mut items } = acc {
                items.push(item);
                return FoldResult::new(Accumulator::InOrderedList { items });
            }
            return FoldResult::new(Accumulator::InOrderedList { items: vec![item] })
                .flush_prior(acc);
        }

        if let Accumulator::InParagraph { mut lines } = acc {
            lines.push(line);
            return FoldResult::new(Accumulator::InParagraph { lines });
        }
        FoldResult::new(Accumulator::InParagraph { lines: vec![line] }).flush_prior(acc)
    }

    fn is_code_fence(line: &str) -> bool {
        SpecialChar::Backtick.count_leading(line.trim_start()) >= 3
    }

    fn extract_code_language(line: &str) -> Option<&str> {
        let after = line.trim_start().trim_start_matches('`').trim();
        if after.is_empty() { None } else { Some(after) }
    }

    #[allow(clippy::cast_possible_truncation)]
    fn try_parse_heading(line: &str) -> Option<Section> {
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
        let trimmed = line.trim();
        if trimmed.len() < 3 {
            return false;
        }
        let first = match trimmed.chars().next().and_then(SpecialChar::from_char) {
            Some(sc) if sc.is_rule_char() => sc,
            _ => return false,
        };
        let non_space: usize = trimmed
            .chars()
            .filter(|c| !c.is_whitespace())
            .take_while(|&c| c == first.as_char())
            .count();
        let total_non_space: usize = trimmed.chars().filter(|c| !c.is_whitespace()).count();
        non_space >= 3 && non_space == total_non_space
    }

    fn try_parse_unordered_item(line: &str) -> Option<(SpecialChar, &str)> {
        let first = line.chars().next().and_then(SpecialChar::from_char)?;
        if !first.is_list_char() {
            return None;
        }
        line.strip_prefix(first.as_char())
            .and_then(|rest| rest.strip_prefix(' '))
            .map(|item| (first, item))
    }

    fn try_parse_ordered_item(line: &str) -> Option<&str> {
        let (num_part, rest) = line.split_once(". ")?;
        if !num_part.is_empty() && num_part.chars().all(|c| c.is_ascii_digit()) && !rest.is_empty()
        {
            Some(rest)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Vec<Inline> {
        vec![Inline::Text(s.into())]
    }

    #[test]
    fn test_heading() {
        let md: MarkdownFile = "# Hello\n## World".parse().unwrap();
        assert_eq!(
            md.sections,
            vec![
                Section::Heading { level: 1, content: text("Hello") },
                Section::Heading { level: 2, content: text("World") },
            ]
        );
    }

    #[test]
    fn test_paragraph() {
        let md: MarkdownFile = "This is a paragraph.\nWith two lines.".parse().unwrap();
        assert_eq!(
            md.sections,
            vec![Section::Paragraph {
                content: text("This is a paragraph.\nWith two lines.")
            }]
        );
    }

    #[test]
    fn test_code_block() {
        let md: MarkdownFile = "```rust\nfn main() {}\n```".parse().unwrap();
        assert_eq!(
            md.sections,
            vec![Section::CodeBlock {
                language: Some("rust".into()),
                code: "fn main() {}".into(),
            }]
        );
    }

    #[test]
    fn test_code_block_no_language() {
        let md: MarkdownFile = "```\nhello\n```".parse().unwrap();
        assert_eq!(
            md.sections,
            vec![Section::CodeBlock {
                language: None,
                code: "hello".into(),
            }]
        );
    }

    #[test]
    fn test_unordered_list() {
        let md: MarkdownFile = "- one\n- two\n- three".parse().unwrap();
        assert_eq!(
            md.sections,
            vec![Section::UnorderedList {
                items: vec![text("one"), text("two"), text("three")],
            }]
        );
    }

    #[test]
    fn test_ordered_list() {
        let md: MarkdownFile = "1. first\n2. second\n3. third".parse().unwrap();
        assert_eq!(
            md.sections,
            vec![Section::OrderedList {
                items: vec![text("first"), text("second"), text("third")],
            }]
        );
    }

    #[test]
    fn test_blockquote() {
        let md: MarkdownFile = "> line one\n> line two".parse().unwrap();
        assert_eq!(
            md.sections,
            vec![Section::Blockquote {
                content: text("line one\nline two"),
            }]
        );
    }

    #[test]
    fn test_horizontal_rule() {
        let md: MarkdownFile = "---".parse().unwrap();
        assert_eq!(md.sections, vec![Section::HorizontalRule]);
    }

    #[test]
    fn test_mixed_document() {
        let md: MarkdownFile =
            "# Title\n\nSome text.\n\n- a\n- b\n\n> quote\n\n---\n\n```\ncode\n```"
                .parse()
                .unwrap();
        assert_eq!(
            md.sections,
            vec![
                Section::Heading { level: 1, content: text("Title") },
                Section::Paragraph { content: text("Some text.") },
                Section::UnorderedList { items: vec![text("a"), text("b")] },
                Section::Blockquote { content: text("quote") },
                Section::HorizontalRule,
                Section::CodeBlock { language: None, code: "code".into() },
            ]
        );
    }

    #[test]
    fn test_heading_without_blank_line() {
        let md: MarkdownFile = "some text\n# Heading".parse().unwrap();
        assert_eq!(
            md.sections,
            vec![
                Section::Paragraph { content: text("some text") },
                Section::Heading { level: 1, content: text("Heading") },
            ]
        );
    }

    #[test]
    fn test_bold() {
        let md: MarkdownFile = "This is **bold** text".parse().unwrap();
        assert_eq!(
            md.sections,
            vec![Section::Paragraph {
                content: vec![
                    Inline::Text("This is ".into()),
                    Inline::Bold(vec![Inline::Text("bold".into())]),
                    Inline::Text(" text".into()),
                ],
            }]
        );
    }

    #[test]
    fn test_italic() {
        let md: MarkdownFile = "This is *italic* text".parse().unwrap();
        assert_eq!(
            md.sections,
            vec![Section::Paragraph {
                content: vec![
                    Inline::Text("This is ".into()),
                    Inline::Italic(vec![Inline::Text("italic".into())]),
                    Inline::Text(" text".into()),
                ],
            }]
        );
    }

    #[test]
    fn test_bold_underscore() {
        let md: MarkdownFile = "__bold__".parse().unwrap();
        assert_eq!(
            md.sections,
            vec![Section::Paragraph {
                content: vec![Inline::Bold(vec![Inline::Text("bold".into())])],
            }]
        );
    }

    #[test]
    fn test_italic_underscore() {
        let md: MarkdownFile = "_italic_".parse().unwrap();
        assert_eq!(
            md.sections,
            vec![Section::Paragraph {
                content: vec![Inline::Italic(vec![Inline::Text("italic".into())])],
            }]
        );
    }

    #[test]
    fn test_link() {
        let md: MarkdownFile = "Click [here](https://example.com) now".parse().unwrap();
        assert_eq!(
            md.sections,
            vec![Section::Paragraph {
                content: vec![
                    Inline::Text("Click ".into()),
                    Inline::Link {
                        text: vec![Inline::Text("here".into())],
                        url: "https://example.com".into(),
                    },
                    Inline::Text(" now".into()),
                ],
            }]
        );
    }

    #[test]
    fn test_image() {
        let md: MarkdownFile = "![alt text](image.png)".parse().unwrap();
        assert_eq!(
            md.sections,
            vec![Section::Paragraph {
                content: vec![Inline::Image {
                    alt: "alt text".into(),
                    url: "image.png".into(),
                }],
            }]
        );
    }

    #[test]
    fn test_bold_inside_link() {
        let md: MarkdownFile = "[**bold link**](url)".parse().unwrap();
        assert_eq!(
            md.sections,
            vec![Section::Paragraph {
                content: vec![Inline::Link {
                    text: vec![Inline::Bold(vec![Inline::Text("bold link".into())])],
                    url: "url".into(),
                }],
            }]
        );
    }

    #[test]
    fn test_inline_in_heading() {
        let md: MarkdownFile = "# A **bold** heading".parse().unwrap();
        assert_eq!(
            md.sections,
            vec![Section::Heading {
                level: 1,
                content: vec![
                    Inline::Text("A ".into()),
                    Inline::Bold(vec![Inline::Text("bold".into())]),
                    Inline::Text(" heading".into()),
                ],
            }]
        );
    }

    #[test]
    fn test_inline_in_list() {
        let md: MarkdownFile = "- *italic item*\n- **bold item**".parse().unwrap();
        assert_eq!(
            md.sections,
            vec![Section::UnorderedList {
                items: vec![
                    vec![Inline::Italic(vec![Inline::Text("italic item".into())])],
                    vec![Inline::Bold(vec![Inline::Text("bold item".into())])],
                ],
            }]
        );
    }

    #[test]
    fn test_parse_readme_file() {
        let md = MarkdownFile::from_file("README.md").unwrap();
        assert_eq!(
            md.sections,
            vec![
                Section::Heading { level: 1, content: text("marki") },
                Section::Paragraph {
                    content: text("A simple Rust library for parsing markdown files into structured sections."),
                },
                Section::Heading { level: 2, content: text("Features") },
                Section::UnorderedList {
                    items: vec![
                        text("Parse markdown from strings or files"),
                        text("Type-safe representation of markdown elements"),
                        text("Zero-copy parsing with fold-based state machine"),
                    ],
                },
                Section::Heading { level: 2, content: text("Supported Sections") },
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
                Section::Heading { level: 2, content: text("Inline Formatting") },
                Section::UnorderedList {
                    items: vec![
                        vec![
                            Inline::Bold(vec![Inline::Text("Bold".into())]),
                            Inline::Text(" text".into()),
                        ],
                        vec![
                            Inline::Italic(vec![Inline::Text("Italic".into())]),
                            Inline::Text(" text".into()),
                        ],
                        vec![Inline::Link {
                            text: vec![Inline::Text("Links".into())],
                            url: "https://example.com".into(),
                        }],
                        vec![Inline::Image {
                            alt: "Images".into(),
                            url: "image.png".into(),
                        }],
                    ],
                },
                Section::Heading { level: 2, content: text("Usage") },
                Section::CodeBlock {
                    language: Some("rust".into()),
                    code: "use marki::MarkdownFile;\n\nlet md: MarkdownFile = \"# Hello\\n\\nWorld\".parse().unwrap();".into(),
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
