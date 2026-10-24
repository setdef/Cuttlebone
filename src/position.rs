//! Converts between the compiler's positions and editors'.
//!
//! The compiler's lines and columns start at 1, and a column counts extended
//! grapheme clusters: the lexer advances the column by
//! `value.graphemes(true).count()` for each token (ast/src/lexer.rs:518-520,
//! and ast/src/lexer.rs:1150 for string text), using the same
//! `unicode-segmentation` crate as this module. An end column is inclusive.
//!
//! A `Position`'s lines and columns start at 0, a column counts code units of
//! an `Encoding` (UTF-16 or UTF-8, as LSP negotiates), and an end is
//! exclusive.
//!
//! The lexer segments each token on its own, and this module segments whole
//! lines. They only disagree when a grapheme cluster would span two tokens,
//! which needs a combining character or joiner right after a token boundary,
//! such as a string that starts with U+0301.
use location::Location;
use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

/// A position in a file, as an editor gives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Position {
    /// 0-based.
    pub line: u32,
    /// 0-based, in code units of the position's encoding.
    pub character: u32,
}

/// What a `Position`'s `character` counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Encoding {
    /// UTF-16 code units, LSP's default.
    #[default]
    Utf16,
    /// Bytes, LSP's `utf-8` position encoding.
    Utf8,
}

impl Encoding {
    /// The number of code units in `text`.
    pub fn len(self, text: &str) -> u32 {
        match self {
            Encoding::Utf16 => text.encode_utf16().count() as u32,
            Encoding::Utf8 => text.len() as u32,
        }
    }
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
    encoding: Encoding,
}

impl<'a> Lines<'a> {
    /// The lines of `text`, for positions in UTF-16.
    pub fn new(text: &'a str) -> Self {
        Lines::with_encoding(text, Encoding::Utf16)
    }

    pub fn with_encoding(text: &'a str, encoding: Encoding) -> Self {
        // The lexer starts a new line after each \n (ast/src/lexer.rs:512-516).
        Lines {
            lines: text.split('\n').collect(),
            encoding,
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
            let width = self.encoding.len(cluster);

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

    /// The offset where the cluster at a compiler column starts. A
    /// column past the end of the line is clamped to the end.
    pub fn from_hir(&self, line: u32, column: u32) -> Position {
        let index = line.saturating_sub(1);
        let text = self.lines.get(index as usize).copied().unwrap_or("");
        let character = text
            .graphemes(true)
            .take(column.saturating_sub(1) as usize)
            .map(|c| self.encoding.len(c))
            .sum();

        Position {
            line: index,
            character,
        }
    }

    /// The text from a compiler position (line and column) up to another,
    /// excluding the latter.
    pub fn between(&self, from: (u32, u32), to: (u32, u32)) -> String {
        let mut out = String::new();

        for line in from.0..=to.0 {
            let Some(text) = self.lines.get(line.saturating_sub(1) as usize) else {
                break;
            };
            let first = if line == from.0 { from.1 } else { 1 };
            let clusters = text.graphemes(true).skip(first.saturating_sub(1) as usize);

            if line == to.0 {
                let count = to.1.saturating_sub(first) as usize;

                out.extend(clusters.take(count));
            } else {
                out.extend(clusters);
                out.push('\n');
            }
        }

        out
    }

    /// A compiler location as a range of positions, with an exclusive end.
    pub fn range(&self, location: &Location) -> (Position, Position) {
        (
            self.from_hir(location.line_start, location.column_start),
            self.from_hir(location.line_end, location.column_end + 1),
        )
    }
}

/// The position of a byte offset in `text`: the start of the character
/// it's in. An offset past the end is the end.
pub fn position_at(text: &str, offset: usize, encoding: Encoding) -> Position {
    let mut offset = offset.min(text.len());

    while !text.is_char_boundary(offset) {
        offset -= 1;
    }

    let before = &text[..offset];
    let start = before.rfind('\n').map_or(0, |i| i + 1);

    Position {
        line: before.matches('\n').count() as u32,
        character: encoding.len(&before[start..]),
    }
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
    fn byte_offsets_as_positions() {
        let text = "a\né😀b";
        let b = text.find('b').unwrap();

        assert_eq!(position_at(text, b, Encoding::Utf16), pos(1, 3));
        assert_eq!(position_at(text, b, Encoding::Utf8), pos(1, 6));
        // Inside 😀, which starts after é.
        assert_eq!(position_at(text, b - 1, Encoding::Utf16), pos(1, 1));
        assert_eq!(position_at(text, 99, Encoding::Utf16), pos(1, 4));
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
    fn utf8_positions_count_bytes() {
        // é is 2 bytes, 😀 is 4, e + U+0301 is 3, the family is 18.
        let text = "'é' '😀' 'e\u{301}' '👩\u{200d}👩\u{200d}👦' x";
        let lines = Lines::with_encoding(text, Encoding::Utf8);
        let x = text.len() as u32 - 1;

        assert_eq!(lines.to_hir(pos(0, x)), Some(hir(1, 17)));
        assert_eq!(lines.from_hir(1, 17), pos(0, x));
        // Inside the 4 bytes of 😀 is the 😀.
        assert_eq!(lines.to_hir(pos(0, 6)), Some(hir(1, 6)));
        assert_eq!(lines.to_hir(pos(0, 9)), Some(hir(1, 6)));
        assert_eq!(lines.to_hir(pos(0, 10)), Some(hir(1, 7)));
        assert_eq!(lines.from_hir(1, 7), pos(0, 10));
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
