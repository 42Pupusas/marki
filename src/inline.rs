use crate::SpecialChar;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline<'src> {
    Text(&'src str),
    Bold(Vec<Inline<'src>>),
    Italic(Vec<Inline<'src>>),
    Link {
        text: Vec<Inline<'src>>,
        url: &'src str,
    },
    Image {
        alt: &'src str,
        url: &'src str,
    },
}

impl<'src> From<&'src str> for Inline<'src> {
    fn from(s: &'src str) -> Self {
        Self::Text(s)
    }
}

fn memchr(needle: u8, haystack: &[u8]) -> Option<usize> {
    haystack.iter().position(|&b| b == needle)
}

impl<'src> Inline<'src> {
    #[must_use]
    pub fn parse(input: &'src str) -> Vec<Self> {
        let bytes = input.as_bytes();
        let mut result = Vec::new();
        let mut plain_start = 0;
        let mut i = 0;

        while i < bytes.len() {
            let b = bytes[i];

            // Image: ![alt](url)
            if b == SpecialChar::ExclamationMark.as_byte()
                && bytes.get(i + 1) == Some(&SpecialChar::OpenBracket.as_byte())
                && let Some((alt, url, end)) = Self::try_parse_bracket_paren(input, bytes, i + 1)
            {
                if plain_start < i {
                    result.push(Self::Text(&input[plain_start..i]));
                }
                result.push(Self::Image { alt, url });
                plain_start = end;
                i = end;
                continue;
            }

            // Link: [text](url)
            if b == SpecialChar::OpenBracket.as_byte()
                && let Some((text_str, url, end)) = Self::try_parse_bracket_paren(input, bytes, i)
            {
                if plain_start < i {
                    result.push(Self::Text(&input[plain_start..i]));
                }
                result.push(Self::Link {
                    text: Self::parse(text_str),
                    url,
                });
                plain_start = end;
                i = end;
                continue;
            }

            // Bold: ** or __
            if let Some(sc) = SpecialChar::from_byte(b)
                && sc.is_emphasis_char()
                && bytes.get(i + 1) == Some(&b)
                && let Some((inner, end)) = Self::try_parse_delimited(input, bytes, i, b, 2)
            {
                if plain_start < i {
                    result.push(Self::Text(&input[plain_start..i]));
                }
                result.push(Self::Bold(Self::parse(inner)));
                plain_start = end;
                i = end;
                continue;
            }

            // Italic: * or _
            if let Some(sc) = SpecialChar::from_byte(b)
                && sc.is_emphasis_char()
                && let Some((inner, end)) = Self::try_parse_delimited(input, bytes, i, b, 1)
            {
                if plain_start < i {
                    result.push(Self::Text(&input[plain_start..i]));
                }
                result.push(Self::Italic(Self::parse(inner)));
                plain_start = end;
                i = end;
                continue;
            }

            i += 1;
        }

        if plain_start < input.len() {
            result.push(Self::Text(&input[plain_start..]));
        }

        result
    }

    fn try_parse_bracket_paren(
        input: &'src str,
        bytes: &[u8],
        start: usize,
    ) -> Option<(&'src str, &'src str, usize)> {
        if bytes.get(start) != Some(&SpecialChar::OpenBracket.as_byte()) {
            return None;
        }

        let bracket_start = start + 1;
        let search_region = bytes.get(bracket_start..)?;
        let bracket_end =
            memchr(SpecialChar::CloseBracket.as_byte(), search_region)? + bracket_start;

        let paren_pos = bracket_end + 1;
        if bytes.get(paren_pos) != Some(&SpecialChar::OpenParen.as_byte()) {
            return None;
        }

        let paren_start = paren_pos + 1;
        let search_region = bytes.get(paren_start..)?;
        let paren_end = memchr(SpecialChar::CloseParen.as_byte(), search_region)? + paren_start;

        Some((
            input.get(bracket_start..bracket_end)?,
            input.get(paren_start..paren_end)?,
            paren_end + 1,
        ))
    }

    fn try_parse_delimited(
        input: &'src str,
        bytes: &[u8],
        start: usize,
        marker: u8,
        count: usize,
    ) -> Option<(&'src str, usize)> {
        for j in 0..count {
            if bytes.get(start + j) != Some(&marker) {
                return None;
            }
        }

        let inner_start = start + count;
        let &first_inner = bytes.get(inner_start)?;

        if first_inner == marker || first_inner.is_ascii_whitespace() {
            return None;
        }

        let mut i = inner_start;
        while i < bytes.len() {
            if bytes.get(i) == Some(&marker) {
                let all_match = (0..count).all(|j| bytes.get(i + j) == Some(&marker));
                if all_match
                    && i > inner_start
                    && bytes.get(i - 1).is_some_and(|b| !b.is_ascii_whitespace())
                {
                    return Some((input.get(inner_start..i)?, i + count));
                }
            }
            i += 1;
        }

        None
    }
}
