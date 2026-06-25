//! Conformance test against CommonMark-style spec vectors.
//!
//! Each fixture entry is `{ markdown, html, example, section }` — the same
//! schema as the official `spec.json` from <https://spec.commonmark.org>. We
//! parse the markdown, render it with [`MarkdownFile::to_html`], and compare
//! against the expected HTML.
//!
//! `marki-parse` implements a subset of `CommonMark`, so we report a pass-rate
//! and assert a floor rather than demanding 100%. To run against the full
//! official suite, drop `spec.json` into `tests/fixtures/` and point
//! `FIXTURE` at it.

use marki_parse::MarkdownFile;

const FIXTURE: &str = include_str!("fixtures/spec.json");

/// One spec test case.
struct Case {
    markdown: String,
    html: String,
    example: u32,
    section: String,
}

/// Minimal JSON reader for the fixed `[{...}, ...]` spec schema. Only needs to
/// understand string values (with the escapes the spec uses) and the integer
/// `example` field — so we avoid pulling in a JSON dependency.
mod mini_json {
    use super::Case;

    pub fn parse(src: &str) -> Vec<Case> {
        let bytes = src.as_bytes();
        let mut i = 0;
        let mut cases = Vec::new();
        expect(bytes, &mut i, b'[');
        skip_ws(bytes, &mut i);
        if peek(bytes, i) == Some(b']') {
            return cases;
        }
        loop {
            skip_ws(bytes, &mut i);
            cases.push(parse_object(bytes, &mut i));
            skip_ws(bytes, &mut i);
            match peek(bytes, i) {
                Some(b',') => i += 1,
                Some(b']') => break,
                other => panic!("expected ',' or ']', got {other:?}"),
            }
        }
        cases
    }

    fn parse_object(bytes: &[u8], i: &mut usize) -> Case {
        expect(bytes, i, b'{');
        let mut markdown = None;
        let mut html = None;
        let mut example = None;
        let mut section = None;
        loop {
            skip_ws(bytes, i);
            let key = parse_string(bytes, i);
            skip_ws(bytes, i);
            expect(bytes, i, b':');
            skip_ws(bytes, i);
            match key.as_str() {
                "markdown" => markdown = Some(parse_string(bytes, i)),
                "html" => html = Some(parse_string(bytes, i)),
                "section" => section = Some(parse_string(bytes, i)),
                "example" => example = Some(parse_number(bytes, i)),
                // Tolerate (and skip) any extra fields like start_line/end_line.
                _ => skip_value(bytes, i),
            }
            skip_ws(bytes, i);
            match peek(bytes, *i) {
                Some(b',') => *i += 1,
                Some(b'}') => {
                    *i += 1;
                    break;
                }
                other => panic!("expected ',' or '}}' at {i}, got {other:?}"),
            }
        }
        Case {
            markdown: markdown.expect("missing markdown"),
            html: html.expect("missing html"),
            example: example.unwrap_or(0),
            section: section.unwrap_or_default(),
        }
    }

