use crate::{AlertKind, Checkbox, ColumnAlignment, Inline, MarkdownFile, Section};

/// Parses one document and exposes structural views of it for assertions.
struct Doc<'src> {
    md: MarkdownFile<'src>,
}

impl<'src> Doc<'src> {
    fn parse(input: &'src str) -> Self {
        Self {
            md: MarkdownFile::parse(input),
        }
    }

    fn html(&self) -> String {
        self.md.to_html()
    }

    fn text_of(&self, span: crate::InlineSpan) -> String {
        let mut s = String::new();
        for inline in self.md.inlines(span) {
            if let Inline::Text(t) = inline {
                s.push_str(t);
            }
        }
        s
    }

    /// `(columns, per row: (is header, cell texts))` of the table at `index`.
    fn table(&self, index: usize) -> (u16, Vec<(bool, Vec<String>)>) {
        let Section::Table { columns, rows } = self.md.sections[index] else {
            panic!("expected table, got {:?}", self.md.sections[index]);
        };
        let rows = self
            .md
            .child_sections(rows)
            .map(|row| {
                let Section::TableRow { header, cells } = row else {
                    panic!("expected table row, got {row:?}");
                };
                let cells = self
                    .md
                    .child_sections(*cells)
                    .map(|cell| match cell {
                        Section::TableCell { content, .. } => self.text_of(*content),
                        other => panic!("expected table cell, got {other:?}"),
                    })
                    .collect();
                (*header, cells)
            })
            .collect();
        (columns, rows)
    }

    fn task_flags(&self) -> Vec<Option<Checkbox>> {
        let (Section::UnorderedList { items, .. } | Section::OrderedList { items, .. }) =
            self.md.sections[0]
        else {
            panic!("expected list, got {:?}", self.md.sections[0]);
        };
        self.md
            .child_sections(items)
            .map(|item| match item {
                Section::ListItem { task, .. } => *task,
                other => panic!("expected list item, got {other:?}"),
            })
            .collect()
    }
}

/// Checks rendered HTML for a batch of `(markdown, html)` pairs.
struct HtmlCases;

impl HtmlCases {
    fn check(cases: &[(&str, &str)]) {
        for (input, expected) in cases {
            assert_eq!(Doc::parse(input).html(), *expected, "input: {input:?}");
        }
    }
}

#[test]
fn table_basic() {
    let doc = Doc::parse("| abc | def |\n| --- | --- |\n| ghi | jkl |\n| mno | pqr |");
    assert_eq!(
        doc.table(0),
        (
            2,
            vec![
                (true, vec!["abc".to_owned(), "def".to_owned()]),
                (false, vec!["ghi".to_owned(), "jkl".to_owned()]),
                (false, vec!["mno".to_owned(), "pqr".to_owned()]),
            ]
        )
    );
}

#[test]
fn table_without_outer_pipes() {
    let (cols, rows) = Doc::parse("abc | def\n--- | ---\nxyz | ghi").table(0);
    assert_eq!(cols, 2);
    assert_eq!(rows[0], (true, vec!["abc".to_owned(), "def".to_owned()]));
    assert_eq!(rows[1], (false, vec!["xyz".to_owned(), "ghi".to_owned()]));
}

#[test]
fn table_alignment() {
    let doc = Doc::parse("a | b | c | d | e\n:-- | --- | :-: | --- | --:\nf | g | h | i | j");
    let Section::Table { rows, .. } = doc.md.sections[0] else {
        panic!("expected table");
    };
    let Some(Section::TableRow { cells, .. }) = doc.md.child_sections(rows).next() else {
        panic!("expected row");
    };
    let aligns: Vec<ColumnAlignment> = doc
        .md
        .child_sections(*cells)
        .map(|c| match c {
            Section::TableCell { align, .. } => *align,
            _ => panic!("expected cell"),
        })
        .collect();
    assert_eq!(
        aligns,
        [
            ColumnAlignment::Left,
            ColumnAlignment::None,
            ColumnAlignment::Center,
            ColumnAlignment::None,
            ColumnAlignment::Right,
        ]
    );
}

#[test]
fn table_rows_padded_and_truncated() {
    let (cols, rows) =
        Doc::parse("| a | b | c |\n| --- | --- | --- |\n| x |\n| 1 | 2 | 3 | 4 | 5 |").table(0);
    assert_eq!(cols, 3);
    assert_eq!(
        rows[1],
        (false, vec!["x".to_owned(), String::new(), String::new()])
    );
    assert_eq!(
        rows[2],
        (false, vec!["1".to_owned(), "2".to_owned(), "3".to_owned()])
    );
}

