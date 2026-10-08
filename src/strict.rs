//! Plain-CommonMark versions of the GFM hooks, compiled when the `gfm` feature
//! is off. Each item mirrors the name and signature of its `gfm` counterpart
//! and recognises nothing.

use std::convert::Infallible;
use std::marker::PhantomData;

use crate::MarkdownFile;
use crate::block::{ParseCtx, Scratch};
use crate::inline::Inline;
use crate::refs::Refs;
use crate::section::{AlertKind, Checkbox, PoolLine, Section};
use crate::simd::ByteSet;
use crate::special_char::SpecialChar;

/// Inline trigger bytes without `~`: strikethrough is off.
pub struct Delims;

impl Delims {
    pub const SET_LEN: usize = 2;
    pub const SPECIAL_LEN: usize = 8;
    pub const SET: ByteSet<{ Self::SET_LEN }> =
        ByteSet::new(&[SpecialChar::Asterisk.byte(), SpecialChar::Underscore.byte()]);
    pub const SPECIAL: ByteSet<{ Self::SPECIAL_LEN }> = ByteSet::new(&[
        SpecialChar::Newline.byte(),
        SpecialChar::Asterisk.byte(),
        SpecialChar::Underscore.byte(),
        SpecialChar::OpenBracket.byte(),
        SpecialChar::ExclamationMark.byte(),
        SpecialChar::Backslash.byte(),
        SpecialChar::Backtick.byte(),
        SpecialChar::LessThan.byte(),
    ]);
}

/// Extension blocks: none exist, so this type has no values.
#[derive(Clone, Copy)]
pub struct RawBlock<'src>(Infallible, PhantomData<&'src ()>);

#[allow(clippy::needless_pass_by_ref_mut)]
impl<'src> RawBlock<'src> {
    pub const fn footnote_at<S>(
        _: &mut S,
        _: &'src str,
        _: usize,
        _: usize,
    ) -> Option<(Self, usize)> {
        None
    }

    pub const fn table_at(_: &'src str, _: usize, _: usize) -> Option<(Self, usize)> {
        None
    }
}

impl<'src, const D: u8, const C: usize, const B: u16> MarkdownFile<'src, D, C, B> {
    pub(crate) const fn resolve_ext_block(
        block: RawBlock<'src>,
        _: &ParseCtx<'src>,
        _: &mut Vec<Inline<'src>>,
        _: &mut Vec<Section<'src>>,
        _: &mut Vec<PoolLine<'src>>,
        _: &mut Scratch<'src>,
        _: Refs<'_, 'src>,
    ) -> Section<'src> {
        match block.0 {}
    }
}

/// Footnotes are not recognised, so the table stores nothing. The methods
/// keep a `self` receiver so call sites are identical in both builds.
#[derive(Default)]
pub struct FootnoteTable<'src>(PhantomData<&'src ()>);

#[allow(clippy::unused_self, clippy::needless_pass_by_ref_mut)]
impl<'src> FootnoteTable<'src> {
    pub const fn clear(&mut self) {}

    pub const fn number_definitions(&self, _: &mut [Section<'src>]) {}
}

/// Alert markers are not recognised.
pub struct Alerts;

impl Alerts {
    pub const fn marker(_: &str) -> Option<(AlertKind, Option<&str>)> {
        None
    }
}

/// Task-list checkboxes are not recognised.
pub struct Tasks;

impl Tasks {
    pub const fn split(_: &str) -> Option<(Checkbox, &str)> {
        None
    }
}

/// Footnote references are not recognised.
pub struct FootnoteRefs;

impl FootnoteRefs {
    pub const fn parse<'src>(
        _: Refs<'_, 'src>,
        _: &'src str,
        _: usize,
    ) -> Option<(Inline<'src>, usize)> {
        None
    }
}

/// Writes inlines to the pool unchanged: no extended autolinks. The methods
/// keep a `self` receiver so call sites are identical in both builds.
#[derive(Clone, Copy)]
pub struct TextLinker;

#[allow(clippy::unused_self)]
impl TextLinker {
    pub const TOP_LEVEL: Self = Self;

    pub const fn inside_link(self) -> Self {
        self
    }

    #[inline]
    pub const fn url_in<'src>(
        self,
        _: &'src str,
        _: usize,
        _: usize,
        _: usize,
        _: &mut u32,
    ) -> Option<(usize, usize, Inline<'src>)> {
        None
    }

    #[inline]
    pub const fn may_have_url(self, _: &str) -> bool {
        false
    }

    #[inline]
    pub const fn has_url(self, _: &str) -> bool {
        false
    }

    #[inline]
    pub fn push<'src>(self, pool: &mut Vec<Inline<'src>>, item: Inline<'src>) {
        pool.push(item);
    }

    #[inline]
    pub fn extend<'src>(
        self,
        pool: &mut Vec<Inline<'src>>,
        items: impl IntoIterator<Item = Inline<'src>>,
    ) {
        pool.extend(items);
    }
}

/// Raw HTML is emitted verbatim.
pub struct TagFilter;

impl TagFilter {
    #[inline]
    pub fn inline(out: &mut String, html: &str) {
        out.push_str(html);
    }

    #[inline]
    pub fn block(out: &mut String, html: &str) {
        out.push_str(html);
    }
}
