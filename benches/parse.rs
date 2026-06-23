use divan::black_box;
use marki_parse::MarkdownFile;

const HEADING: &str = "# Hello World\n";
const PARAGRAPH: &str = "This is a simple paragraph with some text.\n";
const INLINE_RICH: &str =
    "Some **bold** and *italic* and `code` and [a link](http://example.com).\n";
const CODE_BLOCK: &str = "```rust\nfn main() {\n    println!(\"hello\");\n}\n```\n";
const UNORDERED_LIST: &str = "- item one\n- item two\n- item three\n";
const ORDERED_LIST: &str = "1. first\n2. second\n3. third\n";
const BLOCKQUOTE: &str = "> This is a blockquote\n> spanning two lines.\n";
const HORIZONTAL_RULE: &str = "---\n";

/// Fixture names used as bench arguments. Kept as bare names so divan prints
/// readable row labels instead of `Debug`-ing the entire file contents.
const FIXTURES: &[&str] = &["rust_readme", "awesome", "commonmark_spec"];

/// Resolve a fixture name to its embedded contents.
fn fixture_src(name: &str) -> &'static str {
    match name {
        "rust_readme" => include_str!("fixtures/rust_readme.md"),
        "awesome" => include_str!("fixtures/awesome.md"),
        "commonmark_spec" => include_str!("fixtures/commonmark_spec.md"),
        other => panic!("unknown fixture: {other}"),
    }
}

/// The official `CommonMark` spec test suite as raw JSON (652 cases). We embed
/// it and pull out just the `markdown` input strings to build a realistic
/// corpus of small, feature-diverse documents for benchmarking.
const SPEC_JSON: &str = include_str!("../tests/fixtures/spec.json");

/// Extract every `"markdown": "..."` value from the spec JSON, decoding the
/// handful of JSON escapes the suite uses. Deliberately minimal — just enough
/// to turn the embedded fixture into a `Vec<String>` of parser inputs.
fn spec_vectors() -> Vec<String> {
    let bytes = SPEC_JSON.as_bytes();
    let key = b"\"markdown\":";
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(rel) = find(&bytes[i..], key) {
        i += rel + key.len();
        // Skip whitespace up to the opening quote.
        while i < bytes.len() && bytes[i] != b'"' {
            i += 1;
        }
        i += 1; // past opening quote
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
                            let hex = &SPEC_JSON[i + 1..i + 5];
                            if let Ok(code) = u32::from_str_radix(hex, 16) {
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

/// Find the first occurrence of `needle` in `haystack`, returning its offset.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

fn mixed_document() -> String {
    [
        HEADING,
        "\n",
        PARAGRAPH,
        "\n",
        INLINE_RICH,
        "\n",
        CODE_BLOCK,
        "\n",
        UNORDERED_LIST,
        "\n",
        ORDERED_LIST,
        "\n",
        BLOCKQUOTE,
        "\n",
        HORIZONTAL_RULE,
    ]
    .concat()
}

fn main() {
    divan::main();
}

#[divan::bench]
fn heading() {
    let _ = MarkdownFile::<'_, 16, 32>::parse(black_box(HEADING));
}

#[divan::bench]
fn paragraph() {
    let _ = MarkdownFile::<'_, 16, 32>::parse(black_box(PARAGRAPH));
}

#[divan::bench]
fn inline_rich() {
    let _ = MarkdownFile::<'_, 16, 32>::parse(black_box(INLINE_RICH));
}

#[divan::bench]
fn code_block() {
    let _ = MarkdownFile::<'_, 16, 32>::parse(black_box(CODE_BLOCK));
}

#[divan::bench]
fn unordered_list() {
    let _ = MarkdownFile::<'_, 16, 32>::parse(black_box(UNORDERED_LIST));
}

#[divan::bench]
fn ordered_list() {
    let _ = MarkdownFile::<'_, 16, 32>::parse(black_box(ORDERED_LIST));
}

#[divan::bench]
fn blockquote() {
    let _ = MarkdownFile::<'_, 16, 32>::parse(black_box(BLOCKQUOTE));
}

#[divan::bench]
fn horizontal_rule() {
    let _ = MarkdownFile::<'_, 16, 32>::parse(black_box(HORIZONTAL_RULE));
}

#[divan::bench]
fn mixed_document_bench() {
    let doc = mixed_document();
    let _ = MarkdownFile::<'_, 16, 32>::parse(black_box(&doc));
}

#[divan::bench_group]
mod scaling {
    use super::{MarkdownFile, black_box, mixed_document};

    fn large_document(repetitions: usize) -> String {
        mixed_document().repeat(repetitions)
    }

    #[divan::bench(args = [1, 10, 100])]
    fn parse(reps: usize) {
        let doc = large_document(reps);
        let _ = MarkdownFile::<'_, 16, 32>::parse(black_box(&doc));
    }
}

#[divan::bench_group]
mod fixture {
    use super::{FIXTURES, MarkdownFile, black_box, fixture_src};

    #[divan::bench(args = FIXTURES)]
    fn parse(name: &str) {
        let _ = MarkdownFile::<'_, 16, 32>::parse(black_box(fixture_src(name)));
    }
}

/// Benchmarks driven by the official `CommonMark` spec test vectors (652 cases).
mod spec_vectors {
    use super::{MarkdownFile, black_box, spec_vectors};

    /// Parse every spec vector individually — measures per-document throughput
    /// across the full diversity of `CommonMark` constructs. Inputs are gathered
    /// once via `with_inputs` so JSON extraction is excluded from the timing.
    #[divan::bench]
    fn parse_each(bencher: divan::Bencher) {
        bencher
            .with_inputs(spec_vectors)
            .bench_values(|vectors| {
                for v in &vectors {
                    let _ = MarkdownFile::<'_, 16, 32>::parse(black_box(v));
                }
            });
    }

    /// Parse all vectors concatenated into one large document.
    #[divan::bench]
    fn parse_concatenated(bencher: divan::Bencher) {
        bencher
            .with_inputs(|| spec_vectors().join("\n\n"))
            .bench_values(|corpus| {
                let _ = MarkdownFile::<'_, 16, 32>::parse(black_box(&corpus));
            });
    }

    /// Parse and render every vector to HTML — exercises the `to_html` path
    /// added for conformance testing.
    #[divan::bench]
    fn parse_and_render(bencher: divan::Bencher) {
        bencher
            .with_inputs(spec_vectors)
            .bench_values(|vectors| {
                for v in &vectors {
                    let md = MarkdownFile::<'_, 16, 32>::parse(black_box(v));
                    black_box(md.to_html());
                }
            });
    }
}
