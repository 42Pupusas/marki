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

    eprintln!("\ndone — all returned without hang/crash.");
}
