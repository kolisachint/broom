//! broom-links: wikilinks and the folder-wide link index.
//!
//! - [`wikilink`]: parse `[[target]]` and `[[target|alias]]` from text.
//! - [`index::LinkIndex`]: all links in a folder, backlinks, resolution,
//!   completions.
//! - [`LinkIndex::rename_rewrite`]: new contents for every file that links
//!   to a renamed note.
//!
//! Link syntax is parsed from raw text. `broom-core` calls
//! [`find_wikilinks`] on inline text only. It never looks inside code.

#![allow(unused_variables, dead_code)] // scaffold: remove as stubs are implemented

pub mod index;
pub mod wikilink;

pub use index::{Backlink, Completion, LinkIndex};
pub use wikilink::{WikiLinkSpan, WikiTarget, find_wikilinks};

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder() {}
}
