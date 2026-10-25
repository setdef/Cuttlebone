//! The questions Cuttlebone answers: the type at a position, the members
//! available after a `.`, and where the symbol at a position is declared and
//! referred to.
//!
//! Every answer is computed from a type check of the file and what it
//! imports, with the documents' text exactly as it is: a new check, or one
//! a `Cache` kept from an earlier question about the same text.
use crate::check::{Fingerprint, Reported, Target, diagnose, project_root, run, run_after_errors};
use crate::edits::{Changes, Fallback};
use crate::position::{
    Encoding, HirPosition, Lines, Position, byte_offset, contains, line_end, position_at,
};
use crate::units::{Effect, ErrorEffect, Outline, Unit, effect, error_effect, names};
use crate::walker::{
    ArgumentList, Expr, ListArgument, Reference, SymbolId, Walker, declarations,
    expressions_pass_ran, inferred, type_symbol,
};
use compiler::hir;
use crossbeam_channel::Sender;
use location::Location;
use serde::{Deserialize, Serialize};
use std::cell::{OnceCell, RefCell};
use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Once;
use std::time::{Duration, Instant, SystemTime};
use types::format::{TypeFormatter, format_type};
use types::{
    Block, Database, MethodId, MethodKind, MethodLookup, MethodSource, ModuleId, Symbol,
    TypeArguments, TypeEnum, TypeRef,
};

/// What a request needs to know about the project.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    /// The standard library's source directory.
    pub std: PathBuf,
    /// The project roots the editor has open. A file under one's `src/` or
    /// `test/` is checked as part of it; other files (e.g. the standard
    /// library's) are checked as part of the nearest project containing
    /// them.
    pub projects: Vec<PathBuf>,
}

/// A question about a position in a file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub file: PathBuf,
    pub position: Position,
    /// What `position` and the answer's ranges count.
    pub encoding: Encoding,
    /// Text to use instead of files' contents on disk, e.g. unsaved buffers.
    pub contents: HashMap<PathBuf, String>,
}

/// Why there's no answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unavailable {
    /// A stable identifier, documented in docs/lsp.md.
    pub code: String,
    /// A description for people.
    pub message: String,
    /// For `noHir`, the imports of modules that couldn't be found, if any:
    /// each file, and its first and last line (1-based). A repaired copy
    /// has them blank.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unfound: Vec<(PathBuf, u32, u32)>,
}

impl Unavailable {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Unavailable {
            code: code.to_string(),
            message: message.into(),
            unfound: Vec::new(),
        }
    }
}

/// A range of positions, with an exclusive end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeAt {
    pub typ: String,
    pub range: Range,
    /// The HIR node's kind, e.g. `Call`. For debugging, not for logic.
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembersAt {
    /// The receiver's type.
    pub receiver: String,
    pub receiver_range: Range,
    pub members: Vec<Member>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub name: String,
    pub kind: MemberKind,
    /// The declaration, with the receiver's type arguments filled in, e.g.
    /// `fn pub get(index: Int) -> Result[ref Int, OutOfBounds]`.
    pub signature: String,
    /// What using the member gives: a method's return type, a field's or
    /// constant's type, or the type itself.
    pub typ: String,
    /// The module that defines it, e.g. `std.string`.
    pub module: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemberKind {
    Method,
    Field,
    Constant,
    Type,
}

impl MemberKind {
    pub fn name(self) -> &'static str {
        match self {
            MemberKind::Method => "method",
            MemberKind::Field => "field",
            MemberKind::Constant => "constant",
            MemberKind::Type => "type",
        }
    }
}

/// A range in a file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    /// The path the compiler read the file from: the project's own files,
    /// `dep/`'s, or the standard library's in the session's `std`.
    pub file: PathBuf,
    pub range: Range,
}

/// An answer, and how long it took.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Answer<T> {
    /// `Ok(None)` means the check worked but there's nothing at the position.
    pub result: Result<Option<T>, Unavailable>,
    pub elapsed: Duration,
    /// What the answer was computed from.
    #[serde(default)]
    pub source: Source,
    /// Whether a check of the documents as they are (not an edited copy)
    /// typed method bodies. The server keeps those documents for later
    /// answers from older results (`Fallback`).
    #[serde(default)]
    pub typed: bool,
    /// Whether the check the answer came from (the one `source` names)
    /// was kept from an earlier request (`Cache`), rather than run for
    /// this one. For an answer with no result, whether every check it took
    /// was.
    #[serde(default)]
    pub cached: bool,
}

impl<T> Answer<T> {
    /// The answer with its result converted.
    pub fn map<U>(self, f: impl FnOnce(T) -> Option<U>) -> Answer<U> {
        Answer {
            result: self.result.map(|r| r.and_then(f)),
            elapsed: self.elapsed,
            source: self.source,
            typed: self.typed,
            cached: self.cached,
        }
    }
}

/// What an answer was computed from (docs/lsp.md, "Status").
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum Source {
    /// A check of the documents as they are now.
    #[default]
    Fresh,
    /// The last check of the file that typed method bodies, at a position
    /// in text that hasn't changed since, in a unit nothing changed the
    /// meaning of (`units`).
    Stale,
    /// A check of the documents as they are, which an error in a
    /// declaration didn't stop from typing method bodies, at a position in
    /// a unit the error can't change the meaning of (`units`). Shown as
    /// `fresh`.
    Guarded,
    /// A check of the documents as they are, but with the lines that keep
    /// them from parsing (or a changed declaration's) blank (`repaired_copy`), in
    /// a unit the blank lines don't change the meaning of. Shown as
    /// `fresh`.
    Repaired,
}

/// Whether a question may be answered from a source: read-only displays
/// from older results too; completion, which inserts code, and references,
/// which a rename relies on, only from the documents as they are
/// (docs/lsp.md, "Answers in code being edited").
fn allows(question: Question, source: Source) -> bool {
    match source {
        Source::Fresh | Source::Guarded => true,
        // Only what's blank changed, but references must find every use.
        Source::Repaired => !matches!(question, Question::References { .. }),
        Source::Stale => matches!(
            question,
            Question::Type
                | Question::Definition
                | Question::TypeDefinition
                | Question::Highlight
                | Question::SignatureHelp
                | Question::InlayHints { .. }
        ),
    }
}

/// Whether another check may answer where a fresh one couldn't: the code
/// doesn't parse, or a declaration's error stopped the type checker.
fn recoverable(reason: &Unavailable) -> bool {
    reason.code == "noHir" || reason.code == "declarationErrors"
}

/// A question about a position. `ask` answers several with one check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Question {
    /// `type_at`.
    Type,
    /// `members_at`.
    Members,
    /// `complete`.
    Complete,
    /// `definition`.
    Definition,
    /// `type_definition`.
    TypeDefinition,
    /// `references`.
    References { include_declaration: bool },
    /// `highlights`.
    Highlight,
    /// `signature_help`.
    SignatureHelp,
    /// `inlay_hints`, from the position to `end`.
    InlayHints { end: Position },
}

/// The answer to a `Question`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reply {
    Type(TypeAt),
    Members(MembersAt),
    Place(Place),
    Places(Vec<Place>),
    Highlights(Vec<Highlight>),
    Signature(Signature),
    Hints(Hints),
}

/// Answers questions about positions in the request's file (whose own
/// `position` is ignored), in order. Questions about the file as it is
/// share one check, and references another of the whole project.
/// Completion and signature help each check a copy of their own, edited at
/// their position.
pub fn ask(
    session: &Session,
    request: &Request,
    questions: &[(Question, Position)],
) -> Vec<Answer<Reply>> {
    ask_with(&Cache::new(0), session, request, None, true, questions)
}

/// `ask`, where the documents as they are can't be checked, answering
/// from a repaired copy of them (if `repair`), or else from the documents
/// of the file's last check that typed method bodies (`fallback`), if the
/// question allows it (`allows`).
pub fn ask_with(
    cache: &Cache,
    session: &Session,
    request: &Request,
    fallback: Option<&Fallback>,
    repair: bool,
    questions: &[(Question, Position)],
) -> Vec<Answer<Reply>> {
    let older = fallback.map(|f| (request, f));
    let mut answers: Vec<Option<Answer<Reply>>> = vec![None; questions.len()];

    for project in [false, true] {
        let group: Vec<usize> = (0..questions.len())
            .filter(|i| {
                let q = questions[*i].0;

                !edits_text(q) && matches!(q, Question::References { .. }) == project
            })
            .collect();

        if group.is_empty() {
            continue;
        }

        let items: Vec<(Question, Position)> = group.iter().map(|i| questions[*i]).collect();
        let roots = match project {
            true => searched(session, &request.file),
            false => Vec::new(),
        };
        let checking = |root| Checking {
            project,
            keep: true,
            root,
        };
        let found = match roots.split_first() {
            None => pipeline(
                cache,
                session,
                request,
                checking(None),
                &items,
                older,
                repair,
            ),
            Some((first, rest)) => rest.iter().fold(
                pipeline(
                    cache,
                    session,
                    request,
                    checking(Some(first)),
                    &items,
                    older,
                    repair,
                ),
                |all, root| {
                    let more = pipeline(
                        cache,
                        session,
                        request,
                        checking(Some(root)),
                        &items,
                        older,
                        repair,
                    );

                    all.into_iter()
                        .zip(more)
                        .map(|(a, b)| merged(a, b))
                        .collect()
                },
            ),
        };

        for (i, answer) in group.iter().zip(found) {
            answers[*i] = Some(answer);
        }
    }

    for (i, (question, position)) in questions.iter().enumerate() {
        if edits_text(*question) {
            answers[i] = Some(edited_question(
                cache, session, request, *question, *position, older, repair,
            ));
        }
    }

    answers
        .into_iter()
        .zip(questions)
        .map(|(answer, (question, position))| {
            let answer = answer.unwrap();

            match question {
                // Hints are computed for the whole file.
                Question::InlayHints { end } => answer.map(|reply| match reply {
                    Reply::Hints(mut hints) => {
                        let inside = |p: Position| *position <= p && p <= *end;

                        hints.hints.retain(|h| inside(h.position));

                        if let Some(covered) = &mut hints.covered {
                            *covered = covered
                                .iter()
                                .filter(|r| r.start <= *end && *position <= r.end)
                                .map(|r| Range {
                                    start: r.start.max(*position),
                                    end: r.end.min(*end),
                                })
                                .collect();
                        }

                        Some(Reply::Hints(hints))
                    }
                    other => Some(other),
                }),
                _ => answer,
            }
        })
        .collect()
}

/// Whether a question checks a copy of the file edited at its position.
fn edits_text(question: Question) -> bool {
    matches!(question, Question::Complete | Question::SignatureHelp)
}

/// Asks one question at the request's position.
fn ask_one(session: &Session, request: &Request, question: Question) -> Answer<Reply> {
    ask(session, request, &[(question, request.position)])
        .pop()
        .unwrap()
}

/// The type of the expression at a position.
pub fn type_at(session: &Session, request: &Request) -> Answer<TypeAt> {
    ask_one(session, request, Question::Type).map(|r| match r {
        Reply::Type(t) => Some(t),
        _ => None,
    })
}

fn type_at_in(scope: &Scope) -> Result<Option<TypeAt>, Unavailable> {
    let Some(expr) = select(&scope.walker.out, scope.at) else {
        return Ok(None);
    };

    Ok(Some(TypeAt {
        typ: typed(expr)?.to_string(),
        range: range(scope.lines, &expr.location),
        kind: expr.kind.to_string(),
    }))
}

/// The members available on the receiver of the call whose name is at a
/// position, e.g. at `foo` in `value.foo`.
pub fn members_at(session: &Session, request: &Request) -> Answer<MembersAt> {
    ask_one(session, request, Question::Members).map(|r| match r {
        Reply::Members(m) => Some(m),
        _ => None,
    })
}

fn members_in(scope: &Scope) -> Result<Option<MembersAt>, Unavailable> {
    {
        let (db, module, walker, lines, at) =
            (scope.db, scope.module, scope.walker, scope.lines, scope.at);
        let Some(call) = walker
            .calls
            .iter()
            .filter(|c| {
                contains(&c.name, at)
                    || (at.line == c.name.line_end && at.column == c.name.column_end + 1)
            })
            .max_by_key(|c| (c.name.line_start, c.name.column_start))
        else {
            return Ok(None);
        };

        let receiver = &walker.out[call.receiver];
        let receiver_type = typed(receiver)?.to_string();
        let raw = receiver.raw.unwrap();
        let members = match raw.as_type_enum(db) {
            Ok(id) => members(db, raw, id, module, call.receiver_is_self),
            Err(TypeRef::Error) => return Err(untyped()),
            // e.g. Never, or a placeholder the type checker couldn't infer:
            // nothing can be called on those.
            Err(_) => Vec::new(),
        };

        Ok(Some(MembersAt {
            receiver: receiver_type,
            receiver_range: range(lines, &receiver.location),
            members,
        }))
    }
}