#[test]
fn table_shape_mismatch_is_paragraph() {
    let doc = Doc::parse("| a | b | c |\n| --- | --- |\n| this | isn't | okay |");
    assert!(matches!(doc.md.sections[0], Section::Paragraph { .. }));
    let doc = Doc::parse("| Not enough table | to be considered table |");
    assert!(matches!(doc.md.sections[0], Section::Paragraph { .. }));
}

#[test]
fn table_interrupts_paragraph() {
    let doc = Doc::parse("123\n456\n| a | b |\n| ---| --- |\nd | e");
    assert!(matches!(doc.md.sections[0], Section::Paragraph { .. }));
    assert!(matches!(doc.md.sections[1], Section::Table { .. }));
}

#[test]
fn table_header_only() {
    assert_eq!(
        Doc::parse("| xyz |\n| --- |").table(0),
        (1, vec![(true, vec!["xyz".to_owned()])])
    );
}

#[test]
fn table_html() {
    HtmlCases::check(&[(
        "| abc | def |\n| --- | --- |\n| ghi | jkl |",
        "<table>\n<thead>\n<tr>\n<th>abc</th>\n<th>def</th>\n</tr>\n</thead>\n\
<tbody>\n<tr>\n<td>ghi</td>\n<td>jkl</td>\n</tr>\n</tbody>\n</table>\n",
    )]);
}

#[test]
fn strikethrough_structure() {
    let doc = Doc::parse("a ~~gone~~ b");
    let Section::Paragraph { content } = doc.md.sections[0] else {
        panic!("expected paragraph");
    };
    let inlines = doc.md.inlines(content);
    assert_eq!(inlines.len(), 3);
    let Inline::Strikethrough(inner) = inlines[1] else {
        panic!("expected strikethrough, got {:?}", inlines[1]);
    };
    assert_eq!(doc.text_of(inner), "gone");
}

#[test]
fn strikethrough_html() {
    HtmlCases::check(&[
        (
            "~~Hi~~ Hello, ~there~ world!",
            "<p><del>Hi</del> Hello, <del>there</del> world!</p>\n",
        ),
        (
            "This ~~has a\n\nnew paragraph~~.",
            "<p>This ~~has a</p>\n<p>new paragraph~~.</p>\n",
        ),
        (
            "This will ~~~not~~~ strike.",
            "<p>This will ~~~not~~~ strike.</p>\n",
        ),
        ("~~a~ b~", "<p>~~a~ b~</p>\n"),
        ("~~multi\nline~~", "<p><del>multi\nline</del></p>\n"),
        ("**~~both~~**", "<p><strong><del>both</del></strong></p>\n"),
        ("~~*nested*~~", "<p><del><em>nested</em></del></p>\n"),
        ("a ~ b ~~ c", "<p>a ~ b ~~ c</p>\n"),
        ("\\~~x~~", "<p>~~x~~</p>\n"),
        ("`~~code~~`", "<p><code>~~code~~</code></p>\n"),
        ("[~~l~~](u)", "<p><a href=\"u\"><del>l</del></a></p>\n"),
        ("~a ~~b~", "<p>~a ~~b~</p>\n"),
        ("~~a~ b~~", "<p><del>a~ b</del></p>\n"),
        ("*~~e~~*", "<p><em><del>e</del></em></p>\n"),
    ]);
}

#[test]
fn task_list_items() {
    let doc = Doc::parse("- [ ] todo\n- [x] done\n- plain\n- [X] caps");
    assert_eq!(
        doc.task_flags(),
        [
            Some(Checkbox::Unchecked),
            Some(Checkbox::Checked),
            None,
            Some(Checkbox::Checked),
        ]
    );
}

#[test]
fn task_list_html() {
    HtmlCases::check(&[
        (
            "- [ ] foo\n- [x] bar",
            "<ul>\n<li><input type=\"checkbox\" disabled=\"\" /> foo</li>\n\
<li><input type=\"checkbox\" checked=\"\" disabled=\"\" /> bar</li>\n</ul>\n",
        ),
        (
            "- [x] foo\n  - [ ] bar\n  - [x] baz\n- [ ] bim",
            "<ul>\n<li><input type=\"checkbox\" checked=\"\" disabled=\"\" /> foo\n<ul>\n\
<li><input type=\"checkbox\" disabled=\"\" /> bar</li>\n\
<li><input type=\"checkbox\" checked=\"\" disabled=\"\" /> baz</li>\n</ul>\n</li>\n\
<li><input type=\"checkbox\" disabled=\"\" /> bim</li>\n</ul>\n",
        ),
        ("- [!] no", "<ul>\n<li>[!] no</li>\n</ul>\n"),
        ("- [x]done", "<ul>\n<li>[x]done</li>\n</ul>\n"),
    ]);
}

