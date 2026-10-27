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
//! Two make several edits and then undo them, the last first, as an
//! editor's undo does: then an answer from the documents as they are can
//! be about a name a later edit changed, which is counted apart
//! (`Verdict::Renamed`), since the final text has another name there.
//! Every answer given mid-edit is then compared with the fresh answer at the
//! same position in the final text. Only positions whose text didn't change
//! between that edit and the end are asked about, and lists (highlights,
//! hints) are compared on that text only.
//!
//! Completion is asked after a `.` and at the end of a name, as one is
//! typed (`completion, names`).
//!
//! Document symbols, from the text alone, are compared the same way after
//! every edit: each symbol given mid-edit, whose name is in text that
//! didn't change, must be one of the final text's, with the same kind,
//! names around it, detail and ranges (where they didn't change either).
//! One that a copy with lines blank left out is only counted.
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
use cuttlebone::syntax::{self, Outline};
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
    /// What happens before the edit of each index, besides edits:
    /// documents closed and opened, and files changed on disk. A scenario
    /// with any replays in a copy of its project of its own.
    events: Vec<(usize, Event)>,
    /// Files the copy of the project has besides its own.
    files: Vec<(PathBuf, String)>,
}

/// Something a client does besides editing.
#[derive(Clone, Debug)]
enum Event {
    /// The document closed, unsaved: its file's text on disk replaces it.
    Close(PathBuf),
    /// The document opened, with its file's text on disk.
    Open(PathBuf),
    /// The file written on disk (by a `git checkout`, or a refactoring
    /// with no editor open), whether its document is open or not.
    Disk(PathBuf, String),
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

/// Makes each edit in turn (replacing the first `.0` after the ones
/// before with `.1`), then undoes them, the last first, each as one edit
/// that puts back what it replaced: as an editor's undo does.
fn edit_and_undo(file: &Path, text: &str, edits: &[(&str, &str)]) -> Vec<Edit> {
    let mut texts = vec![text.to_string()];
    let mut forward = Vec::new();

    for (from, to) in edits {
        let old = texts.last().unwrap().clone();
        let start = old
            .find(from)
            .unwrap_or_else(|| panic!("{:?} isn't in the text", from));

        forward.push(Edit {
            file: file.to_path_buf(),
            start,
            end: start + from.len(),
            text: to.to_string(),
        });
        texts.push(old.replacen(from, to, 1));
    }

    let undone: Vec<Edit> = forward
        .iter()
        .zip(edits)
        .rev()
        .map(|(edit, (from, to))| Edit {
            file: file.to_path_buf(),
            start: edit.start,
            end: edit.start + to.len(),
            text: from.to_string(),
        })
        .collect();

    forward.into_iter().chain(undone).collect()
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
        events: Vec::new(),
        files: Vec::new(),
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

/// An import (`alpha`) edited, closed unsaved, and then changed on disk
/// while a line of `main` is typed again: its methods swapped (the same
/// length, so only where its edits ended can tell), then cut short, then
/// as it was, and opened again before the last edit. Answers from older
/// checks must map `alpha` through what happened on disk.
fn closed_and_changed_on_disk((session, root): (Session, PathBuf)) -> Scenario {
    let alpha = root.join("src/alpha.inko");
    let text = std::fs::read_to_string(&alpha).unwrap();
    let new = text.find("  fn pub static new").unwrap();
    let message = text.find("  fn pub message").unwrap();
    let end = text.rfind("}").unwrap();
    let swapped = format!(
        "{}{}\n\n{}{}",
        &text[..new],
        text[message..end].trim_end(),
        text[new..message].trim_end(),
        &text[end - 1..]
    );
    let shorter = format!("{}}}\n", &text[..message]);
    // A module of the copy's own, small enough that every name in it is
    // asked about: `first` uses alpha's methods, and answers from the last
    // good check while `second` is typed.
    let uses = root.join("src/uses.inko");
    let uses_text = "import alpha (Error)

fn pub first -> String {
  let error = Error.new('a')

  error.message
}

fn pub second -> Int {
  let values = [1, 2]

  values.size
}
";
    let edit = text.find("@text\n  }\n}").unwrap();
    let mut edits = vec![Edit {
        file: alpha.clone(),
        start: edit,
        end: edit + 5,
        text: "@text + ''".to_string(),
    }];
    let retyped = retype_line(&uses, uses_text, "let values = [1, 2]");
    let n = retyped.len();

    assert_eq!(swapped.len(), text.len(), "{}", swapped);
    edits.extend(retyped);

    // Closed once `let` is typed, so `uses` doesn't parse from then on,
    // and the last good check is one from before, which had alpha open.
    let closed = 4;

    Scenario {
        name: "navigation: close an import and change it on disk".to_string(),
        session,
        file: uses.clone(),
        open: vec![
            (uses.clone(), uses_text.to_string()),
            (alpha.clone(), text.clone()),
        ],
        edits,
        every: 1,
        references: true,
        events: vec![
            (closed, Event::Close(alpha.clone())),
            (closed, Event::Disk(alpha.clone(), swapped)),
            (
                closed + (n - closed) / 2,
                Event::Disk(alpha.clone(), shorter),
            ),
            (
                closed + 3 * (n - closed) / 4,
                Event::Disk(alpha.clone(), text),
            ),
            (n, Event::Open(alpha)),
        ],
        files: vec![(uses, uses_text.to_string())],
    }
}

/// `scenario` in a copy of its project in the tests' temporary
/// directory, named after it and `mode`, with every path in the copy.
fn in_copy(scenario: &Scenario, mode: &str) -> Scenario {
    let root = &scenario.session.projects[0];
    let name: String = scenario
        .name
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let copy = crate::common::tmp().join(format!("replay-{}-{}", name, mode));
    let _ = std::fs::remove_dir_all(&copy);

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();

        for entry in std::fs::read_dir(from).unwrap() {
            let path = entry.unwrap().path();
            let target = to.join(path.file_name().unwrap());

            if path.is_dir() {
                copy_dir(&path, &target);
            } else {
                std::fs::copy(&path, &target).unwrap();
            }
        }
    }

    copy_dir(root, &copy);

    let copy = copy.canonicalize().unwrap();
    let moved = |p: &Path| copy.join(p.strip_prefix(root).unwrap());

    for (path, text) in &scenario.files {
        std::fs::write(moved(path), text).unwrap();
    }

    Scenario {
        name: scenario.name.clone(),
        session: Session {
            std: scenario.session.std.clone(),
            projects: vec![copy.clone()],
        },
        file: moved(&scenario.file),
        open: scenario
            .open
            .iter()
            .map(|(p, t)| (moved(p), t.clone()))
            .collect(),
        edits: scenario
            .edits
            .iter()
            .map(|e| Edit {
                file: moved(&e.file),
                ..e.clone()
            })
            .collect(),
        every: scenario.every,
        references: scenario.references,
        files: Vec::new(),
        events: scenario
            .events
            .iter()
            .map(|(at, e)| {
                (
                    *at,
                    match e {
                        Event::Close(p) => Event::Close(moved(p)),
                        Event::Open(p) => Event::Open(moved(p)),
                        Event::Disk(p, t) => Event::Disk(moved(p), t.clone()),
                    },
                )
            })
            .collect(),
    }
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
        // A use removed, a declaration renamed that's used before and after
        // it, and a line that doesn't parse, then each undone.
        scenario("navigation: edit and undo", navigation(), main, |p, t| {
            edit_and_undo(
                p,
                t,
                &[
                    ("describe(Shape.Square(3))", "0"),
                    ("fn describe(", "fn describ("),
                    ("    case Dot -> 0\n", "    case Dot -> 0\n    if shape {\n"),
                ],
            )
        }),
        elsewhere(
            "navigation: edit and undo an import",
            navigation(),
            main,
            "src/alpha.inko",
            |p, t| {
                edit_and_undo(
                    p,
                    t,
                    &[
                        ("fn pub message -> String", "fn pub messag -> String"),
                        ("type pub Error", "type pub Erro"),
                    ],
                )
            },
        ),
        closed_and_changed_on_disk(navigation()),
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
    "completion, names",
    "signatureHelp",
    "inlayHint",
    "inlayHint, whole file",
    "documentHighlight",
    "references",
    "documentSymbol",
    "declaration",
    "prepareCallHierarchy",
    "outgoingCalls",
    "prepareTypeHierarchy",
    "implementation",
    "incomingCalls",
    "subtypes",
];

fn request_name(q: Question) -> &'static str {
    match q {
        Question::Hover => "hover",
        Question::Definition => "definition",
        Question::TypeDefinition => "typeDefinition",
        Question::Complete => "completion",
        Question::SignatureHelp => "signatureHelp",
        Question::InlayHints { .. } => "inlayHint",
        Question::Highlight => "documentHighlight",
        Question::References { .. } => "references",
        Question::Declaration => "declaration",
        Question::CallItem => "prepareCallHierarchy",
        Question::OutgoingCalls => "outgoingCalls",
        Question::TypeItem => "prepareTypeHierarchy",
        Question::Implementation => "implementation",
        Question::IncomingCalls => "incomingCalls",
        Question::Subtypes => "subtypes",
        other => panic!("not asked: {:?}", other),
    }
}

