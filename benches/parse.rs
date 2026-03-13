use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use marki::MarkdownFile;

const HEADING: &str = "# Hello World\n";
const PARAGRAPH: &str = "This is a simple paragraph with some text.\n";
const INLINE_RICH: &str =
    "Some **bold** and *italic* and `code` and [a link](http://example.com).\n";
const CODE_BLOCK: &str = "```rust\nfn main() {\n    println!(\"hello\");\n}\n```\n";
const UNORDERED_LIST: &str = "- item one\n- item two\n- item three\n";
const ORDERED_LIST: &str = "1. first\n2. second\n3. third\n";
const BLOCKQUOTE: &str = "> This is a blockquote\n> spanning two lines.\n";
const HORIZONTAL_RULE: &str = "---\n";

const FIXTURES: &[(&str, &str)] = &[
    ("rust_readme", include_str!("fixtures/rust_readme.md")),
    ("awesome", include_str!("fixtures/awesome.md")),
    (
        "commonmark_spec",
        include_str!("fixtures/commonmark_spec.md"),
    ),
];

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

fn large_document(repetitions: usize) -> String {
    let section = mixed_document();
    section.repeat(repetitions)
}

fn bench_individual_sections(c: &mut Criterion) {
    let mut group = c.benchmark_group("section");

    group.bench_function("heading", |b| {
        b.iter(|| MarkdownFile::parse(black_box(HEADING)));
    });
    group.bench_function("paragraph", |b| {
        b.iter(|| MarkdownFile::parse(black_box(PARAGRAPH)));
    });
    group.bench_function("inline_rich", |b| {
        b.iter(|| MarkdownFile::parse(black_box(INLINE_RICH)));
    });
    group.bench_function("code_block", |b| {
        b.iter(|| MarkdownFile::parse(black_box(CODE_BLOCK)));
    });
    group.bench_function("unordered_list", |b| {
        b.iter(|| MarkdownFile::parse(black_box(UNORDERED_LIST)));
    });
    group.bench_function("ordered_list", |b| {
        b.iter(|| MarkdownFile::parse(black_box(ORDERED_LIST)));
    });
    group.bench_function("blockquote", |b| {
        b.iter(|| MarkdownFile::parse(black_box(BLOCKQUOTE)));
    });
    group.bench_function("horizontal_rule", |b| {
        b.iter(|| MarkdownFile::parse(black_box(HORIZONTAL_RULE)));
    });

    group.finish();
}

fn bench_mixed_document(c: &mut Criterion) {
    let doc = mixed_document();
    c.bench_function("mixed_document", |b| {
        b.iter(|| MarkdownFile::parse(black_box(&doc)));
    });
}

fn bench_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("scaling");

    for reps in [1, 10, 100] {
        let doc = large_document(reps);
        group.bench_with_input(BenchmarkId::from_parameter(reps), &doc, |b, doc| {
            b.iter(|| MarkdownFile::parse(black_box(doc)));
        });
    }

    group.finish();
}

fn bench_fixtures(c: &mut Criterion) {
    let mut group = c.benchmark_group("fixture");

    for &(name, content) in FIXTURES {
        group.bench_with_input(BenchmarkId::from_parameter(name), &content, |b, doc| {
            b.iter(|| MarkdownFile::parse(black_box(doc)));
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_individual_sections,
    bench_mixed_document,
    bench_scaling,
    bench_fixtures,
);
criterion_main!(benches);
