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
pub enum Section {
    Heading { level: u8, text: String },
    Paragraph { text: String },
    CodeBlock { language: Option<String>, code: String },
    UnorderedList { items: Vec<String> },
    OrderedList { items: Vec<String> },
    Blockquote { text: String },
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
                text: lines.join("\n"),
            }),
            Self::InUnorderedList { items, .. } => Some(Section::UnorderedList {
                items: items.into_iter().map(String::from).collect(),
            }),
            Self::InOrderedList { items } => Some(Section::OrderedList {
                items: items.into_iter().map(String::from).collect(),
            }),
            Self::InParagraph { lines } => Some(Section::Paragraph {
                text: lines.join("\n"),
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
        // Inside a code block, only a closing fence exits
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

        // Blank line flushes current accumulator
        if line.trim().is_empty() {
            return FoldResult::new(Accumulator::Empty).flush_prior(acc);
        }

        // Code fence opens a new code block
        if Self::is_code_fence(line) {
            let language = Self::extract_code_language(line);
            return FoldResult::new(Accumulator::InCodeBlock {
                language,
                lines: Vec::new(),
            })
            .flush_prior(acc);
        }

        // Heading
        if let Some(section) = Self::try_parse_heading(line) {
            return FoldResult::new(Accumulator::Empty)
                .flush_prior(acc)
                .emit(section);
        }

        // Horizontal rule
        if Self::is_horizontal_rule(line) {
            return FoldResult::new(Accumulator::Empty)
                .flush_prior(acc)
                .emit(Section::HorizontalRule);
        }

        // Blockquote
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

        // Unordered list
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

        // Ordered list
        if let Some(item) = Self::try_parse_ordered_item(line) {
            if let Accumulator::InOrderedList { mut items } = acc {
                items.push(item);
                return FoldResult::new(Accumulator::InOrderedList { items });
            }
            return FoldResult::new(Accumulator::InOrderedList { items: vec![item] })
                .flush_prior(acc);
        }

        // Paragraph
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
                text: line[level..].trim().to_string(),
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

    #[test]
    fn test_heading() {
        let md: MarkdownFile = "# Hello\n## World".parse().unwrap();
        assert_eq!(
            md.sections,
            vec![
                Section::Heading { level: 1, text: "Hello".into() },
                Section::Heading { level: 2, text: "World".into() },
            ]
        );
    }

    #[test]
    fn test_paragraph() {
        let md: MarkdownFile = "This is a paragraph.\nWith two lines.".parse().unwrap();
        assert_eq!(
            md.sections,
            vec![Section::Paragraph {
                text: "This is a paragraph.\nWith two lines.".into()
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
                items: vec!["one".into(), "two".into(), "three".into()],
            }]
        );
    }

    #[test]
    fn test_ordered_list() {
        let md: MarkdownFile = "1. first\n2. second\n3. third".parse().unwrap();
        assert_eq!(
            md.sections,
            vec![Section::OrderedList {
                items: vec!["first".into(), "second".into(), "third".into()],
            }]
        );
    }

    #[test]
    fn test_blockquote() {
        let md: MarkdownFile = "> line one\n> line two".parse().unwrap();
        assert_eq!(
            md.sections,
            vec![Section::Blockquote {
                text: "line one\nline two".into(),
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
                Section::Heading { level: 1, text: "Title".into() },
                Section::Paragraph { text: "Some text.".into() },
                Section::UnorderedList { items: vec!["a".into(), "b".into()] },
                Section::Blockquote { text: "quote".into() },
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
                Section::Paragraph { text: "some text".into() },
                Section::Heading { level: 1, text: "Heading".into() },
            ]
        );
    }
}
