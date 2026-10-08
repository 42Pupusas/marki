//! GFM footnotes: the definition registry, reference numbering, and parsing
//! of `[^label]` references.

use std::borrow::Cow;
use std::cell::Cell;
use std::collections::HashMap;

use crate::inline::Inline;
use crate::link_def::LinkLabel;
use crate::refs::Refs;
use crate::section::Section;

/// Parses footnote references (`[^label]`) in inline text.
pub struct FootnoteRefs;

impl FootnoteRefs {
    /// Parse a reference whose `[` is at `start`. Succeeds only when a
    /// definition with the same label exists; the label may not be empty or
    /// contain whitespace.
    pub fn parse<'src>(
        refs: Refs<'_, 'src>,
        input: &'src str,
        start: usize,
    ) -> Option<(Inline<'src>, usize)> {
        let bytes = input.as_bytes();
        if bytes.get(start + 1) != Some(&b'^') {
            return None;
        }
        let label_start = start + 2;
        let mut end = label_start;
        loop {
            match bytes.get(end)? {
                b']' => break,
                b' ' | b'\t' | b'\r' | b'\n' => return None,
                _ => end += 1,
            }
        }
        if end == label_start {
            return None;
        }
        refs.footnote(&input[label_start..end], end + 1)
    }
}

struct FootnoteSlot<'src> {
    label: &'src str,
    number: Cell<u32>,
    references: Cell<u32>,
}

/// Footnote definitions in source order. Numbers are assigned in the order
/// footnotes are first referenced, during pass 2, through a shared reference.
/// The label index is created on the first definition, so a document without
/// footnotes never allocates or hashes.
#[derive(Default)]
pub struct FootnoteTable<'src> {
    index: Option<HashMap<Cow<'src, str>, usize>>,
    slots: Vec<FootnoteSlot<'src>>,
    assigned: Cell<u32>,
}

impl<'src> FootnoteTable<'src> {
    pub fn clear(&mut self) {
        self.index = None;
        self.slots.clear();
        self.assigned.set(0);
    }

    /// Record a definition. A later definition with the same label replaces
    /// the earlier one, which then stays unreferenced and is never rendered.
    pub fn register(&mut self, label: &'src str) {
        let slot = self.slots.len();
        self.slots.push(FootnoteSlot {
            label,
            number: Cell::new(0),
            references: Cell::new(0),
        });
        self.index
            .get_or_insert_default()
            .insert(label.normalize_label_cow(), slot);
    }

    /// Resolve a reference to `label`, numbering the footnote on its first
    /// reference, and return it as an inline ending at `end`.
    pub fn reference(&self, label: &str, end: usize) -> Option<(Inline<'src>, usize)> {
        let &slot = self
            .index
            .as_ref()?
            .get(label.normalize_label_cow().as_ref())?;
        let slot = &self.slots[slot];
        if slot.number.get() == 0 {
            let next = self.assigned.get() + 1;
            self.assigned.set(next);
            slot.number.set(next);
        }
        slot.references.set(slot.references.get() + 1);
        Some((
            Inline::FootnoteRef {
                label: slot.label,
                number: slot.number.get(),
                occurrence: slot.references.get(),
            },
            end,
        ))
    }

    /// Fill in each [`Section::FootnoteDefinition`] from its slot. Definitions
    /// are top-level only and registered in source order, so the k-th
    /// definition section owns the k-th slot.
    pub fn number_definitions(&self, sections: &mut [Section<'src>]) {
        if self.slots.is_empty() {
            return;
        }
        let mut slots = self.slots.iter();
        for section in sections {
            if let Section::FootnoteDefinition {
                number, references, ..
            } = section
                && let Some(slot) = slots.next()
            {
                *number = slot.number.get();
                *references = slot.references.get();
            }
        }
    }
}
