//! Converts between the compiler's positions and the protocol's.
//!
//! The compiler's lines and columns start at 1, and a column counts extended
//! grapheme clusters: the lexer advances the column by
//! `value.graphemes(true).count()` for each token (ast/src/lexer.rs:518-520
//! and :1150), using the same `unicode-segmentation` crate as this module. An
//! end column is inclusive.
//!
//! The protocol's lines and columns start at 0, a column counts UTF-16 code
//! units (as in LSP and IntelliJ), and an end is exclusive.
//!
//! The lexer segments each token on its own, and this module segments whole
//! lines. They only disagree when a grapheme cluster would span two tokens,
//! which needs a combining character or joiner right after a token boundary,
//! such as a string that starts with U+0301.
use location::Location;
use unicode_segmentation::UnicodeSegmentation;

/// A protocol position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    /// 0-based.
    pub line: u32,
    /// 0-based, in UTF-16 code units.
    pub character: u32,
}

/// The compiler's position of one grapheme cluster.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct HirPosition {
    /// 1-based.
    pub line: u32,
    /// 1-based, in grapheme clusters.
    pub column: u32,
}

/// The lines of a source file, for converting positions.
pub struct Lines<'a> {
    lines: Vec<&'a str>,
}

impl<'a> Lines<'a> {
    pub fn new(text: &'a str) -> Self {
        // The lexer starts a new line after each \n (ast/src/lexer.rs:510-515).
        Lines {
            lines: text.split('\n').collect(),
        }
    }

    /// The grapheme cluster that contains `position`. A position at or past
    /// the end of a line is the column after its last cluster. Returns
    /// `None` for a line past the end of the file.
    pub fn to_hir(&self, position: Position) -> Option<HirPosition> {
        let line = self.lines.get(position.line as usize)?;
        let mut units = 0;
        let mut column = 1;

        for cluster in line.graphemes(true) {
            let width = utf16_len(cluster);

            if position.character < units + width {
                break;
            }

            units += width;
            column += 1;
        }

        Some(HirPosition {
            line: position.line + 1,
            column,
        })
    }

    /// The UTF-16 offset where the cluster at a compiler column starts. A
    /// column past the end of the line is clamped to the end.
    pub fn from_hir(&self, line: u32, column: u32) -> Position {
        let index = line.saturating_sub(1);
        let text = self.lines.get(index as usize).copied().unwrap_or("");
        let character = text
            .graphemes(true)
            .take(column.saturating_sub(1) as usize)
            .map(utf16_len)
            .sum();

        Position {
            line: index,
            character,
        }
    }

    /// A compiler location as a protocol range, with an exclusive end.
    pub fn range(&self, location: &Location) -> (Position, Position) {
        (
            self.from_hir(location.line_start, location.column_start),
            self.from_hir(location.line_end, location.column_end + 1),
        )
    }
}

fn utf16_len(text: &str) -> u32 {
    text.encode_utf16().count() as u32
}

/// Whether a location contains a position.
pub fn contains(location: &Location, at: HirPosition) -> bool {
    let start = (location.line_start, location.column_start);
    let end = (location.line_end, location.column_end);

    start <= (at.line, at.column) && (at.line, at.column) <= end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hir(line: u32, column: u32) -> HirPosition {
        HirPosition { line, column }
    }

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    #[test]
    fn ascii_columns_are_offsets_plus_one() {
        let lines = Lines::new("let a = 1\nlet b = 2");

        assert_eq!(lines.to_hir(pos(0, 0)), Some(hir(1, 1)));
        assert_eq!(lines.to_hir(pos(1, 4)), Some(hir(2, 5)));
        assert_eq!(lines.from_hir(2, 5), pos(1, 4));
    }

    #[test]
    fn a_cluster_wider_than_one_unit_is_one_column() {
        // é (precomposed, 1 unit), 😀 (2 units), e + U+0301 (2 units, one
        // cluster), and a family emoji joined with ZWJs (8 units).
        let text = "'é' '😀' 'e\u{301}' '👩\u{200d}👩\u{200d}👦' x";
        let lines = Lines::new(text);
        let x = text.encode_utf16().count() as u32 - 1;

        // Clusters: 'é' (3) + space, '😀' (3) + space, 'é' (3) + space,
        // 'family' (3) + space, x: x is the 17th cluster.
        assert_eq!(lines.to_hir(pos(0, x)), Some(hir(1, 17)));
        assert_eq!(lines.from_hir(1, 17), pos(0, x));
    }

    #[test]
    fn a_position_inside_a_cluster_means_that_cluster() {
        let lines = Lines::new("😀x");

        assert_eq!(lines.to_hir(pos(0, 0)), Some(hir(1, 1)));
        assert_eq!(lines.to_hir(pos(0, 1)), Some(hir(1, 1)));
        assert_eq!(lines.to_hir(pos(0, 2)), Some(hir(1, 2)));
    }

    #[test]
    fn positions_past_the_end_of_a_line_are_the_column_after_it() {
        let lines = Lines::new("ab\ncd");

        assert_eq!(lines.to_hir(pos(0, 2)), Some(hir(1, 3)));
        assert_eq!(lines.to_hir(pos(0, 99)), Some(hir(1, 3)));
        assert_eq!(lines.from_hir(1, 99), pos(0, 2));
        assert_eq!(lines.to_hir(pos(2, 0)), None);
    }

    #[test]
    fn a_tab_is_one_column() {
        let lines = Lines::new("\tx");

        assert_eq!(lines.to_hir(pos(0, 1)), Some(hir(1, 2)));
    }

    #[test]
    fn ranges_have_exclusive_ends() {
        let lines = Lines::new("  'é😀' ");
        let location = Location::new(&(1..=1), &(3..=6));

        assert_eq!(lines.range(&location), (pos(0, 2), pos(0, 7)));
    }

    #[test]
    fn contains_is_inclusive_at_both_ends() {
        let location = Location::new(&(1..=2), &(3..=4));

        assert!(contains(&location, hir(1, 3)));
        assert!(contains(&location, hir(1, 99)));
        assert!(contains(&location, hir(2, 4)));
        assert!(!contains(&location, hir(1, 2)));
        assert!(!contains(&location, hir(2, 5)));
    }
}
