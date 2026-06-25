//! HTML character reference decoding (`CommonMark` §2.5).
//!
//! Entities are resolved *late* — at render time — because they do not affect
//! block or inline structure. This is why `&#42; foo` is a paragraph and not a
//! list: the `*` only exists after decoding, long after the line was scanned.
//!
//! Three forms are handled: decimal numeric (`&#35;`), hexadecimal numeric
//! (`&#x22;`), and the full WHATWG set of named references (`&ouml;`, `&copy;`,
//! …) via the vendored table in [`crate::entities_table`]. `CommonMark`
//! requires the trailing `;` for *all* forms, so the named lookup keys on the
//! bytes between `&` and `;`.

/// Maximum digits in a decimal reference (`CommonMark` allows up to 7).
const MAX_DEC_DIGITS: usize = 7;
/// Maximum digits in a hexadecimal reference (`CommonMark` allows up to 6).
const MAX_HEX_DIGITS: usize = 6;
/// Longest named-entity body (`CounterClockwiseContourIntegral` = 31 bytes);
/// names longer than this can't match, so the scan stops early.
const MAX_NAMED_LEN: usize = 32;

/// A decoded character reference. Numeric references yield exactly one scalar;
/// a handful of named references (e.g. `&NotEqualTilde;`) expand to two, so the
/// named case borrows the table's `&'static str`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entity {
    /// A single decoded scalar (numeric reference, or `U+FFFD` for an invalid
    /// code point).
    Char(char),
    /// One or two scalars from the named-reference table.
    Str(&'static str),
}

impl Entity {
    /// Iterate the decoded scalar value(s).
    pub fn chars(self) -> EntityChars {
        match self {
            Self::Char(c) => EntityChars::One(Some(c)),
            Self::Str(s) => EntityChars::Many(s.chars()),
        }
    }
}

/// Iterator over an [`Entity`]'s scalar values (one for numeric, one or two for
/// named) without allocating.
pub enum EntityChars {
    One(Option<char>),
    Many(std::str::Chars<'static>),
}

impl Iterator for EntityChars {
    type Item = char;
    fn next(&mut self) -> Option<char> {
        match self {
            Self::One(c) => c.take(),
            Self::Many(it) => it.next(),
        }
    }
}

/// Try to decode any character reference whose `&` is at `rest[0]`.
///
/// Returns the decoded [`Entity`] and the number of bytes consumed (including
/// the leading `&` and trailing `;`), or `None` when `rest` is not a
/// well-formed reference (the caller then treats the `&` as literal).
pub fn decode_entity(rest: &[u8]) -> Option<(Entity, usize)> {
    if rest.first() != Some(&b'&') {
        return None;
    }
    if rest.get(1) == Some(&b'#') {
        decode_numeric(rest).map(|(c, n)| (Entity::Char(c), n))
    } else {
        decode_named(rest)
    }
}

/// Decode a named reference (`&name;`) by looking `name` up in the WHATWG
/// table. The body is one or more ASCII alphanumerics terminated by `;`.
fn decode_named(rest: &[u8]) -> Option<(Entity, usize)> {
    // rest[0] is '&'; scan the alphanumeric body.
    let mut i = 1;
    while i < rest.len() && i <= MAX_NAMED_LEN && rest[i].is_ascii_alphanumeric() {
        i += 1;
    }
    // Need at least one body byte and a terminating ';'.
    if i == 1 || rest.get(i) != Some(&b';') {
        return None;
    }
    let value = crate::entities_table::get_entity(&rest[1..i])?;
    Some((Entity::Str(value), i + 1))
}

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
    use super::{Entity, decode_entity, decode_numeric};

    fn dec(s: &str) -> Option<(char, usize)> {
        decode_numeric(s.as_bytes())
    }

    fn ent(s: &str) -> Option<(Entity, usize)> {
        decode_entity(s.as_bytes())
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
        assert_eq!(dec("&#35"), None); // missing terminator
    }

    #[test]
    fn consumed_length_is_exact() {
        // Trailing content after the reference is left untouched.
        assert_eq!(dec("&#35;rest"), Some(('#', 5)));
    }

    #[test]
    fn named_basic() {
        assert_eq!(ent("&ouml;"), Some((Entity::Str("\u{00F6}"), 6)));
        assert_eq!(ent("&copy;"), Some((Entity::Str("\u{00A9}"), 6)));
        assert_eq!(ent("&nbsp;"), Some((Entity::Str("\u{00A0}"), 6)));
        // Two-scalar expansion.
        assert_eq!(
            ent("&NotEqualTilde;"),
            Some((Entity::Str("\u{2242}\u{0338}"), 15))
        );
    }

    #[test]
    fn named_requires_semicolon() {
        // CommonMark requires the trailing ';' for named references.
        assert_eq!(ent("&copy"), None);
        assert_eq!(ent("&notanentity;"), None);
        assert_eq!(ent("&;"), None);
    }

    #[test]
    fn decode_entity_dispatches_numeric() {
        assert_eq!(ent("&#35;"), Some((Entity::Char('#'), 5)));
        assert_eq!(ent("&#x22;"), Some((Entity::Char('"'), 6)));
    }

    #[test]
    fn entity_chars_iterates() {
        let (e, _) = ent("&NotEqualTilde;").unwrap();
        let v: Vec<char> = e.chars().collect();
        assert_eq!(v, vec!['\u{2242}', '\u{0338}']);
        let (e, _) = ent("&#35;").unwrap();
        let v: Vec<char> = e.chars().collect();
        assert_eq!(v, vec!['#']);
    }
}
