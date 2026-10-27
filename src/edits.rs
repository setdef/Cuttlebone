//! How a text changed since an older version of it: which parts of the old
//! text are still in the current one, unchanged, and where. An editor's
//! incremental edits give these exactly. Offsets are bytes.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// The most runs of unchanged text a `Changes` keeps. Past this, the
/// shortest runs count as changed, which is always safe: less text counts
/// as unchanged. 24 bytes each.
pub const MAX_RUNS: usize = 1024;

/// The unchanged parts of an old text in a new one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Changes {
    old_len: usize,
    new_len: usize,
    /// Runs of unchanged text, in order: where each starts in the old text
    /// and in the new one, and its length. Everything between them
    /// changed.
    same: Vec<(usize, usize, usize)>,
}

/// One changed part: what was in the old text, and what's there now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub old: Range<usize>,
    pub new: Range<usize>,
}

impl Changes {
    /// No changes to a text of `len` bytes.
    pub fn none(len: usize) -> Changes {
        Changes {
            old_len: len,
            new_len: len,
            same: if len > 0 {
                vec![(0, 0, len)]
            } else {
                Vec::new()
            },
        }
    }

    /// The changes from `old` to `new`, as one edit between their common
    /// start and end.
    pub fn between(old: &str, new: &str) -> Changes {
        let mut changes = Changes::none(old.len());

        changes.replace_text(old, new);
        changes
    }

    /// Records that `current` (the new text so far) became `new`, as one
    /// edit between their common start and end.
    pub fn replace_text(&mut self, current: &str, new: &str) {
        let (start, end, len) = difference(current, new);

        self.replace(start, end, len);
    }

    /// Records an edit of the new text: bytes `start..end` replaced by
    /// `len` bytes.
    pub fn replace(&mut self, start: usize, end: usize, len: usize) {
        if start == end && len == 0 {
            return;
        }

        let removed = end - start;
        let mut same = Vec::with_capacity(self.same.len() + 1);

        for &(old, new, n) in &self.same {
            if new + n <= start {
                same.push((old, new, n));
                continue;
            }

            if new >= end {
                same.push((old, new + len - removed, n));
                continue;
            }

            // The run overlaps the edit: keep what's before and after it.
            if new < start {
                same.push((old, new, start - new));
            }

            if new + n > end {
                let skip = end.max(new) - new;

                same.push((old + skip, start + len, n - skip));
            }
        }

        // Runs only ever shrink, so the order is kept.
        self.same = same;
        self.new_len = self.new_len + len - removed;
        self.bound();
    }

    fn bound(&mut self) {
        while self.same.len() > MAX_RUNS {
            let shortest = (0..self.same.len())
                .min_by_key(|i| self.same[*i].2)
                .unwrap();

            self.same.remove(shortest);
        }
    }

    /// Whether nothing changed.
    pub fn is_none(&self) -> bool {
        self.old_len == self.new_len
            && match self.same.as_slice() {
                [] => self.old_len == 0,
                [(0, 0, n)] => *n == self.old_len,
                _ => false,
            }
    }

    /// The offset in the old text of the byte at `offset` in the new one,
    /// if that byte is unchanged.
    pub fn old_offset(&self, offset: usize) -> Option<usize> {
        self.same
            .iter()
            .find(|(_, new, n)| *new <= offset && offset < new + n)
            .map(|(old, new, _)| old + offset - new)
    }

    /// The offset in the new text of the byte at `offset` in the old one, if
    /// that byte is unchanged.
    pub fn new_offset(&self, offset: usize) -> Option<usize> {
        self.same
            .iter()
            .find(|(old, _, n)| *old <= offset && offset < old + n)
            .map(|(old, new, _)| new + offset - old)
    }

    /// The range in the new text of the old `range`, if the range and the
    /// bytes just before and after it are unchanged, so it's still the
    /// same text with the same text around it: an identifier that text was
    /// typed right after isn't the same identifier any more. At the start
    /// or the end of the text, nothing may have been added before or after.
    pub fn new_range(&self, range: Range<usize>) -> Option<Range<usize>> {
        let from = range.start.checked_sub(1);
        let to = if range.end < self.old_len {
            Some(range.end + 1)
        } else {
            None
        };
        let (old, new, n) = self.same.iter().copied().find(|(old, _, n)| {
            *old <= from.unwrap_or(range.start) && to.unwrap_or(range.end) <= old + n
        })?;

        if (from.is_none() && (old, new) != (0, 0))
            || (to.is_none() && (old + n, new + n) != (self.old_len, self.new_len))
        {
            return None;
        }

        Some(range.start - old + new..range.end - old + new)
    }

