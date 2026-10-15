//! Wikilink syntax: `[[target]]` and `[[target|alias]]`.
//!
//! Rules:
//! - `target` is trimmed. It may contain `/` for a folder path.
//! - Without `|`, the alias is `None`.
//! - Empty target (`[[]]` or `[[|x]]`) is not a link.
//! - Links do not span lines.
//! - A `]]` ends the link. `[[a]b]]` is not a link.

use std::ops::Range;

/// The parts of a wikilink.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct WikiTarget {
    /// Note name or relative path, trimmed, without the `.md` extension.
    pub target: String,
    /// Text after `|`, trimmed. `None` when there is no `|`.
    pub alias: Option<String>,
}

/// A wikilink found in text, with its location.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WikiLinkSpan {
    pub link: WikiTarget,
    /// Byte range in the searched text, covering `[[` through `]]` inclusive.
    pub span: Range<usize>,
}

/// Finds all wikilinks in `text`, in order of appearance.
pub fn find_wikilinks(text: &str) -> Vec<WikiLinkSpan> {
    todo!()
}

/// Normalizes a target for comparison: lowercase, no `.md`, forward slashes.
/// `"Notes/Ideas.md"` and `"notes/ideas"` give the same key.
pub fn target_key(target: &str) -> String {
    todo!()
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder() {}
}
