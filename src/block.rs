use crate::section::{OrderedListDelimiter, Section};
use crate::special_char::SpecialChar;
use crate::{Inline, MarkdownFile};

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
        delimiter: OrderedListDelimiter,
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
            Self::InOrderedList {
                start,
                delimiter,
                items,
            } => Some(Section::OrderedList {
                start,
                delimiter,
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

        let bytes = line.as_bytes();
        let first = bytes.first().copied().unwrap_or(b' ');

        if first.is_ascii_whitespace() && line.trim().is_empty() {
            acc.flush_into(sections);
            return Accumulator::Empty;
        }

        // Code fences (CommonMark §4.5): 3+ backticks open a fenced code block.
        // Guard: only worth checking if the first byte is a backtick or
        // whitespace (indented fence).
        if first == SpecialChar::Backtick.byte() || first.is_ascii_whitespace() {
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
        }

        // ATX headings (CommonMark §4.2): only if line starts with '#'.
        if first == SpecialChar::Hash.byte() {
            if let Some(section) = Self::try_parse_heading(line) {
                acc.flush_into(sections);
                sections.push(section);
                return Accumulator::Empty;
            }
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
        // Guard: only check for closing fence if first byte could be a backtick
        // or whitespace (indented fence). Avoids calling code_fence_len on
        // every code block content line.
        let first = line.as_bytes().first().copied().unwrap_or(0);
        if (first == SpecialChar::Backtick.byte() || first.is_ascii_whitespace())
            && Self::code_fence_len(line) >= fence_len
        {
            sections.push(Section::CodeBlock {
                language,
                code: content.unwrap_or(""),
            });
            return Accumulator::Empty;
        }
        let content = content.map_or(line, |existing| {
            Self::merge_slices(input, existing, line)
                .expect("merge_slices failed in code block: slices not from same input")
        });
        Accumulator::InCodeBlock {
            language,
            content: Some(content),
            fence_len,
        }
    }

    /// Lookup table: true for bytes that could start a block-level element
    /// (blockquote, list marker, HR character, or digit for ordered lists).
    const COULD_START_BLOCK: [bool; 256] = {
        let mut table = [false; 256];
        table[SpecialChar::GreaterThan.byte() as usize] = true;
        table[SpecialChar::Dash.byte() as usize] = true;
        table[SpecialChar::Asterisk.byte() as usize] = true;
        table[SpecialChar::Plus.byte() as usize] = true;
        table[SpecialChar::Underscore.byte() as usize] = true;
        let mut d = b'0';
        while d <= b'9' {
            table[d as usize] = true;
            d += 1;
        }
        table
    };

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
            && !Self::COULD_START_BLOCK[first as usize]
        {
            return Self::fold_paragraph(input, sections, acc, line);
        }

        if line.as_bytes().first() == SpecialChar::GreaterThan {
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

        // Blockquote lazy continuation (CommonMark §5.1): a non-blank line
        // that doesn't start a new block-level construct continues the
        // current blockquote.
        let acc = if let Accumulator::InBlockquote { mut lines } = acc {
            if !Self::is_horizontal_rule(line)
                && Self::try_parse_heading(line).is_none()
                && Self::code_fence_len(line) == 0
                && Self::try_parse_unordered_item(line).is_none()
                && Self::try_parse_ordered_item(line).is_none()
            {
                lines.push(line);
                return Accumulator::InBlockquote { lines };
            }
            // Line starts a new block — flush the blockquote and fall through.
            Accumulator::InBlockquote { lines }.flush_into(sections);
            Accumulator::Empty
        } else {
            acc
        };

        // Horizontal rules (CommonMark §4.1): three or more -, *, or _
        // characters (optionally with spaces) on a line by themselves.
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

        if let Some((num, delim, item)) = Self::try_parse_ordered_item(line) {
            if let Accumulator::InOrderedList {
                start,
                delimiter,
                mut items,
            } = acc
            {
                if delimiter == delim {
                    items.push(item);
                    return Accumulator::InOrderedList {
                        start,
                        delimiter,
                        items,
                    };
                }
                Accumulator::InOrderedList {
                    start,
                    delimiter,
                    items,
                }
                .flush_into(sections);
                return Accumulator::InOrderedList {
                    start: num,
                    delimiter: delim,
                    items: vec_with_first(item),
                };
            }
            acc.flush_into(sections);
            return Accumulator::InOrderedList {
                start: num,
                delimiter: delim,
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
    /// code fence (`CommonMark` §4.5), or 0 if it is not. A valid fence has 3+
    /// backticks with no backticks in the info string.
    #[inline]
    fn code_fence_len(line: &str) -> usize {
        let bytes = line.as_bytes();
        // Skip leading whitespace.
        let mut first = 0;
        while first < bytes.len() && bytes[first].is_ascii_whitespace() {
            first += 1;
        }
        // Quick reject: first non-whitespace byte must be a backtick.
        let backtick = SpecialChar::Backtick.byte();
        if first >= bytes.len() || bytes[first] != backtick {
            return 0;
        }
        // Count backticks directly from the offset we already found.
        let mut end = first + 1;
        while end < bytes.len() && bytes[end] == backtick {
            end += 1;
        }
        let len = end - first;
        if len >= 3 && !bytes[end..].contains(&backtick) {
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

    /// Parse an ATX heading (`CommonMark` §4.2). Strips optional closing `#`
    /// sequences when preceded by whitespace.
    #[allow(clippy::cast_possible_truncation)]
    fn try_parse_heading(line: &str) -> Option<Section<'_>> {
        let level = SpecialChar::Hash.count_leading(line);
        if (1..=6).contains(&level) && line.as_bytes().get(level) == SpecialChar::Space {
            let text = line[level..].trim();
            // Strip optional closing # sequence per CommonMark §4.2:
            // trailing #s are removed only if preceded by whitespace (or they
            // are the entire content after the opening).
            let stripped = text.trim_end_matches('#');
            let text = if stripped.is_empty()
                || stripped
                    .as_bytes()
                    .last()
                    .is_some_and(|&b| b == SpecialChar::Space || b == SpecialChar::Tab)
            {
                stripped.trim_end()
            } else {
                text
            };
            Some(Section::Heading {
                level: level as u8,
                content: Inline::parse(text),
            })
        } else {
            None
        }
    }

    /// Check whether a line is a thematic break / horizontal rule
    /// (`CommonMark` §4.1): three or more matching `-`, `*`, or `_` characters,
    /// optionally separated by spaces, with nothing else on the line.
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

    fn try_parse_ordered_item(line: &str) -> Option<(u32, OrderedListDelimiter, &str)> {
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
        let delimiter = OrderedListDelimiter::from_byte(bytes.get(digits).copied()?)?;
        if bytes.get(digits + 1) != SpecialChar::Space {
            return None;
        }
        let rest = line.get(digits + 2..)?;
        if rest.is_empty() {
            return None;
        }
        Some((num, delimiter, rest))
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
