//! Definition registries collected in pass 1 and consulted by the inline
//! parser in pass 2: link reference definitions, plus footnote definitions
//! when the `gfm` feature is enabled.

use crate::ext::FootnoteTable;
#[cfg(feature = "gfm")]
use crate::inline::Inline;
use crate::link_def::LinkDefs;
use crate::section::Section;

/// Owns every definition registry for one parse. Without the `gfm` feature
/// the footnote table is a zero-sized type that stores nothing.
#[derive(Default)]
pub struct RefStore<'src> {
    pub links: LinkDefs<'src>,
    pub footnotes: FootnoteTable<'src>,
}

impl<'src> RefStore<'src> {
    pub fn clear(&mut self) {
        self.links.clear();
        self.footnotes.clear();
    }

    /// A borrowed view that resolves links against `links`, which may be the
    /// stored map or a copy extended with container-nested definitions.
    #[cfg(feature = "gfm")]
    pub const fn view_with<'p>(&'p self, links: &'p LinkDefs<'src>) -> Refs<'p, 'src> {
        Refs {
            links,
            footnotes: &self.footnotes,
        }
    }

    #[cfg(not(feature = "gfm"))]
    #[allow(clippy::unused_self)]
    pub const fn view_with<'p>(&'p self, links: &'p LinkDefs<'src>) -> Refs<'p, 'src> {
        Refs { links }
    }

    /// Copy each footnote definition's final number and reference count into
    /// its section, once every reference has been parsed. Not `const`: the
    /// `gfm` table's implementation is not.
    #[allow(clippy::missing_const_for_fn)]
    pub fn number_footnotes(&self, sections: &mut [Section<'src>]) {
        self.footnotes.number_definitions(sections);
    }
}

/// A cheap, copyable view of a [`RefStore`] handed to the inline parser.
#[derive(Clone, Copy)]
pub struct Refs<'p, 'src> {
    links: &'p LinkDefs<'src>,
    #[cfg(feature = "gfm")]
    footnotes: &'p FootnoteTable<'src>,
}

impl<'p, 'src> Refs<'p, 'src> {
    pub fn is_empty(self) -> bool {
        self.links.is_empty()
    }

    pub fn get(self, label: &str) -> Option<&'p (&'src str, Option<&'src str>)> {
        self.links.get(label)
    }

    /// Resolve a footnote reference to `label` whose source ends at `end`.
    #[cfg(feature = "gfm")]
    pub fn footnote(self, label: &str, end: usize) -> Option<(Inline<'src>, usize)> {
        self.footnotes.reference(label, end)
    }
}
