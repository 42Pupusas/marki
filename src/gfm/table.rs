//! GFM tables (GFM §4.10): pass-1 detection of the header/delimiter/body rows,
//! and pass-2 assembly into `Table`/`TableRow`/`TableCell` sections.

use crate::OffsetExt;
use crate::block::BlockBytes;
use crate::section::{ColumnAlignment, InlineSpan, Section, SectionRange};
use crate::simd::ByteSliceExt;
use crate::special_char::SpecialChar;

/// Splits one GFM table row into its cell slices, honouring backslash-escaped
/// pipes (`\|`) and stripping a single optional leading/trailing delimiter
/// pipe. Cells are borrowed and untrimmed.
struct TableCells<'src> {
    rest: Option<&'src str>,
}

impl<'src> TableCells<'src> {
    fn new(row: &'src str) -> Self {
        let trimmed = row.trim();
        let without_lead = trimmed.strip_prefix('|').unwrap_or(trimmed);
        let body = without_lead.strip_suffix('|').unwrap_or(without_lead);
        Self { rest: Some(body) }
    }

    /// Byte index of the next pipe not escaped by an odd backslash run.
    const fn next_pipe(s: &str) -> Option<usize> {
        let bytes = s.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'|' {
                let mut backslashes = 0;
                let mut j = i;
                while j > 0 && bytes[j - 1] == b'\\' {
                    backslashes += 1;
                    j -= 1;
                }
                if backslashes % 2 == 0 {
                    return Some(i);
                }
            }
            i += 1;
        }
        None
    }

    fn delimiter_columns(row: &str) -> Option<usize> {
        let mut n = 0;
        for cell in TableCells::new(row) {
            let c = cell.trim().as_bytes();
            let no_lead = c.strip_prefix(b":").unwrap_or(c);
            let inner = no_lead.strip_suffix(b":").unwrap_or(no_lead);
            if inner.is_empty() || !inner.iter().all(|&b| b == b'-') {
                return None;
            }
            n += 1;
        }
        (n > 0).then_some(n)
    }
}

impl<'src> Iterator for TableCells<'src> {
    type Item = &'src str;

    fn next(&mut self) -> Option<Self::Item> {
        let s = self.rest?;
        if let Some(i) = Self::next_pipe(s) {
            self.rest = Some(&s[i + 1..]);
            Some(&s[..i])
        } else {
            self.rest = None;
            Some(s)
        }
    }
}

/// A table detected in pass 1: its verbatim source span (no trailing newline),
/// its column count, and the byte offset to resume block parsing at.
pub struct TableScan<'src> {
    pub source: &'src str,
    pub columns: u16,
    pub resume: usize,
}

impl<'src> TableScan<'src> {
    /// Upper bound on tracked column alignments; wider columns keep
    /// [`ColumnAlignment::None`].
    const MAX_COLUMNS: usize = 128;

    /// Detect a table whose header row is `input[pos..line_end]`: a row with an
    /// unescaped pipe, followed by a delimiter row with the same column count,
    /// then any body rows up to a blank line or another block.
    pub fn detect(input: &'src str, pos: usize, line_end: usize) -> Option<Self> {
        let bytes = input.as_bytes();
        bytes[pos..line_end].find_byte(0, b'|')?;
        let indent = bytes[pos..line_end].strip_indent()?;
        let header = input.get(pos + indent..line_end)?;
        TableCells::next_pipe(header)?;
        let header_cols = TableCells::new(header).count();

        let delim_start = line_end + 1;
        if delim_start > bytes.len() {
            return None;
        }
        let delim_end = bytes
            .find_byte(delim_start, SpecialChar::Newline.byte())
            .unwrap_or(bytes.len());
        let delim_indent = bytes[delim_start..delim_end].strip_indent()?;
        let delim = input.get(delim_start + delim_indent..delim_end)?;
        if TableCells::delimiter_columns(delim)? != header_cols {
            return None;
        }
        let columns = u16::try_from(header_cols).ok()?;

        let source_start = pos + indent;
        let mut row_pos = delim_end + 1;
        let mut source_end = delim_end;
        while row_pos < bytes.len() {
            let row_end = bytes
                .find_byte(row_pos, SpecialChar::Newline.byte())
                .unwrap_or(bytes.len());
            if bytes.is_blank_line(row_pos, row_end) {
                break;
            }
            let Some(rind) = bytes[row_pos..row_end].strip_indent() else {
                break;
            };
            if bytes[row_pos + rind..row_end].begins_block() {
                break;
            }
            source_end = row_end;
            row_pos = row_end + 1;
        }

        Some(Self {
            source: input.get(source_start..source_end)?,
            columns,
            resume: source_end.min(bytes.len()).saturating_add(1),
        })
    }
}

/// Builds the pre-order `Table` sub-forest for one table in pass 2.
pub struct TableAssembler<'src> {
    source: &'src str,
    columns: u16,
    aligns: [ColumnAlignment; TableScan::MAX_COLUMNS],
}

impl<'src> TableAssembler<'src> {
    pub fn new(source: &'src str, columns: u16) -> Self {
        let mut aligns = [ColumnAlignment::None; TableScan::MAX_COLUMNS];
        let delimiter = source.split('\n').nth(1).unwrap_or("");
        for (i, cell) in TableCells::new(delimiter)
            .take(columns as usize)
            .enumerate()
        {
            if let Some(slot) = aligns.get_mut(i) {
                *slot = ColumnAlignment::from_delimiter(cell.trim().as_bytes());
            }
        }
        Self {
            source,
            columns,
            aligns,
        }
    }

    fn align_of(&self, i: usize) -> ColumnAlignment {
        self.aligns.get(i).copied().unwrap_or(ColumnAlignment::None)
    }

    /// Append the rows and cells to `section_pool`, parsing each cell's text
    /// with `parse_cell`, and return the `Table` section.
    pub fn assemble(
        &self,
        section_pool: &mut Vec<Section<'src>>,
        mut parse_cell: impl FnMut(&'src str) -> InlineSpan,
    ) -> Section<'src> {
        let rows_start = section_pool.len().pool_offset();
        let mut lines = self.source.split('\n');
        if let Some(header) = lines.next() {
            self.push_row(section_pool, &mut parse_cell, header, true);
        }
        lines.next();
        for line in lines.filter(|l| !l.is_empty()) {
            self.push_row(section_pool, &mut parse_cell, line, false);
        }
        let rows_len = section_pool.len().pool_offset() - rows_start;
        Section::Table {
            columns: self.columns,
            rows: SectionRange::new(rows_start, rows_len),
        }
    }

    fn push_row(
        &self,
        section_pool: &mut Vec<Section<'src>>,
        parse_cell: &mut impl FnMut(&'src str) -> InlineSpan,
        line: &'src str,
        header: bool,
    ) {
        let ncols = self.columns as usize;
        let row_at = section_pool.len();
        section_pool.push(Section::TableRow {
            header,
            cells: SectionRange::EMPTY,
        });
        let cells_start = section_pool.len().pool_offset();
        let mut n = 0;
        for cell in TableCells::new(line).take(ncols) {
            let content = parse_cell(cell.trim());
            section_pool.push(Section::TableCell {
                align: self.align_of(n),
                content,
            });
            n += 1;
        }
        while n < ncols {
            section_pool.push(Section::TableCell {
                align: self.align_of(n),
                content: InlineSpan::EMPTY,
            });
            n += 1;
        }
        let cells_len = section_pool.len().pool_offset() - cells_start;
        section_pool[row_at] = Section::TableRow {
            header,
            cells: SectionRange::new(cells_start, cells_len),
        };
    }
}
