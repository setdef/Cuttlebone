//! The edit-replay corpus: the oracle for answers in code being edited.
//!
//! Real files (fixtures, and std's own modules and tests) are edited the way
//! people edit: a statement typed character by character, lines deleted and
//! typed again, a method's signature typed before its body, a name in a
//! declaration deleted and typed again (which breaks the declaration for a
//! while), in the file asked about, in a module it imports, in one that
//! imports it, or in an unrelated one. After each edit every query is asked
//! at many positions, the way a server sees them (`query::ask`, which the
//! worker runs too), and in the fixtures, references too.
//!
//! Each scenario ends with the files as they began, so they compile again.
//! Every answer given mid-edit is then compared with the fresh answer at the
//! same position in the final text. Only positions whose text didn't change
//! between that edit and the end are asked about, and lists (highlights,
//! hints) are compared on that text only.
//!
//! An answer from anything but the documents as they are (`Source`), or
//! from the documents as they are past an error in a declaration
//! (`Source::Guarded`), that disagrees with the final fresh one is
//! **wrong**, and fails the test. A
//! fresh answer can disagree too, because the code meant something else
//! then (after a `let` is deleted, its variable is undefined); those are
//! counted apart. The test also fails if fewer questions get an answer
//! than were measured (`COVERAGE`).
//!
//! Every commit replays a sample (`SAMPLE`); with `CUTTLEBONE_CORPUS_FULL`
//! set, the milestone tier and the mutation runner replay all of it
//! (AGENTS.md). `cargo test --test integration replay:: -- --nocapture` prints the
//! tables.

// The tables are this test's report.
#![allow(clippy::print_stderr)]

use cuttlebone::dump::upstream_std;
use cuttlebone::edits::{Changes, Snapshots};
use cuttlebone::position::{self, Encoding, Position, position_at};
use cuttlebone::query::{self, Answer, Question, Reply, Request, Session, Source};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const ENCODING: Encoding = Encoding::Utf16;

/// Positions asked about per edit, at most, for the questions one check
/// answers together.
const POSITIONS: usize = 40;

/// References are asked at this many of the positions, at most.
const REFERENCE_POSITIONS: usize = 10;

/// Completion and signature help each check a copy of their own, so they're
/// asked at fewer positions, and only after every `EVERY`th edit.
const EDITED_POSITIONS: usize = 2;
const EVERY: usize = 3;

/// An edit: bytes `start..end` of a file replaced by `text`.
#[derive(Clone, Debug)]
struct Edit {
    file: PathBuf,
    start: usize,
    end: usize,
    text: String,
}

struct Scenario {
    name: String,
    session: Session,
    /// The file the questions are about.
    file: PathBuf,
    /// Every file the scenario opens, with its text.
    open: Vec<(PathBuf, String)>,
    edits: Vec<Edit>,
    /// Questions are asked after every `every`th edit (and the last): 1
    /// for the full corpus, more for a scenario in the sample.
    every: usize,
    /// Whether references are asked too. Each step's are a check of the
    /// whole project, which std is too big to afford at every edit.
    references: bool,
}

/// The line containing `needle`: its start, and its end before the `\n`.
fn line_of(text: &str, needle: &str) -> (usize, usize) {
    let at = text
        .find(needle)
        .unwrap_or_else(|| panic!("{:?} isn't in the text", needle));
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let end = at + text[at..].find('\n').unwrap_or(text.len() - at);

    (start, end)
}

/// Deletes the statement on the line containing `needle` (keeping its
/// indentation), then types it again, one character at a time.
fn retype_line(file: &Path, text: &str, needle: &str) -> Vec<Edit> {
    let (start, end) = line_of(text, needle);
    let line = &text[start..end];
    let from = start + (line.len() - line.trim_start().len());
    let statement = &text[from..end];
    let mut edits = vec![Edit {
        file: file.to_path_buf(),
        start: from,
        end,
        text: String::new(),
    }];
    let mut at = from;

    for c in statement.chars() {
        edits.push(Edit {
            file: file.to_path_buf(),
            start: at,
            end: at,
            text: c.to_string(),
        });
        at += c.len_utf8();
    }

    edits
}

/// Deletes `count` lines starting at the one containing `needle`, the last
/// first, then types them again, one line at a time.
fn retype_lines(file: &Path, text: &str, needle: &str, count: usize) -> Vec<Edit> {
    let (start, _) = line_of(text, needle);
    let mut lines = Vec::new();
    let mut at = start;

    for _ in 0..count {
        let end = at + text[at..].find('\n').unwrap() + 1;

        lines.push((at, text[at..end].to_string()));
        at = end;
    }

    let mut edits: Vec<Edit> = lines
        .iter()
        .rev()
        .map(|(at, line)| Edit {
            file: file.to_path_buf(),
            start: *at,
            end: at + line.len(),
            text: String::new(),
        })
        .collect();

    for (at, line) in &lines {
        edits.push(Edit {
            file: file.to_path_buf(),
            start: *at,
            end: *at,
            text: line.clone(),
        });
    }

    edits
}

