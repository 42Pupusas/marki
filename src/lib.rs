use std::fs;
use std::path::Path;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecialChar {
    Hash,
    Dash,
    Asterisk,
    Underscore,
    GreaterThan,
    Backtick,
    ExclamationMark,
    OpenBracket,
    CloseBracket,
    OpenParen,
    CloseParen,
}

impl SpecialChar {
    #[must_use]
    pub const fn from_char(c: char) -> Option<Self> {
        match c {
            '#' => Some(Self::Hash),
            '-' => Some(Self::Dash),
            '*' => Some(Self::Asterisk),
            '_' => Some(Self::Underscore),
            '>' => Some(Self::GreaterThan),
            '`' => Some(Self::Backtick),
            '!' => Some(Self::ExclamationMark),
            '[' => Some(Self::OpenBracket),
            ']' => Some(Self::CloseBracket),
            '(' => Some(Self::OpenParen),
            ')' => Some(Self::CloseParen),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_char(self) -> char {
        match self {
            Self::Hash => '#',
            Self::Dash => '-',
            Self::Asterisk => '*',
            Self::Underscore => '_',
            Self::GreaterThan => '>',
            Self::Backtick => '`',
            Self::ExclamationMark => '!',
            Self::OpenBracket => '[',
            Self::CloseBracket => ']',
            Self::OpenParen => '(',
            Self::CloseParen => ')',
        }
    }

    #[must_use]
    pub const fn is_rule_char(self) -> bool {
        matches!(self, Self::Dash | Self::Asterisk | Self::Underscore)
    }

    #[must_use]
    pub const fn is_list_char(self) -> bool {
        matches!(self, Self::Dash | Self::Asterisk)
    }

    #[must_use]
    pub const fn is_emphasis_char(self) -> bool {
        matches!(self, Self::Asterisk | Self::Underscore)
    }

    #[must_use]
    pub fn count_leading(self, s: &str) -> usize {
        s.chars().take_while(|&c| c == self.as_char()).count()
    }
}

impl std::fmt::Display for SpecialChar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_char())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    Bold(Vec<Inline>),
    Italic(Vec<Inline>),
    Link { text: Vec<Inline>, url: String },
    Image { alt: String, url: String },
}

impl From<&str> for Inline {
    fn from(s: &str) -> Self {
        Self::Text(s.to_string())
    }
}

impl Inline {
    #[must_use]
    pub fn parse(input: &str) -> Vec<Self> {
        let chars: Vec<char> = input.chars().collect();
        let mut result = Vec::new();
        let mut i = 0;
        let mut plain_start = 0;

        while i < chars.len() {
            // Image: ![alt](url)
            if chars[i] == SpecialChar::ExclamationMark.as_char()
                && chars.get(i + 1) == Some(&SpecialChar::OpenBracket.as_char())
                && let Some((alt, url, end)) = Self::try_parse_bracket_paren(&chars, i + 1)
            {
                Self::flush_plain(&chars, plain_start, i, &mut result);
                result.push(Self::Image { alt, url });
                i = end;
                plain_start = i;
                continue;
            }

            // Link: [text](url)
            if chars[i] == SpecialChar::OpenBracket.as_char()
                && let Some((text, url, end)) = Self::try_parse_bracket_paren(&chars, i)
            {
                Self::flush_plain(&chars, plain_start, i, &mut result);
                result.push(Self::Link {
                    text: Self::parse(&text),
                    url,
                });
                i = end;
                plain_start = i;
                continue;
            }

            // Bold: ** or __
            if let Some(sc) = SpecialChar::from_char(chars[i])
                && sc.is_emphasis_char()
                && chars.get(i + 1) == Some(&chars[i])
                && let Some((inner, end)) = Self::try_parse_delimited(&chars, i, chars[i], 2)
            {
                Self::flush_plain(&chars, plain_start, i, &mut result);
                result.push(Self::Bold(Self::parse(&inner)));
                i = end;
                plain_start = i;
                continue;
            }

            // Italic: * or _
            if let Some(sc) = SpecialChar::from_char(chars[i])
                && sc.is_emphasis_char()
                && let Some((inner, end)) = Self::try_parse_delimited(&chars, i, chars[i], 1)
            {
                Self::flush_plain(&chars, plain_start, i, &mut result);
                result.push(Self::Italic(Self::parse(&inner)));
                i = end;
                plain_start = i;
                continue;
            }

            i += 1;
        }

        Self::flush_plain(&chars, plain_start, chars.len(), &mut result);
        result
    }

    fn flush_plain(chars: &[char], start: usize, end: usize, result: &mut Vec<Self>) {
        if start < end {
            let text: String = chars[start..end].iter().collect();
            result.push(Self::Text(text));
        }
    }

    fn try_parse_bracket_paren(chars: &[char], start: usize) -> Option<(String, String, usize)> {
        if chars.get(start) != Some(&SpecialChar::OpenBracket.as_char()) {
            return None;
        }

        let mut i = start + 1;
        let bracket_start = i;

        while i < chars.len() && chars[i] != SpecialChar::CloseBracket.as_char() {
            i += 1;
        }
        if i >= chars.len() {
            return None;
        }
        let bracket_content: String = chars[bracket_start..i].iter().collect();
        i += 1;

        if chars.get(i) != Some(&SpecialChar::OpenParen.as_char()) {
            return None;
        }
        i += 1;
        let paren_start = i;

        while i < chars.len() && chars[i] != SpecialChar::CloseParen.as_char() {
            i += 1;
        }
        if i >= chars.len() {
            return None;
        }
        let paren_content: String = chars[paren_start..i].iter().collect();
        i += 1;

        Some((bracket_content, paren_content, i))
    }

    fn try_parse_delimited(
        chars: &[char],
        start: usize,
        marker: char,
        count: usize,
    ) -> Option<(String, usize)> {
        for j in 0..count {
            if chars.get(start + j) != Some(&marker) {
                return None;
            }
        }

        let inner_start = start + count;
        if inner_start >= chars.len() {
            return None;
        }

        if chars[inner_start] == marker || chars[inner_start].is_whitespace() {
            return None;
        }

        let mut i = inner_start;
        while i < chars.len() {
            if chars[i] == marker {
                let all_match = (0..count).all(|j| chars.get(i + j) == Some(&marker));
                if all_match && i > inner_start && !chars[i - 1].is_whitespace() {
                    let inner: String = chars[inner_start..i].iter().collect();
                    return Some((inner, i + count));
                }
            }
            i += 1;
        }

        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Section {
    Heading { level: u8, content: Vec<Inline> },
    Paragraph { content: Vec<Inline> },
    CodeBlock { language: Option<String>, code: String },
    UnorderedList { items: Vec<Vec<Inline>> },
    OrderedList { items: Vec<Vec<Inline>> },
    Blockquote { content: Vec<Inline> },
    HorizontalRule,
}

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
