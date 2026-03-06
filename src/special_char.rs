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
}

impl SpecialChar {
    #[must_use]
    pub const fn from_char(c: char) -> Option<Self> {
        match c {
            '#' => Some(Self::Hash),
            '-' => Some(Self::Dash),
            '*' => Some(Self::Asterisk),
            '_' => Some(Self::Underscore),
            '>' => Some(Self::GreaterThan),
            '`' => Some(Self::Backtick),
            '!' => Some(Self::ExclamationMark),
            '[' => Some(Self::OpenBracket),
            ']' => Some(Self::CloseBracket),
            '(' => Some(Self::OpenParen),
            ')' => Some(Self::CloseParen),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_char(self) -> char {
        match self {
            Self::Hash => '#',
            Self::Dash => '-',
            Self::Asterisk => '*',
            Self::Underscore => '_',
            Self::GreaterThan => '>',
            Self::Backtick => '`',
            Self::ExclamationMark => '!',
            Self::OpenBracket => '[',
            Self::CloseBracket => ']',
            Self::OpenParen => '(',
            Self::CloseParen => ')',
        }
    }

    #[must_use]
    pub const fn as_byte(self) -> u8 {
        self.as_char() as u8
    }

    #[must_use]
    pub const fn from_byte(b: u8) -> Option<Self> {
        Self::from_char(b as char)
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
        s.chars().take_while(|&c| c == self.as_char()).count()
    }
}

impl std::fmt::Display for SpecialChar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_char())
    }
}
