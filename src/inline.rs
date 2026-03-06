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

struct InlineAccumulator {
    result: Vec<Inline>,
    plain_start: usize,
    skip_to: usize,
}

impl InlineAccumulator {
    const fn new() -> Self {
        Self {
            result: Vec::new(),
            plain_start: 0,
            skip_to: 0,
        }
    }

    fn flush_plain(mut self, input: &str, end: usize) -> Self {
        if self.plain_start < end {
            self.result
                .push(Inline::Text(input[self.plain_start..end].to_string()));
        }
        self
    }

    fn emit(mut self, input: &str, pos: usize, inline: Inline, skip_to: usize) -> Self {
        self = self.flush_plain(input, pos);
        self.result.push(inline);
        self.skip_to = skip_to;
        self.plain_start = skip_to;
        self
    }

    fn finish(self, input: &str) -> Vec<Inline> {
        let acc = self.flush_plain(input, input.len());
        acc.result
    }
}

impl Inline {
    #[must_use]
    pub fn parse(input: &str) -> Vec<Self> {
        let bytes = input.as_bytes();

        bytes
            .iter()
            .enumerate()
            .fold(InlineAccumulator::new(), |acc, (i, _)| {
                Self::fold_byte(input, bytes, acc, i)
            })
            .finish(input)
    }

    fn fold_byte(input: &str, bytes: &[u8], acc: InlineAccumulator, i: usize) -> InlineAccumulator {
        if i < acc.skip_to {
            return acc;
        }

        let Some(&b) = bytes.get(i) else {
            return acc;
        };

        // Image: ![alt](url)
        if b == SpecialChar::ExclamationMark.as_byte()
            && bytes.get(i + 1) == Some(&SpecialChar::OpenBracket.as_byte())
            && let Some((alt, url, end)) = Self::try_parse_bracket_paren(input, bytes, i + 1)
        {
            return acc.emit(input, i, Self::Image { alt, url }, end);
        }

        // Link: [text](url)
        if b == SpecialChar::OpenBracket.as_byte()
            && let Some((text, url, end)) = Self::try_parse_bracket_paren(input, bytes, i)
        {
            return acc.emit(
                input,
                i,
                Self::Link {
                    text: Self::parse(&text),
                    url,
                },
                end,
            );
        }

        // Bold: ** or __
        if let Some(sc) = SpecialChar::from_byte(b)
            && sc.is_emphasis_char()
            && bytes.get(i + 1) == Some(&b)
            && let Some((inner, end)) = Self::try_parse_delimited(input, bytes, i, b, 2)
        {
            return acc.emit(input, i, Self::Bold(Self::parse(&inner)), end);
        }

        // Italic: * or _
        if let Some(sc) = SpecialChar::from_byte(b)
            && sc.is_emphasis_char()
            && let Some((inner, end)) = Self::try_parse_delimited(input, bytes, i, b, 1)
        {
            return acc.emit(input, i, Self::Italic(Self::parse(&inner)), end);
        }

        acc
    }

    fn try_parse_bracket_paren(
        input: &str,
        bytes: &[u8],
        start: usize,
    ) -> Option<(String, String, usize)> {
        fn memchr(needle: u8, haystack: &[u8]) -> Option<usize> {
            haystack.iter().position(|&b| b == needle)
        }
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
            input.get(bracket_start..bracket_end)?.to_string(),
            input.get(paren_start..paren_end)?.to_string(),
            paren_end + 1,
        ))
    }

    fn try_parse_delimited(
        input: &str,
        bytes: &[u8],
        start: usize,
        marker: u8,
        count: usize,
    ) -> Option<(String, usize)> {
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
                    return Some((input.get(inner_start..i)?.to_string(), i + count));
                }
            }
            i += 1;
        }

        None
    }
}
