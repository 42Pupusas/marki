#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SpecialChar {
    ExclamationMark = b'!',
    Hash = b'#',
    OpenParen = b'(',
    CloseParen = b')',
    Asterisk = b'*',
    Dash = b'-',
    GreaterThan = b'>',
    OpenBracket = b'[',
    Backslash = b'\\',
    CloseBracket = b']',
    Underscore = b'_',
    Backtick = b'`',
}

impl SpecialChar {
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
        let byte = self as u8;
        s.as_bytes().iter().take_while(|&&b| b == byte).count()
    }
}

impl PartialEq<u8> for SpecialChar {
    fn eq(&self, other: &u8) -> bool {
        *self as u8 == *other
    }
}

impl PartialEq<SpecialChar> for u8 {
    fn eq(&self, other: &SpecialChar) -> bool {
        *self == *other as Self
    }
}

impl std::fmt::Display for SpecialChar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", *self as u8 as char)
    }
}
