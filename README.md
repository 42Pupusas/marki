# marki

A zero-copy Markdown parser for Rust. Parses markdown strings into structured sections and inline elements, borrowing directly from the input with no intermediate allocations for text content.

**100% CommonMark conformant** — passes all 652 examples in the CommonMark spec test suite, with the zero-copy guarantee fully intact.

## Features

- Zero-copy parsing — all text slices borrow from the input
- Full CommonMark conformance (652/652 spec examples), gated in CI
- Type-safe representation of markdown elements via `SpecialChar`, `Section`, and `Inline`
- Fold-based state machine for single-pass block-level parsing

## Supported Sections

- ATX headings (levels 1-6) and setext headings (`===` / `---`)
- Paragraphs (with multi-line continuation and lazy continuation)
- Fenced code blocks (backtick or tilde, optional info string) and indented code blocks
- HTML blocks
- Unordered lists (`-`, `*`, or `+` markers) and ordered lists (with preserved start number)
- Nested lists and multi-block list items, loose/tight detection
- Blockquotes (including nested and lazy continuation)
- Link reference definitions
- Horizontal rules (`---`, `***`, `___`)
- Tabs expanded to 4-column stops per spec, including partial expansion inside containers

## Inline Formatting

- **Bold** text (`**` or `__`) and *italic* text (`*` or `_`), with full delimiter-stack resolution
- `Code` spans (backtick-delimited, CommonMark space-stripping)
- [Links](https://example.com) (inline, reference, collapsed, shortcut) and ![images](image.png)
- Autolinks (`<https://...>`, `<user@host>`) and raw inline HTML
- Hard and soft line breaks
- Entity and numeric character references
- Backslash escapes

## Usage

```rust
use marki_parse::MarkdownFile;

let md = MarkdownFile::parse("# Hello\n\nWorld");
for section in &md.sections {
    println!("{section:?}");
}
```

## CRLF Support

The parser operates on LF (`\n`) line endings. For CRLF (`\r\n`) input, call `normalize` before parsing — it returns the input borrowed when no `\r` is present (zero-cost), or an owned copy with `\r` stripped:

```rust
use marki_parse::MarkdownFile;

let normalized = MarkdownFile::normalize(input);
let md = MarkdownFile::parse(&normalized);
```

## Notes

- For CRLF (`\r\n`) input, call `MarkdownFile::normalize` before parsing (see above).
