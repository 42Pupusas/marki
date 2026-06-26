//! Probe the known markdown algorithmic-complexity / resource-exhaustion CVE
//! vectors against marki, timing each. Not a test — a manual experiment to see
//! which (if any) blow up before they are enshrined as regression tests.
//!
//! ```text
//! cargo run --release --example dos_probe
//! ```
use std::hint::black_box;
use std::time::Instant;

use marki_parse::MarkdownFile;

fn probe(name: &str, input: &str) {
    let t = Instant::now();
    let md = MarkdownFile::<'_, 16, 32>::parse(black_box(input));
    let html = md.to_html();
    let dt = t.elapsed();
    eprintln!(
        "  {name:<38} in={:>8}B  out={:>8}B  {:>9.2?}",
        input.len(),
        html.len(),
        dt
    );
}

fn main() {
    eprintln!("DoS probes (input size scaled so a quadratic parser would hang):\n");

    // CVE-2023-26485 / CVE-2023-22486: quadratic on `_` runs.
    probe("underscore_run_100k", &"_".repeat(100_000));
    probe("star_run_100k", &"*".repeat(100_000));

    // marked O(n^2): unclosed emphasis openers `*a ` repeated.
    probe("emphasis_openers_50k", &"*a ".repeat(50_000));
    probe("emphasis_openers_underscore_50k", &"_a ".repeat(50_000));

    // Bracket nesting: link/image reference scanning.
    probe("open_brackets_50k", &"[".repeat(50_000));
    probe("nested_brackets_link", &format!("{}{}", "[".repeat(20_000), "]".repeat(20_000)));
    probe("bang_bracket_image_50k", &"![".repeat(50_000));

    // "No links in links" backtracking: nested link text where each level
    // contains a real link. The parse-then-revert design was exponential here
    // (`T(d)=2*T(d-1)`); a 126-byte input took seconds. Now linear via a
    // non-recursive `region_has_link` pre-check over the bracket table.
    probe(
        "nested_link_revert_5k",
        &format!("{}[a](b){}", "[".repeat(5_000), "](c)".repeat(5_000)),
    );
    probe(
        "nested_image_revert_5k",
        &format!("{}![a](b){}", "![".repeat(5_000), "](c)".repeat(5_000)),
    );

    // Backslash escapes.
    probe("backslash_run_100k", &"\\".repeat(100_000));

    // Backtick / code-span scanning (unbalanced runs are O(n^2) classically).
    probe("backtick_runs", &"` ".repeat(50_000));

    // Autolink / raw-HTML angle scanning.
    probe("angle_open_run_50k", &"<".repeat(50_000));

    // Parenthesis after link text (destination scanning).
    probe("link_paren_bomb", &format!("[a]{}", "(".repeat(50_000)));

    // Deep block nesting (RECURSION — stack-overflow risk in resolve_blocks).
    probe("blockquote_depth_50k", &">".repeat(50_000));
    probe(
        "blockquote_spaced_depth_10k",
        &"> ".repeat(10_000),
    );
    probe(
        "nested_list_depth_2k",
        &(0..2_000)
            .map(|i| format!("{}- x", "  ".repeat(i)))
            .collect::<Vec<_>>()
            .join("\n"),
    );

    // Many blank lines after a marker (GHSL-2023-119 footnote analogue:
    // dangling open blocks rescanned per blank line).
    probe(
        "list_then_blanks",
        &format!("- x\n{}", "\n".repeat(50_000)),
    );

    // Hard-break / trailing-space lines.
    probe("trailing_space_lines", &"x  \n".repeat(50_000));

    // Replay any `slow-unit-*` inputs libFuzzer flagged (>1s under ASAN). We
    // re-time them in a release build to see the true cost without sanitizer
    // overhead, normalizing first to mirror the fuzz target.
    eprintln!("\nslow-unit artifacts from the fuzzer:");
    for target in ["normalize_and_parse", "parse"] {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fuzz/artifacts")
            .join(target);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut files: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("slow-unit-"))
            })
            .collect();
        files.sort();
        for path in files {
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let Ok(text) = std::str::from_utf8(&bytes) else {
                continue;
            };
            let normalized = MarkdownFile::<'_, 16, 32>::normalize(text);
            let short = path.file_name().unwrap().to_str().unwrap();
            let short = &short[..short.len().min(26)];
            probe(short, &normalized);
        }
    }

    eprintln!("\ndone — all returned without hang/crash.");
}
