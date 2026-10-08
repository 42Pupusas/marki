//! GFM task-list items: a list item whose content begins with `[ ]`, `[x]`, or
//! `[X]` followed by whitespace or the end of the line.

use crate::section::Checkbox;

/// Splits a task-list checkbox off the first line of a list item.
pub struct Tasks;

impl Tasks {
    /// Recognise the checkbox at the start of `first_line` (after at most three
    /// spaces, so indented code is not mistaken for a task), returning it and
    /// the rest of the line.
    pub fn split(first_line: &str) -> Option<(Checkbox, &str)> {
        let bytes = first_line.as_bytes();
        let start = bytes.iter().take_while(|&&b| b == b' ').count();
        if start > 3 || bytes.get(start) != Some(&b'[') || bytes.get(start + 2) != Some(&b']') {
            return None;
        }
        let checkbox = match bytes[start + 1] {
            b' ' => Checkbox::Unchecked,
            b'x' | b'X' => Checkbox::Checked,
            _ => return None,
        };
        let after = start + 3;
        if !bytes.get(after).is_none_or(u8::is_ascii_whitespace) {
            return None;
        }
        Some((checkbox, &first_line[after..]))
    }
}

#[cfg(test)]
mod tests {
    use super::Tasks;
    use crate::section::Checkbox;

    #[test]
    fn unchecked_and_checked() {
        assert_eq!(
            Tasks::split("[ ] todo"),
            Some((Checkbox::Unchecked, " todo"))
        );
        assert_eq!(Tasks::split("[X] done"), Some((Checkbox::Checked, " done")));
    }

    #[test]
    fn bare_marker() {
        assert_eq!(Tasks::split("[x]"), Some((Checkbox::Checked, "")));
    }

    #[test]
    fn requires_separator_and_shallow_indent() {
        assert_eq!(Tasks::split("[x]done"), None);
        assert_eq!(Tasks::split("[!] no"), None);
        assert_eq!(Tasks::split("[] no"), None);
        assert_eq!(Tasks::split("    [x] code"), None);
    }
}