/// Where the symbol at a position is declared: a method (for a call, the
/// called method), a variable, a field, a type, trait, type parameter,
/// constant, constructor or module. On a declaration's own name, that
/// declaration. Usually one place; a name that refers to several symbols
/// (`c.value += 1` calls the setter `value=` and the getter `value`) has
/// each one's, as `symbols_at` orders them.
pub fn definition(session: &Session, request: &Request) -> Answer<Vec<Place>> {
    ask_one(session, request, Question::Definition).map(|r| match r {
        Reply::Places(p) => Some(p),
        _ => None,
    })
}

fn definition_in(scope: &Scope) -> Result<Option<Vec<Place>>, Unavailable> {
    let places: Vec<Place> = symbols_at(
        &scope.walker.references,
        &scope.walker.declarations,
        scope.at,
    )
    .into_iter()
    .filter_map(|r| scope.declaration(r.symbol))
    .map(|(file, location)| scope.place(&file, location))
    .collect();

    Ok((!places.is_empty()).then_some(places))
}

/// Where the type of the expression at a position is declared, without
/// its ownership or type arguments: `ref Option[String]` is `Option`. On a
/// type name (such as in a signature), that type. `None` for types without
/// a declaration: closures, C types and `Never`.
pub fn type_definition(session: &Session, request: &Request) -> Answer<Place> {
    ask_one(session, request, Question::TypeDefinition).map(|r| match r {
        Reply::Place(p) => Some(p),
        _ => None,
    })
}

fn type_definition_in(scope: &Scope) -> Result<Option<Place>, Unavailable> {
    {
        let db = scope.db;
        let symbol = match select(&scope.walker.out, scope.at) {
            Some(expr) => {
                typed(expr)?;

                let Some(symbol) = expr.raw.and_then(|t| type_symbol(db, t)) else {
                    return Ok(None);
                };

                symbol
            }
            None => match symbol_at(
                &scope.walker.references,
                &scope.walker.declarations,
                scope.at,
            ) {
                Some(
                    r @ Reference {
                        symbol: SymbolId::Type(_) | SymbolId::Trait(_) | SymbolId::TypeParameter(_),
                        ..
                    },
                ) => r.symbol,
                _ => return Ok(None),
            },
        };
        Ok(scope
            .declaration(symbol)
            .map(|(file, location)| scope.place(&file, location)))
    }
}

/// Every reference to the symbol at a position in the project's own
/// modules (its `src/` and `test/`, not `dep/` or the standard library),
/// sorted by file and position. With `include_declaration`, its declaration
/// too, if that's in the project.
///
/// This type checks the whole project, not only the file and what it
/// imports, so modules that don't import the file are searched too.
pub fn references(
    session: &Session,
    request: &Request,
    include_declaration: bool,
) -> Answer<Vec<Place>> {
    ask_one(
        session,
        request,
        Question::References {
            include_declaration,
        },
    )
    .map(|r| match r {
        Reply::Places(p) => Some(p),
        _ => None,
    })
}

fn references_in(
    scope: &Scope,
    include_declaration: bool,
) -> Result<Option<Vec<Place>>, Unavailable> {
    {
        let Some(reference) = symbol_at(
            &scope.walker.references,
            &scope.walker.declarations,
            scope.at,
        ) else {
            return Ok(None);
        };
        let symbol = reference.symbol;
        let mut found: Vec<(PathBuf, Location)> = Vec::new();
        let mut collect = |file: &Path, walker: &Walker| {
            let declared = walker.declarations.iter().filter(|_| include_declaration);

            for r in walker.references.iter().chain(declared) {
                if r.symbol == symbol {
                    found.push((file.to_path_buf(), r.location));
                }
            }
        };

        // Not even variables are local to one module: a named argument
        // (`moved(by: 2)`) refers to a method's argument.
        for module in scope.modules {
            let file = module.module_id.file(scope.db);

            if !scope.in_project(&file) {
                continue;
            } else if file == scope.file {
                collect(&file, scope.walker);
            } else if let Some(walker) = scope.checked.walker(&file) {
                collect(&file, &walker);
            }
        }

        found.sort_by_key(|(file, l)| (file.clone(), l.line_start, l.column_start));
        found.dedup();

        Ok(Some(
            found
                .into_iter()
                .map(|(file, location)| scope.place(&file, location))
                .collect(),
        ))
    }
}

/// A diagnostic, as `inko check` reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub file: PathBuf,
    /// An error, or else a warning.
    pub error: bool,
    /// The compiler's ID for it, e.g. `invalid-symbol`.
    pub id: String,
    pub message: String,
    /// In the request's encoding.
    pub range: Range,
    /// The first and last line, as the compiler reports them (1-based).
    pub lines: [u32; 2],
    /// The first and last column, as the compiler reports them (1-based
    /// grapheme clusters, inclusive).
    pub columns: [u32; 2],
}

/// Everything a full check of a project (or a file outside one) reported.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostics {
    /// In the order the compiler reported them, which may be in any file
    /// the check read.
    pub diagnostics: Vec<Diagnostic>,
    /// Why the list may be incomplete: the compiler panicked
    /// (`compilerPanic`), and these are what it reported before that.
    pub incomplete: Option<Unavailable>,
}

/// What diagnostics for a file come from: a full check of its project
/// directory if it's in the project's `src/` or `test/`, or else of the file
/// alone, as `inko check` would check it. `None` for the standard library's
/// files and dependencies' (in a `dep/` directory next to a `src/`), which
/// aren't the user's to fix.
pub fn diagnostics_target(session: &Session, file: &Path) -> Option<PathBuf> {
    let file = canonical(file).ok()?;

    if file.starts_with(session.std.canonicalize().unwrap_or(session.std.clone())) {
        return None;
    }

    if file.ancestors().any(|a| {
        a.file_name().is_some_and(|n| n == "dep")
            && a.parent().is_some_and(|p| p.join("src").is_dir())
    }) {
        return None;
    }

    let root = root_of(session, &file);

    Some(if in_sources(&root, &file) { root } else { file })
}

/// Every diagnostic `inko check` reports for a target (from
/// `diagnostics_target`, in `request.file`): a project directory, or a
/// file. Unlike the other queries, this lowers to MIR, which reports some
/// errors (e.g. moves) and warnings of its own. `request.position` is
/// ignored.
pub fn diagnostics(session: &Session, request: &Request) -> Answer<Diagnostics> {
    let start = Instant::now();
    let mut typed = false;
    let result = canonical(&request.file).and_then(|path| {
        let mut overlay = HashMap::new();

        for (p, text) in &request.contents {
            overlay.insert(canonical(p)?, text.clone());
        }

        let target = if path.is_dir() {
            Target {
                root: path,
                file: None,
                tests: true,
                std: session.std.clone(),
                overlay,
            }
        } else {
            Target {
                root: root_of(session, &path),
                file: Some(path),
                tests: true,
                std: session.std.clone(),
                overlay,
            }
        };

        // The panic hook records the message on the thread that panicked.
        guarded(|| {
            diagnose(&target, |_, modules| typed = expressions_pass_ran(modules))
                .map(|d| (d, PANIC.with(|p| p.borrow_mut().take())))
        })
        .map_err(|message| Unavailable::new("compilerPanic", message))?
        .map_err(|e| Unavailable::new("compilerError", e))
        .map(|(diagnosis, panic)| {
            let incomplete = diagnosis.panicked.then(|| {
                Unavailable::new(
                    "compilerPanic",
                    panic.unwrap_or_else(|| "the compiler panicked".to_string()),
                )
            });
            let mut texts: HashMap<PathBuf, String> = HashMap::new();
            let diagnostics = diagnosis
                .diagnostics
                .into_iter()
                .map(|d| {
                    let text =
                        texts.entry(d.file.clone()).or_insert_with(|| {
                            target.overlay.get(&d.file).cloned().unwrap_or_else(|| {
                                std::fs::read_to_string(&d.file).unwrap_or_default()
                            })
                        });
                    let l = d.location;

                    Diagnostic {
                        range: range(&Lines::with_encoding(text, request.encoding), &l),
                        file: d.file,
                        error: d.error,
                        id: d.id,
                        message: d.message,
                        lines: [l.line_start, l.line_end],
                        columns: [l.column_start, l.column_end],
                    }
                })
                .collect();

            Some(Diagnostics {
                diagnostics,
                incomplete,
            })
        })
    });

    Answer {
        result,
        elapsed: start.elapsed(),
        // Always from the documents as they are.
        source: Source::Fresh,
        typed,
        cached: false,
    }
}

/// How a highlighted name uses its symbol.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Access {
    /// The declaration of something other than a variable: a method, type,
    /// field, constant and so on.
    Declaration,
    /// A use that reads the symbol (or calls it, or names it as a type).
    Read,
    /// A variable's binding (`let`, a pattern, an argument), or an
    /// assignment to a variable or field.
    Write,
}

/// A name in a file that refers to or declares a symbol.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Highlight {
    pub range: Range,
    pub access: Access,
}

/// Every name in the requested file that refers to or declares the symbol
/// at a position, by the compiler's ids as for `references`, sorted by
/// position.
pub fn highlights(session: &Session, request: &Request) -> Answer<Vec<Highlight>> {
    ask_one(session, request, Question::Highlight).map(|r| match r {
        Reply::Highlights(h) => Some(h),
        _ => None,
    })
}

fn highlights_in(scope: &Scope) -> Result<Option<Vec<Highlight>>, Unavailable> {
    {
        let walker = scope.walker;
        let Some(reference) = symbol_at(&walker.references, &walker.declarations, scope.at) else {
            return Ok(None);
        };
        let symbol = reference.symbol;
        let mut found: Vec<(Location, Access)> = Vec::new();

        for r in walker.references.iter().filter(|r| r.symbol == symbol) {
            let access = if walker.writes.contains(&r.location) {
                Access::Write
            } else {
                Access::Read
            };

            found.push((r.location, access));
        }

        for d in walker.declarations.iter().filter(|d| d.symbol == symbol) {
            let access = match d.symbol {
                SymbolId::Variable(_) => Access::Write,
                _ => Access::Declaration,
            };

            found.push((d.location, access));
        }

        found.sort_by_key(|(l, _)| (l.line_start, l.column_start, l.line_end, l.column_end));
        found.dedup_by_key(|(l, _)| *l);

        Ok(Some(
            found
                .into_iter()
                .map(|(location, access)| Highlight {
                    range: range(scope.lines, &location),
                    access,
                })
                .collect(),
        ))
    }
}

/// The name at a position that refers to or declares a symbol: the
/// innermost one containing it.
pub fn symbol_at<'a>(
    references: &'a [Reference],
    declarations: &'a [Reference],
    at: HirPosition,
) -> Option<&'a Reference> {
    references
        .iter()
        .chain(declarations)
        .filter(|r| contains(&r.location, at))
        .min_by_key(|r| {
            let l = r.location;

            (
                std::cmp::Reverse((l.line_start, l.column_start)),
                (l.line_end, l.column_end),
            )
        })
}

/// Every symbol the name at a position refers to or declares: the one
/// `symbol_at` selects, and any other symbol with a name at exactly its
/// location. A compound assignment to a setter (`c.value += 1`) is a call
/// of the getter and of the setter, which the compiler gives the same
/// name. The selected one comes first (there, the setter), so a client
/// that takes only the first gets what it did before.
pub fn symbols_at<'a>(
    references: &'a [Reference],
    declarations: &'a [Reference],
    at: HirPosition,
) -> Vec<&'a Reference> {
    let Some(selected) = symbol_at(references, declarations, at) else {
        return Vec::new();
    };
    let mut found: Vec<&Reference> = Vec::new();

    for r in references.iter().chain(declarations) {
        if r.location == selected.location && found.iter().all(|f| f.symbol != r.symbol) {
            found.push(r);
        }
    }

    found
}

/// Where each symbol of a check is declared.
pub struct Index {
    declared: HashMap<SymbolId, (PathBuf, Location)>,
}

impl Index {
    /// The declarations of `modules`, without their variables other than
    /// methods' arguments (which need walking method bodies).
    pub fn from_modules(db: &Database, modules: &[hir::Module]) -> Index {
        let mut index = Index {
            declared: HashMap::new(),
        };

        for module in modules {
            let file = module.module_id.file(db);

            index.declared.insert(
                SymbolId::Module(module.module_id),
                (
                    file.clone(),
                    // The start of the file, with no width.
                    Location {
                        line_start: 1,
                        line_end: 1,
                        column_start: 1,
                        column_end: 0,
                    },
                ),
            );

            for d in declarations(db, module) {
                index.add(&file, &d);
            }
        }

        index
    }

    /// Adds a declaration. The first one wins: a variable in an `or`
    /// pattern is declared once per alternative.
    pub fn add(&mut self, file: &Path, declaration: &Reference) {
        if let Entry::Vacant(e) = self.declared.entry(declaration.symbol) {
            e.insert((file.to_path_buf(), declaration.location));
        }
    }

    /// The file and the name's location, or `None` if the symbol has no
    /// declaration in the checked modules.
    pub fn declaration(&self, symbol: SymbolId) -> Option<(PathBuf, Location)> {
        self.declared.get(&symbol).cloned()
    }
}