    /// The range in the old text of a `range` of the new one, under the
    /// same conditions as `new_range`.
    pub fn old_range(&self, range: Range<usize>) -> Option<Range<usize>> {
        self.inverse().new_range(range)
    }

    fn inverse(&self) -> Changes {
        Changes {
            old_len: self.new_len,
            new_len: self.old_len,
            same: self.same.iter().map(|(o, n, l)| (*n, *o, *l)).collect(),
        }
    }

    /// Every changed part, in order. An insertion has an empty `old`
    /// range, and a deletion an empty `new` one.
    pub fn changed(&self) -> Vec<Change> {
        let mut out = Vec::new();
        let (mut old, mut new) = (0, 0);

        for &(o, n, len) in self
            .same
            .iter()
            .chain([(self.old_len, self.new_len, 0)].iter())
        {
            if o > old || n > new {
                out.push(Change {
                    old: old..o,
                    new: new..n,
                });
            }

            old = o + len;
            new = n + len;
        }

        out
    }
}

/// The documents as they were at a check that typed method bodies, and how
/// each changed since: what answers from that check need.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fallback {
    /// Every open document's text then. Other files were read from disk.
    pub contents: HashMap<PathBuf, String>,
    /// How each file changed since. Files without an entry didn't.
    pub changes: HashMap<PathBuf, Changes>,
    /// For each file closed since, the text its `changes` end at: its
    /// file's text on disk when it was closed. The file may have changed
    /// on disk since, which no edit records (`exact`).
    #[serde(default)]
    pub ends: HashMap<PathBuf, String>,
}

/// How a file changed from its text then (`old`) to its text now (`new`):
/// the `changes` recorded, which end at the document's text, or for a file
/// closed since, at its text on disk then (`end`), followed by how that
/// changed on disk since. Changes recorded against another text than
/// `old` and `new` (which nothing should leave, but a file can change on
/// disk at any time) are never paired with them: then it's the difference
/// between the two, one change.
pub fn exact(changes: &Changes, end: Option<&str>, old: &str, new: &str) -> Changes {
    let mut out = changes.clone();

    if let Some(end) = end
        && out.new_len == end.len()
        && end != new
    {
        out.replace_text(end, new);
    }

    if out.old_len != old.len() || out.new_len != new.len() {
        return Changes::between(old, new);
    }

    out
}

/// The most versions of the documents `Snapshots` keeps.
pub const MAX_VERSIONS: usize = 4;

/// The most running checks `Snapshots` remembers the documents of.
const MAX_RUNNING: usize = 4;

/// What the server keeps for answers from older checks: for each file, the
/// documents as they were at its last check that typed method bodies, and
/// every edit since. Versions are shared by the files whose last good
/// check used them, and there are at most `MAX_VERSIONS`, so this holds at
/// most that many copies of the open documents.
#[derive(Debug, Default)]
pub struct Snapshots {
    versions: Vec<(u64, Fallback)>,
    /// The version each file's last good check used.
    of: HashMap<PathBuf, u64>,
    /// The documents sent with each check still running.
    running: HashMap<u64, Fallback>,
}

impl Snapshots {
    /// A check started with these documents. Only the newest few checks
    /// are remembered: one runs for queries and one for diagnostics at a
    /// time, and a check that's killed never finishes.
    pub fn sent(&mut self, check: u64, contents: &HashMap<PathBuf, String>) {
        while self.running.len() >= MAX_RUNNING {
            let oldest = *self.running.keys().min().unwrap();

            self.running.remove(&oldest);
        }

        self.running.insert(
            check,
            Fallback {
                contents: contents.clone(),
                changes: HashMap::new(),
                ends: HashMap::new(),
            },
        );
    }

    fn all(&mut self) -> impl Iterator<Item = &mut Fallback> {
        self.versions
            .iter_mut()
            .map(|(_, v)| v)
            .chain(self.running.values_mut())
    }

