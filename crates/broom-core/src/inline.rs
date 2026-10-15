//! Inline runs: visible text plus style flags.

use broom_links::WikiTarget;

/// A run of visible text with one style. A paragraph is a list of runs.
///
/// Adjacent runs with equal `style` should be merged by the parser and the editor.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Run {
    /// Visible text. For math, the TeX source. For a wikilink, the alias
    /// (or the target when there is no alias).
    pub text: String,
    pub style: Style,
}

/// Style flags for a run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    /// Inline code span. The text is shown verbatim, with no other flags.
    pub code: bool,
    /// Inline math `$...$`. `Run::text` is the TeX source, without dollars.
    pub math: bool,
    /// Markdown link `[text](url)`. Holds the URL. `Run::text` is the link text.
    pub link: Option<String>,
    /// Wikilink `[[target|alias]]`.
    pub wikilink: Option<WikiTarget>,
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder() {}
}
