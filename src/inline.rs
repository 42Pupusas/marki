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
    Code(&'src str),
}

impl<'src> From<&'src str> for Inline<'src> {
    fn from(s: &'src str) -> Self {
        Self::Text(s)
    }
}

/// Lookup table: true for bytes that can start an inline element.
static SPECIAL: [bool; 256] = {
    let mut table = [false; 256];
    table[SpecialChar::Asterisk as u8 as usize] = true;
    table[SpecialChar::Underscore as u8 as usize] = true;
    table[SpecialChar::OpenBracket as u8 as usize] = true;
    table[SpecialChar::ExclamationMark as u8 as usize] = true;
    table[SpecialChar::Backslash as u8 as usize] = true;
    table[SpecialChar::Backtick as u8 as usize] = true;
    table
};

/// What emphasis types remain possible for a given delimiter character.
#[derive(Clone, Copy)]
enum DelimiterAvail {
    /// Both bold and italic are still possible.
    Both,
    /// Bold failed; only italic can be attempted.
    ItalicOnly,
    /// Italic failed; only bold can be attempted.
    BoldOnly,
    /// Neither bold nor italic can succeed.
    None,
}

impl DelimiterAvail {
    const fn can_bold(self) -> bool {
        matches!(self, Self::Both | Self::BoldOnly)
    }

    const fn can_italic(self) -> bool {
        matches!(self, Self::Both | Self::ItalicOnly)
    }

    const fn bold_failed(&mut self) {
        *self = match *self {
            Self::Both => Self::ItalicOnly,
            Self::BoldOnly => Self::None,
            other => other,
        };
    }

    const fn italic_failed(&mut self) {
        *self = match *self {
            Self::Both => Self::BoldOnly,
            Self::ItalicOnly => Self::None,
            other => other,
        };
    }

    const fn from_count(count: usize) -> Self {
        match count {
            0 | 1 => Self::None,
            2 | 3 => Self::ItalicOnly,
            _ => Self::Both,
        }
    }
}

/// Tracks delimiter availability per character type, avoiding O(n²)
/// re-scanning in both top-level and recursive parse calls.
struct EmphasisState {
    star: DelimiterAvail,
    under: DelimiterAvail,
}

impl EmphasisState {
    fn from_bytes(bytes: &[u8]) -> Self {
        let mut stars: u8 = 0;
        let mut unders: u8 = 0;
        for &b in bytes {
            if b == SpecialChar::Asterisk {
                stars += 1;
                if stars >= 4 && unders >= 4 {
                    break;
                }
            } else if b == SpecialChar::Underscore {
                unders += 1;
                if stars >= 4 && unders >= 4 {
                    break;
                }
            }
        }
        Self {
            star: DelimiterAvail::from_count(stars as usize),
            under: DelimiterAvail::from_count(unders as usize),
        }
    }

    const fn avail_mut(&mut self, is_star: bool) -> &mut DelimiterAvail {
        if is_star { &mut self.star } else { &mut self.under }
    }
}

impl<'src> Inline<'src> {
    #[must_use]
    pub fn parse(input: &'src str) -> Vec<Self> {
        let bytes = input.as_bytes();
        let emph = EmphasisState::from_bytes(bytes);
        Self::parse_with_emph(input, bytes, emph)
    }

    fn parse_inner(input: &'src str) -> Vec<Self> {
        let bytes = input.as_bytes();
        let emph = EmphasisState {
            star: DelimiterAvail::Both,
            under: DelimiterAvail::Both,
        };
        Self::parse_with_emph(input, bytes, emph)
    }

    fn parse_with_emph(input: &'src str, bytes: &[u8], emph: EmphasisState) -> Vec<Self> {
        let mut result = Vec::with_capacity(4);
        Self::parse_into_with_emph(input, bytes, emph, &mut result);
        result
    }

    /// Push parsed inline elements directly into `out`, avoiding temporary Vec
    /// allocations when building blockquotes or list items.
    pub fn parse_into(input: &'src str, out: &mut Vec<Self>) {
        let bytes = input.as_bytes();
        let emph = EmphasisState::from_bytes(bytes);
        Self::parse_into_with_emph(input, bytes, emph, out);
    }

