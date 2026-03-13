#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SpecialChar {
    Tab = b'\t',
    Newline = b'\n',
    CarriageReturn = b'\r',
    Space = b' ',
    ExclamationMark = b'!',
    DoubleQuote = b'"',
    Hash = b'#',
    SingleQuote = b'\'',
    OpenParen = b'(',
    CloseParen = b')',
    Asterisk = b'*',
    Plus = b'+',
    Dash = b'-',
    GreaterThan = b'>',
    OpenBracket = b'[',
    Backslash = b'\\',
    CloseBracket = b']',
    Underscore = b'_',
    Backtick = b'`',
}

impl SpecialChar {
    /// Returns the `u8` value of this character.
    #[inline]
    #[must_use]
    pub const fn byte(self) -> u8 {
        self as u8
    }

    #[inline]
    #[must_use]
    pub const fn from_byte(b: u8) -> Option<Self> {
        match b {
            b'\t' => Some(Self::Tab),
            b'\n' => Some(Self::Newline),
            b'\r' => Some(Self::CarriageReturn),
            b' ' => Some(Self::Space),
            b'!' => Some(Self::ExclamationMark),
            b'"' => Some(Self::DoubleQuote),
            b'#' => Some(Self::Hash),
            b'\'' => Some(Self::SingleQuote),
            b'(' => Some(Self::OpenParen),
            b')' => Some(Self::CloseParen),
            b'*' => Some(Self::Asterisk),
            b'+' => Some(Self::Plus),
            b'-' => Some(Self::Dash),
            b'>' => Some(Self::GreaterThan),
            b'[' => Some(Self::OpenBracket),
            b'\\' => Some(Self::Backslash),
            b']' => Some(Self::CloseBracket),
            b'_' => Some(Self::Underscore),
            b'`' => Some(Self::Backtick),
            _ => None,
        }
    }

    #[inline]
    #[must_use]
    pub const fn is_rule_char(self) -> bool {
        matches!(self, Self::Dash | Self::Asterisk | Self::Underscore)
    }

    #[inline]
    #[must_use]
    pub const fn is_list_char(self) -> bool {
        matches!(self, Self::Dash | Self::Asterisk | Self::Plus)
    }

    #[inline]
    #[must_use]
    pub const fn is_emphasis_char(self) -> bool {
        matches!(self, Self::Asterisk | Self::Underscore)
    }

    #[inline]
    #[must_use]
    pub fn count_leading(self, s: &str) -> usize {
        let byte = self.byte();
        s.as_bytes().iter().take_while(|&&b| b == byte).count()
    }
}

impl PartialEq<u8> for SpecialChar {
    #[inline]
    fn eq(&self, other: &u8) -> bool {
        self.byte() == *other
    }
}

impl PartialEq<SpecialChar> for u8 {
    #[inline]
    fn eq(&self, other: &SpecialChar) -> bool {
        *self == other.byte()
    }
}

impl PartialEq<SpecialChar> for Option<&u8> {
    #[inline]
    fn eq(&self, other: &SpecialChar) -> bool {
        matches!(self, Some(b) if **b == other.byte())
    }
}

impl PartialEq<SpecialChar> for Option<u8> {
    #[inline]
    fn eq(&self, other: &SpecialChar) -> bool {
        matches!(self, Some(b) if *b == other.byte())
    }
}

impl std::fmt::Display for SpecialChar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.byte() as char)
    }
}
