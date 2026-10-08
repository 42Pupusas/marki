//! GFM disallowed raw HTML (GFM §6.11): the `<` of a `title`, `textarea`,
//! `style`, `xmp`, `iframe`, `noembed`, `noframes`, `script`, or `plaintext`
//! tag is rendered as `&lt;`.

/// Writes raw HTML with disallowed tags neutralised.
pub struct TagFilter;

impl TagFilter {
    const BLOCKED: [&'static [u8]; 9] = [
        b"title",
        b"textarea",
        b"style",
        b"xmp",
        b"iframe",
        b"noembed",
        b"noframes",
        b"script",
        b"plaintext",
    ];

    /// Whether `html` starts with an opening or closing disallowed tag.
    fn blocks(html: &[u8]) -> bool {
        if html.len() < 3 || html[0] != b'<' {
            return false;
        }
        let name_start = if html[1] == b'/' { 2 } else { 1 };
        let rest = &html[name_start..];
        Self::BLOCKED.iter().any(|tag| {
            rest.len() >= tag.len()
                && rest[..tag.len()].eq_ignore_ascii_case(tag)
                && rest.get(tag.len()).is_some_and(|&b| {
                    b.is_ascii_whitespace()
                        || b == b'>'
                        || (b == b'/' && rest.get(tag.len() + 1) == Some(&b'>'))
                })
        })
    }

    /// Append one inline raw HTML tag, escaping its `<` when disallowed.
    pub fn inline(out: &mut String, html: &str) {
        if Self::blocks(html.as_bytes()) {
            out.push_str("&lt;");
            out.push_str(&html[1..]);
        } else {
            out.push_str(html);
        }
    }

    /// Append an HTML block, escaping the `<` of every disallowed tag in it.
    pub fn block(out: &mut String, html: &str) {
        let bytes = html.as_bytes();
        let mut copied = 0;
        for (i, &b) in bytes.iter().enumerate() {
            if b == b'<' && Self::blocks(&bytes[i..]) {
                out.push_str(&html[copied..i]);
                out.push_str("&lt;");
                copied = i + 1;
            }
        }
        out.push_str(&html[copied..]);
    }
}

#[cfg(test)]
mod tests {
    use super::TagFilter;

    #[test]
    fn filters_disallowed_tags() {
        let mut out = String::new();
        TagFilter::block(&mut out, "<strong> <title> <style> <em>");
        assert_eq!(out, "<strong> &lt;title> &lt;style> <em>");
    }

    #[test]
    fn filters_closing_and_self_closing() {
        let mut out = String::new();
        TagFilter::block(&mut out, "</script><xmp/>");
        assert_eq!(out, "&lt;/script>&lt;xmp/>");
    }

    #[test]
    fn keeps_similar_names() {
        let mut out = String::new();
        TagFilter::block(&mut out, "<titles><scripted>");
        assert_eq!(out, "<titles><scripted>");
    }

    #[test]
    fn inline_checks_only_the_tag_itself() {
        let mut out = String::new();
        TagFilter::inline(&mut out, "<iframe src=x>");
        TagFilter::inline(&mut out, "<!-- <script> -->");
        assert_eq!(out, "&lt;iframe src=x><!-- <script> -->");
    }
}
