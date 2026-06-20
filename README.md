# marki

A zero-copy Markdown parser for Rust. Parses markdown strings into structured sections and inline elements, borrowing directly from the input with no intermediate allocations for text content.

## Features

- Zero-copy parsing — all text slices borrow from the input
- Type-safe representation of markdown elements via `SpecialChar`, `Section`, and `Inline`
- Fold-based state machine for single-pass block-level parsing

## Supported Sections

- Headings (levels 1-6)
- Paragraphs (with multi-line continuation)
- Code blocks (fenced with backticks, optional language)
- Unordered lists (`-`, `*`, or `+` markers)
- Ordered lists (with preserved start number)
- Blockquotes
- Horizontal rules (`---`, `***`, `___`)

## Inline Formatting

- **Bold** text (`**` or `__`)
- *Italic* text (`*` or `_`)
- `Code` spans (backtick-delimited, CommonMark space-stripping)
- [Links](https://example.com)
- ![Images](image.png)
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

## Known Limitations

- List items are single-line only (no continuation with indentation)
- Emphasis cannot span across blockquote lines (`> **bold\n> continues**` is not recognized)
- For CRLF (`\r\n`) input, call `MarkdownFile::normalize` before parsing