    fn parse_into_with_emph(
        input: &'src str,
        bytes: &[u8],
        mut emph: EmphasisState,
        result: &mut Vec<Self>,
    ) {
        let mut plain_start = 0;
        let mut i = 0;

        while let Some(&b) = bytes.get(i) {
            // Fast-skip non-special bytes via lookup table
            if !SPECIAL[b as usize] {
                i += 1;
                continue;
            }

            // Backslash escape: skip the backslash, include the escaped char as text
            if b == SpecialChar::Backslash && i + 1 < bytes.len() {
                if plain_start < i {
                    result.push(Self::Text(&input[plain_start..i]));
                }
                plain_start = i + 1;
                i += 2;
                continue;
            }

            // Inline code: `code` or ``code``
            if b == SpecialChar::Backtick
                && let Some((code, end)) = Self::try_parse_inline_code(input, bytes, i)
            {
                if plain_start < i {
                    result.push(Self::Text(&input[plain_start..i]));
                }
                result.push(Self::Code(code));
                plain_start = end;
                i = end;
                continue;
            }

            // Image: ![alt](url)
            if b == SpecialChar::ExclamationMark
                && bytes.get(i + 1).copied() == Some(SpecialChar::OpenBracket as u8)
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
                    text: Self::parse_inner(text_str),
                    url,
                });
                plain_start = end;
                i = end;
                continue;
            }

            // Bold/Italic: ** __ * _
            if let Some((elem, end)) = Self::try_parse_emphasis(input, bytes, i, b, &mut emph) {
                if plain_start < i {
                    result.push(Self::Text(&input[plain_start..i]));
                }
                result.push(elem);
                plain_start = end;
                i = end;
                continue;
            }

            i += 1;
        }

        if plain_start < input.len() {
            result.push(Self::Text(&input[plain_start..]));
        }
    }

    #[inline]
    fn try_parse_emphasis(
        input: &'src str,
        bytes: &[u8],
        i: usize,
        b: u8,
        emph: &mut EmphasisState,
    ) -> Option<(Self, usize)> {
        let is_emphasis = matches!(
            SpecialChar::from_byte(b),
            Some(sc) if sc.is_emphasis_char()
        );
        if !is_emphasis {
            return None;
        }

        let is_star = b == SpecialChar::Asterisk;
        let avail = emph.avail_mut(is_star);

        // Bold: ** or __
        if avail.can_bold() && bytes.get(i + 1) == Some(&b) {
            if let Some((inner, end)) = Self::try_parse_delimited(input, bytes, i, b, 2) {
                return Some((Self::Bold(Self::parse_inner(inner)), end));
            }
            avail.bold_failed();
        }

        // Italic: * or _
        if avail.can_italic() {
            if let Some((inner, end)) = Self::try_parse_delimited(input, bytes, i, b, 1) {
                return Some((Self::Italic(Self::parse_inner(inner)), end));
            }
            avail.italic_failed();
        }

        None
    }

    /// Find the position of a matching closing delimiter, handling backslash
    /// escapes and nested pairs.
    fn find_matching_close(
        bytes: &[u8],
        start: usize,
        open: SpecialChar,
        close: SpecialChar,
    ) -> Option<usize> {
        let mut depth = 0u32;
        let mut j = start;
        while let Some(&b) = bytes.get(j) {
            if b == SpecialChar::Backslash {
                j += 2;
                continue;
            }
            if b == open {
                depth += 1;
            } else if b == close {
                if depth == 0 {
                    return Some(j);
                }
                depth -= 1;
            }
            j += 1;
        }
        None
    }

    fn try_parse_bracket_paren(
        input: &'src str,
        bytes: &[u8],
        start: usize,
    ) -> Option<(&'src str, &'src str, usize)> {
        if bytes.get(start).copied() != Some(SpecialChar::OpenBracket as u8) {
            return None;
        }

        let bracket_start = start + 1;
        let bracket_end = Self::find_matching_close(
            bytes,
            bracket_start,
            SpecialChar::OpenBracket,
            SpecialChar::CloseBracket,
        )?;

        let paren_pos = bracket_end + 1;
        if bytes.get(paren_pos).copied() != Some(SpecialChar::OpenParen as u8) {
            return None;
        }

        let paren_start = paren_pos + 1;
        let paren_end = Self::find_matching_close(
            bytes,
            paren_start,
            SpecialChar::OpenParen,
            SpecialChar::CloseParen,
        )?;

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
        let inner_start = start + count;
        let &first_inner = bytes.get(inner_start)?;

        if first_inner == marker || first_inner.is_ascii_whitespace() {
            return None;
        }

        let mut i = inner_start;
        while let Some(&b) = bytes.get(i) {
            if b == SpecialChar::Backslash {
                i += 2;
                continue;
            }

            if b != marker {
                i += 1;
                continue;
            }

            // Found a marker byte — check for a valid closing run.
            let all_match = (1..count).all(|j| bytes.get(i + j) == Some(&marker));
            if all_match
                && i > inner_start
                && bytes.get(i - 1).is_some_and(|prev| {
                    // Raw whitespace blocks closing, but escaped whitespace does not.
                    !prev.is_ascii_whitespace()
                        || (i >= inner_start + 2
                            && bytes.get(i - 2).is_some_and(|&b| b == SpecialChar::Backslash))
                })
            {
                return Some((input.get(inner_start..i)?, i + count));
            }
            i += 1;
        }

        None
    }

    /// Parse inline code: `` `code` `` or ``` ``code`` ```.
    /// The opening and closing backtick sequences must have the same length.
    /// Content is taken verbatim (no backslash escaping inside code spans).
    fn try_parse_inline_code(
        input: &'src str,
        bytes: &[u8],
        start: usize,
    ) -> Option<(&'src str, usize)> {
        let backtick_count = bytes
            .get(start..)?
            .iter()
            .take_while(|&&b| b == SpecialChar::Backtick)
            .count();
        if backtick_count == 0 {
            return None;
        }

        let content_start = start + backtick_count;
        let mut i = content_start;
        while i < bytes.len() {
            // Find next backtick
            let remaining = bytes.get(i..)?;
            let offset = remaining.iter().position(|&b| b == SpecialChar::Backtick)?;
            i += offset;

            // Count consecutive backticks
            let close_count = bytes
                .get(i..)?
                .iter()
                .take_while(|&&b| b == SpecialChar::Backtick)
                .count();

            if close_count == backtick_count {
                // Strip single leading/trailing space per CommonMark
                let mut cs = content_start;
                let mut ce = i;
                if ce - cs >= 2
                    && bytes.get(cs) == Some(&b' ')
                    && bytes.get(ce - 1) == Some(&b' ')
                {
                    cs += 1;
                    ce -= 1;
                }
                return Some((input.get(cs..ce)?, i + close_count));
            }
            i += close_count;
        }

        None
    }
}