/// Deletes the method whose first line contains `needle` (up to the line
/// where its braces balance), then types its signature one character at a
/// time, and its body one line at a time.
fn retype_method(file: &Path, text: &str, needle: &str) -> Vec<Edit> {
    let (start, first_end) = line_of(text, needle);
    let mut depth = 0;
    let mut end = start;

    for line in text[start..].split_inclusive('\n') {
        depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
        end += line.len();

        if depth == 0 {
            break;
        }
    }

    let mut edits = vec![Edit {
        file: file.to_path_buf(),
        start,
        end,
        text: String::new(),
    }];
    let mut at = start;

    for c in text[start..=first_end].chars() {
        edits.push(Edit {
            file: file.to_path_buf(),
            start: at,
            end: at,
            text: c.to_string(),
        });
        at += c.len_utf8();
    }

    for line in text[first_end + 1..end].split_inclusive('\n') {
        edits.push(Edit {
            file: file.to_path_buf(),
            start: at,
            end: at,
            text: line.to_string(),
        });
        at += line.len();
    }

    edits
}

/// Deletes `word` (its first occurrence in the line containing `needle`)
/// one character at a time from its end, then types it again.
fn retype_word(file: &Path, text: &str, needle: &str, word: &str) -> Vec<Edit> {
    let (start, end) = line_of(text, needle);
    let from = start
        + text[start..end]
            .find(word)
            .unwrap_or_else(|| panic!("{:?} isn't in {:?}", word, &text[start..end]));
    let chars: Vec<(usize, char)> = word.char_indices().collect();
    let mut edits = Vec::new();

    for (i, c) in chars.iter().rev() {
        edits.push(Edit {
            file: file.to_path_buf(),
            start: from + i,
            end: from + i + c.len_utf8(),
            text: String::new(),
        });
    }

    for (i, c) in &chars {
        edits.push(Edit {
            file: file.to_path_buf(),
            start: from + i,
            end: from + i,
            text: c.to_string(),
        });
    }

    edits
}

fn fixture_session(name: &str) -> (Session, PathBuf) {
    let root = crate::common::fixture(name);

    (
        Session {
            std: upstream_std(),
            projects: vec![root.clone()],
        },
        root,
    )
}

fn std_session() -> (Session, PathBuf) {
    let root = upstream_std().parent().unwrap().to_path_buf();

    (
        Session {
            std: upstream_std(),
            projects: vec![root.clone()],
        },
        root,
    )
}

/// A scenario editing one file and asking about it.
fn scenario(
    name: &str,
    (session, root): (Session, PathBuf),
    file: &str,
    edits: impl FnOnce(&Path, &str) -> Vec<Edit>,
) -> Scenario {
    let path = root.join(file);
    let text = std::fs::read_to_string(&path).unwrap();

    Scenario {
        name: name.to_string(),
        session,
        edits: edits(&path, &text),
        file: path.clone(),
        open: vec![(path, text)],
        every: 1,
        references: !root.join("src/std").is_dir(),
    }
}

/// A scenario editing `edited` and asking about `file`, which imports it.
fn elsewhere(
    name: &str,
    (session, root): (Session, PathBuf),
    file: &str,
    edited: &str,
    edits: impl FnOnce(&Path, &str) -> Vec<Edit>,
) -> Scenario {
    let mut s = scenario(name, (session, root.clone()), edited, edits);
    let path = root.join(file);

    s.open
        .push((path.clone(), std::fs::read_to_string(&path).unwrap()));
    s.file = path;
    s
}

