//! Profiling driver for the link-dense `awesome` fixture.
//!
//! Parses the fixture in a tight loop so `perf record` can attribute samples to
//! the actual hot functions (the divan bench re-execs and confuses perf). Build
//! with full debuginfo (the `release` profile sets `debug = true`) and run:
//!
//! ```text
//! perf record -g target/release/examples/profile_awesome
//! perf report
//! ```
//!
//! The iteration count is large enough that fixed startup cost is negligible.

use std::hint::black_box;

use marki_parse::MarkdownFile;

const AWESOME: &str = include_str!("../benches/fixtures/awesome.md");

fn main() {
    // Allow overriding the iteration count: `profile_awesome 20000`.
    let iters: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(10_000);

    for _ in 0..iters {
        let md = MarkdownFile::<'_, 16, 32>::parse(black_box(AWESOME));
        black_box(&md);
    }
}