    /// An edit of an open document: bytes `start..end` replaced by `len`
    /// bytes. `text` is its text before the edit, for documents a version
    /// read from disk.
    pub fn edited(&mut self, file: &Path, text: &str, start: usize, end: usize, len: usize) {
        for v in self.all() {
            v.contents
                .entry(file.to_path_buf())
                .or_insert_with(|| text.to_string());
            v.changes
                .entry(file.to_path_buf())
                .or_insert_with(|| Changes::none(text.len()))
                .replace(start, end, len);
        }
    }

    /// A document's whole text replaced (a change without a range, a save
    /// with other text, or closing it with unsaved changes).
    pub fn replaced(&mut self, file: &Path, text: &str, new: &str) {
        let (start, end, len) = difference(text, new);

        self.edited(file, text, start, end, len);
    }

    /// A document opened with `text`, while the file on disk has `disk`.
    /// A version that read the file from disk sees that as an edit. One
    /// whose changes of it end where it was closed sees an edit from that
    /// text, whatever happened to the file since.
    pub fn opened(&mut self, file: &Path, disk: Option<&str>, text: &str) {
        for v in self.all() {
            if let Some(end) = v.ends.remove(file) {
                v.changes
                    .entry(file.to_path_buf())
                    .or_insert_with(|| Changes::none(end.len()))
                    .replace_text(&end, text);
            } else if let Some(disk) = disk
                && !v.contents.contains_key(file)
            {
                v.contents.insert(file.to_path_buf(), disk.to_string());
                v.changes
                    .insert(file.to_path_buf(), Changes::between(disk, text));
            }
        }
    }

    /// A document closed: its text was `text`, and its file now has `disk`
    /// (`None` if it's gone), which replaces it. Every version's changes of
    /// it end at that text from now on, and the file can change on disk
    /// after (`exact`).
    pub fn closed(&mut self, file: &Path, text: &str, disk: Option<&str>) {
        if let Some(disk) = disk {
            self.replaced(file, text, disk);
        }

        for v in self.all() {
            if v.changes.contains_key(file) {
                v.ends
                    .insert(file.to_path_buf(), disk.unwrap_or(text).to_string());
            }
        }
    }

    /// A check finished. If it typed method bodies, its documents are now
    /// the last good ones of `files`: files, or a diagnostics target (a
    /// project directory) for all of its files.
    pub fn finished(&mut self, check: u64, files: &[PathBuf]) {
        let Some(version) = self.running.remove(&check) else {
            return;
        };

        if files.is_empty() {
            return;
        }

        for f in files {
            self.of.insert(f.clone(), check);
        }

        self.versions.push((check, version));

        // Keep the versions some file uses, newest first, up to the limit.
        let used: Vec<u64> = self.of.values().copied().collect();

        self.versions.retain(|(id, _)| used.contains(id));

        while self.versions.len() > MAX_VERSIONS {
            let (oldest, _) = self.versions.remove(0);

            self.of.retain(|_, id| *id != oldest);
        }
    }

    /// The documents of `file`'s last good check, and how they changed:
    /// the newer of its own and its diagnostics target's (`target`).
    pub fn fallback(&self, file: &Path, target: Option<&Path>) -> Option<Fallback> {
        let id = [Some(file), target]
            .into_iter()
            .flatten()
            .filter_map(|p| self.of.get(p))
            .max()?;

        self.versions
            .iter()
            .find(|(v, _)| v == id)
            .map(|(_, v)| v.clone())
    }

    /// How many versions are kept, for tests.
    pub fn len(&self) -> usize {
        self.versions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.versions.is_empty()
    }
}

