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
//! The two count lines differently. The lexer starts a line after each
//! `\n` only (ast/src/lexer.rs:512-516): a lone `\r` is whitespace in the
//! line. LSP ends a line at `\n`, `\r\n` or a lone `\r`. So conversions go
//! through byte offsets: the compiler's line and column to a byte, and the
//! byte to LSP's line and character, and back.
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
    text: &'a str,
    /// Where each of the compiler's lines starts.
    starts: Vec<usize>,
    encoding: Encoding,
}

impl<'a> Lines<'a> {
    /// The lines of `text`, for positions in UTF-16.
    pub fn new(text: &'a str) -> Self {
        Lines::with_encoding(text, Encoding::Utf16)
    }

    pub fn with_encoding(text: &'a str, encoding: Encoding) -> Self {
        Lines {
            text,
            starts: std::iter::once(0)
                .chain(text.match_indices('\n').map(|(i, _)| i + 1))
                .collect(),
            encoding,
        }
    }

    /// The compiler's line `index` (0-based), without its `\n`.
    fn line(&self, index: usize) -> Option<&'a str> {
        let start = *self.starts.get(index)?;
        let end = self
            .starts
            .get(index + 1)
            .map_or(self.text.len(), |next| next - 1);

        Some(&self.text[start..end])
    }

    /// The grapheme cluster that contains `position`. A position at or past
    /// the end of a line is the column after its last cluster. Returns
    /// `None` for a line past the end of the file.
    pub fn to_hir(&self, position: Position) -> Option<HirPosition> {
        let byte = byte_offset(self.text, position, self.encoding)?;
        let index = self.starts.partition_point(|s| *s <= byte) - 1;
        let mut at = self.starts[index];
        let mut column = 1;

        for cluster in self.line(index).unwrap_or("").graphemes(true) {
            if byte < at + cluster.len() {
                break;
            }

            at += cluster.len();
            column += 1;
        }

        Some(HirPosition {
            line: index as u32 + 1,
            column,
        })
    }

    /// The byte offset where the cluster at a compiler line and column
    /// starts. A column past the end of the line is clamped to the end.
    /// `None` for a line past the end of the file.
    pub fn offset(&self, line: u32, column: u32) -> Option<usize> {
        let index = line.saturating_sub(1) as usize;
        let width: usize = self
            .line(index)?
            .graphemes(true)
            .take(column.saturating_sub(1) as usize)
            .map(str::len)
            .sum();

        Some(self.starts[index] + width)
    }

    /// The position where the cluster at a compiler column starts. A
    /// column past the end of the line is clamped to the end, and a line
    /// past the end of the file to the end of the file.
    pub fn from_hir(&self, line: u32, column: u32) -> Position {
        let offset = self.offset(line, column).unwrap_or(self.text.len());

        position_at(self.text, offset, self.encoding)
    }

    /// The text from a compiler position (line and column) up to another,
    /// excluding the latter.
    pub fn between(&self, from: (u32, u32), to: (u32, u32)) -> String {
        let mut out = String::new();

        for line in from.0..=to.0 {
            let Some(text) = self.line(line.saturating_sub(1) as usize) else {
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
/// it's in. An offset past the end is the end, and one between a `\r` and
/// its `\n` is the end of that line.
pub fn position_at(text: &str, offset: usize, encoding: Encoding) -> Position {
    let bytes = text.as_bytes();
    let mut offset = offset.min(text.len());

    while !text.is_char_boundary(offset) {
        offset -= 1;
    }

    if offset > 0 && bytes[offset - 1] == b'\r' && bytes.get(offset) == Some(&b'\n') {
        offset -= 1;
    }

    let before = &text[..offset];

    // Most text has no `\r`, and then only `\n` ends lines.
    if !before.as_bytes().contains(&b'\r') {
        let start = before.rfind('\n').map_or(0, |i| i + 1);

        return Position {
            line: before.matches('\n').count() as u32,
            character: encoding.len(&before[start..]),
        };
    }

    let mut line = 0;
    let mut start = 0;

    while let Some(next) = next_line(text, start).filter(|n| *n <= offset) {
        line += 1;
        start = next;
    }

    Position {
        line,
        character: encoding.len(&text[start..offset]),
    }
}

/// `position_at` for many offsets in one text: where each line starts is
/// found once.
pub struct Positions<'a> {
    text: &'a str,
    /// Where each of LSP's lines starts.
    starts: std::borrow::Cow<'a, [usize]>,
    encoding: Encoding,
}

/// Where each of LSP's lines in `text` starts, for `Positions::with_starts`.
pub fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];

    while let Some(next) = next_line(text, *starts.last().unwrap()) {
        starts.push(next);
    }

    starts
}

impl<'a> Positions<'a> {
    pub fn new(text: &'a str, encoding: Encoding) -> Self {
        Positions {
            text,
            starts: std::borrow::Cow::Owned(line_starts(text)),
            encoding,
        }
    }

