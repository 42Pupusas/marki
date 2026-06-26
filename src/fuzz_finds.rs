//! Regression tests for inputs discovered by fuzzing.
//!
//! Each test parses the input and calls [`crate::MarkdownFile::walk_all_inlines`]
//! to assert the parser does not panic or crash.

/// crash-eb78fafcb151602b035bad4c8b369aa8144f2608
/// Deeply nested `*` emphasis causing unbounded recursion (stack overflow).
#[test]
fn fuzz_deep_emphasis_recursion() {
    let md: crate::MarkdownFile<'_> = crate::MarkdownFile::parse(
        "\n\n\n****************************************************\\*************b*****************************************************\\*************b*****************\\********************b*****************\\*********\\{***\\{***\\{***********\\********************b*****************\\*********\\{***\\{***\\{***\\{*********\\",
    );
    md.walk_all_inlines();
}

/// Replay the on-disk fuzz corpus through the full pipeline (normalize ->
/// parse -> walk every inline). Under plain `cargo test` this is a fast
/// panic/UB-free regression over thousands of adversarial inputs. Under
/// **Miri** it is the real soundness audit: libFuzzer+ASAN cannot observe
/// Stacked Borrows violations (the `contiguous_merge` / arena-transmute class
/// of UB we found), but Miri can -- and these inputs are what exercise those
/// paths, unlike the hand-written unit tests.
///
/// Miri is ~1000x slower, so the file count is bounded by the env var
/// `MARKI_CORPUS_LIMIT` (0 / unset = all). For a Miri run, set e.g.
/// `MARKI_CORPUS_LIMIT=200` to keep it tractable.
#[test]
fn fuzz_corpus_replay() {
    use std::path::Path;

    let limit: usize = std::env::var("MARKI_CORPUS_LIMIT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fuzz/corpus");
    let mut checked = 0usize;

    for target in ["parse", "normalize_and_parse"] {
        let dir = root.join(target);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue; // corpus not present (e.g. packaged crate) -- skip.
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            // Both targets gate on valid UTF-8, so mirror that here.
            let Ok(text) = std::str::from_utf8(&bytes) else {
                continue;
            };
            let normalized = crate::MarkdownFile::normalize(text);
            let md: crate::MarkdownFile<'_> = crate::MarkdownFile::parse(&normalized);
            md.walk_all_inlines();

            checked += 1;
            if limit != 0 && checked >= limit {
                return;
            }
        }
    }
}