fn scenarios() -> Vec<Scenario> {
    let features = || fixture_session("features");
    let navigation = || fixture_session("navigation");
    let main = "src/main.inko";
    let set = "src/std/set.inko";
    let range = "src/std/range.inko";
    let test_set = "test/std/test_set.inko";

    vec![
        scenario("features: retype a let", features(), main, |p, t| {
            retype_line(p, t, "let doubled = numbers.iter")
        }),
        scenario("features: retype a call", features(), main, |p, t| {
            retype_line(p, t, "counter.add(amount: 1")
        }),
        scenario(
            "features: delete and retype lines",
            features(),
            main,
            |p, t| retype_lines(p, t, "total = total + 1", 3),
        ),
        scenario("features: retype a method", features(), main, |p, t| {
            retype_method(p, t, "fn describe(name")
        }),
        scenario(
            "features: break an argument's type",
            features(),
            main,
            |p, t| retype_word(p, t, "fn mut add(amount", "Int"),
        ),
        scenario(
            "features: break a field's type",
            features(),
            main,
            |p, t| retype_word(p, t, "let @label: String", "String"),
        ),
        scenario("navigation: retype a let", navigation(), main, |p, t| {
            retype_line(p, t, "let areas = circles")
        }),
        scenario("navigation: break an import", navigation(), main, |p, t| {
            retype_word(p, t, "import alpha (Error)", "Error")
        }),
        elsewhere(
            "navigation: break a return type in an import",
            navigation(),
            main,
            "src/alpha.inko",
            |p, t| retype_word(p, t, "fn pub message -> String", "String"),
        ),
        elsewhere(
            "navigation: break an impl's trait in an import",
            navigation(),
            main,
            "src/geometry/shapes.inko",
            |p, t| retype_word(p, t, "impl Area for Circle", "Area"),
        ),
        scenario("navigation: rename a type", navigation(), main, |p, t| {
            retype_word(p, t, "type pub Point {", "Point")
        }),
        elsewhere(
            "navigation: break a module that imports the file",
            navigation(),
            "src/alpha.inko",
            main,
            |p, t| retype_word(p, t, "fn describe(shape: ref Shape) -> Int", "Shape"),
        ),
        elsewhere(
            "navigation: break an unrelated module",
            navigation(),
            "src/qualified.inko",
            "src/beta.inko",
            |p, t| retype_word(p, t, "fn pub message -> String", "String"),
        ),
        elsewhere(
            "navigation: retype a method in an import",
            navigation(),
            main,
            "src/beta.inko",
            |p, t| retype_method(p, t, "fn pub message"),
        ),
        scenario("std set: retype a method", std_session(), set, |p, t| {
            retype_method(p, t, "fn pub mut remove(value: ref V)")
        }),
        scenario(
            "std set: break a return type",
            std_session(),
            set,
            |p, t| retype_word(p, t, "fn pub iter -> Iter[V]", "Iter"),
        ),
        scenario(
            "std range: delete and retype lines",
            std_session(),
            range,
            |p, t| retype_lines(p, t, "fn pub inline size -> Int", 4),
        ),
        scenario(
            "std test_set: retype a let",
            std_session(),
            test_set,
            |p, t| retype_line(p, t, "let set = Set.from_array([10, 20, 30])"),
        ),
        // The array's type comes from the pushes typed again.
        scenario(
            "std test_int: retype a closure",
            std_session(),
            "test/std/test_int.inko",
            |p, t| retype_line(p, t, "3.times(fn (v) { vals.push(v) })"),
        ),
        scenario(
            "std test_array: delete and retype pushes",
            std_session(),
            "test/std/test_array.inko",
            |p, t| retype_lines(p, t, "    vals.push(10)\n    vals.push(20)", 2),
        ),
    ]
}

/// The requests the corpus asks, as the server names them.
const REQUESTS: &[&str] = &[
    "hover",
    "definition",
    "typeDefinition",
    "completion",
    "signatureHelp",
    "inlayHint",
    "inlayHint, whole file",
    "documentHighlight",
    "references",
];

fn request_name(q: Question) -> &'static str {
    match q {
        Question::Type => "hover",
        Question::Definition => "definition",
        Question::TypeDefinition => "typeDefinition",
        Question::Complete => "completion",
        Question::SignatureHelp => "signatureHelp",
        Question::InlayHints { .. } => "inlayHint",
        Question::Highlight => "documentHighlight",
        Question::References { .. } => "references",
        other => panic!("not asked: {:?}", other),
    }
}

/// What happened to one question.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Verdict {
    Right(Source),
    Wrong(Source),
    None,
}

#[derive(Default, Clone)]
struct Tally {
    counts: BTreeMap<&'static str, BTreeMap<Verdict, usize>>,
    /// The first few wrong answers, to show.
    wrong: Vec<String>,
    /// The first few fresh answers that differ at the end, to show.
    differs: Vec<String>,
    /// Inlay hint answers that cover only part of what they were asked.
    partial: usize,
    /// Hints the final text has, in text that was there then, where those
    /// answers say they don't cover: where milestone 5's answers looked
    /// like "no hints here".
    uncovered: usize,
}

impl Tally {
    fn add(&mut self, request: &'static str, verdict: Verdict) {
        *self
            .counts
            .entry(request)
            .or_default()
            .entry(verdict)
            .or_default() += 1;
    }

    fn merge(&mut self, other: &Tally) {
        for (request, counts) in &other.counts {
            for (verdict, n) in counts {
                *self
                    .counts
                    .entry(request)
                    .or_default()
                    .entry(*verdict)
                    .or_default() += n;
            }
        }

        self.wrong.extend(other.wrong.iter().cloned());
        self.differs.extend(other.differs.iter().cloned());
        self.partial += other.partial;
        self.uncovered += other.uncovered;
    }

    fn get(&self, request: &str, verdict: Verdict) -> usize {
        self.counts
            .get(request)
            .and_then(|c| c.get(&verdict))
            .copied()
            .unwrap_or(0)
    }

    fn asked(&self, request: &str) -> usize {
        self.counts.get(request).map_or(0, |c| c.values().sum())
    }

    fn answered(&self, request: &str) -> usize {
        self.asked(request) - self.get(request, Verdict::None)
    }

    fn wrong(&self, request: &str, source: Source) -> usize {
        self.get(request, Verdict::Wrong(source))
    }