    /// The same, with `text`'s `line_starts` kept from before.
    pub fn with_starts(text: &'a str, starts: &'a [usize], encoding: Encoding) -> Self {
        Positions {
            text,
            starts: std::borrow::Cow::Borrowed(starts),
            encoding,
        }
    }

    /// `position_at(text, offset, encoding)`.
    pub fn at(&self, offset: usize) -> Position {
        let bytes = self.text.as_bytes();
        let mut offset = offset.min(self.text.len());

        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }

        if offset > 0 && bytes[offset - 1] == b'\r' && bytes.get(offset) == Some(&b'\n') {
            offset -= 1;
        }

        let line = self.starts.partition_point(|s| *s <= offset) - 1;

        Position {
            line: line as u32,
            character: self.encoding.len(&self.text[self.starts[line]..offset]),
        }
    }
}

/// Where the line after the one starting at `start` starts, if there's
/// one: after the next `\n`, `\r\n` or lone `\r`, LSP's line ends.
pub fn next_line(text: &str, start: usize) -> Option<usize> {
    let end = line_end(text, start);

    match text.as_bytes().get(end)? {
        b'\r' if text.as_bytes().get(end + 1) == Some(&b'\n') => Some(end + 2),
        _ => Some(end + 1),
    }
}

/// Where the line starting at `start` ends: the offset of its line end,
/// or of the end of the text.
pub fn line_end(text: &str, start: usize) -> usize {
    // Two searches for one byte each are much faster than one for either.
    let rest = &text[start..];
    let line = rest.find('\n').map_or(rest, |i| &rest[..i]);

    start + line.find('\r').unwrap_or(line.len())
}

/// The byte offset of an LSP position in `text`: the start of the
/// character it's in. A character past the end of its line is the end of
/// the line. `None` for a line past the end of the text.
pub fn byte_offset(text: &str, position: Position, encoding: Encoding) -> Option<usize> {
    let mut start = 0;

    for _ in 0..position.line {
        start = next_line(text, start)?;
    }

    let line = &text[start..line_end(text, start)];
    let mut units = 0;

    for (i, c) in line.char_indices() {
        let width = encoding.len(c.encode_utf8(&mut [0; 4]));

        if position.character < units + width {
            return Some(start + i);
        }

        units += width;
    }

    Some(start + line.len())
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
    fn positions_are_position_at() {
        let text = "a\r\nb日本\rc👩\nd\n\r\ne";

        for encoding in [Encoding::Utf8, Encoding::Utf16] {
            let positions = Positions::new(text, encoding);

            for offset in 0..=text.len() + 2 {
                assert_eq!(
                    positions.at(offset),
                    position_at(text, offset, encoding),
                    "{}",
                    offset
                );
            }
        }
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

    /// LSP ends lines at `\n`, `\r\n` and a lone `\r`; the compiler only
    /// at `\n`, so a lone `\r` is a column of its line.
    #[test]
    fn lone_carriage_returns_end_lines_for_editors_only() {
        let text = "ab\rcd\r\nef\ngh";
        let lines = Lines::new(text);

        // `c`: the compiler's line 1, column 4, after `a`, `b` and `\r`.
        assert_eq!(lines.to_hir(pos(1, 0)), Some(hir(1, 4)));
        assert_eq!(lines.from_hir(1, 4), pos(1, 0));
        assert_eq!(lines.offset(1, 4), Some(3));
        // `e`, after `\r\n`: line 2 for the compiler, 2 for editors.
        assert_eq!(lines.to_hir(pos(2, 0)), Some(hir(2, 1)));
        assert_eq!(lines.from_hir(2, 1), pos(2, 0));
        // `g`.
        assert_eq!(lines.to_hir(pos(3, 1)), Some(hir(3, 2)));
        assert_eq!(lines.from_hir(3, 2), pos(3, 1));
        assert_eq!(lines.to_hir(pos(4, 0)), None);
        // The end of `cd`'s line is before its `\r\n`, and no position is
        // between the two.
        assert_eq!(lines.from_hir(1, 99), pos(1, 2));
        assert_eq!(position_at(text, 6, Encoding::Utf16), pos(1, 2));
        assert_eq!(lines.to_hir(pos(1, 99)), Some(hir(1, 6)));
        assert_eq!(lines.to_hir(pos(0, 99)), Some(hir(1, 3)));

        for (offset, position) in [
            (0, pos(0, 0)),
            (2, pos(0, 2)),
            (3, pos(1, 0)),
            (7, pos(2, 0)),
            (10, pos(3, 0)),
            (12, pos(3, 2)),
        ] {
            assert_eq!(position_at(text, offset, Encoding::Utf8), position);
            assert_eq!(byte_offset(text, position, Encoding::Utf8), Some(offset));
        }

        assert_eq!(byte_offset(text, pos(1, 99), Encoding::Utf8), Some(5));
        assert_eq!(byte_offset(text, pos(4, 0), Encoding::Utf8), None);
        // A file ending in a line break has an empty last line.
        assert_eq!(byte_offset("a\r", pos(1, 0), Encoding::Utf8), Some(2));
        assert_eq!(position_at("a\r", 2, Encoding::Utf8), pos(1, 0));
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
