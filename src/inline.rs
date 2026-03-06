use crate::SpecialChar;

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
