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
- Unordered lists (`-` or `*` markers)
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
use marki::MarkdownFile;

let md = MarkdownFile::parse("# Hello\n\nWorld");
for section in &md.sections {
    println!("{section:?}");
}
```

## Known Limitations

- List items are single-line only (no continuation with indentation)
- Emphasis cannot span across blockquote lines (`> **bold\n> continues**` is not recognized)
- Input should use LF (`\n`) line endings; CRLF (`\r\n`) input will preserve `\r` in merged paragraph and code block content