/// Where two texts differ: the start of the difference, its end in `old`,
/// and its length in `new`. Both ends are on character boundaries.
pub fn difference(old: &str, new: &str) -> (usize, usize, usize) {
    let prefix = old
        .char_indices()
        .zip(new.chars())
        .find(|((_, a), b)| a != b)
        .map_or(old.len().min(new.len()), |((i, _), _)| i);
    let prefix = floor_boundary(new, floor_boundary(old, prefix));
    let suffix = old[prefix..]
        .chars()
        .rev()
        .zip(new[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum::<usize>();

    (prefix, old.len() - suffix, new.len() - suffix - prefix)
}

fn floor_boundary(text: &str, mut i: usize) -> usize {
    while !text.is_char_boundary(i) {
        i -= 1;
    }

    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(text: &mut String, changes: &mut Changes, start: usize, end: usize, with: &str) {
        text.replace_range(start..end, with);
        changes.replace(start, end, with.len());
    }

    #[test]
    fn exact_changes_go_on_from_where_they_end() {
        let mut changes = Changes::none(7);

        // "a b c d" to "a B c d", where a file was closed; then "a B c d x"
        // on disk. `c` is unchanged throughout, which the difference
        // between the first and the last alone can't tell.
        changes.replace(2, 3, 1);

        let on = exact(&changes, Some("a B c d"), "a b c d", "a B c d x");

        assert_eq!(on.new_offset(4), Some(4));
        assert_eq!(on.new_offset(2), None);

        // Recorded against other text: the difference alone.
        let other = exact(&changes, None, "a b c d", "a b c d e f");

        assert_eq!(other, Changes::between("a b c d", "a b c d e f"));
    }

    #[test]
    fn a_closed_file_opened_again_goes_on_from_its_text_then() {
        let file = Path::new("/p/src/lib.inko");
        let mut snapshots = Snapshots::default();

        snapshots.sent(
            1,
            &HashMap::from([(file.to_path_buf(), "a b c".to_string())]),
        );
        snapshots.finished(1, &[file.to_path_buf()]);
        snapshots.edited(file, "a b c", 2, 3, 1);
        // Closed unsaved: the file still has "a b c". It changes on disk,
        // and is opened with that text.
        snapshots.closed(file, "a B c", Some("a b c"));
        snapshots.opened(file, Some("zz a b c"), "zz a b c");

        let fallback = snapshots.fallback(file, None).unwrap();
        let changes = &fallback.changes[file];

        assert!(fallback.ends.is_empty());
        // `c` and `a` were never touched; `b` was edited and changed back.
        assert_eq!(changes.new_offset(4), Some(7));
        assert_eq!(changes.new_offset(0), Some(3));
        assert_eq!(changes.new_offset(2), None);
    }

    #[test]
    fn edits_map_unchanged_text() {
        let old = "let a = 1\nlet b = 2\n";
        let mut text = old.to_string();
        let mut changes = Changes::none(old.len());

        apply(&mut text, &mut changes, 4, 5, "abc");
        assert_eq!(text, "let abc = 1\nlet b = 2\n");
        assert_eq!(changes.new_range(14..15), Some(16..17));
        // `a` was replaced, and the space after it is next to the edit.
        assert_eq!(changes.new_range(4..5), None);
        assert_eq!(changes.new_range(5..6), None);
        assert_eq!(changes.new_range(6..7), Some(8..9));
        assert_eq!(changes.new_range(7..8), Some(9..10));
        assert_eq!(changes.old_offset(16), Some(14));
        assert_eq!(changes.old_offset(5), None);
        assert_eq!(changes.new_offset(14), Some(16));
        assert_eq!(changes.new_offset(4), None);
        assert_eq!(
            changes.changed(),
            vec![Change {
                old: 4..5,
                new: 4..7
            }]
        );
    }

    #[test]
    fn insertions_and_deletions() {
        let old = "abcdef";
        let mut text = old.to_string();
        let mut changes = Changes::none(old.len());

        apply(&mut text, &mut changes, 3, 3, "XY");
        apply(&mut text, &mut changes, 0, 1, "");
        assert_eq!(text, "bcXYdef");
        assert_eq!(
            changes.changed(),
            vec![
                Change {
                    old: 0..1,
                    new: 0..0
                },
                Change {
                    old: 3..3,
                    new: 2..4
                }
            ]
        );
        assert_eq!(changes.new_range(4..5), Some(5..6));
        // Text at the end of the file, with nothing appended.
        assert_eq!(changes.new_range(5..6), Some(6..7));
        // Text right next to an insertion.
        assert_eq!(changes.new_range(3..4), None);
        assert_eq!(changes.old_range(5..6), Some(4..5));
        assert!(!changes.is_none());

        // Text at the start of the file, with something typed before it.
        let mut changes = Changes::none(3);

        changes.replace(0, 0, 1);
        assert_eq!(changes.new_range(0..1), None);
        assert_eq!(changes.new_range(1..2), Some(2..3));
        assert!(Changes::none(3).is_none());
    }

    #[test]
    fn whole_texts() {
        let changes = Changes::between("fn a { 1 }", "fn a { 12 }");

        assert_eq!(
            changes.changed(),
            vec![Change {
                old: 8..8,
                new: 8..9
            }]
        );
        assert!(Changes::between("é", "é").is_none());

        let changes = Changes::between("aé", "aè");

        assert_eq!(
            changes.changed(),
            vec![Change {
                old: 1..3,
                new: 1..3
            }]
        );
    }

    #[test]
    fn snapshots_follow_edits() {
        let a = PathBuf::from("a.inko");
        let b = PathBuf::from("b.inko");
        let mut snapshots = Snapshots::default();
        let docs = HashMap::from([(a.clone(), "abc".to_string())]);

        snapshots.sent(1, &docs);
        // An edit while the check runs.
        snapshots.edited(&a, "abc", 0, 0, 1);
        snapshots.finished(1, std::slice::from_ref(&a));

        let f = snapshots.fallback(&a, None).unwrap();

        assert_eq!(f.contents[&a], "abc");
        assert_eq!(f.changes[&a].new_range(1..2), Some(2..3));
        assert_eq!(snapshots.fallback(&b, None), None);

        // A check that didn't type method bodies changes nothing.
        snapshots.sent(2, &HashMap::from([(a.clone(), "xabc".to_string())]));
        snapshots.finished(2, &[]);
        assert_eq!(snapshots.fallback(&a, None), Some(f.clone()));

        // A file opened later was read from disk.
        snapshots.opened(&b, Some("disk"), "disk!");
        assert_eq!(snapshots.fallback(&a, None).unwrap().contents[&b], "disk");
        assert_eq!(
            snapshots.fallback(&a, None).unwrap().changes[&b].changed(),
            vec![Change {
                old: 4..4,
                new: 4..5
            }]
        );

        snapshots.replaced(&a, "xabc", "xab");
        assert_eq!(
            snapshots.fallback(&a, None).unwrap().changes[&a].changed(),
            vec![
                Change {
                    old: 0..0,
                    new: 0..1
                },
                Change {
                    old: 2..3,
                    new: 3..3
                }
            ]
        );
    }

    #[test]
    fn snapshots_are_bounded() {
        let mut snapshots = Snapshots::default();

        for check in 0..10 {
            let file = PathBuf::from(format!("{}.inko", check));

            snapshots.sent(check, &HashMap::from([(file.clone(), String::new())]));
            snapshots.finished(check, &[file]);
        }

        assert_eq!(snapshots.len(), MAX_VERSIONS);
        assert!(snapshots.fallback(Path::new("0.inko"), None).is_none());

        // Checks that never finish (they were killed) are forgotten.
        for check in 100..110 {
            snapshots.sent(check, &HashMap::new());
        }

        assert_eq!(snapshots.running.len(), MAX_RUNNING);
        assert!(snapshots.fallback(Path::new("9.inko"), None).is_some());

        // Versions no file uses any more go.
        let file = PathBuf::from("9.inko");

        snapshots.sent(10, &HashMap::from([(file.clone(), String::new())]));
        snapshots.finished(10, std::slice::from_ref(&file));
        assert_eq!(snapshots.len(), MAX_VERSIONS);

        // A version no file uses any more goes.
        let mut snapshots = Snapshots::default();
        let a = PathBuf::from("a.inko");

        for check in 0..2 {
            snapshots.sent(check, &HashMap::new());
            snapshots.finished(check, std::slice::from_ref(&a));
        }

        assert_eq!(snapshots.len(), 1);
    }

    #[test]
    fn the_number_of_runs_is_bounded() {
        let mut text = "x".repeat(MAX_RUNS * 4);
        let mut changes = Changes::none(text.len());

        for i in 0..MAX_RUNS + 10 {
            apply(&mut text, &mut changes, i * 3, i * 3 + 1, "y");
        }

        assert!(changes.same.len() <= MAX_RUNS);
    }
}
