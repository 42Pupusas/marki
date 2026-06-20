use divan::black_box;
use marki_parse::MarkdownFile;
use pulldown_cmark::{Options, Parser};

const FIXTURES: &[(&str, &str)] = &[
    ("rust_readme", include_str!("fixtures/rust_readme.md")),
    ("awesome", include_str!("fixtures/awesome.md")),
    (
        "commonmark_spec",
        include_str!("fixtures/commonmark_spec.md"),
    ),
];

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
    fn marki(fixture: &(&str, &str)) {
        let _ = MarkdownFile::<'_, 16, 32>::parse(black_box(fixture.1));
    }

    #[divan::bench(args = FIXTURES)]
    fn pulldown_cmark(fixture: &(&str, &str)) {
        pulldown_parse(black_box(fixture.1));
    }
}