/// The identifier `complete` inserts at the cursor. Any identifier that
/// can name a method works; this one is unlikely to be a real one.
pub const PLACEHOLDER: &str = "cuttlebone_completion_placeholder";

/// The members available after a `.` at the cursor, e.g. at `|` in
/// `value.|` or `value.si|`.
///
/// A buffer with a `.` and nothing after it doesn't parse, so this inserts
/// `PLACEHOLDER` at the cursor in a copy of the file's text (from the
/// request's contents, or from disk), and asks `members_at` about it. That
/// gives `value.PLACEHOLDER` or `value.siPLACEHOLDER`, which parse as a call
/// whose receiver keeps its type.
pub fn complete(session: &Session, request: &Request) -> Answer<MembersAt> {
    ask_one(session, request, Question::Complete).map(|r| match r {
        Reply::Members(m) => Some(m),
        _ => None,
    })
}

/// Completion or signature help: a check of a copy of the file, edited at
/// the position. Completion inserts `PLACEHOLDER`, and signature help a `)`
/// if the file doesn't parse but does with it.
fn edited_question(
    cache: &Cache,
    session: &Session,
    request: &Request,
    question: Question,
    position: Position,
    older: Option<(&Request, &Fallback)>,
    repair: bool,
) -> Answer<Reply> {
    let start = Instant::now();
    let request = Request {
        position,
        ..request.clone()
    };
    let (prepared, inspect) = match question {
        Question::Complete => (
            edited(&request, |text, offset| {
                let mut copy = text.to_string();

                copy.insert_str(offset, PLACEHOLDER);
                Some(copy)
            }),
            Question::Members,
        ),
        _ => (
            edited(&request, |text, offset| {
                // Inserting a `)` into code that parses never makes it
                // parse, so this only saves a parse.
                if parses(text) {
                    return None;
                }

                let mut copy = text.to_string();

                copy.insert(offset, ')');
                parses(&copy).then_some(copy)
            }),
            Question::SignatureHelp,
        ),
    };
    let mut answer = match prepared {
        Ok((copy, changed)) => {
            // A copy edited at the position is asked about once: keeping
            // it would only push out checks that answer more.
            let mut answer = pipeline(
                cache,
                session,
                &copy,
                Checking {
                    project: false,
                    keep: !changed,
                    root: None,
                },
                &[(inspect, position)],
                older,
                repair,
            )
            .pop()
            .unwrap();

            // A copy's check says nothing of the documents as they are.
            answer.typed &= !changed;
            answer
        }
        Err(e) => Answer {
            result: Err(e),
            elapsed: Duration::ZERO,
            source: Source::Fresh,
            typed: false,
            cached: false,
        },
    };

    answer.elapsed = start.elapsed();
    answer
}

/// A copy of a request whose file's text (from the request's contents, or
/// from disk) is what `edit` makes of it, given the byte offset of the
/// request's position, and whether it changed. If `edit` returns `None`,
/// the text is as it was.
fn edited(
    request: &Request,
    edit: impl FnOnce(&str, usize) -> Option<String>,
) -> Result<(Request, bool), Unavailable> {
    let file = canonical(&request.file)?;
    let text = match request
        .contents
        .iter()
        .find(|(p, _)| canonical(p).ok() == Some(file.clone()))
    {
        Some((_, text)) => text.clone(),
        None => read(&file)?,
    };
    let offset = byte_offset(&text, request.position, request.encoding)
        .ok_or_else(|| invalid_position(request.position))?;
    let copy = edit(&text, offset);
    let changed = copy.is_some();
    let copy = copy.unwrap_or(text);
    let mut contents: HashMap<PathBuf, String> = request
        .contents
        .iter()
        .filter(|(p, _)| canonical(p).ok() != Some(file.clone()))
        .map(|(p, t)| (p.clone(), t.clone()))
        .collect();

    contents.insert(file.clone(), copy);
    Ok((
        Request {
            file,
            position: request.position,
            encoding: request.encoding,
            contents,
        },
        changed,
    ))
}

/// A method's signature, for signature help.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    /// The declaration as completion shows it, with the call's type
    /// arguments filled in, e.g. `fn pub mut add(amount: Int, times: Int)`.
    pub label: String,
    /// Each parameter as `name: Type`, and its start and end in `label`, in
    /// the request's encoding.
    pub parameters: Vec<(String, [u32; 2])>,
    /// The index of the parameter the argument at the position is for, if
    /// it's one of them.
    pub active: Option<u32>,
}

/// The signature of the method called by the innermost call, written with
/// parentheses, whose parentheses contain a position, and which of its
/// parameters the argument at the position is for. `None` if there's no
/// such call, or it doesn't call a method that resolved (a closure, say).
///
/// While an argument list is being typed (`foo(` or `foo(a, `) the file
/// doesn't parse, so if it doesn't, this inserts a `)` at the position in a
/// copy of the file, if that makes it parse.
pub fn signature_help(session: &Session, request: &Request) -> Answer<Signature> {
    ask_one(session, request, Question::SignatureHelp).map(|r| match r {
        Reply::Signature(s) => Some(s),
        _ => None,
    })
}

/// Whether a module's source parses.
fn parses(text: &str) -> bool {
    ast::parser::Parser::new(text.as_bytes().to_vec(), PathBuf::from("check.inko"))
        .parse()
        .is_ok()
}

fn signature(scope: &Scope) -> Option<Signature> {
    let db = scope.db;
    let at = (scope.at.line, scope.at.column);
    // The `(` follows the name, or the type arguments' `]`, at once.
    let paren = |list: &ArgumentList| {
        let end = list
            .types
            .map_or((list.name.line_end, list.name.column_end), |t| {
                (t.line_end, t.column_end + 1)
            });

        (end.0, end.1 + 1)
    };
    let list = scope
        .walker
        .argument_lists
        .iter()
        .filter(|l| paren(l) < at && at <= (l.location.line_end, l.location.column_end))
        .max_by_key(|l| paren(l))?;
    let (method, arguments) = list.method.as_ref()?;
    let format = || TypeFormatter::new(db, None, Some(arguments));
    let label = format().format(*method);
    let mut params = method.arguments(db);

    params.sort_by_key(|p| p.index);

    let labels: Vec<String> = params
        .iter()
        .map(|p| format!("{}: {}", p.name, format().format(p.value_type)))
        .collect();
    let mut offset = label
        .find(&format!("({})", labels.join(", ")))
        .map_or(0, |i| i + 1);
    let units = |byte: usize| scope.encoding.len(&label[..byte.min(label.len())]);
    let parameters = labels
        .into_iter()
        .map(|l| {
            let range = [units(offset), units(offset + l.len())];

            offset += l.len() + 2;
            (l, range)
        })
        .collect();

    // The argument at the position: one per comma after an argument and
    // before the position.
    let slot = list
        .arguments
        .iter()
        .filter(|a| {
            let l = a.location;

            (l.line_end, l.column_end) < at
                && scope
                    .lines
                    .between((l.line_end, l.column_end + 1), at)
                    .trim_start()
                    .starts_with(',')
        })
        .count();
    let active = match list.arguments.get(slot) {
        Some(ListArgument {
            name: Some(name), ..
        }) => params.iter().position(|p| &p.name == name),
        Some(_) => Some(slot),
        // An argument not written yet: the next positional one, or the
        // first not given by name.
        None => {
            let positional = list.arguments.iter().filter(|a| a.name.is_none()).count();
            let named: Vec<&String> = list
                .arguments
                .iter()
                .filter_map(|a| a.name.as_ref())
                .collect();

            (positional..params.len()).find(|i| !named.contains(&&params[*i].name))
        }
    };

    Some(Signature {
        label,
        parameters,
        active: active.filter(|i| *i < params.len()).map(|i| i as u32),
    })
}

/// What an inlay hint shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HintKind {
    /// A variable's inferred type, after its name: `: Int`.
    Type,
    /// A parameter's name, before a positional argument: `times:`.
    Parameter,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hint {
    pub position: Position,
    pub label: String,
    pub kind: HintKind,
}

/// Inlay hints, and the ranges of the request they cover.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hints {
    pub hints: Vec<Hint>,
    /// Where these are all the hints there are, each range including its
    /// end, within the request's: `None` for all of it, as from a check of
    /// the documents as they are. From another check, only the parts that
    /// still hold (`Guard::coverage`): elsewhere there may be hints the
    /// answer doesn't have, and it has none.
    pub covered: Option<Vec<Range>>,
}

/// Inlay hints between the request's position and `end` (both included),
/// sorted by position:
///
/// - the inferred type after a variable a `let` binds to a name without a
///   type annotation, and after a closure's argument without one, unless the
///   code already says it (`Untyped::obvious`), or the type isn't fully
///   known. (`let _ = ...` binds nothing.)
/// - the parameter's name before each positional argument of a call
///   written with parentheses to a method that resolved, unless the
///   argument is a variable or field with the parameter's name
pub fn inlay_hints(session: &Session, request: &Request, end: Position) -> Answer<Hints> {
    ask_one(session, request, Question::InlayHints { end }).map(|r| match r {
        Reply::Hints(h) => Some(h),
        _ => None,
    })
}

/// Every inlay hint in the file, sorted by position.
fn hints_in(scope: &Scope) -> Result<Option<Vec<Hint>>, Unavailable> {
    {
        let (db, walker, lines) = (scope.db, scope.walker, scope.lines);
        let mut hints = Vec::new();

        for u in walker.untyped.iter().filter(|u| !u.obvious) {
            let l = u.name;
            let Some(typ) = u.typ.map(|t| format_type(db, t)) else {
                continue;
            };

            if typ.contains("<error>") || typ.contains('?') {
                continue;
            }

            hints.push(Hint {
                position: lines.from_hir(l.line_end, l.column_end + 1),
                label: format!(": {}", typ),
                kind: HintKind::Type,
            });
        }

        for list in &walker.argument_lists {
            // An enum constructor's (`Option.Some(1)`) parameters have
            // generated names: `arg0`.
            let Some((method, _)) = list
                .method
                .as_ref()
                .filter(|(m, _)| m.kind(db) != MethodKind::Constructor)
            else {
                continue;
            };
            let mut params = method.arguments(db);

            params.sort_by_key(|p| p.index);

            // Positional arguments come before named ones.
            for (arg, param) in list
                .arguments
                .iter()
                .take_while(|a| a.name.is_none())
                .zip(&params)
            {
                if arg.variable.as_ref() == Some(&param.name) {
                    continue;
                }

                hints.push(Hint {
                    position: lines.from_hir(arg.location.line_start, arg.location.column_start),
                    label: format!("{}:", param.name),
                    kind: HintKind::Parameter,
                });
            }
        }

        hints.sort_by_key(|h| h.position);
        Ok(Some(hints))
    }
}

fn invalid_position(position: Position) -> Unavailable {
    Unavailable::new(
        "invalidPosition",
        format!("line {} is past the end of the file", position.line),
    )
}

fn read(file: &Path) -> Result<String, Unavailable> {
    std::fs::read_to_string(file)
        .map_err(|e| Unavailable::new("fileNotFound", format!("{}: {}", file.display(), e)))
}

/// Which expression a position means: the innermost expression containing
/// it that desugaring didn't introduce. When several have the same range,
/// the outermost one (the first in the walk) wins.
pub fn select(exprs: &[Expr], at: HirPosition) -> Option<&Expr> {
    exprs
        .iter()
        .enumerate()
        .filter(|(_, e)| !e.synthetic && contains(&e.location, at))
        .min_by_key(|(i, e)| {
            let l = e.location;

            (
                std::cmp::Reverse((l.line_start, l.column_start)),
                (l.line_end, l.column_end),
                *i,
            )
        })
        .map(|(_, e)| e)
}

fn typed(expr: &Expr) -> Result<&str, Unavailable> {
    match expr.typ.as_deref() {
        Some(t) if !t.contains("<error>") => Ok(t),
        _ => Err(untyped()),
    }
}

fn untyped() -> Unavailable {
    Unavailable::new(
        "untyped",
        "the compiler couldn't type this expression, likely because of an error in it",
    )
}

fn range(lines: &Lines, location: &Location) -> Range {
    let (start, end) = lines.range(location);

    Range { start, end }
}

/// What a query sees: the checked modules, the requested file's walked HIR,
/// and the position.
struct Scope<'a, 'db> {
    db: &'db Database,
    /// Every module checked, including imported ones.
    modules: &'db [hir::Module],
    /// The check, and what it kept.
    checked: &'a Checked<'db>,
    /// The requested file.
    file: PathBuf,
    module: ModuleId,
    walker: &'a Walker<'db>,
    lines: &'a Lines<'a>,
    at: HirPosition,
    target: &'a Target,
    encoding: Encoding,
}

