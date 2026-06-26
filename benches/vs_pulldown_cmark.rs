//! Comparative benchmarks: `marki-parse` against the two industry-standard
//! Rust Markdown parsers on every run.
//!
//! * `pulldown-cmark` — the fast pull parser used by rustdoc / mdBook.
//! * `comrak` — the CommonMark 0.31.2 reference-compliant parser used by
//!   crates.io, docs.rs, and GitLab.
//!
//! Each parser is timed both on the bundled prose fixtures and on the official
//! CommonMark spec test vectors (652 cases), so regressions in our throughput
//! relative to the reference implementations show up immediately.

use divan::black_box;
use marki_parse::MarkdownFile;
use pulldown_cmark::{Options, Parser, html};

/// Track heap traffic per benchmark (alloc count + bytes) alongside timings,
/// so we can compare not just *speed* but *allocation discipline* against
/// pulldown-cmark and comrak. Wraps the system allocator.
#[global_allocator]
static ALLOC: divan::AllocProfiler = divan::AllocProfiler::system();

/// Fixture names used as bench arguments. Kept as bare names so divan prints
/// readable row labels instead of `Debug`-ing the entire file contents.
const FIXTURES: &[&str] = &["rust_readme", "awesome", "commonmark_spec"];

/// Resolve a fixture name to its embedded contents.
fn fixture(name: &str) -> &'static str {
    match name {
        "rust_readme" => include_str!("fixtures/rust_readme.md"),
        "awesome" => include_str!("fixtures/awesome.md"),
        "commonmark_spec" => include_str!("fixtures/commonmark_spec.md"),
        other => panic!("unknown fixture: {other}"),
    }
}

/// The official CommonMark spec test suite as raw JSON (652 cases).
const SPEC_JSON: &str = include_str!("../tests/fixtures/spec.json");

/// Extract every `"markdown"` input string from the embedded spec JSON.
fn spec_vectors() -> Vec<String> {
    let bytes = SPEC_JSON.as_bytes();
    let key = b"\"markdown\":";
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(rel) = bytes[i..].windows(key.len()).position(|w| w == key) {
        i += rel + key.len();
        while i < bytes.len() && bytes[i] != b'"' {
            i += 1;
        }
        i += 1;
        let mut s = String::new();
        while i < bytes.len() {
            match bytes[i] {
                b'"' => {
                    i += 1;
                    break;
                }
                b'\\' => {
                    i += 1;
                    match bytes.get(i) {
                        Some(b'n') => s.push('\n'),
                        Some(b't') => s.push('\t'),
                        Some(b'r') => s.push('\r'),
                        Some(b'"') => s.push('"'),
                        Some(b'\\') => s.push('\\'),
                        Some(b'/') => s.push('/'),
                        Some(b'u') => {
                            if let Ok(code) = u32::from_str_radix(&SPEC_JSON[i + 1..i + 5], 16) {
                                s.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                            }
                            i += 4;
                        }
                        _ => {}
                    }
                    i += 1;
                }
                _ => {
                    let start = i;
                    i += 1;
                    while i < bytes.len() && bytes[i] & 0xC0 == 0x80 {
                        i += 1;
                    }
                    s.push_str(&SPEC_JSON[start..i]);
                }
            }
        }
        out.push(s);
    }
    out
}

// --- parser-under-test entry points (parse + render to HTML) ---------------

fn marki_render(input: &str) -> String {
    MarkdownFile::<'_, 16, 32>::parse(input).to_html()
}

fn pulldown_render(input: &str) -> String {
    let parser = Parser::new_ext(input, Options::empty());
    let mut out = String::new();
    html::push_html(&mut out, parser);
    out
}

fn comrak_render(input: &str) -> String {
    comrak::markdown_to_html(input, &comrak::Options::default())
}

fn main() {
    divan::main();
}

/// Per-fixture comparison (parse + render to HTML).
#[divan::bench_group]
mod fixtures {
    use super::{FIXTURES, black_box, comrak_render, fixture, marki_render, pulldown_render};

    #[divan::bench(args = FIXTURES)]
    fn marki(name: &str) {
        black_box(marki_render(black_box(fixture(name))));
    }

    #[divan::bench(args = FIXTURES)]
    fn pulldown_cmark(name: &str) {
        black_box(pulldown_render(black_box(fixture(name))));
    }

    #[divan::bench(args = FIXTURES)]
    fn comrak(name: &str) {
        black_box(comrak_render(black_box(fixture(name))));
    }
}

/// Comparison across the full CommonMark spec test-vector corpus (652 cases),
/// concatenated into one document. Inputs are gathered via `with_inputs` so
/// JSON extraction is excluded from the measured region.
#[divan::bench_group]
mod spec_corpus {
    use super::{black_box, comrak_render, marki_render, pulldown_render, spec_vectors};

    fn corpus() -> String {
        spec_vectors().join("\n\n")
    }

    #[divan::bench]
    fn marki(bencher: divan::Bencher) {
        bencher
            .with_inputs(corpus)
            .bench_values(|c| black_box(marki_render(black_box(&c))));
    }

    #[divan::bench]
    fn pulldown_cmark(bencher: divan::Bencher) {
        bencher
            .with_inputs(corpus)
            .bench_values(|c| black_box(pulldown_render(black_box(&c))));
    }

    #[divan::bench]
    fn comrak(bencher: divan::Bencher) {
        bencher
            .with_inputs(corpus)
            .bench_values(|c| black_box(comrak_render(black_box(&c))));
    }
}
