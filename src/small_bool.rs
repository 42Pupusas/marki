//! A minimal small-vector specialized for `bool`, used for the per-container
//! "lazy paragraph continuation" flag arrays in block parsing.
//!
//! These arrays are built fresh for every list item and nested blockquote,
//! pushed one flag per collected line, then read as a `&[bool]` slice by
//! `resolve_blocks`. Container content is almost always short (a handful of
//! lines), so storing the flags inline on the stack avoids a heap allocation
//! per item on prose-heavy documents. When an item exceeds the inline capacity
//! the storage transparently spills to a heap `Vec` — there is no hard cap and
//! no correctness difference, only the allocation is deferred to the rare long
//! item.

/// Inline capacity before spilling to the heap. Eight flags cover the
/// overwhelming majority of real list items and blockquote runs while keeping
/// the stack footprint tiny (`[bool; 8]` is 8 bytes).
const INLINE: usize = 8;

/// A growable `bool` sequence that stores its first [`INLINE`] elements inline
/// on the stack and spills to a `Vec<bool>` only when it grows past that.
pub(crate) enum SmallBoolVec {
    /// Inline storage: `buf[..len]` are live.
    Inline { buf: [bool; INLINE], len: usize },
    /// Spilled to the heap once the inline capacity was exceeded.
    Spilled(Vec<bool>),
}

impl SmallBoolVec {
    /// A new, empty sequence using inline storage (no allocation).
    pub(crate) const fn new() -> Self {
        Self::Inline {
            buf: [false; INLINE],
            len: 0,
        }
    }

    /// Append a flag, spilling to the heap if the inline buffer is full.
    pub(crate) fn push(&mut self, value: bool) {
        match self {
            Self::Inline { buf, len } => {
                if *len < INLINE {
                    buf[*len] = value;
                    *len += 1;
                } else {
                    // Inline buffer full: migrate to a heap Vec and continue.
                    let mut v = Vec::with_capacity(INLINE * 2);
                    v.extend_from_slice(&buf[..*len]);
                    v.push(value);
                    *self = Self::Spilled(v);
                }
            }
            Self::Spilled(v) => v.push(value),
        }
    }

    /// Remove and return the last flag, or `None` when empty.
    pub(crate) fn pop(&mut self) -> Option<bool> {
        match self {
            Self::Inline { buf, len } => {
                if *len == 0 {
                    None
                } else {
                    *len -= 1;
                    Some(buf[*len])
                }
            }
            Self::Spilled(v) => v.pop(),
        }
    }

    /// View the live flags as a slice.
    pub(crate) fn as_slice(&self) -> &[bool] {
        match self {
            Self::Inline { buf, len } => &buf[..*len],
            Self::Spilled(v) => v.as_slice(),
        }
    }
}

impl std::ops::Deref for SmallBoolVec {
    type Target = [bool];

    fn deref(&self) -> &[bool] {
        self.as_slice()
    }
}

#[cfg(test)]
mod tests {
    use super::{INLINE, SmallBoolVec};

    #[test]
    fn inline_push_pop_roundtrips() {
        let mut v = SmallBoolVec::new();
        for i in 0..INLINE {
            v.push(i % 2 == 0);
        }
        assert!(matches!(v, SmallBoolVec::Inline { .. }));
        assert_eq!(v.len(), INLINE);
        assert_eq!(v.last().copied(), Some((INLINE - 1) % 2 == 0));
        assert_eq!(&*v, &[true, false, true, false, true, false, true, false][..]);
    }

    #[test]
    fn spills_past_inline_capacity_and_preserves_order() {
        let mut v = SmallBoolVec::new();
        let pattern: Vec<bool> = (0..INLINE + 5).map(|i| i % 3 == 0).collect();
        for &b in &pattern {
            v.push(b);
        }
        assert!(matches!(v, SmallBoolVec::Spilled(_)));
        assert_eq!(&*v, pattern.as_slice());
    }

    #[test]
    fn pop_drains_in_lifo_order() {
        let mut v = SmallBoolVec::new();
        v.push(true);
        v.push(false);
        assert_eq!(v.pop(), Some(false));
        assert_eq!(v.pop(), Some(true));
        assert_eq!(v.pop(), None);
        assert!(v.is_empty());
    }
}
