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

/// Lookup table: true for bytes that can start an inline element.
static SPECIAL: [bool; 256] = {
    let mut table = [false; 256];
    table[b'*' as usize] = true;
    table[b'_' as usize] = true;
    table[b'[' as usize] = true;
    table[b'!' as usize] = true;
    table
};

impl<'src> Inline<'src> {
    #[must_use]
    pub fn parse(input: &'src str) -> Vec<Self> {
        let bytes = input.as_bytes();
        let mut result = Vec::new();
        let mut plain_start = 0;
        let mut i = 0;

        // Track failed delimiter scans to avoid O(n²) re-scanning.
        // Once we scan forward for a closing delimiter and find none,
        // no later position can find one either — skip future attempts.
        let mut no_close_bold_star = false;
        let mut no_close_bold_under = false;
        let mut no_close_italic_star = false;
        let mut no_close_italic_under = false;

        while i < bytes.len() {
            // Fast-skip non-special bytes via lookup table
            if !SPECIAL[bytes[i] as usize] {
                i += 1;
                continue;
            }

            let b = bytes[i];

            // Image: ![alt](url)
            if b == SpecialChar::ExclamationMark
                && bytes.get(i + 1) == Some(SpecialChar::OpenBracket.as_ref())
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
            if b == SpecialChar::OpenBracket
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

            let is_emphasis = matches!(
                SpecialChar::from_byte(b),
                Some(sc) if sc.is_emphasis_char()
            );

            // Bold: ** or __
            if is_emphasis
                && bytes.get(i + 1) == Some(&b)
                && !(b == SpecialChar::Asterisk && no_close_bold_star)
                && !(b == SpecialChar::Underscore && no_close_bold_under)
            {
                if let Some((inner, end)) = Self::try_parse_delimited(input, bytes, i, b, 2) {
                    if plain_start < i {
                        result.push(Self::Text(&input[plain_start..i]));
                    }
                    result.push(Self::Bold(Self::parse(inner)));
                    plain_start = end;
                    i = end;
                    continue;
                }
                if b == SpecialChar::Asterisk {
                    no_close_bold_star = true;
                } else {
                    no_close_bold_under = true;
                }
            }

            // Italic: * or _
            if is_emphasis
                && !(b == SpecialChar::Asterisk && no_close_italic_star)
                && !(b == SpecialChar::Underscore && no_close_italic_under)
            {
                if let Some((inner, end)) = Self::try_parse_delimited(input, bytes, i, b, 1) {
                    if plain_start < i {
                        result.push(Self::Text(&input[plain_start..i]));
                    }
                    result.push(Self::Italic(Self::parse(inner)));
                    plain_start = end;
                    i = end;
                    continue;
                }
                if b == SpecialChar::Asterisk {
                    no_close_italic_star = true;
                } else {
                    no_close_italic_under = true;
                }
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
        if bytes.get(start) != Some(SpecialChar::OpenBracket.as_ref()) {
            return None;
        }

        let bracket_start = start + 1;
        let close_bracket = *SpecialChar::CloseBracket.as_ref();
        let bracket_end =
            bytes.get(bracket_start..)?.iter().position(|&b| b == close_bracket)? + bracket_start;

        let paren_pos = bracket_end + 1;
        if bytes.get(paren_pos) != Some(SpecialChar::OpenParen.as_ref()) {
            return None;
        }

        let paren_start = paren_pos + 1;
        let close_paren = *SpecialChar::CloseParen.as_ref();
        let paren_end =
            bytes.get(paren_start..)?.iter().position(|&b| b == close_paren)? + paren_start;

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
        // Caller already verified opening markers exist; skip straight to inner content
        let inner_start = start + count;
        let &first_inner = bytes.get(inner_start)?;

        if first_inner == marker || first_inner.is_ascii_whitespace() {
            return None;
        }

        let mut i = inner_start;
        while i < bytes.len() {
            // Skip to next occurrence of the marker byte
            match bytes.get(i..)?.iter().position(|&b| b == marker) {
                Some(offset) => i += offset,
                None => return None,
            }

            let all_match = (0..count).all(|j| bytes.get(i + j) == Some(&marker));
            if all_match
                && i > inner_start
                && bytes.get(i - 1).is_some_and(|b| !b.is_ascii_whitespace())
            {
                return Some((input.get(inner_start..i)?, i + count));
            }
            i += 1;
        }

        None
    }
}