/// What happened to one question.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Verdict {
    Right(Source),
    Wrong(Source),
    /// An answer from the documents as they are (with lines blank, or
    /// past a declaration's error) about a name a later edit changed: it
    /// was about another name than the final text has there, so the final
    /// answer says nothing about it. Only an undo, which changes a name
    /// back after other edits, gives these.
    Renamed(Source),
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
    /// Document symbols the final text has, in text that was there then,
    /// that an answer from a copy with lines blank left out.
    missing_symbols: usize,
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
        self.missing_symbols += other.missing_symbols;
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

    fn renamed(&self) -> usize {
        self.counts
            .values()
            .flat_map(|c| c.iter())
            .filter(|(v, _)| matches!(v, Verdict::Renamed(_)))
            .map(|(_, n)| n)
            .sum()
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

/// The name around byte `at` of `text`: the identifier characters before
/// and after it.
fn word_at(text: &str, at: usize) -> &str {
    let start = text[..at].trim_end_matches(is_ident).len();
    let end = at + text[at..].len() - text[at..].trim_start_matches(is_ident).len();

    &text[start..end]
}

/// Byte offsets in `text` to ask about: where names start, after a `.`
/// (for completion) and after a `(` or `, ` (for signature help). Only
/// those whose text is unchanged at the end (`changes`), spread over the
/// file, with every one near the edit at `near`.
fn positions(
    text: &str,
    changes: &Changes,
    near: usize,
) -> (Vec<usize>, Vec<usize>, Vec<usize>, Vec<usize>) {
    let mut names = Vec::new();
    let mut dots = Vec::new();
    let mut calls = Vec::new();
    let mut ends = Vec::new();
    let mut previous = '\n';

    for (i, c) in text.char_indices() {
        let unchanged = changes.new_range(i..i + c.len_utf8()).is_some();

        if unchanged && (is_ident(c) || c == '@') && !is_ident(previous) && previous != '@' {
            names.push(i);
        }

        // Right after a name, not after a `.`'s: as a name is typed.
        if unchanged
            && is_ident(previous)
            && !is_ident(c)
            && changes.new_range(i - previous.len_utf8()..i).is_some()
        {
            let start = text[..i].trim_end_matches(is_ident).len();

            if !text[..start].ends_with('.') {
                ends.push(i);
            }
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
        spread(ends, EDITED_POSITIONS),
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

/// A document's symbols as lines to compare: each with the names it's in,
/// its kind, name and detail, and its name's and its own ranges. Mid-edit
/// (`now`) ones are mapped to the final text, and left out if their names
/// changed in between; final ones are left out if their names weren't
/// there then. A range that changed in between is `<changed>`.
fn symbol_lines(
    outline: &Outline,
    text: &str,
    file: &Path,
    mapping: &Mapping,
    now: bool,
) -> Vec<String> {
    let range = |r: &std::ops::Range<usize>| query::Range {
        start: position_at(text, r.start, ENCODING),
        end: position_at(text, r.end, ENCODING),
    };
    let shown = |r: query::Range| -> Option<String> {
        if now {
            mapping.forward(file, r).map(|r| format!("{:?}", r))
        } else {
            mapping.existed(file, r).then(|| format!("{:?}", r))
        }
    };

    outline
        .all()
        .into_iter()
        .filter_map(|(d, parents)| {
            let name = shown(range(&d.name_range))?;

            Some(format!(
                "{} {:?} {} | {} | {} | {}",
                parents
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
                    .join("."),
                d.kind,
                d.name,
                d.detail(),
                name,
                shown(range(&d.range)).unwrap_or_else(|| "<changed>".to_string())
            ))
        })
        .collect()
}

/// One step's document symbols, waiting for the final ones.
struct AskedSymbols {
    step: usize,
    /// `None` if there was no answer: the text didn't parse, nor any copy.
    source: Option<Source>,
    lines: Vec<String>,
    context: String,
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
        Some(Reply::Hover(h)) => format!(
            "{:?} {} {:?}",
            h.typ
                .as_ref()
                .map(|t| format!("{} {}", t.typ, range(t.range))),
            range(h.range),
            h.declared
                .as_ref()
                .map(|d| (&d.signature, &d.path, &d.documentation))
        ),
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
        Some(Reply::Items(items)) => items
            .iter()
            .filter(|i| keep(&i.selection.file, i.selection.range))
            .map(|i| item(i, &place, &keep))
            .collect::<Vec<_>>()
            .join(", "),
        // Calls, compared as references are: in the text both have.
        Some(Reply::Calls(calls)) => calls
            .iter()
            .filter(|c| keep(&c.item.selection.file, c.item.selection.range))
            .map(|c| {
                format!(
                    "{} from {}",
                    item(&c.item, &place, &keep),
                    c.ranges
                        .iter()
                        .filter(|p| keep(&p.file, p.range))
                        .map(place)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
            .collect::<Vec<_>>()
            .join("; "),
        Some(Reply::Uses(uses)) => uses
            .iter()
            .filter(|u| keep(&u.place.file, u.place.range))
            .map(|u| format!("{} {}", u.path, place(&u.place)))
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// A hierarchy's item, as `summary` compares it (only those whose name is
/// in text both have, as references are): its whole declaration only if
/// that is too (a later edit may change the declaration's body, and so
/// where it ends).
fn item(
    item: &query::Item,
    place: &dyn Fn(&query::Place) -> String,
    keep: &dyn Fn(&Path, query::Range) -> bool,
) -> String {
    let whole = if keep(&item.place.file, item.place.range) {
        place(&item.place)
    } else {
        String::new()
    };

    format!(
        "{} {:?} {} {} {}",
        item.name,
        item.kind,
        item.detail,
        place(&item.selection),
        whole
    )
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
    /// Whether the name at the position is another in the final text.
    renamed: bool,
    context: String,
}

/// Runs a scenario: applies each edit, asks every question after it, and
/// compares the answers with the final ones.
/// Without `memory`, the server keeps no older checks (as just after it
/// starts, or for a file that was never good), so no answer is stale.
/// Without `repair`, no repaired copies are checked.
fn replay(scenario: &Scenario, memory: bool, repair: bool) -> Tally {
    // A scenario that writes files replays in a copy of its project, one
    // for each mode, since the modes replay at once.
    let copied;
    let scenario = if scenario.events.is_empty() && scenario.files.is_empty() {
        scenario
    } else {
        copied = in_copy(scenario, &format!("{}-{}", memory, repair));
        &copied
    };
    let mut docs = Documents {
        texts: scenario.open.iter().cloned().collect(),
    };
    let finals: HashMap<PathBuf, String> = docs.texts.clone();
    let file = &scenario.file;
    let mut asked: Vec<Asked> = Vec::new();
    let mut asked_symbols: Vec<AskedSymbols> = Vec::new();
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
        for (_, event) in scenario.events.iter().filter(|(at, _)| *at == step) {
            match event {
                Event::Close(path) => {
                    let text = docs.texts.remove(path).unwrap();
                    let disk = std::fs::read_to_string(path).ok();

                    snapshots.closed(path, &text, disk.as_deref());
                }
                Event::Open(path) => {
                    let disk = std::fs::read_to_string(path).unwrap();

                    snapshots.opened(path, Some(&disk), &disk);
                    docs.texts.insert(path.clone(), disk);
                }
                Event::Disk(path, text) => std::fs::write(path, text).unwrap(),
            }
        }

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
                    // A closed document's text is its file's.
                    let a = docs
                        .texts
                        .get(path)
                        .cloned()
                        .unwrap_or_else(|| std::fs::read_to_string(path).unwrap());
                    let b = &finals[path];

                    (
                        path.clone(),
                        (a.clone(), b.clone(), Changes::between(&a, b)),
                    )
                })
                .collect(),
        };
        let changes = &mapping.files[file].2;
        let near = if &edit.file == file { edit.start } else { 0 };
        let (names, dots, calls, ends) = positions(&now, changes, near);
        let at = |byte: usize| position_at(&now, byte, ENCODING);
        let mut questions: Vec<(Question, Position)> = Vec::new();
        let mut lines = Vec::new();

        for byte in &names {
            let p = at(*byte);

            for q in [
                Question::Hover,
                Question::Definition,
                Question::TypeDefinition,
                Question::Highlight,
                Question::Declaration,
                Question::CallItem,
                Question::OutgoingCalls,
                Question::TypeItem,
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

            questions.extend(names.iter().step_by(stride).flat_map(|b| {
                [
                    Question::References {
                        include_declaration: true,
                    },
                    Question::Implementation,
                    Question::IncomingCalls,
                    Question::Subtypes,
                ]
                .map(|q| (q, at(*b)))
            }));
        }

        if step % EVERY == 0 {
            questions.extend(dots.iter().map(|b| (Question::Complete, at(*b))));
            questions.extend(ends.iter().map(|b| (Question::Complete, at(*b))));
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
                } else if question == Question::Complete
                    && !position::byte_offset(&now, p, ENCODING)
                        .is_some_and(|b| now[..b].trim_end_matches(is_ident).ends_with('.'))
                {
                    "completion, names"
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
                renamed: position::byte_offset(&now, p, ENCODING).map(|b| word_at(&now, b))
                    != position::byte_offset(last, final_question.1, ENCODING)
                        .map(|b| word_at(last, b)),
                context: format!(
                    "{} after edit {} ({:?}), {:?} at {:?}: {:?}",
                    scenario.name, step, edit.text, question, p, answer.source
                ),
                answer,
            });
        }

        // Document symbols, from the text alone.
        let read = syntax::outline(&now);

        asked_symbols.push(AskedSymbols {
            step: mappings.len(),
            source: read.as_ref().map(|r| match r.blank.is_empty() {
                true => Source::Fresh,
                false => Source::Repaired,
            }),
            lines: read
                .as_ref()
                .map(|r| symbol_lines(r, &now, file, &mapping, true))
                .unwrap_or_default(),
            context: format!(
                "{} after edit {} ({:?}), documentSymbol, lines blank: {:?}",
                scenario.name,
                step,
                edit.text,
                read.as_ref().map(|r| &r.blank)
            ),
        });
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
    let last = &finals[file];
    let final_outline =
        syntax::parse(last).unwrap_or_else(|| panic!("{} doesn't parse at the end", scenario.name));

    // A symbol given mid-edit must be one of the final text's; one left
    // out of an answer from a copy with lines blank only counts as missing.
    for a in &asked_symbols {
        let expected = symbol_lines(&final_outline, last, file, &mappings[a.step], false);
        let verdict = match a.source {
            None => Verdict::None,
            Some(source) => {
                let extra: Vec<&String> =
                    a.lines.iter().filter(|l| !expected.contains(l)).collect();

                if source == Source::Repaired {
                    tally.missing_symbols +=
                        expected.iter().filter(|l| !a.lines.contains(l)).count();
                }

                if extra.is_empty() && (source != Source::Fresh || a.lines == expected) {
                    Verdict::Right(source)
                } else {
                    let shown = format!(
                        "{}\n  now:   {:?}\n  final: {:?}",
                        a.context, extra, expected
                    );

                    if source != Source::Fresh {
                        tally.wrong.push(shown);
                    } else if tally.differs.len() < 3 {
                        tally.differs.push(shown);
                    }

                    Verdict::Wrong(source)
                }
            }
        };

        tally.add("documentSymbol", verdict);
    }

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
                } else if a.renamed && matches!(a.answer.source, Source::Repaired | Source::Guarded)
                {
                    Verdict::Renamed(a.answer.source)
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
    ("navigation: edit and undo", 1),
    ("navigation: close an import and change it on disk", 1),
    ("navigation: break a module that imports the file", 2),
    ("std range: delete and retype lines", 1),
];

/// The questions that got an answer, at least, per request: what the
/// corpus measured. Raise these when coverage improves.
/// The same as `COVERAGE` and the two below, for the sample.
const SAMPLE_COVERAGE: &[(&str, usize)] = &[
    ("hover", 2115),
    ("definition", 2145),
    ("typeDefinition", 2117),
    ("completion", 25),
    ("completion, names", 46),
    ("signatureHelp", 59),
    ("inlayHint", 1016),
    ("inlayHint, whole file", 79),
    ("documentHighlight", 2062),
    ("references", 339),
    ("documentSymbol", 79),
    ("declaration", 2145),
    ("prepareCallHierarchy", 2127),
    ("outgoingCalls", 1914),
    ("prepareTypeHierarchy", 2145),
    ("implementation", 270),
    ("incomingCalls", 270),
    ("subtypes", 270),
];
const SAMPLE_COVERAGE_WITHOUT_MEMORY: &[(&str, usize)] = &[
    ("hover", 1873),
    ("definition", 1901),
    ("typeDefinition", 1873),
    ("completion", 25),
    ("completion, names", 46),
    ("signatureHelp", 54),
    ("inlayHint", 909),
    ("inlayHint, whole file", 67),
    ("documentHighlight", 1850),
    ("references", 344),
    ("documentSymbol", 79),
    ("declaration", 1901),
    ("prepareCallHierarchy", 1894),
    ("outgoingCalls", 1678),
    ("prepareTypeHierarchy", 1901),
    ("implementation", 270),
    ("incomingCalls", 270),
    ("subtypes", 270),
];
const SAMPLE_COVERAGE_WITHOUT_REPAIR: &[(&str, usize)] = &[
    ("hover", 1882),
    ("definition", 1914),
    ("typeDefinition", 1886),
    ("completion", 25),
    ("completion, names", 44),
    ("signatureHelp", 54),
    ("inlayHint", 945),
    ("inlayHint, whole file", 70),
    ("documentHighlight", 1840),
    ("references", 339),
    ("documentSymbol", 79),
    ("declaration", 1914),
    ("prepareCallHierarchy", 1914),
    ("outgoingCalls", 1914),
    ("prepareTypeHierarchy", 1914),
    ("implementation", 270),
    ("incomingCalls", 270),
    ("subtypes", 270),
];

const COVERAGE: &[(&str, usize)] = &[
    ("hover", 12673),
    ("definition", 13097),
    ("typeDefinition", 12761),
    ("completion", 121),
    ("completion, names", 274),
    ("signatureHelp", 406),
    ("inlayHint", 6979),
    ("inlayHint, whole file", 514),
    ("documentHighlight", 11568),
    ("references", 1393),
    ("documentSymbol", 488),
    ("declaration", 13097),
    ("prepareCallHierarchy", 13012),
    ("outgoingCalls", 10830),
    ("prepareTypeHierarchy", 13097),
    ("implementation", 1065),
    ("incomingCalls", 1065),
    ("subtypes", 1065),
];

/// What the server answers without older checks, from repaired copies
/// only.
const COVERAGE_WITHOUT_MEMORY: &[(&str, usize)] = &[
    ("hover", 10948),
    ("definition", 11337),
    ("typeDefinition", 11001),
    ("completion", 115),
    ("completion, names", 223),
    ("signatureHelp", 330),
    ("inlayHint", 6059),
    ("inlayHint, whole file", 408),
    ("documentHighlight", 10226),
    ("references", 1405),
    ("documentSymbol", 488),
    ("declaration", 11337),
    ("prepareCallHierarchy", 11276),
    ("outgoingCalls", 8375),
    ("prepareTypeHierarchy", 11337),
    ("implementation", 1065),
    ("incomingCalls", 1065),
    ("subtypes", 1065),
];

/// What older checks answer alone, without repaired copies.
const COVERAGE_WITHOUT_REPAIR: &[(&str, usize)] = &[
    ("hover", 10490),
    ("definition", 10830),
    ("typeDefinition", 10494),
    ("completion", 99),
    ("completion, names", 157),
    ("signatureHelp", 317),
    ("inlayHint", 6110),
    ("inlayHint, whole file", 505),
    ("documentHighlight", 9957),
    ("references", 1393),
    ("documentSymbol", 488),
    ("declaration", 10830),
    ("prepareCallHierarchy", 10830),
    ("outgoingCalls", 10830),
    ("prepareTypeHierarchy", 10830),
    ("implementation", 1065),
    ("incomingCalls", 1065),
    ("subtypes", 1065),
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

    assert_eq!(scenarios.len(), if full { 23 } else { SAMPLE.len() });
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
            "\nAll scenarios{}\n\n{}\ninlay hint answers covering part of what they were asked: {}, hints of the final text, in text that was there, where they don't cover: {}, document symbols left out with lines blank: {}, answers about a name a later edit changed: {}",
            name,
            total.table(),
            total.partial,
            total.uncovered,
            total.missing_symbols,
            total.renamed()
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
