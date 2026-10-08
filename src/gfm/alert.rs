//! GFM alerts: a blockquote whose first line is `[!NOTE]`, `[!TIP]`,
//! `[!IMPORTANT]`, `[!WARNING]`, or `[!CAUTION]` (case-insensitive),
//! optionally followed by a custom title.

use crate::section::AlertKind;

/// Recognises the marker line that opens an alert.
pub struct Alerts;

impl Alerts {
    const KINDS: [(&'static [u8], AlertKind); 5] = [
        (b"note", AlertKind::Note),
        (b"tip", AlertKind::Tip),
        (b"important", AlertKind::Important),
        (b"warning", AlertKind::Warning),
        (b"caution", AlertKind::Caution),
    ];

    /// Parse the first content line of a blockquote (the text after `> `),
    /// returning the alert kind and the custom title, if any.
    pub fn marker(first_line: &str) -> Option<(AlertKind, Option<&str>)> {
        let rest = first_line.strip_prefix("[!")?;
        let close = rest.find(']')?;
        let word = &rest.as_bytes()[..close];
        let kind = Self::KINDS
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(word))?
            .1;
        let title = rest[close + 1..].trim();
        Some((kind, (!title.is_empty()).then_some(title)))
    }
}

#[cfg(test)]
mod tests {
    use super::Alerts;
    use crate::section::AlertKind;

    #[test]
    fn plain_marker() {
        assert_eq!(Alerts::marker("[!NOTE]"), Some((AlertKind::Note, None)));
    }

    #[test]
    fn case_insensitive_with_title() {
        assert_eq!(
            Alerts::marker("[!warning] Watch out  "),
            Some((AlertKind::Warning, Some("Watch out")))
        );
    }

    #[test]
    fn unknown_kind_rejected() {
        assert_eq!(Alerts::marker("[!DANGER]"), None);
        assert_eq!(Alerts::marker("[NOTE]"), None);
        assert_eq!(Alerts::marker(" [!NOTE]"), None);
    }
}
