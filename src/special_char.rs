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
    Backslash,
}

impl SpecialChar {
    #[must_use]
    pub const fn as_byte(self) -> u8 {
        match self {
            Self::Hash => b'#',
            Self::Dash => b'-',
            Self::Asterisk => b'*',
            Self::Underscore => b'_',
            Self::GreaterThan => b'>',
            Self::Backtick => b'`',
            Self::ExclamationMark => b'!',
            Self::OpenBracket => b'[',
            Self::CloseBracket => b']',
            Self::OpenParen => b'(',
            Self::CloseParen => b')',
            Self::Backslash => b'\\',
        }
    }

    #[must_use]
    pub const fn from_byte(b: u8) -> Option<Self> {
        match b {
            b'#' => Some(Self::Hash),
            b'-' => Some(Self::Dash),
            b'*' => Some(Self::Asterisk),
            b'_' => Some(Self::Underscore),
            b'>' => Some(Self::GreaterThan),
            b'`' => Some(Self::Backtick),
            b'!' => Some(Self::ExclamationMark),
            b'[' => Some(Self::OpenBracket),
            b']' => Some(Self::CloseBracket),
            b'(' => Some(Self::OpenParen),
            b')' => Some(Self::CloseParen),
            b'\\' => Some(Self::Backslash),
            _ => None,
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
        let byte = self.as_byte();
        s.as_bytes().iter().take_while(|&&b| b == byte).count()
    }
}

impl PartialEq<u8> for SpecialChar {
    fn eq(&self, other: &u8) -> bool {
        self.as_byte() == *other
    }
}

impl PartialEq<SpecialChar> for u8 {
    fn eq(&self, other: &SpecialChar) -> bool {
        *self == other.as_byte()
    }
}

impl std::fmt::Display for SpecialChar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_byte() as char)
    }
}
