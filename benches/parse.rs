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
    use super::*;

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
    use super::*;

    #[divan::bench(args = FIXTURES)]
    fn parse(name: &str) {
        let _ = MarkdownFile::<'_, 16, 32>::parse(black_box(fixture_src(name)));
    }
}