impl Scope<'_, '_> {
    /// Where a symbol is declared: in the checked modules, or else among
    /// the walked module's variables. The first declaration wins, as
    /// `Index::add` has it.
    fn declaration(&self, symbol: SymbolId) -> Option<(PathBuf, Location)> {
        self.checked.index().declaration(symbol).or_else(|| {
            self.walker
                .declarations
                .iter()
                .find(|d| d.symbol == symbol)
                .map(|d| (self.file.clone(), d.location))
        })
    }

    /// A location in any file as a range in the request's encoding.
    fn place(&self, file: &Path, location: Location) -> Place {
        if file == self.file {
            return Place {
                file: file.to_path_buf(),
                range: range(self.lines, &location),
            };
        }

        let text = self.checked.text(file);

        Place {
            file: file.to_path_buf(),
            range: range(&Lines::with_encoding(&text, self.encoding), &location),
        }
    }

    /// Whether a module is one of the project's own (in its `src/` or
    /// `test/`), rather than a dependency's or the standard library's.
    fn in_project(&self, file: &Path) -> bool {
        match &self.target.file {
            Some(f) => f == file,
            None => {
                file.starts_with(self.target.root.join("src"))
                    || file.starts_with(self.target.root.join("test"))
            }
        }
    }
}

/// What `pipeline` checks: the whole project, or the file and what it
/// imports; and whether the check may be kept for later questions.
#[derive(Clone, Copy)]
struct Checking<'a> {
    project: bool,
    keep: bool,
    /// The project to check instead of the file's own, for references to
    /// a declaration outside the session's projects (`searched`).
    root: Option<&'a Path>,
}

/// Type checks the request's file and answers each question from its
/// walked HIR, in order. With `project` set, it checks the whole project
/// the file is part of (unless the file is outside its `src/` and
/// `test/`), not just the file and what it imports.
///
/// With `older` (the documents as they are, which `request` may be an
/// edited copy of, and the last good check's), a question the check can't
/// answer because the code doesn't parse or a declaration has an error is
/// answered from the older documents where that's safe (`from_older`).
fn pipeline(
    cache: &Cache,
    session: &Session,
    request: &Request,
    Checking {
        project,
        keep,
        root,
    }: Checking,
    questions: &[(Question, Position)],
    older: Option<(&Request, &Fallback)>,
    repair: bool,
) -> Vec<Answer<Reply>> {
    let start = Instant::now();
    let (mut results, typed, guarded, kept) = match prepare(session, request) {
        Ok((mut target, text)) => {
            let file = target.file.clone().unwrap();

            if let Some(root) = root {
                target.root = root.to_path_buf();
                target.file = None;
            } else if project && in_sources(&target.root, &file) {
                target.file = None;
            }

            analyze(
                cache,
                &target,
                keep,
                (&file, &text),
                request.encoding,
                questions,
                older.map_or(Some(Vec::new()), |(now, f)| declaration_changes(now, f)),
            )
        }
        Err(e) => (vec![Err(e); questions.len()], false, false, false),
    };
    let mut sources: Vec<Source> = results
        .iter()
        .map(|r| match r {
            Ok(_) if guarded => Source::Guarded,
            _ => Source::Fresh,
        })
        .collect();
    // Whether the check each answer came from was kept from before, or for
    // one with none, every check it took.
    let mut cached = vec![kept; questions.len()];
    let wanted: Vec<usize> = (0..questions.len())
        .filter(|i| {
            matches!(&results[*i], Err(r) if recoverable(r))
                && allows(questions[*i].0, Source::Repaired)
        })
        .collect();
    let parse = results
        .iter()
        .any(|r| matches!(r, Err(e) if e.code == "noHir"));
    let unfound: Vec<(PathBuf, u32, u32)> = results
        .iter()
        .filter_map(|r| r.as_ref().err())
        .flat_map(|e| e.unfound.iter().cloned())
        .collect();

    if repair
        && !wanted.is_empty()
        && let Some(repaired) = repaired_copy(request, older, parse, &unfound)
    {
        let asked: Vec<(Question, Position)> = wanted.iter().map(|i| questions[*i]).collect();
        let (found, kept) = from_older(cache, session, request, &repaired, true, &asked);

        for (i, found) in wanted.iter().zip(found) {
            if let Some(result) = found {
                results[*i] = result;
                sources[*i] = Source::Repaired;
                cached[*i] = kept;
            } else {
                // Still no answer: a lookup only if this check was one too.
                cached[*i] &= kept;
            }
        }
    }

    if let Some((now, fallback)) = older {
        let wanted: Vec<usize> = (0..questions.len())
            .filter(|i| {
                matches!(&results[*i], Err(r) if recoverable(r))
                    && allows(questions[*i].0, Source::Stale)
            })
            .collect();

        if !wanted.is_empty() {
            let asked: Vec<(Question, Position)> = wanted.iter().map(|i| questions[*i]).collect();
            let (found, kept) = from_older(cache, session, now, fallback, false, &asked);

            for (i, found) in wanted.iter().zip(found) {
                if let Some(result) = found {
                    results[*i] = result;
                    sources[*i] = Source::Stale;
                    cached[*i] = kept;
                } else {
                    cached[*i] &= kept;
                }
            }
        }
    }

    results
        .into_iter()
        .zip(sources)
        .zip(cached)
        .map(|((result, source), cached)| Answer {
            result,
            elapsed: start.elapsed(),
            source,
            typed,
            cached,
        })
        .collect()
}

/// Answers a question from a check.
fn inspect(question: Question, scope: &Scope) -> Result<Option<Reply>, Unavailable> {
    Ok(match question {
        Question::Type => type_at_in(scope)?.map(Reply::Type),
        Question::Members | Question::Complete => members_in(scope)?.map(Reply::Members),
        Question::Definition => definition_in(scope)?.map(Reply::Places),
        Question::TypeDefinition => type_definition_in(scope)?.map(Reply::Place),
        Question::References {
            include_declaration,
        } => references_in(scope, include_declaration)?.map(Reply::Places),
        Question::Highlight => highlights_in(scope)?.map(Reply::Highlights),
        Question::SignatureHelp => signature(scope).map(Reply::Signature),
        Question::InlayHints { .. } => hints_in(scope)?.map(|hints| {
            Reply::Hints(Hints {
                hints,
                covered: None,
            })
        }),
    })
}

/// The projects whose references to a file's symbols are asked about when
/// the file isn't any project's own: for a file in the standard library,
/// every project of the session (each with a `src/` or `test/`); for one
/// in a project's `dep/`, that project. Empty for any other file, whose
/// own project is searched (`root_of`).
fn searched(session: &Session, file: &Path) -> Vec<PathBuf> {
    let Ok(file) = canonical(file) else {
        return Vec::new();
    };
    let projects = session
        .projects
        .iter()
        .filter_map(|p| p.canonicalize().ok())
        .filter(|p| p.join("src").is_dir() || p.join("test").is_dir());
    let std = session.std.canonicalize().unwrap_or(session.std.clone());

    if session.projects.iter().any(|p| in_sources(p, &file)) {
        Vec::new()
    } else if file.starts_with(&std) {
        projects.collect()
    } else {
        projects
            .filter(|p| file.starts_with(p.join("dep")))
            .take(1)
            .collect()
    }
}

/// Two projects' answers to one question about references, as one: every
/// reference either found, if both have an answer.
fn merged(a: Answer<Reply>, b: Answer<Reply>) -> Answer<Reply> {
    let result = match (a.result, b.result) {
        (Err(e), _) | (_, Err(e)) => Err(e),
        (Ok(Some(Reply::Places(mut places))), Ok(Some(Reply::Places(more)))) => {
            places.extend(more);
            places.sort_by(|x, y| {
                (&x.file, x.range.start.line, x.range.start.character).cmp(&(
                    &y.file,
                    y.range.start.line,
                    y.range.start.character,
                ))
            });
            places.dedup();
            Ok(Some(Reply::Places(places)))
        }
        (Ok(a), Ok(b)) => Ok(a.or(b)),
    };

    Answer {
        result,
        elapsed: a.elapsed + b.elapsed,
        source: a.source,
        typed: a.typed && b.typed,
        cached: a.cached && b.cached,
    }
}

/// Works out what to check, and reads the file's text.
fn prepare(session: &Session, request: &Request) -> Result<(Target, String), Unavailable> {
    let file = canonical(&request.file)?;
    let mut overlay = HashMap::new();

    for (path, text) in &request.contents {
        overlay.insert(canonical(path)?, text.clone());
    }

    let text = match overlay.get(&file) {
        Some(text) => text.clone(),
        None => read(&file)?,
    };

    let target = Target {
        root: root_of(session, &file),
        file: Some(file),
        tests: true,
        std: session.std.clone(),
        overlay,
    };

    Ok((target, text))
}

/// The project a file is checked as part of. The innermost project root
/// wins, e.g. a dependency checked out inside another project.
fn root_of(session: &Session, file: &Path) -> PathBuf {
    session
        .projects
        .iter()
        .filter(|p| in_sources(p, file))
        .max_by_key(|p| p.components().count())
        .cloned()
        .unwrap_or_else(|| project_root(file))
}

/// Whether a file is in a project's `src/` or `test/`.
fn in_sources(root: &Path, file: &Path) -> bool {
    file.starts_with(root.join("src")) || file.starts_with(root.join("test"))
}

fn canonical(path: &Path) -> Result<PathBuf, Unavailable> {
    path.canonicalize()
        .map_err(|e| Unavailable::new("fileNotFound", format!("{}: {}", path.display(), e)))
}

type Results = Vec<Result<Option<Reply>, Unavailable>>;

/// The answers, whether the check typed method bodies (with no error in a
/// declaration), whether it had such an error, so that its answers are
/// only those it can't affect (`Source::Guarded`), and whether it was kept
/// from an earlier request (`Cache`).
fn analyze(
    cache: &Cache,
    target: &Target,
    keep: bool,
    (file, text): (&Path, &str),
    encoding: Encoding,
    questions: &[(Question, Position)],
    changed: Option<Vec<(PathBuf, Effect)>>,
) -> (Results, bool, bool, bool) {
    let lines = Lines::with_encoding(text, encoding);
    let ats: Vec<Result<HirPosition, Unavailable>> = questions
        .iter()
        .map(|(_, position)| {
            lines
                .to_hir(*position)
                .ok_or_else(|| invalid_position(*position))
        })
        .collect();
    let all = |reason: Unavailable| -> Results {
        ats.iter()
            .map(|at| Err(at.clone().err().unwrap_or_else(|| reason.clone())))
            .collect()
    };
    let (file, text, asked, positions) = (
        file.to_path_buf(),
        text.to_string(),
        questions.to_vec(),
        ats.clone(),
    );
    let (found, kept) = cache.with(target, keep, move |checked| {
        let lines = Lines::with_encoding(&text, encoding);
        let Some(walker) = checked.walker(&file) else {
            // A project's check for references to a declaration outside
            // it (`searched`) may not use the file's module at all. Then
            // nothing in the project refers to anything in it.
            if checked.target.file.is_none() && !in_sources(&checked.target.root, &file) {
                let none = |(question, _): &(Question, Position)| match question {
                    Question::References { .. } => Ok(Some(Reply::Places(Vec::new()))),
                    _ => Ok(None),
                };

                return Ok((asked.iter().map(none).collect(), false));
            }

            return Err(Unavailable::new("internal", "the file wasn't type checked"));
        };
        let module = walker_module(checked, &file);
        let scope = |at: HirPosition| Scope {
            db: checked.db,
            modules: checked.modules,
            checked,
            file: file.clone(),
            module,
            walker: &walker,
            lines: &lines,
            at,
            target: checked.target,
            encoding,
        };

        if !checked.guarded() {
            return Ok((
                asked
                    .iter()
                    .zip(&positions)
                    .map(|((question, _), at)| inspect(*question, &scope(at.clone()?)))
                    .collect::<Results>(),
                false,
            ));
        }

        // A declaration has an error: answer only where neither it nor
        // what changed in declarations since the last good check (which
        // may be why) can matter.
        let mut errors = checked
            .declaration_errors()
            .ok_or_else(declaration_errors)?
            .to_vec();

        errors.extend(changed.ok_or_else(declaration_errors)?);

        let errors = errors.as_slice();
        let own = Version {
            old: text.clone(),
            new: text.clone(),
            changes: Changes::none(text.len()),
        };
        let versions = HashMap::new();
        let guard = Guard::new(&file, &own, &versions, &walker, encoding, false, errors)
            .ok_or_else(declaration_errors)?;

        Ok((
            asked
                .iter()
                .zip(&positions)
                .map(|((question, position), at)| {
                    let scope = scope(at.clone()?);
                    let offset = byte_offset(&text, *position, encoding)
                        .ok_or_else(|| invalid_position(*position))?;
                    let found = through(&guard, *question, *position, offset, &scope)
                        .unwrap_or_else(|| Err(declaration_errors()))?;

                    if matches!(question, Question::References { .. })
                        && found.is_some()
                        && hidden(&scope, errors)
                    {
                        return Err(declaration_errors());
                    }

                    Ok(found)
                })
                .collect::<Results>(),
            true,
        ))
    });

    match found.and_then(|r| r) {
        Ok((results, guarded)) => (results, !guarded, guarded, kept),
        Err(reason) => (all(reason), false, false, kept),
    }
}

