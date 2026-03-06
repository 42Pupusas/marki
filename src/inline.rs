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
    table[b'*' as usize] = true;
    table[b'_' as usize] = true;
    table[b'[' as usize] = true;
    table[b'!' as usize] = true;
    table[b'\\' as usize] = true;
    table[b'`' as usize] = true;
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
        let star_count = bytes.iter().filter(|&&b| b == b'*').take(4).count();
        let under_count = bytes.iter().filter(|&&b| b == b'_').take(4).count();
        Self {
            star: DelimiterAvail::from_count(star_count),
            under: DelimiterAvail::from_count(under_count),
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
        let mut result = Vec::new();
        let mut plain_start = 0;
        let mut i = 0;
        let mut emph = EmphasisState::from_bytes(bytes);

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

        result
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
                return Some((Self::Bold(Self::parse(inner)), end));
            }
            avail.bold_failed();
        }

        // Italic: * or _
        if avail.can_italic() {
            if let Some((inner, end)) = Self::try_parse_delimited(input, bytes, i, b, 1) {
                return Some((Self::Italic(Self::parse(inner)), end));
            }
            avail.italic_failed();
        }

        None
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
        let mut depth = 0u32;
        let mut bracket_end = None;
        let mut j = bracket_start;
        while j < bytes.len() {
            match bytes.get(j).and_then(|&b| SpecialChar::from_byte(b)) {
                Some(SpecialChar::Backslash) => {
                    j += 2;
                    continue;
                }
                Some(SpecialChar::OpenBracket) => depth += 1,
                Some(SpecialChar::CloseBracket) => {
                    if depth == 0 {
                        bracket_end = Some(j);
                        break;
                    }
                    depth -= 1;
                }
                _ => {}
            }
            j += 1;
        }
        let bracket_end = bracket_end?;

        let paren_pos = bracket_end + 1;
        if bytes.get(paren_pos) != Some(SpecialChar::OpenParen.as_ref()) {
            return None;
        }

        let paren_start = paren_pos + 1;
        let mut depth = 0u32;
        let mut paren_end = None;
        let mut j = paren_start;
        while j < bytes.len() {
            match bytes.get(j).and_then(|&b| SpecialChar::from_byte(b)) {
                Some(SpecialChar::Backslash) => {
                    j += 2;
                    continue;
                }
                Some(SpecialChar::OpenParen) => depth += 1,
                Some(SpecialChar::CloseParen) => {
                    if depth == 0 {
                        paren_end = Some(j);
                        break;
                    }
                    depth -= 1;
                }
                _ => {}
            }
            j += 1;
        }
        let paren_end = paren_end?;

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
            // Skip backslash-escaped bytes
            if bytes.get(i) == Some(SpecialChar::Backslash.as_ref()) {
                i += 2;
                continue;
            }

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