    fn table(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "| request | asked | fresh | fresh, past declaration errors | repaired | stale | no answer | fresh, differs at the end | wrong |"
        );
        let _ = writeln!(out, "|---|---|---|---|---|---|---|---|---|");

        for request in REQUESTS {
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} |",
                request,
                self.asked(request),
                self.get(request, Verdict::Right(Source::Fresh)),
                self.get(request, Verdict::Right(Source::Guarded)),
                self.get(request, Verdict::Right(Source::Repaired)),
                self.get(request, Verdict::Right(Source::Stale)),
                self.get(request, Verdict::None),
                self.wrong(request, Source::Fresh),
                self.wrong(request, Source::Guarded)
                    + self.wrong(request, Source::Stale)
                    + self.wrong(request, Source::Repaired),
            );
        }

        out
    }
}

/// The open documents, edited as a server would see them.
struct Documents {
    texts: HashMap<PathBuf, String>,
}

impl Documents {
    fn request(&self, file: &Path) -> Request {
        Request {
            file: file.to_path_buf(),
            position: Position {
                line: 0,
                character: 0,
            },
            encoding: ENCODING,
            contents: self.texts.clone(),
        }
    }

    fn apply(&mut self, edit: &Edit) {
        self.texts
            .get_mut(&edit.file)
            .unwrap()
            .replace_range(edit.start..edit.end, &edit.text);
    }
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Byte offsets in `text` to ask about: where names start, after a `.`
/// (for completion) and after a `(` or `, ` (for signature help). Only
/// those whose text is unchanged at the end (`changes`), spread over the
/// file, with every one near the edit at `near`.
fn positions(text: &str, changes: &Changes, near: usize) -> (Vec<usize>, Vec<usize>, Vec<usize>) {
    let mut names = Vec::new();
    let mut dots = Vec::new();
    let mut calls = Vec::new();
    let mut previous = '\n';

    for (i, c) in text.char_indices() {
        let unchanged = changes.new_range(i..i + c.len_utf8()).is_some();

        if unchanged && (is_ident(c) || c == '@') && !is_ident(previous) && previous != '@' {
            names.push(i);
        }

        if unchanged && previous == '.' && is_ident(c) {
            dots.push(i);
        }

        if unchanged && (previous == '(' || (previous == ' ' && text[..i].ends_with(", "))) {
            calls.push(i);
        }

        previous = c;
    }

    let line = |i: usize| text[..i].matches('\n').count();
    let edited = line(near.min(text.len()));
    let spread = |all: Vec<usize>, limit: usize| -> Vec<usize> {
        let (close, far): (Vec<usize>, Vec<usize>) = all
            .into_iter()
            .partition(|i| line(*i).abs_diff(edited) <= 3);
        let room = limit.saturating_sub(close.len()).max(limit / 2);
        let stride = far.len().div_ceil(room.max(1)).max(1);
        let mut out: Vec<usize> = close.into_iter().take(limit).collect();

        out.extend(far.into_iter().step_by(stride));
        out.sort();
        out
    };

    (
        spread(names, POSITIONS),
        spread(dots, EDITED_POSITIONS),
        spread(calls, EDITED_POSITIONS),
    )
}

/// The end of `position`'s line.
fn line_end(text: &str, position: Position) -> Position {
    let line = text.split('\n').nth(position.line as usize).unwrap_or("");

    Position {
        line: position.line,
        character: ENCODING.len(line),
    }
}

/// A position in one text as one in another, through `changes`.
fn map_position(old: &str, new: &str, changes: &Changes, p: Position) -> Option<Position> {
    let byte = position::byte_offset(old, p, ENCODING)?;
    let mapped = changes.new_range(byte..byte)?;

    Some(position_at(new, mapped.start, ENCODING))
}

/// A range in one text as one in another.
fn map_range(old: &str, new: &str, changes: &Changes, r: query::Range) -> Option<query::Range> {
    let start = position::byte_offset(old, r.start, ENCODING)?;
    let end = position::byte_offset(old, r.end, ENCODING)?;
    let mapped = changes.new_range(start..end)?;

    Some(query::Range {
        start: position_at(new, mapped.start, ENCODING),
        end: position_at(new, mapped.end, ENCODING),
    })
}

/// How the texts at one point of the scenario relate to the final ones.
struct Mapping {
    /// Each edited file's text now, its final text, and the changes from
    /// one to the other.
    files: HashMap<PathBuf, (String, String, Changes)>,
}

impl Mapping {
    /// A range in `file` now, as one in the final text. Files that aren't
    /// edited stay as they are.
    fn forward(&self, file: &Path, r: query::Range) -> Option<query::Range> {
        match self.files.get(file) {
            Some((now, last, changes)) => map_range(now, last, changes, r),
            None => Some(r),
        }
    }

    /// A range of the final text as one now, if it was there, unchanged.
    fn backward(&self, file: &Path, r: query::Range) -> Option<query::Range> {
        match self.files.get(file) {
            Some((now, last, changes)) => {
                let start = position::byte_offset(last, r.start, ENCODING)?;
                let end = position::byte_offset(last, r.end, ENCODING)?;
                let mapped = changes.old_range(start..end)?;

                Some(query::Range {
                    start: position_at(now, mapped.start, ENCODING),
                    end: position_at(now, mapped.end, ENCODING),
                })
            }
            None => Some(r),
        }
    }

