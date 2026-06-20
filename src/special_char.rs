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
    Dot = b'.',
    Zero = b'0',
    GreaterThan = b'>',
    OpenBracket = b'[',
    Backslash = b'\\',
    CloseBracket = b']',
    Underscore = b'_',
    Tilde = b'~',
    Backtick = b'`',
}

/// Static lookup table for `from_byte`. Built at compile time.
static FROM_BYTE: [Option<SpecialChar>; 256] = {
    use SpecialChar as S;
    let mut table: [Option<SpecialChar>; 256] = [None; 256];
    table[b'\t' as usize] = Some(S::Tab);
    table[b'\n' as usize] = Some(S::Newline);
    table[b'\r' as usize] = Some(S::CarriageReturn);
    table[b' ' as usize] = Some(S::Space);
    table[b'!' as usize] = Some(S::ExclamationMark);
    table[b'"' as usize] = Some(S::DoubleQuote);
    table[b'#' as usize] = Some(S::Hash);
    table[b'\'' as usize] = Some(S::SingleQuote);
    table[b'(' as usize] = Some(S::OpenParen);
    table[b')' as usize] = Some(S::CloseParen);
    table[b'*' as usize] = Some(S::Asterisk);
    table[b'+' as usize] = Some(S::Plus);
    table[b'-' as usize] = Some(S::Dash);
    table[b'.' as usize] = Some(S::Dot);
    table[b'0' as usize] = Some(S::Zero);
    table[b'>' as usize] = Some(S::GreaterThan);
    table[b'[' as usize] = Some(S::OpenBracket);
    table[b'\\' as usize] = Some(S::Backslash);
    table[b']' as usize] = Some(S::CloseBracket);
    table[b'_' as usize] = Some(S::Underscore);
    table[b'~' as usize] = Some(S::Tilde);
    table[b'`' as usize] = Some(S::Backtick);
    table
};

impl SpecialChar {
    /// Returns the `u8` value of this character.
    #[inline]
    #[must_use]
    pub const fn byte(self) -> u8 {
        self as u8
    }

    /// Look up a byte in the static table. O(1).
    #[inline]
    #[must_use]
    pub fn from_byte(b: u8) -> Option<Self> {
        FROM_BYTE[b as usize]
    }

    #[inline]
    #[must_use]
    pub const fn is_list_char(self) -> bool {
        matches!(self, Self::Dash | Self::Asterisk | Self::Plus)
    }

    #[inline]
    #[must_use]
    pub fn count_leading_bytes(self, bytes: &[u8]) -> usize {
        #[cfg(target_arch = "x86_64")]
        {
            // SAFETY: SSE2 is baseline on all x86_64 processors.
            unsafe { self.count_leading_sse2(bytes) }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            self.count_leading_scalar(bytes)
        }
    }

    #[cfg(not(target_arch = "x86_64"))]
    fn count_leading_scalar(self, bytes: &[u8]) -> usize {
        let needle = self.byte();
        let mut n = 0;
        while n < bytes.len() && bytes[n] == needle {
            n += 1;
        }
        n
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "sse2")]
    #[allow(clippy::cast_ptr_alignment)]
    unsafe fn count_leading_sse2(self, bytes: &[u8]) -> usize {
        use std::arch::x86_64::{_mm_cmpeq_epi8, _mm_loadu_si128, _mm_movemask_epi8, _mm_set1_epi8};
        let needle = self.byte();
        let len = bytes.len();
        let ptr = bytes.as_ptr();
        unsafe {
            let n = _mm_set1_epi8(i8::from_ne_bytes([needle]));
            let mut i = 0;
            while i + 16 <= len {
                let chunk = _mm_loadu_si128(ptr.add(i).cast());
                let mask = u16::try_from(_mm_movemask_epi8(_mm_cmpeq_epi8(chunk, n)))
                    .expect("movemask produces a 16-bit value");
                if mask == 0xFFFF {
                    i += 16;
                } else {
                    let matched: usize = mask
                        .trailing_ones()
                        .try_into()
                        .expect("trailing ones fit in usize");
                    return i + matched;
                }
            }
            while i < len && bytes[i] == needle {
                i += 1;
            }
            i
        }
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

#[cfg(test)]
mod tests {
    use super::SpecialChar;

    #[test]
    fn count_leading_bytes_empty() {
        assert_eq!(SpecialChar::Asterisk.count_leading_bytes(b""), 0);
    }

    #[test]
    fn count_leading_bytes_none() {
        assert_eq!(SpecialChar::Asterisk.count_leading_bytes(b"abc"), 0);
    }

    #[test]
    fn count_leading_bytes_short_run() {
        assert_eq!(SpecialChar::Backtick.count_leading_bytes(b"``code"), 2);
    }

    #[test]
    fn count_leading_bytes_exact_boundary() {
        // Exactly 16 characters exercises the SSE2 fast-path boundary.
        let input = b"****************";
        assert_eq!(SpecialChar::Asterisk.count_leading_bytes(input), 16);
    }

    #[test]
    fn count_leading_bytes_long_run() {
        let input = vec![SpecialChar::Hash.byte(); 100];
        assert_eq!(SpecialChar::Hash.count_leading_bytes(&input), 100);
    }

    #[test]
    fn count_leading_bytes_partial_chunk() {
        let input = b"*****x**********";
        assert_eq!(SpecialChar::Asterisk.count_leading_bytes(input), 5);
    }

    #[test]
    fn count_leading_bytes_tilde() {
        assert_eq!(SpecialChar::Tilde.count_leading_bytes(b"~~~rust"), 3);
    }
}
