use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use marki::MarkdownFile;
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
    // Consume the iterator to force full parse
    for _ in parser {}
}

fn bench_vs(c: &mut Criterion) {
    for &(name, content) in FIXTURES {
        let mut group = c.benchmark_group(name);

        group.bench_with_input(BenchmarkId::new("marki", ""), &content, |b, doc| {
            b.iter(|| MarkdownFile::<'_, 16, 32>::parse(black_box(doc)));
        });

        group.bench_with_input(
            BenchmarkId::new("pulldown_cmark", ""),
            &content,
            |b, doc| {
                b.iter(|| pulldown_parse(black_box(doc)));
            },
        );

        group.finish();
    }
}

criterion_group!(benches, bench_vs);
criterion_main!(benches);