#[test]
fn extended_autolinks_html() {
    HtmlCases::check(&[
        (
            "www.commonmark.org",
            "<p><a href=\"http://www.commonmark.org\">www.commonmark.org</a></p>\n",
        ),
        (
            "Visit www.commonmark.org/help for more information.",
            "<p>Visit <a href=\"http://www.commonmark.org/help\">www.commonmark.org/help</a> for more information.</p>\n",
        ),
        (
            "Visit www.commonmark.org.",
            "<p>Visit <a href=\"http://www.commonmark.org\">www.commonmark.org</a>.</p>\n",
        ),
        (
            "http://commonmark.org",
            "<p><a href=\"http://commonmark.org\">http://commonmark.org</a></p>\n",
        ),
        (
            "foo@bar.baz",
            "<p><a href=\"mailto:foo@bar.baz\">foo@bar.baz</a></p>\n",
        ),
        (
            "**www.x.org**",
            "<p><strong><a href=\"http://www.x.org\">www.x.org</a></strong></p>\n",
        ),
        ("[www.x.org](/u)", "<p><a href=\"/u\">www.x.org</a></p>\n"),
        ("`www.x.org`", "<p><code>www.x.org</code></p>\n"),
        ("www.a_b.com/x_y_", "<p>www.a_b.com/x_y_</p>\n"),
        (
            "http://a.com/x_y_z_ end",
            "<p><a href=\"http://a.com/x_y_z\">http://a.com/x_y_z</a>_ end</p>\n",
        ),
        ("[see www.x.org", "<p>[see www.x.org</p>\n"),
        (
            "mailto:a@b.co",
            "<p><a href=\"mailto:a@b.co\">mailto:a@b.co</a></p>\n",
        ),
    ]);
}

#[test]
fn tagfilter_html() {
    HtmlCases::check(&[
        (
            "<strong> <title> <style> <em>",
            "<p><strong> &lt;title> &lt;style> <em></p>\n",
        ),
        (
            "<blockquote>\n  <xmp> is disallowed.  <XMP> is also disallowed.\n</blockquote>",
            "<blockquote>\n  &lt;xmp> is disallowed.  &lt;XMP> is also disallowed.\n</blockquote>\n",
        ),
    ]);
}

#[test]
fn footnote_numbering_follows_references() {
    let doc = Doc::parse("a[^b] c[^a] d[^b]\n\n[^a]: first\n[^b]: second\n[^z]: unused");
    let numbers: Vec<(&str, u32, u32)> = doc
        .md
        .sections
        .iter()
        .filter_map(|s| match s {
            Section::FootnoteDefinition {
                label,
                number,
                references,
                ..
            } => Some((*label, *number, *references)),
            _ => None,
        })
        .collect();
    assert_eq!(numbers, [("a", 2, 1), ("b", 1, 2), ("z", 0, 0)]);
}

#[test]
fn footnote_html() {
    HtmlCases::check(&[
        (
            "Hi[^x].\n\n[^x]: A greeting.\n",
            "<p>Hi<sup class=\"footnote-ref\"><a href=\"#fn-x\" id=\"fnref-x\" data-footnote-ref>1</a></sup>.</p>\n\
<section class=\"footnotes\" data-footnotes>\n<ol>\n<li id=\"fn-x\">\n\
<p>A greeting. <a href=\"#fnref-x\" class=\"footnote-backref\" data-footnote-backref data-footnote-backref-idx=\"1\" aria-label=\"Back to reference 1\">\u{21a9}</a></p>\n\
</li>\n</ol>\n</section>\n",
        ),
        (
            "Foo.[^]\n\n[^]: Here is the footnote.\n",
            "<p>Foo.[^]</p>\n<p>[^]: Here is the footnote.</p>\n",
        ),
        ("x[^nope]", "<p>x[^nope]</p>\n"),
    ]);
}

#[test]
fn alert_structure_and_html() {
    let doc = Doc::parse("> [!TIP] Hint\n> body");
    assert!(matches!(
        doc.md.sections[0],
        Section::Alert {
            kind: AlertKind::Tip,
            title: Some("Hint"),
            ..
        }
    ));
    HtmlCases::check(&[
        (
            "> [!NOTE]\n> Highlights information.",
            "<div class=\"markdown-alert markdown-alert-note\">\n\
<p class=\"markdown-alert-title\">Note</p>\n<p>Highlights information.</p>\n</div>\n",
        ),
        (
            "> [!warning] **Pay** attention <x>\n> body",
            "<div class=\"markdown-alert markdown-alert-warning\">\n\
<p class=\"markdown-alert-title\">**Pay** attention &lt;x&gt;</p>\n<p>body</p>\n</div>\n",
        ),
        (
            "> [!NOTE]\n> one\n>\n> two",
            "<div class=\"markdown-alert markdown-alert-note\">\n\
<p class=\"markdown-alert-title\">Note</p>\n<p>one</p>\n<p>two</p>\n</div>\n",
        ),
    ]);
}
