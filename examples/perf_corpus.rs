//! Long-running profiling driver for `perf record`.
//!
//! Unlike the divan benches (which are dominated by harness/cargo startup when
//! sampled), this is a plain release binary whose entire runtime is spent in
//! `marki_parse`. Run it under perf so virtually every sample lands in parser
//! code:
//!
//! ```text
//! cargo build --release --example perf_corpus
//! perf record -g --call-graph=dwarf target/release/examples/perf_corpus
//! perf report
//! ```
//!
//! Args: `perf_corpus [iterations] [mode]`
//!   iterations — how many times to parse the whole corpus (default 20000)
//!   mode       — `parse` (default) or `render` (parse + `to_html`)

use std::hint::black_box;
use std::time::Instant;

use marki_parse::MarkdownFile;

/// The official `CommonMark` spec suite, embedded so the binary is self-contained.
const SPEC_JSON: &str = include_str!("../tests/fixtures/spec.json");

/// Pull every `"markdown"` input out of the spec JSON (minimal escape handling —
/// enough for the suite's strings). Mirrors the bench extractor.
fn spec_vectors() -> Vec<String> {
    let bytes = SPEC_JSON.as_bytes();
    let key = b"\"markdown\":";
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(rel) = find(&bytes[i..], key) {
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

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let iterations: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(20_000);
    let mode = args.next().unwrap_or_else(|| "parse".to_string());

    // Build the corpus once; profiling should only see parsing work.
    let vectors = spec_vectors();
    let total_bytes: usize = vectors.iter().map(String::len).sum();
    eprintln!(
        "corpus: {} vectors, {} bytes; {iterations} iterations, mode={mode}",
        vectors.len(),
        total_bytes,
    );

    let render = mode == "render";
    let start = Instant::now();
    let mut sink = 0usize;
    for _ in 0..iterations {
        for v in &vectors {
            let md = MarkdownFile::<'_, 16, 32>::parse(black_box(v.as_str()));
            if render {
                sink ^= black_box(md.to_html()).len();
            } else {
                sink ^= black_box(&md).sections.len();
            }
        }
    }
    let elapsed = start.elapsed();

    #[allow(clippy::cast_precision_loss)]
    let docs = iterations * vectors.len();
    #[allow(clippy::cast_precision_loss)]
    let per_doc = elapsed.as_secs_f64() * 1e9 / docs as f64;
    #[allow(clippy::cast_precision_loss)]
    let mb = (total_bytes * iterations) as f64 / 1e6;
    let throughput = mb / elapsed.as_secs_f64();
    eprintln!(
        "done in {elapsed:.2?}: {docs} docs, {per_doc:.1} ns/doc, {throughput:.1} MB/s (sink={sink})"
    );
}
