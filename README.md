# marki

A simple Rust library for parsing markdown files into structured sections.

## Features

- Parse markdown from strings or files
- Type-safe representation of markdown elements
- Zero-copy parsing with fold-based state machine

## Supported Sections

- Headings (levels 1-6)
- Paragraphs
- Code blocks (with optional language)
- Unordered lists
- Ordered lists
- Blockquotes
- Horizontal rules

## Usage

```rust
use marki::MarkdownFile;

let md: MarkdownFile = "# Hello\n\nWorld".parse().unwrap();
```

1. Parse a string
2. Read a file
3. Inspect sections