/// What the changes since the last good check (`fallback`, to `now`) do,
/// as names (`Effect::affects`; a change in a method's body has none): what
/// they changed is part of the documents as they are. `None` if a changed
/// file's text then can't be read or doesn't parse, which can't happen
/// for a check that typed method bodies.
fn declaration_changes(now: &Request, fallback: &Fallback) -> Option<Vec<(PathBuf, Effect)>> {
    let canonical_keys = |map: &HashMap<PathBuf, String>| -> HashMap<PathBuf, String> {
        map.iter()
            .filter_map(|(p, t)| Some((canonical(p).ok()?, t.clone())))
            .collect()
    };
    let (old, new) = (
        canonical_keys(&fallback.contents),
        canonical_keys(&now.contents),
    );
    let mut out = Vec::new();

    for (path, changes) in &fallback.changes {
        let Ok(path) = canonical(path) else {
            continue;
        };
        let text =
            |map: &HashMap<PathBuf, String>| map.get(&path).cloned().or_else(|| read(&path).ok());
        let (before, after) = (text(&old)?, text(&new)?);
        let outline = Outline::parse(&before)?;

        for change in changes.changed() {
            // Its range is in the text then, not the text checked.
            let e = Effect {
                old: None,
                ..effect(&outline, &before, &after, &change)
            };

            out.push((path.clone(), e));
        }
    }

    Some(out)
}

/// Whether an error in a declaration could hide a reference to the symbol
/// at the scope's position: whether a unit of the project's modules that
/// isn't safe, because of the check's errors in declarations
/// (`Guard::safe`), mentions its name. Such a unit may refer to it once
/// the error is fixed, or does now through what the error left untyped.
fn hidden(scope: &Scope, errors: &[(PathBuf, Effect)]) -> bool {
    let Some(reference) = symbol_at(
        &scope.walker.references,
        &scope.walker.declarations,
        scope.at,
    ) else {
        return true;
    };
    let text = scope.checked.text(&scope.file);
    let lines = Lines::new(&text);
    let l = &reference.location;
    let (Some(start), Some(end)) = (
        lines.offset(l.line_start, l.column_start),
        lines.offset(l.line_end, l.column_end + 1),
    ) else {
        return true;
    };
    let name = text[start..end].to_string();

    scope.modules.iter().any(|module| {
        let file = module.module_id.file(scope.db);

        if file != scope.file && !scope.in_project(&file) {
            return false;
        }

        let Some(walker) = scope.checked.walker(&file) else {
            return true;
        };
        let text = scope.checked.text(&file);
        let own = Version {
            old: text.clone(),
            new: text.clone(),
            changes: Changes::none(text.len()),
        };
        let versions = HashMap::new();

        Guard::new(
            &file,
            &own,
            &versions,
            &walker,
            scope.encoding,
            false,
            errors,
        )
        .is_none_or(|guard| guard.mentioned_unsafely(&name))
    })
}

/// The module of a file the check walked.
fn walker_module(checked: &Checked, file: &Path) -> ModuleId {
    checked.modules[checked.files[file]].module_id
}

/// How many checks a `Cache` keeps by default. Each holds its compiler's
/// state: about 33 MB for a file that imports all of std.
pub const KEPT_CHECKS: usize = 4;

/// How many checks with nothing to answer from a `Cache` keeps, besides
/// its checks. Each holds only the text it checked and the fingerprint of
/// what it could have read. Typing in code that doesn't parse makes one for
/// the documents as they are at each edit, and maybe one for a repaired
/// copy.
pub const KEPT_FAILURES: usize = 8;

/// Checks kept to answer later questions about the same text, without
/// checking again (docs/lsp.md, "Freshness, cost and side effects").
///
/// The compiler hands its typed HIR to a callback, and drops it when the
/// callback returns. So each check runs on a thread of its own, and stays
/// inside the callback, answering the questions sent to it, until the
/// cache lets it go. A check is found by its target, which includes the
/// exact text of every document it used, and is used only while the files
/// and directories it read on disk are as they were (`Fingerprint`).
pub struct Cache {
    capacity: usize,
    /// The least recently used first.
    kept: RefCell<Vec<Kept>>,
    /// Checks with nothing to answer from (`Kept::state` is `Err`), the
    /// least recently used first. They hold no compiler state, so they
    /// don't count against `capacity`, and don't push out checks that do.
    failed: RefCell<Vec<Kept>>,
    /// For testing only (`stopping_at_errors`): checks of files whose path
    /// ends with this (and of whole projects) stop at the first pass with
    /// an error, as they do after a panic in a later pass.
    stopping: Option<String>,
}

struct Kept {
    target: Target,
    fingerprint: Fingerprint,
    /// Sends questions to the check's thread, or why it has nothing to
    /// answer from: an error in a declaration that kept it from typing
    /// method bodies (`declarationErrors`), code that doesn't parse
    /// (`noHir`), or the compiler's own error (`compilerError`).
    state: Result<Sender<Task>, Unavailable>,
}

/// Something to run inside a kept check.
type Task = Box<dyn for<'a> FnOnce(&Checked<'a>) + Send>;

impl Default for Cache {
    fn default() -> Self {
        Cache::new(KEPT_CHECKS)
    }
}

impl Cache {
    /// A cache of at most `capacity` checks. With 0, every question is
    /// answered by a check of its own, as if there were no cache.
    pub fn new(capacity: usize) -> Cache {
        Cache {
            capacity,
            kept: RefCell::new(Vec::new()),
            failed: RefCell::new(Vec::new()),
            stopping: None,
        }
    }

    /// For testing only: the cache, with checks of files whose path ends
    /// with `suffix` (and of whole projects) stopping at the first pass
    /// that reports an error, as `inko check`'s do. That's what a check
    /// does when a pass after an error panics (`start_check`), so the
    /// answers a check that stops can't give can be tested.
    pub fn stopping_at_errors(mut self, suffix: String) -> Cache {
        self.stopping = Some(suffix);
        self
    }

    /// How many checks it holds that have something to answer from.
    pub fn len(&self) -> usize {
        self.kept.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.kept.borrow().is_empty() && self.failed.borrow().is_empty()
    }

    /// How many checks it holds with nothing to answer from.
    pub fn failures(&self) -> usize {
        self.failed.borrow().len()
    }

    /// Runs `f` with a check of `target`: a kept one, if its fingerprint
    /// is current, or else a new one, which is kept if `keep` and nothing
    /// it read changed while it ran. Also says whether the check was kept
    /// from before. `Err` if there's no typed HIR to run `f` with: nothing
    /// parsed (`noHir`), a declaration's error (`declarationErrors`), the
    /// compiler failed or panicked (`compilerError`, `compilerPanic`), or
    /// `f` panicked.
    fn with<R: Send + 'static>(
        &self,
        target: &Target,
        keep: bool,
        f: impl for<'a> FnOnce(&Checked<'a>) -> R + Send + 'static,
    ) -> (Result<R, Unavailable>, bool) {
        let mut kept = self.kept.borrow_mut();
        let mut failed = self.failed.borrow_mut();

        for list in [&mut *kept, &mut *failed] {
            if let Some(i) = list.iter().position(|k| k.target == *target) {
                let entry = list.remove(i);

                if entry.fingerprint.current() {
                    let result = match &entry.state {
                        Ok(tasks) => run_task(tasks, f),
                        Err(reason) => Err(reason.clone()),
                    };

                    list.push(entry);
                    return (result, true);
                }
            }
        }

        let stop = self.stopping.as_ref().is_some_and(|suffix| {
            target
                .file
                .as_ref()
                .is_none_or(|f| f.to_string_lossy().ends_with(suffix.as_str()))
        });
        let (state, fingerprint) = match start_check(target.clone(), stop) {
            Ok(started) => started,
            Err(reason) => return (Err(reason), false),
        };
        let result = match &state {
            Ok(tasks) => run_task(tasks, f),
            Err(reason) => Err(reason.clone()),
        };

        if keep && self.capacity > 0 && fingerprint.settled() {
            let (list, capacity) = match state {
                Ok(_) => (&mut *kept, self.capacity),
                Err(_) => (&mut *failed, KEPT_FAILURES),
            };

            list.push(Kept {
                target: target.clone(),
                fingerprint,
                state,
            });

            if list.len() > capacity {
                list.remove(0);
            }
        }

        (result, false)
    }
}

/// Starts a check of `target` on a thread of its own, and waits until it
/// has typed the HIR. Returns the channel to send it tasks on (or the
/// reason it has nothing to answer from: a declaration's error kept it from
/// typing method bodies, nothing parsed, or the compiler's own error), and
/// the fingerprint of what it read. `Err` if the compiler panicked. The
/// thread ends when the channel is dropped.
fn start_check(
    target: Target,
    stop: bool,
) -> Result<(Result<Sender<Task>, Unavailable>, Fingerprint), Unavailable> {
    install_panic_hook();

    let (ready, typed) = crossbeam_channel::bounded(1);
    let (tasks, queue) = crossbeam_channel::unbounded::<Task>();
    let started = SystemTime::now();
    let spawned = std::thread::Builder::new()
        .name("check".to_string())
        .stack_size(STACK_SIZE)
        .spawn(move || {
            // Serves the questions sent to a check that typed method
            // bodies, until the cache lets it go.
            let serve = |db: &Database, modules: &[hir::Module], errors: &[Reported]| {
                let fingerprint = Fingerprint::take(
                    &target,
                    modules.iter().map(|m| m.module_id.file(db)),
                    started,
                );

                if !expressions_pass_ran(modules) {
                    let _ = ready.send(Ok((false, fingerprint)));
                    return;
                }

                let checked = Checked::new(db, modules, &target, errors.to_vec());
                let _ = ready.send(Ok((true, fingerprint)));

                while let Ok(task) = queue.recv() {
                    task(&checked);
                }
            };
            // As `inko check` runs the passes: stopping at the first that
            // reports an error. Its bodies are typed only if there's none.
            let stopping = || {
                catch_unwind(AssertUnwindSafe(|| {
                    run(&target, false, false, |db, modules: &[hir::Module]| {
                        serve(db, modules, &[])
                    })
                }))
            };
            let checking = if stop {
                stopping()
            } else {
                // A pass after an error may panic on what the error left
                // undefined, where the compiler would have stopped (see
                // patches/README.md). Then it's checked again, stopping
                // there.
                match catch_unwind(AssertUnwindSafe(|| run_after_errors(&target, serve))) {
                    Err(_) => stopping(),
                    other => other,
                }
            };
            // A failure that's the same for the same text and files is
            // kept like a check, with what it could have read, since the
            // compiler doesn't say which modules it got to. A panic isn't.
            let failure = match checking {
                Ok(Ok(run)) if run.inspected.is_some() => return,
                Ok(Ok(run)) => Ok(Unavailable {
                    unfound: run.unfound,
                    ..Unavailable::new(
                        "noHir",
                        "the file or a module it imports couldn't be parsed or found, \
                         so nothing was type checked",
                    )
                }),
                Ok(Err(e)) => Ok(Unavailable::new("compilerError", e)),
                Err(_) => Err(Unavailable::new("compilerPanic", panic_message())),
            };

            let _ = ready.send(Err(
                failure.map(|reason| (reason, Fingerprint::take_all(&target, started)))
            ));
        });

    if let Err(e) = spawned {
        return Err(Unavailable::new(
            "compilerPanic",
            format!("failed to start a thread: {}", e),
        ));
    }

    match typed.recv() {
        Ok(Ok((true, fingerprint))) => Ok((Ok(tasks), fingerprint)),
        Ok(Ok((false, fingerprint))) => Ok((Err(declaration_errors()), fingerprint)),
        Ok(Err(Ok((reason, fingerprint)))) => Ok((Err(reason), fingerprint)),
        Ok(Err(Err(reason))) => Err(reason),
        Err(_) => Err(Unavailable::new("compilerPanic", "the compiler panicked")),
    }
}

/// Runs `f` in a kept check, and waits for its result. `compilerPanic` if
/// it panicked. The check is only read, so it stays as it was, and the
/// same question panics again, as it would in a new check.
fn run_task<R: Send + 'static>(
    tasks: &Sender<Task>,
    f: impl for<'a> FnOnce(&Checked<'a>) -> R + Send + 'static,
) -> Result<R, Unavailable> {
    let (done, result) = crossbeam_channel::bounded(1);
    let task: Task = Box::new(move |checked| {
        let outcome = catch_unwind(AssertUnwindSafe(|| f(checked))).map_err(|_| panic_message());

        let _ = done.send(outcome);
    });

    if tasks.send(task).is_err() {
        return Err(Unavailable::new("compilerPanic", "the check ended"));
    }

    match result.recv() {
        Ok(Ok(r)) => Ok(r),
        Ok(Err(message)) => Err(Unavailable::new("compilerPanic", message)),
        Err(_) => Err(Unavailable::new("compilerPanic", "the check ended")),
    }
}

