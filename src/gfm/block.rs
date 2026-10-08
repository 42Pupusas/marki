//! Pass-1 blocks contributed by GFM extensions and their pass-2 assembly.

use crate::MarkdownFile;
use crate::block::{ParseCtx, Scratch};
use crate::gfm::footnote_def::{FootnoteSink, FootnoteStart};
use crate::gfm::table::{TableAssembler, TableScan};
use crate::inline::{Inline, InlineParser};
use crate::refs::Refs;
use crate::section::{PoolLine, Section};

/// A block recognised in pass 1 by a GFM extension.
#[derive(Clone, Copy)]
pub enum RawBlock<'src> {
    /// A table's verbatim source span and its column count.
    Table { source: &'src str, columns: u16 },
    /// A footnote definition whose dedented content lines live in the pass-1
    /// line pool.
    Footnote {
        label: &'src str,
        lines_start: u32,
        lines_len: u32,
    },
}

impl<'src> RawBlock<'src> {
    /// A table whose header row is the line `input[pos..line_end]`, with the
    /// offset to resume at.
    pub fn table_at(input: &'src str, pos: usize, line_end: usize) -> Option<(Self, usize)> {
        let scan = TableScan::detect(input, pos, line_end)?;
        Some((
            Self::Table {
                source: scan.source,
                columns: scan.columns,
            },
            scan.resume,
        ))
    }

    /// A footnote definition starting on the line `input[pos..line_end]`,
    /// with the offset to resume at. Its content lines are appended to `sink`.
    pub fn footnote_at(
        sink: &mut impl FootnoteSink<'src>,
        input: &'src str,
        pos: usize,
        line_end: usize,
    ) -> Option<(Self, usize)> {
        let scan = FootnoteStart::scan(sink, input, pos, line_end)?;
        Some((
            Self::Footnote {
                label: scan.label,
                lines_start: scan.lines_start,
                lines_len: scan.lines_len,
            },
            scan.resume,
        ))
    }
}

impl<'src, const D: u8, const C: usize, const B: u16> MarkdownFile<'src, D, C, B> {
    /// Build the final section for a GFM block, inline-parsing table cells
    /// and resolving the child blocks of a footnote definition.
    pub(crate) fn resolve_ext_block(
        block: RawBlock<'src>,
        ctx: &ParseCtx<'src>,
        pool: &mut Vec<Inline<'src>>,
        section_pool: &mut Vec<Section<'src>>,
        line_pool: &mut Vec<PoolLine<'src>>,
        scratch: &mut Scratch<'src>,
        refs: Refs<'_, 'src>,
    ) -> Section<'src> {
        match block {
            RawBlock::Table { source, columns } => TableAssembler::new(source, columns)
                .assemble(section_pool, |cell| {
                    InlineParser::<D, C>::parse_configured(cell, pool, refs)
                }),
            RawBlock::Footnote {
                label,
                lines_start,
                lines_len,
            } => {
                let span = lines_start as usize..(lines_start + lines_len) as usize;
                let children = Self::resolve_blocks(
                    ctx.input,
                    1,
                    ctx.lines.get(span.clone()).unwrap_or(&[]),
                    ctx.lazy.get(span.clone()).unwrap_or(&[]),
                    ctx.pad.get(span).unwrap_or(&[]),
                    pool,
                    section_pool,
                    line_pool,
                    scratch,
                    refs,
                );
                Section::FootnoteDefinition {
                    label,
                    number: 0,
                    references: 0,
                    children,
                }
            }
        }
    }
}
