//! Numeric character reference decoding (`CommonMark` §2.5).
//!
//! Entities are resolved *late* — at render time — because they do not affect
//! block or inline structure. This is why `&#42; foo` is a paragraph and not a
//! list: the `*` only exists after decoding, long after the line was scanned.
//!
//! Only numeric references are handled here (decimal `&#35;` and hex
//! `&#x22;`). Named references like `&ouml;` would require the full ~2125-entry
//! HTML5 table; a partial table yields no conformance gain, so they are left
//! verbatim (the `&` is escaped to `&amp;` like any other literal).

/// Maximum digits in a decimal reference (`CommonMark` allows up to 7).
const MAX_DEC_DIGITS: usize = 7;
/// Maximum digits in a hexadecimal reference (`CommonMark` allows up to 6).
const MAX_HEX_DIGITS: usize = 6;

/// Try to decode a numeric character reference whose `&` is at `rest[0]`.
///
/// Returns the decoded [`char`] and the number of bytes consumed (including the
/// leading `&` and trailing `;`) on success, or `None` when `rest` is not a
/// well-formed numeric reference (the caller then treats the `&` as literal).
///
/// A syntactically valid reference to an invalid code point (zero, a surrogate,
/// or beyond `U+10FFFF`) decodes to the replacement character `U+FFFD`, matching
/// the reference implementation (e.g. `&#0;` → `�`).
pub fn decode_numeric(rest: &[u8]) -> Option<(char, usize)> {
    if rest.first() != Some(&b'&') || rest.get(1) != Some(&b'#') {
        return None;
    }
    let (radix, digits_start, max_digits) = match rest.get(2) {
        Some(b'x' | b'X') => (16u32, 3, MAX_HEX_DIGITS),
        _ => (10, 2, MAX_DEC_DIGITS),
    };

    let mut i = digits_start;
    let mut value: u32 = 0;
    let mut count = 0;
    while let Some(&b) = rest.get(i) {
        let digit = match b {
            b'0'..=b'9' => u32::from(b - b'0'),
            b'a'..=b'f' if radix == 16 => u32::from(b - b'a') + 10,
            b'A'..=b'F' if radix == 16 => u32::from(b - b'A') + 10,
            _ => break,
        };
        value = value.saturating_mul(radix).saturating_add(digit);
        count += 1;
        i += 1;
        if count > max_digits {
            return None;
        }
    }

    if count == 0 || rest.get(i) != Some(&b';') {
        return None;
    }
    i += 1; // consume ';'

    let ch = match value {
        0 => '\u{FFFD}',
        v => char::from_u32(v).unwrap_or('\u{FFFD}'),
    };
    Some((ch, i))
}

#[cfg(test)]
mod tests {
    use super::decode_numeric;

    fn dec(s: &str) -> Option<(char, usize)> {
        decode_numeric(s.as_bytes())
    }

    #[test]
    fn decimal_basic() {
        assert_eq!(dec("&#35;"), Some(('#', 5)));
        assert_eq!(dec("&#1234;"), Some(('Ӓ', 7)));
    }

    #[test]
    fn hex_basic() {
        assert_eq!(dec("&#X22;"), Some(('"', 6)));
        assert_eq!(dec("&#xcab;"), Some(('ಫ', 7)));
    }

    #[test]
    fn zero_and_invalid_become_replacement() {
        assert_eq!(dec("&#0;"), Some(('\u{FFFD}', 4)));
        // Surrogate code point.
        assert_eq!(dec("&#xD800;"), Some(('\u{FFFD}', 8)));
    }

    #[test]
    fn malformed_is_rejected() {
        assert_eq!(dec("&#;"), None);
        assert_eq!(dec("&#x;"), None);
        assert_eq!(dec("&#87654321;"), None); // too many digits
        assert_eq!(dec("&#abc;"), None); // non-decimal digits
        assert_eq!(dec("&nbsp;"), None); // named: not handled here
        assert_eq!(dec("&#35"), None); // missing terminator
    }

    #[test]
    fn consumed_length_is_exact() {
        // Trailing content after the reference is left untouched.
        assert_eq!(dec("&#35;rest"), Some(('#', 5)));
    }
}