/// A check that typed method bodies, and what's been worked out from it
/// so far, kept for later questions.
struct Checked<'db> {
    db: &'db Database,
    modules: &'db [hir::Module],
    target: &'db Target,
    /// Each module's index in `modules`, by file.
    files: HashMap<PathBuf, usize>,
    walkers: RefCell<HashMap<PathBuf, Rc<Walker<'db>>>>,
    index: OnceCell<Index>,
    /// The text of files other than the requested one, as read for
    /// `Scope::place`: the check's own modules, so the fingerprint covers
    /// the ones read from disk.
    texts: RefCell<HashMap<PathBuf, String>>,
    /// The errors the check reported.
    errors: Vec<Reported>,
    /// What its errors in declarations do (`declaration_errors`).
    declarations: OnceCell<Option<Vec<(PathBuf, Effect)>>>,
}

impl<'db> Checked<'db> {
    fn new(
        db: &'db Database,
        modules: &'db [hir::Module],
        target: &'db Target,
        errors: Vec<Reported>,
    ) -> Self {
        Checked {
            db,
            modules,
            target,
            files: modules
                .iter()
                .enumerate()
                .map(|(i, m)| (m.module_id.file(db), i))
                .collect(),
            walkers: RefCell::new(HashMap::new()),
            index: OnceCell::new(),
            texts: RefCell::new(HashMap::new()),
            errors,
            declarations: OnceCell::new(),
        }
    }

    /// A checked file's text, as the check read it.
    fn text(&self, file: &Path) -> String {
        self.texts
            .borrow_mut()
            .entry(file.to_path_buf())
            .or_insert_with(|| {
                self.target
                    .overlay
                    .get(file)
                    .cloned()
                    .unwrap_or_else(|| std::fs::read_to_string(file).unwrap_or_default())
            })
            .clone()
    }

    /// What the check's errors in declarations do (`units::error_effect`),
    /// by file: empty if it has none, and `None` if one can't be placed in
    /// a declaration, which leaves nothing safe to answer.
    fn declaration_errors(&self) -> Option<&[(PathBuf, Effect)]> {
        self.declarations
            .get_or_init(|| {
                let mut outlines: HashMap<PathBuf, Option<(String, Outline)>> = HashMap::new();
                let mut out = Vec::new();

                for error in &self.errors {
                    let entry = outlines.entry(error.file.clone()).or_insert_with(|| {
                        let text = self.text(&error.file);

                        Outline::parse(&text).map(|o| (text, o))
                    });
                    let (text, outline) = entry.as_ref()?;
                    let lines = Lines::new(text);
                    let l = &error.location;
                    let start = lines.offset(l.line_start, l.column_start)?;
                    let end = lines
                        .offset(l.line_end, l.column_end + 1)
                        .unwrap_or(text.len())
                        .max(start);

                    match error_effect(outline, text, start..end) {
                        ErrorEffect::Body => {}
                        ErrorEffect::Declaration(effect) => out.push((error.file.clone(), effect)),
                        ErrorEffect::Unknown => return None,
                    }
                }

                Some(out)
            })
            .as_deref()
    }

    /// Whether it has an error in a declaration, so that some of what it
    /// typed may depend on a declaration that will be different.
    fn guarded(&self) -> bool {
        !self.declaration_errors().is_some_and(|e| e.is_empty())
    }

    /// The walked HIR of a checked file, walked the first time it's asked
    /// for.
    fn walker(&self, file: &Path) -> Option<Rc<Walker<'db>>> {
        if let Some(walker) = self.walkers.borrow().get(file) {
            return Some(walker.clone());
        }

        let module = &self.modules[*self.files.get(file)?];
        let mut walker = Walker::new(self.db, module.module_id);

        walker.module(module);

        let walker = Rc::new(walker);

        self.walkers
            .borrow_mut()
            .insert(file.to_path_buf(), walker.clone());
        Some(walker)
    }

    /// Where each symbol of the checked modules is declared.
    fn index(&self) -> &Index {
        self.index
            .get_or_init(|| Index::from_modules(self.db, self.modules))
    }
}

/// A copy of the documents that may type check where they don't, as a
/// `Fallback` (the copy, and how the documents differ from it). With
/// `parse`, each document that doesn't parse gets lines blank, the first of
/// these that parses: the lines changed since the last good check
/// (`older`), then also the line of the bracket nothing closes (`unclosed`),
/// then also each line the parser stops at in turn.
/// Otherwise (a declaration has an error), each changed document gets the
/// units (as that check's outline has them) of its changes outside method
/// bodies blank. Blank lines keep every
/// other line where it was. `None` if nothing changes, or no copy parses.
fn repaired_copy(
    request: &Request,
    older: Option<(&Request, &Fallback)>,
    parse: bool,
    unfound: &[(PathBuf, u32, u32)],
) -> Option<Fallback> {
    let file = canonical(&request.file).ok()?;
    let mut docs: HashMap<PathBuf, String> = request
        .contents
        .iter()
        .filter_map(|(p, t)| Some((canonical(p).ok()?, t.clone())))
        .collect();

    if !docs.contains_key(&file) {
        docs.insert(file.clone(), read(&file).ok()?);
    }

    // The imports of modules that couldn't be found are blank, in modules
    // that aren't open too.
    let mut imports: HashMap<PathBuf, BTreeSet<usize>> = HashMap::new();

    for (path, first, last) in unfound {
        let Ok(path) = canonical(path) else {
            continue;
        };

        if !docs.contains_key(&path) {
            docs.insert(path.clone(), read(&path).ok()?);
        }

        imports
            .entry(path)
            .or_default()
            .extend((*first as usize).saturating_sub(1)..(*last as usize));
    }

    let mut out = Fallback {
        contents: docs.clone(),
        changes: HashMap::new(),
    };

    for (path, text) in &docs {
        let imports = imports.remove(path).unwrap_or_default();

        if parse && parses(text) {
            if !imports.is_empty() {
                blank_lines(&mut out, path, text, &imports);
            }

            continue;
        }

        let (changed, declarations) = older.map_or_else(Default::default, |(now, fallback)| {
            changed_lines(now, fallback, path)
        });
        let mut candidates: Vec<BTreeSet<usize>> = Vec::new();

        if parse {
            candidates.push(changed.clone());
        } else {
            candidates.push(declarations);
        }

        if parse {
            let mut with = candidates.last().cloned().unwrap_or_default();

            if let Some(line) = unclosed(&blank(text, &with)) {
                with.insert(line);
                candidates.push(with);
            }
        }

        let found = candidates
            .iter()
            .filter(|lines| !lines.is_empty())
            .find(|lines| parses(&blank(text, lines)))
            .cloned()
            .or_else(|| {
                // The lines the parser stops at, one by one.
                let mut lines = candidates.last().cloned().unwrap_or_default();

                for _ in 0..8 {
                    if !parse {
                        return None;
                    }

                    match parse_error_line(&blank(text, &lines)) {
                        None => return Some(lines),
                        Some(line) if lines.insert(line) => {}
                        Some(_) => return None,
                    }
                }

                None
            });
        let Some(mut lines) = found else {
            if parse {
                return None;
            }

            continue;
        };

        lines.extend(imports);

        if lines.is_empty() {
            continue;
        }

        blank_lines(&mut out, path, text, &lines);
    }

    (!out.changes.is_empty()).then_some(out)
}

/// `text` with `lines` blank in `out`, as the document at `path`, with
/// what changed.
fn blank_lines(out: &mut Fallback, path: &Path, text: &str, lines: &BTreeSet<usize>) {
    {
        let copy = blank(text, lines);
        let mut changes = Changes::none(copy.len());
        let starts: Vec<usize> = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        let end_of = |line: usize| starts.get(line + 1).map_or(text.len(), |next| next - 1);
        let mut runs: Vec<(usize, usize)> = Vec::new();

        // Consecutive blank lines are one change: a whole method blank is
        // one method, not lines of one.
        for line in lines {
            match runs.last_mut() {
                Some((_, last)) if *last + 1 == *line => *last = *line,
                _ => runs.push((*line, *line)),
            }
        }

        // Each run's lines are empty in the copy apart from their `\n`s,
        // and the earlier runs are back by the time it's replaced.
        for (first, last) in runs {
            let at = starts[first];

            changes.replace(at, at + (last - first), end_of(last) - at);
        }

        out.contents.insert(path.to_path_buf(), copy);
        out.changes.insert(path.to_path_buf(), changes);
    }
}

/// The lines of `path` (as it is in `now`) that changed since the last
/// good check (`fallback`), and those of the units of the changes outside
/// method bodies.
fn changed_lines(
    now: &Request,
    fallback: &Fallback,
    path: &Path,
) -> (BTreeSet<usize>, BTreeSet<usize>) {
    let mut changed = BTreeSet::new();
    let mut declarations = BTreeSet::new();
    fn find<'a, T>(map: &'a HashMap<PathBuf, T>, path: &Path) -> Option<&'a T> {
        map.iter()
            .find(|(p, _)| canonical(p).ok().as_deref() == Some(path))
            .map(|(_, v)| v)
    }

    let (Some(changes), Some(old)) = (
        find(&fallback.changes, path),
        find(&fallback.contents, path),
    ) else {
        return (changed, declarations);
    };
    let Some(new) = find(&now.contents, path)
        .cloned()
        .or_else(|| read(path).ok())
    else {
        return (changed, declarations);
    };
    let line = |byte: usize| new[..byte.min(new.len())].matches('\n').count();
    let outline = Outline::parse(old);

    for c in changes.changed() {
        let (start, end) = (c.new.start, c.new.end.max(c.new.start + 1) - 1);

        changed.extend(line(start)..=line(end));

        if let Some(outline) = &outline
            && let Unit::Item(i) = outline.unit_at(c.old.start)
        {
            let range = &outline.items[i].range;
            let from = changes.new_offset(range.start).unwrap_or(c.new.start);
            let to = changes
                .new_offset(range.end.saturating_sub(1))
                .unwrap_or(end)
                .max(end);

            if !effect(outline, old, &new, &c).body {
                declarations.extend(line(from)..=line(to));
            }
        }
    }

    (changed, declarations)
}

/// `text` with `lines` (0-based) blank: empty, apart from their `\n`.
fn blank(text: &str, lines: &BTreeSet<usize>) -> String {
    text.split_inclusive('\n')
        .enumerate()
        .map(|(i, l)| {
            if !lines.contains(&i) {
                l
            } else if l.ends_with('\n') {
                "\n"
            } else {
                ""
            }
        })
        .collect()
}

/// The line (0-based) of the bracket nothing closes: the one a closing
/// bracket of another kind meets, or the innermost left open at the end.
/// The parser reports these where it notices, which may be lines later.
/// Strings and comments are skipped.
fn unclosed(text: &str) -> Option<usize> {
    let mut open: Vec<(char, usize)> = Vec::new();
    let mut quote = None;

    for (number, line) in text.lines().enumerate() {
        let mut chars = line.chars();

        while let Some(c) = chars.next() {
            match (quote, c) {
                (Some(_), '\\') => {
                    chars.next();
                }
                (Some(q), c) if c == q => quote = None,
                (Some(_), _) => {}
                (None, '"' | '\'') => quote = Some(c),
                (None, '#') => break,
                (None, '(' | '[' | '{') => open.push((c, number)),
                (None, ')' | ']' | '}') => {
                    let expected = match c {
                        ')' => '(',
                        ']' => '[',
                        _ => '{',
                    };

                    match open.pop() {
                        Some((o, _)) if o == expected => {}
                        Some((_, line)) => return Some(line),
                        None => return Some(number),
                    }
                }
                _ => {}
            }
        }
    }

    open.last().map(|(_, line)| *line)
}

/// The line (0-based) where parsing a module's source stops, or `None` if
/// it parses.
fn parse_error_line(text: &str) -> Option<usize> {
    ast::parser::Parser::new(text.as_bytes().to_vec(), PathBuf::from("check.inko"))
        .parse()
        .err()
        .map(|e| e.location.line_start.saturating_sub(1) as usize)
}

/// An answer from another check, or `None` where it doesn't hold.
type Found = Option<Result<Option<Reply>, Unavailable>>;

/// A file as an older check read it, and as it is now.
struct Version {
    old: String,
    new: String,
    changes: Changes,
}

