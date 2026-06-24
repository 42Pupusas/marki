//! Allocation-counting driver. Wraps the global allocator to tally how many
//! allocations a single corpus parse performs, so we can tell whether the
//! nested-children path or the mandatory per-document pools dominate.
//!
//! ```text
//! cargo run --release --example alloc_count
//! ```
#![allow(clippy::cast_precision_loss)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};

use marki_parse::{MarkdownFile, Section};

struct Counting;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

const SPEC_JSON: &str = include_str!("../tests/fixtures/spec.json");

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
    // Build corpus outside the measured region.
    let vectors = spec_vectors();

    // Warm any one-time lazy statics by parsing once untracked.
    for v in &vectors {
        let _ = black_box(MarkdownFile::<'_, 16, 32>::parse(v));
    }

    // Bucket each document by whether it produces nested sections (lists or
    // blockquotes) — the only structures the flat-tree refactor would touch.
    let mut nested_docs = 0usize;
    let mut nested_allocs = 0usize;
    let mut flat_docs = 0usize;
    let mut flat_allocs = 0usize;
    for v in &vectors {
        let a0 = ALLOCS.load(Ordering::Relaxed);
        let md = MarkdownFile::<'_, 16, 32>::parse(black_box(v.as_str()));
        let has_nesting = md.sections.iter().any(|s| {
            matches!(
                s,
                Section::UnorderedList { .. }
                    | Section::OrderedList { .. }
                    | Section::Blockquote { .. }
            )
        });
        black_box(&md);
        let used = ALLOCS.load(Ordering::Relaxed) - a0;
        if has_nesting {
            nested_docs += 1;
            nested_allocs += used;
        } else {
            flat_docs += 1;
            flat_allocs += used;
        }
    }

    eprintln!(
        "flat   docs: {flat_docs:4}  allocs/doc: {:.2}",
        flat_allocs as f64 / flat_docs.max(1) as f64,
    );
    eprintln!(
        "nested docs: {nested_docs:4}  allocs/doc: {:.2}",
        nested_allocs as f64 / nested_docs.max(1) as f64,
    );
    eprintln!(
        "total       {:4}  allocs/doc: {:.2}",
        flat_docs + nested_docs,
        (flat_allocs + nested_allocs) as f64 / (flat_docs + nested_docs) as f64,
    );
}
