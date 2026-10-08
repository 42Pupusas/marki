//! GitHub Flavored Markdown extensions, compiled only with the `gfm` feature.
//! `crate::strict` mirrors every item exported here with a no-op twin.

mod alert;
mod autolink;
mod block;
mod footnote;
mod footnote_def;
mod table;
mod tagfilter;
mod task;
#[cfg(test)]
mod tests;

pub use alert::Alerts;
pub use autolink::TextLinker;
pub use block::RawBlock;
pub use footnote::{FootnoteRefs, FootnoteTable};
pub use tagfilter::TagFilter;
pub use task::Tasks;

use crate::simd::ByteSet;
use crate::special_char::SpecialChar;

/// Inline trigger bytes, with `~` added for strikethrough.
pub struct Delims;

impl Delims {
    pub const SET_LEN: usize = 3;
    pub const SPECIAL_LEN: usize = 9;
    pub const SET: ByteSet<{ Self::SET_LEN }> = ByteSet::new(&[
        SpecialChar::Asterisk.byte(),
        SpecialChar::Underscore.byte(),
        SpecialChar::Tilde.byte(),
    ]);
    pub const SPECIAL: ByteSet<{ Self::SPECIAL_LEN }> = ByteSet::new(&[
        SpecialChar::Newline.byte(),
        SpecialChar::Asterisk.byte(),
        SpecialChar::Underscore.byte(),
        SpecialChar::OpenBracket.byte(),
        SpecialChar::ExclamationMark.byte(),
        SpecialChar::Backslash.byte(),
        SpecialChar::Backtick.byte(),
        SpecialChar::LessThan.byte(),
        SpecialChar::Tilde.byte(),
    ]);
}