/// Answers questions from the documents of an earlier check (`fallback`),
/// at positions in the documents as they are (`now`): each position is
/// mapped back, and each range in an answer forward, through the edits
/// since. An answer is given only where it holds now: the position, and
/// everything in the answer, is in text that hasn't changed, in a unit no
/// change since could make mean something else (`units`). `None` where
/// there's no such answer, and for every question if that check didn't
/// type method bodies.
///
/// With `repaired`, the older documents are a repaired copy of these
/// (`repaired_copy`), and a method with blank lines still answers before the
/// first of them (`Guard::limit`).
fn from_older(
    cache: &Cache,
    session: &Session,
    now: &Request,
    fallback: &Fallback,
    repaired: bool,
    questions: &[(Question, Position)],
) -> (Vec<Found>, bool) {
    let count = questions.len();
    let nothing = move || vec![None; count];
    let canonical_keys = |map: &HashMap<PathBuf, String>| -> HashMap<PathBuf, String> {
        map.iter()
            .filter_map(|(p, t)| Some((canonical(p).ok()?, t.clone())))
            .collect()
    };
    let Ok(file) = canonical(&now.file) else {
        return (nothing(), false);
    };
    let contents = canonical_keys(&fallback.contents);
    let current = canonical_keys(&now.contents);
    let text =
        |map: &HashMap<PathBuf, String>, p: &Path| map.get(p).cloned().or_else(|| read(p).ok());
    let mut versions: HashMap<PathBuf, Version> = HashMap::new();

    for (path, changes) in &fallback.changes {
        let Ok(path) = canonical(path) else {
            continue;
        };

        if changes.is_none() {
            continue;
        }

        let (Some(old), Some(new)) = (text(&contents, &path), text(&current, &path)) else {
            return (nothing(), false);
        };

        versions.insert(
            path,
            Version {
                old,
                new,
                changes: changes.clone(),
            },
        );
    }

    let Some(old) = text(&contents, &file) else {
        return (nothing(), false);
    };
    let new = versions
        .get(&file)
        .map_or_else(|| old.clone(), |v| v.new.clone());
    let unchanged = Changes::none(old.len());
    let changes = versions.get(&file).map_or(&unchanged, |v| &v.changes);
    let encoding = now.encoding;
    // Each position in the old text, if its character is unchanged.
    let positions: Vec<Option<Position>> = questions
        .iter()
        .map(|(question, position)| match question {
            // Hints are computed for the whole file.
            Question::InlayHints { .. } => Some(Position {
                line: 0,
                character: 0,
            }),
            _ => {
                let byte = byte_offset(&new, *position, encoding)?;

                Some(position_at(&old, changes.old_offset(byte)?, encoding))
            }
        })
        .collect();
    let target = Target {
        root: root_of(session, &file),
        file: Some(file.clone()),
        tests: true,
        std: session.std.clone(),
        overlay: contents.clone(),
    };
    let questions = questions.to_vec();
    let changes = changes.clone();
    let (found, kept) = cache.with(&target, true, move |checked| {
        let lines = Lines::with_encoding(&old, encoding);
        let Some(walker) = checked.walker(&file) else {
            return nothing();
        };
        let module = walker_module(checked, &file);
        let own = Version {
            old: old.clone(),
            new: new.clone(),
            changes: changes.clone(),
        };
        let Some(errors) = checked.declaration_errors() else {
            return nothing();
        };
        let Some(guard) = Guard::new(&file, &own, &versions, &walker, encoding, repaired, errors)
        else {
            return nothing();
        };

        questions
            .iter()
            .zip(&positions)
            .map(|((question, position), old_position)| {
                let old_position = (*old_position)?;
                let at = lines.to_hir(old_position)?;
                let scope = Scope {
                    db: checked.db,
                    modules: checked.modules,
                    checked,
                    file: file.clone(),
                    module,
                    walker: &walker,
                    lines: &lines,
                    at,
                    target: checked.target,
                    encoding,
                };

                through(
                    &guard,
                    *question,
                    *position,
                    byte_offset(&old, old_position, encoding)?,
                    &scope,
                )
            })
            .collect()
    });

    (found.unwrap_or_else(|_| vec![None; count]), kept)
}

/// Answers a question from a check through `guard`, at `offset` in the
/// checked text (`position` in the documents as they are): `None` where
/// the answer doesn't hold.
fn through(
    guard: &Guard,
    question: Question,
    position: Position,
    offset: usize,
    scope: &Scope,
) -> Found {
    if !matches!(question, Question::InlayHints { .. }) && !guard.safe(offset) {
        return None;
    }

    let reply = inspect(question, scope).ok()?;

    match (question, reply) {
        (Question::InlayHints { end }, Some(Reply::Hints(hints))) => {
            guard.hints(hints, position, end).map(|h| Ok(Some(h)))
        }
        (_, reply) => guard.reply(reply).map(Ok),
    }
}

/// Decides which parts of an older check of a file still hold, and maps
/// them to the documents as they are.
struct Guard<'a> {
    file: &'a Path,
    old: &'a str,
    new: &'a str,
    changes: &'a Changes,
    versions: &'a HashMap<PathBuf, Version>,
    outline: Outline,
    /// The bytes of the old text each expression of the walk covers, its
    /// type (`None` if the check couldn't type it), and whether there's
    /// nothing left to infer in it (`inferred`).
    types: Vec<(usize, usize, Option<&'a str>, bool)>,
    /// Whether the old text is a repaired copy of the new.
    repaired: bool,
    /// What the changes to the file did.
    effects: Vec<Effect>,
    /// What the changes to every file did.
    all: Vec<Effect>,
    /// Each unit's `limit`.
    limits: RefCell<HashMap<Unit, usize>>,
    encoding: Encoding,
}

impl<'a> Guard<'a> {
    /// `None` if a changed file's old text doesn't parse, which can't
    /// happen for a check that typed method bodies. The check's errors in
    /// declarations (`errors`) count as changes in those files.
    fn new(
        file: &'a Path,
        own: &'a Version,
        versions: &'a HashMap<PathBuf, Version>,
        walker: &'a Walker,
        encoding: Encoding,
        repaired: bool,
        errors: &[(PathBuf, Effect)],
    ) -> Option<Guard<'a>> {
        let (old, new, changes) = (own.old.as_str(), own.new.as_str(), &own.changes);
        let outline = Outline::parse(old)?;
        let mut effects = Vec::new();
        let mut all = Vec::new();

        for (path, v) in versions {
            let other = Outline::parse(&v.old)?;

            for change in v.changes.changed() {
                if path == file {
                    effects.push(effect(&other, &v.old, &v.new, &change));
                }

                all.push(effect(&other, &v.old, &v.new, &change));
            }
        }

        for (path, e) in errors {
            if path == file {
                effects.push(e.clone());
            }

            all.push(e.clone());
        }

        let lines = Lines::new(old);
        // Completion's placeholder is never a method: the call it's in is
        // never typed, which isn't an error in the code.
        let placeholders: Vec<usize> = old.match_indices(PLACEHOLDER).map(|(i, _)| i).collect();
        let types = walker
            .out
            .iter()
            .filter_map(|e| {
                let l = e.location;
                let byte = |line: u32, column: u32| lines.offset(line, column);

                Some((
                    byte(l.line_start, l.column_start)?,
                    byte(l.line_end, l.column_end + 1)?,
                    e.typ.as_deref().filter(|t| !t.contains("<error>")),
                    e.raw.is_some_and(|t| inferred(walker.db, t)),
                ))
            })
            .filter(|(start, end, ..)| !placeholders.iter().any(|p| start <= p && p < end))
            .collect();

        Some(Guard {
            file,
            old,
            new,
            changes,
            versions,
            outline,
            types,
            effects,
            all,
            repaired,
            limits: RefCell::new(HashMap::new()),
            encoding,
        })
    }

    /// Whether the unit the old text's byte `offset` is in means what it
    /// did there (`limit`).
    fn safe(&self, offset: usize) -> bool {
        offset < self.limit(self.outline.unit_at(offset))
    }

    /// Where a unit stops meaning what it did: the end of the old text if
    /// no change touched it, it mentions none of the names a change is
    /// about, and the check typed all of it; 0 if not. A unit with an error
    /// may refer to something that didn't exist then, and is being typed
    /// now.
    ///
    /// In a repaired copy, a method whose body has blank lines still means
    /// what it will before the first: the blank text can't change a type
    /// that's complete without it (it could only fill in a `?`, or fail to
    /// type), and what's before it can't refer to what it declares. So
    /// there, the limit is the first blank line, if everything that ends
    /// before it has a complete type. An older check has no such limit: a
    /// complete type there may have come from text that changed since.
    fn limit(&self, unit: Unit) -> usize {
        if let Some(limit) = self.limits.borrow().get(&unit) {
            return *limit;
        }

        let limit = self.find_limit(unit);

        self.limits.borrow_mut().insert(unit, limit);
        limit
    }

    fn find_limit(&self, unit: Unit) -> usize {
        // Only comments and blank lines are between items, and they mean
        // nothing.
        if unit == Unit::Between {
            return usize::MAX;
        }

        let own = self.outline.own(unit);
        let touching: Vec<&Effect> = self
            .effects
            .iter()
            .filter(|e| {
                e.old.as_ref().is_some_and(|t| {
                    own.iter().any(|r| {
                        if t.is_empty() {
                            r.start < t.start && t.start < r.end
                        } else {
                            t.start < r.end && r.start < t.end
                        }
                    })
                })
            })
            .collect();
        let limit = if touching.is_empty() {
            usize::MAX
        } else if self.repaired && touching.iter().all(|e| e.body) {
            touching
                .iter()
                .filter_map(|e| e.old.as_ref().map(|r| r.start))
                .min()
                .unwrap()
        } else {
            return 0;
        };
        let before: Vec<std::ops::Range<usize>> = own
            .iter()
            .filter(|r| r.start < limit)
            .map(|r| r.start..r.end.min(limit))
            .collect();
        let types: Vec<(Option<&str>, bool)> = self
            .types
            .iter()
            .filter(|(start, ..)| before.iter().any(|r| r.contains(start)))
            .map(|(_, _, typ, complete)| (*typ, *complete))
            .collect();

        if types.iter().any(|(t, _)| t.is_none()) {
            return 0;
        }

        if limit < usize::MAX && types.iter().any(|(_, complete)| !complete) {
            return 0;
        }

        let types: Vec<Option<&str>> = types.into_iter().map(|(t, _)| t).collect();

        let mentioned: HashSet<&str> = before
            .iter()
            .flat_map(|r| names(&self.old[r.clone()]))
            .chain(types.into_iter().flatten().flat_map(names))
            .collect();

        if mentioned
            .iter()
            .any(|n| self.all.iter().any(|e| e.affects(n)))
        {
            return 0;
        }

        limit
    }

    /// A range of the old text as a range of the new one, if it's
    /// unchanged and in a safe unit.
    fn range(&self, range: Range) -> Option<Range> {
        let encoding = self.encoding;
        let start = byte_offset(self.old, range.start, encoding)?;
        let end = byte_offset(self.old, range.end, encoding)?;
        let mapped = self.changes.new_range(start..end)?;

        Some(Range {
            start: position_at(self.new, mapped.start, encoding),
            end: position_at(self.new, mapped.end, encoding),
        })
    }

    /// A place in any file, mapped to the file as it is: unchanged files
    /// stay as they are.
    fn place(&self, place: Place) -> Option<Place> {
        let encoding = self.encoding;
        if place.file == self.file {
            return Some(Place {
                range: self.range(place.range)?,
                file: place.file,
            });
        }

        let Some(v) = self.versions.get(&place.file) else {
            return Some(place);
        };
        let start = byte_offset(&v.old, place.range.start, encoding)?;
        let end = byte_offset(&v.old, place.range.end, encoding)?;
        let mapped = v.changes.new_range(start..end)?;

        Some(Place {
            range: Range {
                start: position_at(&v.new, mapped.start, encoding),
                end: position_at(&v.new, mapped.end, encoding),
            },
            file: place.file,
        })
    }

    /// A reply mapped to the documents as they are, or `None` if any of it
    /// doesn't hold.
    fn reply(&self, reply: Option<Reply>) -> Option<Option<Reply>> {
        Some(match reply {
            None => None,
            Some(Reply::Type(t)) => Some(Reply::Type(TypeAt {
                range: self.range(t.range)?,
                ..t
            })),
            Some(Reply::Members(m)) => Some(Reply::Members(MembersAt {
                receiver_range: self.range(m.receiver_range)?,
                ..m
            })),
            Some(Reply::Place(p)) => Some(Reply::Place(self.place(p)?)),
            Some(Reply::Places(ps)) => Some(Reply::Places(
                ps.into_iter()
                    .map(|p| self.place(p))
                    .collect::<Option<_>>()?,
            )),
            // The changed text may use the symbol where the old text didn't.
            Some(Reply::Highlights(hs)) if hs.iter().any(|h| self.typed_since(h.range)) => {
                return None;
            }
            Some(Reply::Highlights(hs)) => Some(Reply::Highlights(
                hs.into_iter()
                    .map(|h| {
                        Some(Highlight {
                            range: self.range(h.range)?,
                            access: h.access,
                        })
                    })
                    .collect::<Option<_>>()?,
            )),
            Some(Reply::Signature(s)) => Some(Reply::Signature(s)),
            Some(Reply::Hints(hs)) => Some(Reply::Hints(hs)),
        })
    }

    /// Whether the name at a range of the old text may have uses the check
    /// didn't see: in text added to the file since (or the start of the
    /// name is), or in a unit that isn't safe (`safe`), which the check may
    /// not have typed.
    fn typed_since(&self, range: Range) -> bool {
        let (Some(start), Some(end)) = (
            byte_offset(self.old, range.start, self.encoding),
            byte_offset(self.old, range.end, self.encoding),
        ) else {
            return true;
        };

        let text = &self.old[start..end];

        // An operator, such as `+`, is a name too, of a method.
        if names(text).next().is_none() {
            return !self.changes.is_none() || self.mentioned_unsafely(text);
        }

        names(text).any(|name| {
            let added = self
                .effects
                .iter()
                .flat_map(|e| &e.added)
                .any(|added| name.starts_with(added.as_str()));

            added || self.mentioned_unsafely(name)
        })
    }

    /// Whether a unit that isn't safe (`limit`) mentions `name` where it
    /// isn't safe: the check may not have seen a use of it there. A name
    /// that isn't an identifier (an operator) is looked for as text.
    fn mentioned_unsafely(&self, name: &str) -> bool {
        let operator = names(name).next().is_none();

        (0..self.outline.items.len())
            .map(Unit::Item)
            .filter(|u| self.outline.unit_at_item(*u))
            .any(|u| {
                let limit = self.limit(u);

                self.outline
                    .own(u)
                    .iter()
                    .filter(|r| r.end > limit)
                    .any(|r| {
                        let text = &self.old[r.start.max(limit)..r.end];

                        if operator {
                            text.contains(name)
                        } else {
                            names(text).any(|n| n == name)
                        }
                    })
            })
    }

    /// The hints that hold, mapped, with the parts of `start` to `end` (in
    /// the documents as they are) where they're all there are
    /// (`coverage`), or `None` if no line of it is in a safe unit.
    fn hints(&self, hints: Hints, start: Position, end: Position) -> Option<Reply> {
        let e = self.encoding;
        let covered = (start.line..=end.line).any(|line| {
            let Some(from) = byte_offset(self.new, Position { line, character: 0 }, e) else {
                return false;
            };
            let to = line_end(self.new, from);

            (from..to)
                .filter(|b| {
                    self.new.is_char_boundary(*b)
                        && !self.new[*b..].starts_with(char::is_whitespace)
                })
                .filter_map(|b| self.changes.old_offset(b))
                .any(|b| self.safe(b))
        });

        let coverage = self.coverage(start, end);

        if !covered || coverage.is_empty() {
            return None;
        }

        Some(Reply::Hints(Hints {
            hints: hints
                .hints
                .into_iter()
                .filter_map(|h| {
                    let at = self.hint_at(byte_offset(self.old, h.position, e)?)?;

                    Some(Hint {
                        position: position_at(self.new, at, e),
                        ..h
                    })
                })
                .collect(),
            covered: Some(coverage),
        }))
    }

    /// Where a hint at a byte of the old text goes in the new one, if it
    /// holds: in a unit that still means what it did, in text that's
    /// unchanged with the characters on either side.
    fn hint_at(&self, old: usize) -> Option<usize> {
        if !self.safe(old) {
            return None;
        }

        self.changes.new_range(old..old).map(|r| r.start)
    }

    /// Where, from `start` to `end` in the documents as they are, a hint
    /// from this check would be given if it had one (`hint_at`). There, the
    /// hints given are all there are. As ranges that include their ends.
    fn coverage(&self, start: Position, end: Position) -> Vec<Range> {
        let e = self.encoding;
        let Some(from) = byte_offset(self.new, start, e) else {
            return Vec::new();
        };
        let to = byte_offset(self.new, end, e)
            .unwrap_or(self.new.len())
            .max(from);
        let covered =
            |b: usize| self.changes.old_offset(b).and_then(|o| self.hint_at(o)) == Some(b);
        let mut runs: Vec<(usize, usize)> = Vec::new();
        let mut run: Option<(usize, usize)> = None;

        for b in (from..=to).filter(|b| self.new.is_char_boundary(*b)) {
            match (covered(b), &mut run) {
                (true, Some((_, last))) => *last = b,
                (true, None) => run = Some((b, b)),
                (false, Some(_)) => runs.extend(run.take()),
                (false, None) => {}
            }
        }

        runs.extend(run);
        runs.into_iter()
            .map(|(a, b)| Range {
                start: position_at(self.new, a, e),
                end: position_at(self.new, b, e),
            })
            .collect()
    }
}