    /// Whether a range of the final text was there, unchanged, now.
    fn existed(&self, file: &Path, r: query::Range) -> bool {
        match self.files.get(file) {
            Some((_, last, changes)) => {
                let (Some(start), Some(end)) = (
                    position::byte_offset(last, r.start, ENCODING),
                    position::byte_offset(last, r.end, ENCODING),
                ) else {
                    return false;
                };

                changes.old_range(start..end).is_some()
            }
            None => true,
        }
    }
}

/// Whether a position is in one of `covered`'s ranges, ends included.
fn within(covered: &[query::Range], p: Position) -> bool {
    covered.iter().any(|r| r.start <= p && p <= r.end)
}

/// A reply as something to compare: mid-edit (`now`) replies are mapped to
/// the final text first. Items of a list in text that changed in between
/// are left out on both sides: `now` drops what doesn't map forward, the
/// final reply what didn't exist now. Hints are compared where the
/// mid-edit answer says it covers (`covered`, ranges now), on both sides:
/// there, it must have exactly the final text's.
fn normalize(
    reply: &Option<Reply>,
    file: &Path,
    mapping: &Mapping,
    now: bool,
    covered: Option<&[query::Range]>,
) -> String {
    let range = |r: query::Range| -> String {
        if now {
            match mapping.forward(file, r) {
                Some(r) => format!("{:?}", r),
                None => "<changed>".to_string(),
            }
        } else {
            format!("{:?}", r)
        }
    };
    let keep = |f: &Path, r: query::Range| -> bool {
        if now {
            mapping.forward(f, r).is_some()
        } else {
            mapping.existed(f, r)
        }
    };
    let place = |p: &query::Place| -> String {
        if now {
            match mapping.forward(&p.file, p.range) {
                Some(r) => format!("{}:{:?}", p.file.display(), r),
                None => "<changed>".to_string(),
            }
        } else {
            format!("{}:{:?}", p.file.display(), p.range)
        }
    };

    match reply {
        None => "null".to_string(),
        Some(Reply::Type(t)) => format!("{} {}", t.typ, range(t.range)),
        Some(Reply::Members(m)) => format!(
            "{} {} {:?}",
            m.receiver,
            range(m.receiver_range),
            m.members
                .iter()
                .map(|m| format!("{} {}", m.name, m.signature))
                .collect::<Vec<_>>()
        ),
        Some(Reply::Place(p)) => place(p),
        // References, compared as highlights are: in the text both have.
        Some(Reply::Places(ps)) => ps
            .iter()
            .filter(|p| keep(&p.file, p.range))
            .map(place)
            .collect::<Vec<_>>()
            .join(", "),
        Some(Reply::Highlights(hs)) => hs
            .iter()
            .filter(|h| keep(file, h.range))
            .map(|h| format!("{:?} {}", h.access, range(h.range)))
            .collect::<Vec<_>>()
            .join(", "),
        Some(Reply::Signature(s)) => format!("{} {:?} {:?}", s.label, s.parameters, s.active),
        Some(Reply::Hints(hs)) => hs
            .hints
            .iter()
            .filter(|h| {
                let at = query::Range {
                    start: h.position,
                    end: h.position,
                };
                let then = if now {
                    Some(at)
                } else {
                    mapping.backward(file, at)
                };

                keep(file, at) && covered.is_none_or(|c| then.is_some_and(|r| within(c, r.start)))
            })
            .map(|h| {
                let at = query::Range {
                    start: h.position,
                    end: h.position,
                };

                format!("{} {:?} {}", h.label, h.kind, range(at))
            })
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// One question asked mid-edit, waiting for the final answer to compare
/// with.
struct Asked {
    request: &'static str,
    /// The question at its position in the final text.
    question: (Question, Position),
    /// The index of the edit it was asked after, in `mappings`.
    /// Which of `mappings` the answer's text maps to the end through.
    step: usize,
    answer: Answer<Reply>,
    /// For inlay hints from another check, the parts of the request the
    /// answer covers (its own `covered`), and the whole request.
    covered: Option<(Vec<query::Range>, query::Range)>,
    /// A hint given outside what the answer says it covers.
    outside: bool,
    /// `answer`, normalized against the final text.
    normalized: String,
    context: String,
}

/// Runs a scenario: applies each edit, asks every question after it, and
/// compares the answers with the final ones.
/// Without `memory`, the server keeps no older checks (as just after it
/// starts, or for a file that was never good), so no answer is stale.
/// Without `repair`, no repaired copies are checked.
fn replay(scenario: &Scenario, memory: bool, repair: bool) -> Tally {
    let mut docs = Documents {
        texts: scenario.open.iter().cloned().collect(),
    };
    let finals: HashMap<PathBuf, String> = docs.texts.clone();
    let file = &scenario.file;
    let mut asked: Vec<Asked> = Vec::new();
    let mut mappings: Vec<Mapping> = Vec::new();
    // What the server keeps, from a check when the files are opened (as
    // their diagnostics would be) and each check since.
    let mut snapshots = Snapshots::default();
    // As a worker keeps checks between jobs (none with
    // CUTTLEBONE_NO_CACHE set).
    let cache = query::Cache::new(cuttlebone::worker::kept_checks());
    let ask = |snapshots: &mut Snapshots,
               docs: &Documents,
               check: u64,
               questions: &[(Question, Position)]| {
        snapshots.sent(check, &docs.texts);

        let fallback = snapshots.fallback(file, None).filter(|_| memory);
        let answers = query::ask_with(
            &cache,
            &scenario.session,
            &docs.request(file),
            fallback.as_ref(),
            repair,
            questions,
        );
        let typed = answers.iter().any(|a| a.typed);

        snapshots.finished(
            check,
            if typed {
                std::slice::from_ref(file)
            } else {
                &[]
            },
        );
        answers
    };

    ask(
        &mut snapshots,
        &docs,
        0,
        &[(
            Question::Type,
            Position {
                line: 0,
                character: 0,
            },
        )],
    );

    for (step, edit) in scenario.edits.iter().enumerate() {
        snapshots.edited(
            &edit.file,
            &docs.texts[&edit.file],
            edit.start,
            edit.end,
            edit.text.len(),
        );
        docs.apply(edit);

        // The sample asks after every `every`th edit, and the last.
        if step % scenario.every != 0 && step + 1 != scenario.edits.len() {
            continue;
        }

        let now = docs.texts[file].clone();
        let mapping = Mapping {
            files: scenario
                .open
                .iter()
                .map(|(path, _)| {
                    let (a, b) = (&docs.texts[path], &finals[path]);

                    (path.clone(), (a.clone(), b.clone(), Changes::between(a, b)))
                })
                .collect(),
        };
        let changes = &mapping.files[file].2;
        let near = if &edit.file == file { edit.start } else { 0 };
        let (names, dots, calls) = positions(&now, changes, near);
        let at = |byte: usize| position_at(&now, byte, ENCODING);
        let mut questions: Vec<(Question, Position)> = Vec::new();
        let mut lines = Vec::new();

        for byte in &names {
            let p = at(*byte);

            for q in [
                Question::Type,
                Question::Definition,
                Question::TypeDefinition,
                Question::Highlight,
            ] {
                questions.push((q, p));
            }

            if !lines.contains(&p.line) {
                lines.push(p.line);

                let start = Position {
                    line: p.line,
                    character: 0,
                };

                questions.push((
                    Question::InlayHints {
                        end: line_end(&now, start),
                    },
                    start,
                ));
            }
        }

        // The whole file, as clients ask for hints.
        questions.push((
            Question::InlayHints {
                end: position_at(&now, now.len(), ENCODING),
            },
            Position {
                line: 0,
                character: 0,
            },
        ));

        if scenario.references {
            let stride = names.len().div_ceil(REFERENCE_POSITIONS).max(1);

            questions.extend(names.iter().step_by(stride).map(|b| {
                (
                    Question::References {
                        include_declaration: true,
                    },
                    at(*b),
                )
            }));
        }

        if step % EVERY == 0 {
            questions.extend(dots.iter().map(|b| (Question::Complete, at(*b))));
            questions.extend(calls.iter().map(|b| (Question::SignatureHelp, at(*b))));
        }

        let answers = ask(&mut snapshots, &docs, step as u64 + 1, &questions);
        let last = &finals[file];

        for ((question, p), answer) in questions.into_iter().zip(answers) {
            let whole = matches!(question, Question::InlayHints { end } if end == position_at(&now, now.len(), ENCODING) && p == Position { line: 0, character: 0 });
            let final_question = match question {
                Question::InlayHints { .. } if whole => (
                    Question::InlayHints {
                        end: position_at(last, last.len(), ENCODING),
                    },
                    p,
                ),
                Question::InlayHints { .. } => {
                    let Some(mapped) = map_position(&now, last, changes, p)
                        .or_else(|| first_unchanged_on_line(&now, last, changes, p))
                    else {
                        continue;
                    };
                    let start = Position {
                        line: mapped.line,
                        character: 0,
                    };

                    (
                        Question::InlayHints {
                            end: line_end(last, start),
                        },
                        start,
                    )
                }
                _ => (question, map_position(&now, last, changes, p).unwrap()),
            };
            let reply = answer.result.clone().unwrap_or(None);
            let covered = match (&reply, question) {
                (Some(Reply::Hints(h)), Question::InlayHints { end }) => h
                    .covered
                    .clone()
                    .map(|c| (c, query::Range { start: p, end })),
                _ => None,
            };
            let outside = match (&reply, &covered) {
                (Some(Reply::Hints(h)), Some((c, _))) => {
                    h.hints.iter().any(|h| !within(c, h.position))
                }
                _ => false,
            };

            asked.push(Asked {
                step: mappings.len(),
                request: if whole {
                    "inlayHint, whole file"
                } else {
                    request_name(question)
                },
                question: final_question,
                normalized: normalize(
                    &reply,
                    file,
                    &mapping,
                    true,
                    covered.as_ref().map(|c| c.0.as_slice()),
                ),
                covered,
                outside,
                context: format!(
                    "{} after edit {} ({:?}), {:?} at {:?}: {:?}",
                    scenario.name, step, edit.text, question, p, answer.source
                ),
                answer,
            });
        }

        mappings.push(mapping);
    }

    for (path, text) in &finals {
        assert_eq!(
            &docs.texts[path], text,
            "{} doesn't end as it began",
            scenario.name
        );
    }

    // The final answers, from the final text, which is the original.
    let mut unique: Vec<(Question, Position)> = Vec::new();

    for a in &asked {
        if !unique.contains(&a.question) {
            unique.push(a.question);
        }
    }

    let finals_answered = query::ask(&scenario.session, &docs.request(file), &unique);
    let mut tally = Tally::default();

    for a in &asked {
        let index = unique.iter().position(|q| *q == a.question).unwrap();
        let expected = &finals_answered[index];

        assert_eq!(
            expected.source,
            Source::Fresh,
            "the final text has errors: {}",
            a.context
        );

        let covered = a.covered.as_ref().map(|c| c.0.as_slice());

        // The final hints in text that was there, where the answer doesn't
        // cover.
        if let (Some((c, asked)), Ok(Some(Reply::Hints(h)))) = (&a.covered, &expected.result) {
            if c.as_slice() != [*asked] {
                tally.partial += 1;
            }

            tally.uncovered += h
                .hints
                .iter()
                .filter_map(|h| {
                    mappings[a.step].backward(
                        file,
                        query::Range {
                            start: h.position,
                            end: h.position,
                        },
                    )
                })
                .filter(|r| !within(c, r.start))
                .count();
        }

        let verdict = match &a.answer.result {
            Err(_) => Verdict::None,
            Ok(_) => {
                let expected = match &expected.result {
                    Ok(reply) => normalize(reply, file, &mappings[a.step], false, covered),
                    Err(e) => format!("unavailable: {}", e.code),
                };

                if a.outside {
                    tally
                        .wrong
                        .push(format!("{}\n  a hint outside what it covers", a.context));
                    Verdict::Wrong(a.answer.source)
                } else if expected == a.normalized {
                    Verdict::Right(a.answer.source)
                } else {
                    let shown = format!(
                        "{}\n  now:   {}\n  final: {}",
                        a.context, a.normalized, expected
                    );

                    if a.answer.source != Source::Fresh {
                        tally.wrong.push(shown);
                    } else if tally.differs.len() < 3 {
                        tally.differs.push(shown);
                    }

                    Verdict::Wrong(a.answer.source)
                }
            }
        };

        tally.add(a.request, verdict);
    }

    tally
}

/// For a line whose start changed, the first unchanged position on it.
fn first_unchanged_on_line(
    now: &str,
    last: &str,
    changes: &Changes,
    p: Position,
) -> Option<Position> {
    let line = now.split('\n').nth(p.line as usize)?;

    (0..ENCODING.len(line)).find_map(|character| {
        map_position(
            now,
            last,
            changes,
            Position {
                line: p.line,
                character,
            },
        )
    })
}

/// With this set (to anything), the test replays the whole corpus, as the
/// milestone tier and the mutation runner do (AGENTS.md); without it, the
/// sample below, as every commit does.
const FULL_VARIABLE: &str = "CUTTLEBONE_CORPUS_FULL";

/// The scenarios every commit replays, and after every how many edits each
/// asks its questions: one of each kind of edit, the long ones thinned.
const SAMPLE: &[(&str, usize)] = &[
    ("features: retype a let", 8),
    ("features: delete and retype lines", 1),
    ("features: retype a method", 8),
    ("features: break an argument's type", 1),
    ("navigation: break a return type in an import", 2),
    ("navigation: break a module that imports the file", 2),
    ("std range: delete and retype lines", 1),
];

/// The questions that got an answer, at least, per request: what the
/// corpus measured. Raise these when coverage improves.
/// The same as `COVERAGE` and the two below, for the sample.
const SAMPLE_COVERAGE: &[(&str, usize)] = &[
    ("hover", 1492),
    ("definition", 1505),
    ("typeDefinition", 1492),
    ("completion", 19),
    ("signatureHelp", 41),
    ("inlayHint", 770),
    ("inlayHint, whole file", 52),
    ("documentHighlight", 1426),
    ("references", 233),
];
const SAMPLE_COVERAGE_WITHOUT_MEMORY: &[(&str, usize)] = &[
    ("hover", 1426),
    ("definition", 1439),
    ("typeDefinition", 1426),
    ("completion", 19),
    ("signatureHelp", 41),
    ("inlayHint", 723),
    ("inlayHint, whole file", 49),
    ("documentHighlight", 1389),
    ("references", 238),
];
const SAMPLE_COVERAGE_WITHOUT_REPAIR: &[(&str, usize)] = &[
    ("hover", 1434),
    ("definition", 1447),
    ("typeDefinition", 1434),
    ("completion", 19),
    ("signatureHelp", 41),
    ("inlayHint", 747),
    ("inlayHint, whole file", 52),
    ("documentHighlight", 1377),
    ("references", 233),
];

const COVERAGE: &[(&str, usize)] = &[
    ("hover", 11904),
    ("definition", 12311),
    ("typeDefinition", 11990),
    ("completion", 115),
    ("signatureHelp", 382),
    ("inlayHint", 6652),
    ("inlayHint, whole file", 483),
    ("documentHighlight", 10789),
    ("references", 1249),
];

/// What the server answers without older checks, from repaired copies
/// only.
const COVERAGE_WITHOUT_MEMORY: &[(&str, usize)] = &[
    ("hover", 10350),
    ("definition", 10721),
    ("typeDefinition", 10400),
    ("completion", 109),
    ("signatureHelp", 311),
    ("inlayHint", 5784),
    ("inlayHint, whole file", 386),
    ("documentHighlight", 9616),
    ("references", 1261),
];

/// What older checks answer alone, without repaired copies.
const COVERAGE_WITHOUT_REPAIR: &[(&str, usize)] = &[
    ("hover", 9896),
    ("definition", 10217),
    ("typeDefinition", 9896),
    ("completion", 93),
    ("signatureHelp", 298),
    ("inlayHint", 5831),
    ("inlayHint, whole file", 483),
    ("documentHighlight", 9351),
    ("references", 1249),
];

#[test]
fn answers_mid_edit_agree_with_the_final_ones() {
    let full = std::env::var_os(FULL_VARIABLE).is_some();
    let scenarios: Vec<Scenario> = scenarios()
        .into_iter()
        .filter_map(|mut s| {
            if full {
                return Some(s);
            }

            let (_, every) = SAMPLE.iter().find(|(name, _)| *name == s.name)?;

            s.every = *every;
            Some(s)
        })
        .collect();
    let (coverage, without_memory, without_repair) = if full {
        (COVERAGE, COVERAGE_WITHOUT_MEMORY, COVERAGE_WITHOUT_REPAIR)
    } else {
        (
            SAMPLE_COVERAGE,
            SAMPLE_COVERAGE_WITHOUT_MEMORY,
            SAMPLE_COVERAGE_WITHOUT_REPAIR,
        )
    };
    let modes = [
        ("", true, true, coverage),
        (", without older checks", false, true, without_memory),
        (", without repaired copies", true, false, without_repair),
    ];

    assert_eq!(scenarios.len(), if full { 20 } else { SAMPLE.len() });
    let tallies: Vec<Vec<(Tally, std::time::Duration)>> = std::thread::scope(|s| {
        let handles: Vec<Vec<_>> = scenarios
            .iter()
            .map(|scenario| {
                modes
                    .iter()
                    .map(|(_, memory, repair, _)| {
                        s.spawn(move || {
                            let start = std::time::Instant::now();
                            let tally = replay(scenario, *memory, *repair);

                            (tally, start.elapsed())
                        })
                    })
                    .collect()
            })
            .collect();

        handles
            .into_iter()
            .map(|h| h.into_iter().map(|h| h.join().unwrap()).collect())
            .collect()
    });
    let mut totals = vec![Tally::default(); modes.len()];

    for (scenario, tallies) in scenarios.iter().zip(&tallies) {
        eprintln!(
            "\n{} ({} edits, asked after every {}; {:.1} s)\n\n{}",
            scenario.name,
            scenario.edits.len(),
            scenario.every,
            tallies[0].1.as_secs_f64(),
            tallies[0].0.table()
        );

        for (total, (tally, _)) in totals.iter_mut().zip(tallies) {
            total.merge(tally);
        }
    }

    for ((name, ..), total) in modes.iter().zip(&totals) {
        eprintln!(
            "\nAll scenarios{}\n\n{}\ninlay hint answers covering part of what they were asked: {}, hints of the final text, in text that was there, where they don't cover: {}",
            name,
            total.table(),
            total.partial,
            total.uncovered
        );
    }

    for differs in &totals[0].differs {
        eprintln!("FRESH, DIFFERS: {}", differs);
    }

    for wrong in totals.iter().flat_map(|t| &t.wrong).take(20) {
        eprintln!("WRONG: {}", wrong);
    }

    for ((name, _, _, coverage), tally) in modes.iter().zip(&totals) {
        for (request, minimum) in *coverage {
            assert!(
                tally.answered(request) >= *minimum,
                "{}{}: {} answered, fewer than the {} measured",
                request,
                name,
                tally.answered(request),
                minimum
            );
        }

        assert!(
            tally.wrong.is_empty(),
            "{} wrong answers{}",
            tally.wrong.len(),
            name
        );
    }
}