    fn parse_string(bytes: &[u8], i: &mut usize) -> String {
        expect(bytes, i, b'"');
        let mut out = String::new();
        while let Some(b) = peek(bytes, *i) {
            *i += 1;
            match b {
                b'"' => return out,
                b'\\' => {
                    let esc = bytes[*i];
                    *i += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'n' => out.push('\n'),
                        b't' => out.push('\t'),
                        b'r' => out.push('\r'),
                        b'b' => out.push('\u{0008}'),
                        b'f' => out.push('\u{000C}'),
                        b'u' => {
                            let hex = std::str::from_utf8(&bytes[*i..*i + 4]).unwrap();
                            let code = u32::from_str_radix(hex, 16).unwrap();
                            *i += 4;
                            out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                        }
                        other => panic!("bad escape \\{}", other as char),
                    }
                }
                _ => {
                    // Copy a full UTF-8 sequence starting at the byte we consumed.
                    let start = *i - 1;
                    let len = utf8_len(b);
                    *i = start + len;
                    out.push_str(std::str::from_utf8(&bytes[start..*i]).unwrap());
                }
            }
        }
        panic!("unterminated string");
    }

    fn parse_number(bytes: &[u8], i: &mut usize) -> u32 {
        let start = *i;
        while peek(bytes, *i).is_some_and(|b| b.is_ascii_digit()) {
            *i += 1;
        }
        std::str::from_utf8(&bytes[start..*i])
            .unwrap()
            .parse()
            .unwrap()
    }

    /// Skip any JSON value (used for unknown keys).
    fn skip_value(bytes: &[u8], i: &mut usize) {
        match peek(bytes, *i) {
            Some(b'"') => {
                let _ = parse_string(bytes, i);
            }
            Some(b'{') => {
                *i += 1;
                let mut depth = 1;
                while depth > 0 {
                    match peek(bytes, *i) {
                        Some(b'"') => {
                            let _ = parse_string(bytes, i);
                        }
                        Some(b'{') => {
                            depth += 1;
                            *i += 1;
                        }
                        Some(b'}') => {
                            depth -= 1;
                            *i += 1;
                        }
                        Some(_) => *i += 1,
                        None => break,
                    }
                }
            }
            _ => {
                while peek(bytes, *i).is_some_and(|b| b != b',' && b != b'}' && b != b']') {
                    *i += 1;
                }
            }
        }
    }

    const fn utf8_len(b: u8) -> usize {
        if b < 0x80 {
            1
        } else if b >> 5 == 0b110 {
            2
        } else if b >> 4 == 0b1110 {
            3
        } else {
            4
        }
    }

    fn skip_ws(bytes: &[u8], i: &mut usize) {
        while peek(bytes, *i).is_some_and(|b| b.is_ascii_whitespace()) {
            *i += 1;
        }
    }

    fn expect(bytes: &[u8], i: &mut usize, c: u8) {
        skip_ws(bytes, i);
        assert_eq!(peek(bytes, *i), Some(c), "expected '{}' at {i}", c as char);
        *i += 1;
    }

    fn peek(bytes: &[u8], i: usize) -> Option<u8> {
        bytes.get(i).copied()
    }
}

fn cases() -> Vec<Case> {
    mini_json::parse(FIXTURE)
}

/// Per-section tally of pass/fail counts, kept in first-seen order.
#[derive(Default)]
struct SectionStats {
    order: Vec<String>,
    passed: std::collections::HashMap<String, u32>,
    total: std::collections::HashMap<String, u32>,
}

impl SectionStats {
    fn record(&mut self, section: &str, ok: bool) {
        if !self.total.contains_key(section) {
            self.order.push(section.to_string());
        }
        *self.total.entry(section.to_string()).or_default() += 1;
        if ok {
            *self.passed.entry(section.to_string()).or_default() += 1;
        } else {
            self.passed.entry(section.to_string()).or_default();
        }
    }
}

#[test]
fn commonmark_conformance() {
    let cases = cases();
    assert!(!cases.is_empty(), "fixture loaded zero cases");

    let mut passed = 0u32;
    let mut stats = SectionStats::default();
    let mut failures = Vec::new();

    for case in &cases {
        let normalized = MarkdownFile::normalize(&case.markdown);
        let md: MarkdownFile<'_> = MarkdownFile::parse(&normalized);
        let got = md.to_html();
        let ok = got == case.html;
        stats.record(&case.section, ok);
        if ok {
            passed += 1;
        } else {
            failures.push(format!(
                "example {} ({}):\n  input:    {:?}\n  expected: {:?}\n  got:      {:?}",
                case.example, case.section, case.markdown, case.html, got
            ));
        }
    }

    let total = u32::try_from(cases.len()).expect("fixture count fits in u32");
    let pct = f64::from(passed) / f64::from(total) * 100.0;

    eprintln!("\n=== CommonMark conformance: {passed}/{total} ({pct:.1}%) ===\n");
    eprintln!("Per-section breakdown:");
    for section in &stats.order {
        let p = stats.passed[section];
        let t = stats.total[section];
        let mark = if p == t { "OK " } else { "   " };
        eprintln!("  [{mark}] {p:>3}/{t:<3}  {section}");
    }

    eprintln!("\nFailures ({}):", failures.len());
    for f in &failures {
        eprintln!("\nFAIL {f}");
    }

    // Full CommonMark conformance reached (652/652). This test now gates:
    // every spec vector must render byte-for-byte, so any regression fails CI.
    assert_eq!(
        passed, total,
        "CommonMark conformance regressed: {passed}/{total} ({pct:.1}%); {} failing",
        failures.len()
    );
}

/// Sanity: every fixture input must parse and render without panicking, even
/// when the output doesn't match — a no-crash corpus.
#[test]
fn no_panic_on_any_fixture() {
    for case in &cases() {
        let normalized = MarkdownFile::normalize(&case.markdown);
        let md: MarkdownFile<'_> = MarkdownFile::parse(&normalized);
        let _ = md.to_html();
    }
}