fn declaration_errors() -> Unavailable {
    Unavailable::new(
        "declarationErrors",
        "an error in a declaration (e.g. an undefined type in a signature) \
         stopped the type checker before it typed method bodies",
    )
}

thread_local! {
    static PANIC: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Records each panic's message and location for `panic_message`, as
/// well as printing it as usual.
fn install_panic_hook() {
    static HOOK: Once = Once::new();

    HOOK.call_once(|| {
        let default = std::panic::take_hook();

        std::panic::set_hook(Box::new(move |info| {
            PANIC.with(|p| *p.borrow_mut() = Some(info.to_string()));
            default(info);
        }));
    });
}

/// The message of the last panic on this thread.
fn panic_message() -> String {
    PANIC
        .with(|p| p.borrow_mut().take())
        .unwrap_or_else(|| "the compiler panicked".to_string())
}

/// The stack size for a type check. The compiler is recursive, and the
/// default 2 MiB for spawned threads is less than the main thread's 8 MiB.
const STACK_SIZE: usize = 64 * 1024 * 1024;

/// Runs `f` on its own thread and returns `Err` with the panic's message
/// and location if it panics, so a compiler panic can't take the process
/// down. Nothing outlives `f`: each type check builds its own compiler
/// state, so there's nothing a panic can leave half-updated.
pub fn guarded<R: Send>(f: impl FnOnce() -> R + Send) -> Result<R, String> {
    install_panic_hook();

    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("check".to_string())
            .stack_size(STACK_SIZE)
            .spawn_scoped(scope, || {
                catch_unwind(AssertUnwindSafe(f)).map_err(|_| panic_message())
            })
            .map_err(|e| format!("failed to start a thread: {}", e))?
            .join()
            .unwrap_or_else(|_| Err("the compiler panicked".to_string()))
    })
}

/// The members `receiver.name` could call or read: each candidate name is
/// kept only if the compiler's own `TypeEnum::lookup_method` accepts it
/// (types/src/lib.rs:6075-6105, called the same way as
/// compiler/src/type_check/expressions.rs:3771-3776), so visibility and static/instance
/// rules come from upstream. Methods that need an owned or mutable receiver
/// are left out when the receiver isn't one, as `check_mutability` reports
/// (compiler/src/type_check/expressions.rs:458-499). Fields follow
/// `lookup_field_with_receiver` (compiler/src/type_check/expressions.rs:4888-4922):
/// instance fields of a type that's not async, visible from the calling
/// module.
fn members(
    db: &Database,
    value: TypeRef,
    receiver: TypeEnum,
    module: ModuleId,
    is_self: bool,
) -> Vec<Member> {
    let arguments = type_arguments(db, receiver);
    let format = |typ: &dyn Fn(TypeFormatter) -> String| {
        typ(TypeFormatter::new(db, Some(receiver), arguments.as_ref()))
    };
    let mut names = Vec::new();

    candidate_methods(db, receiver, &mut names);

    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();

    for name in names {
        if !seen.insert(name.clone()) {
            continue;
        }

        let MethodLookup::Ok(id) = receiver.lookup_method(db, &name, module, is_self) else {
            continue;
        };

        if id.is_generated(db)
            || (id.is_moving(db) && !value.allow_moving(db))
            || (id.is_mutable(db) && !value.allow_mutating(db))
        {
            continue;
        }

        // A default method a type gets from a generic trait uses the trait's
        // type parameters, which the type's implementation of the trait
        // assigns (e.g. `impl Iter[ref T] for Iter`).
        let mut arguments = arguments.clone();

        if let MethodSource::Implemented(ins, _) | MethodSource::Inherited(ins, _) = id.source(db)
            && let Some(inherited) = ins.type_arguments(db)
        {
            let args = arguments.get_or_insert_with(TypeArguments::new);

            for (param, value) in inherited.pairs() {
                if args.get(param).is_none() {
                    args.assign(param, value);
                }
            }
        }

        // A method in an `impl ... if T: Trait` block uses a copy of `T`
        // with those bounds, which gets the same argument as `T`.
        if let Some(args) = arguments.as_mut() {
            for (original, bounded) in id.bounds(db).iter() {
                if let Some(value) = args.get(*original) {
                    args.assign(*bounded, value);
                }
            }
        }

        let format = |typ: &dyn Fn(TypeFormatter) -> String| {
            typ(TypeFormatter::new(db, Some(receiver), arguments.as_ref()))
        };

        out.push(Member {
            name,
            kind: MemberKind::Method,
            signature: format(&|f| f.format(id)),
            typ: format(&|f| f.format(id.return_type(db))),
            module: id.module(db).name(db).to_string(),
        });
    }

    if let TypeEnum::TypeInstance(ins) = receiver {
        let typ = ins.instance_of();

        if !typ.is_async(db) {
            for field in typ.fields(db) {
                if !field.is_visible_to(db, module) || !seen.insert(field.name(db).clone()) {
                    continue;
                }

                let value = format(&|f| f.format(field.value_type(db)));
                let visibility = if field.is_public(db) { "pub " } else { "" };

                out.push(Member {
                    name: field.name(db).clone(),
                    kind: MemberKind::Field,
                    signature: format!("let {}@{}: {}", visibility, field.name(db), value),
                    typ: value,
                    module: typ.module(db).name(db).to_string(),
                });
            }
        }
    }

    if let TypeEnum::Module(id) = receiver {
        module_symbols(db, id, module, &mut seen, &mut out);
    }

    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// `module.CONSTANT` and `module.Type`, as `call_with_receiver` resolves them
/// (compiler/src/type_check/expressions.rs:3819-3830). Only symbols the module defines,
/// and that the calling module can see, are listed.
fn module_symbols(
    db: &Database,
    id: ModuleId,
    from: ModuleId,
    seen: &mut std::collections::HashSet<String>,
    out: &mut Vec<Member>,
) {
    for (name, symbol) in id.symbols(db) {
        if name.starts_with('$') || !symbol.is_visible_to(db, from) || seen.contains(&name) {
            continue;
        }

        let (kind, signature, typ, owner) = match symbol {
            Symbol::Constant(c) if c.module(db) == id => {
                let typ = types::format::format_type(db, c.value_type(db));
                let visibility = if c.is_public(db) { "pub " } else { "" };

                (
                    MemberKind::Constant,
                    format!("let {}{}: {}", visibility, name, typ),
                    typ,
                    c.module(db),
                )
            }
            Symbol::Type(t) if t.module(db) == id => {
                let typ = types::format::format_type(db, t);

                (MemberKind::Type, format!("type {}", typ), typ, t.module(db))
            }
            _ => continue,
        };

        seen.insert(name.clone());
        out.push(Member {
            name,
            kind,
            signature,
            typ,
            module: owner.name(db).to_string(),
        });
    }
}

fn type_arguments(db: &Database, receiver: TypeEnum) -> Option<TypeArguments> {
    match receiver {
        TypeEnum::TypeInstance(ins) => ins.type_arguments(db).cloned(),
        TypeEnum::TraitInstance(ins) => ins.type_arguments(db).cloned(),
        _ => None,
    }
}

/// The names `TypeEnum::method` (types/src/lib.rs:6107-6119) could find on a
/// receiver, in lookup order.
fn candidate_methods(db: &Database, receiver: TypeEnum, out: &mut Vec<String>) {
    let mut add = |ids: Vec<MethodId>| out.extend(ids.into_iter().map(|m| m.name(db).clone()));

    match receiver {
        TypeEnum::Type(id) => add(id.methods(db)),
        TypeEnum::TypeInstance(ins) => add(ins.instance_of().methods(db)),
        TypeEnum::Module(id) => {
            add(id.methods(db));
            add(id.extern_methods(db).values().copied().collect());
        }
        TypeEnum::Trait(id) => trait_methods(db, id, &mut add, &mut Vec::new()),
        TypeEnum::TraitInstance(ins) => {
            trait_methods(db, ins.instance_of(), &mut add, &mut Vec::new())
        }
        TypeEnum::TypeParameter(id) | TypeEnum::RigidTypeParameter(id) => {
            let mut visited = Vec::new();

            for req in id.requirements(db) {
                trait_methods(db, req.instance_of(), &mut add, &mut visited);
            }
        }
        TypeEnum::AtomicTypeParameter(_) | TypeEnum::Closure(_) | TypeEnum::Foreign(_) => {}
    }
}

/// A trait's default and required methods, then those of the traits it
/// requires (`TraitId::method`, types/src/lib.rs:1025-1043).
fn trait_methods(
    db: &Database,
    id: types::TraitId,
    add: &mut impl FnMut(Vec<MethodId>),
    visited: &mut Vec<types::TraitId>,
) {
    if visited.contains(&id) {
        return;
    }

    visited.push(id);
    add(id.default_methods(db));
    add(id.required_methods(db));

    for req in id.required_traits(db) {
        trait_methods(db, req.instance_of(), add, visited);
    }
}
