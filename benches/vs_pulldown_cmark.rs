use divan::black_box;
use marki_parse::MarkdownFile;
use pulldown_cmark::{Options, Parser};

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

fn pulldown_parse(input: &str) {
    let parser = Parser::new_ext(input, Options::empty());
    // Consume the iterator to force full parse.
    for _ in parser {}
}

fn main() {
    divan::main();
}

#[divan::bench_group]
mod vs {
    use super::*;

    #[divan::bench(args = FIXTURES)]
    fn marki(name: &str) {
        let _ = MarkdownFile::<'_, 16, 32>::parse(black_box(fixture(name)));
    }

    #[divan::bench(args = FIXTURES)]
    fn pulldown_cmark(name: &str) {
        pulldown_parse(black_box(fixture(name)));
    }
}
