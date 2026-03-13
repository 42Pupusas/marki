#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Accept arbitrary bytes: normalize handles \r, and we convert to str.
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    let normalized = marki::normalize(&s);
    let _: marki::MarkdownFile<'_> = marki::MarkdownFile::parse(&normalized);
});
