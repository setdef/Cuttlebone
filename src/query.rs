//! The questions Cuttlebone answers: the type at a position, the members
//! available after a `.`, and where the symbol at a position is declared and
//! referred to.
//!
//! Every answer is computed from a fresh type check of the file and what it
//! imports. Nothing is kept between requests.
use crate::check::{Target, diagnose, project_root, run};
use crate::position::{Encoding, HirPosition, Lines, Position, contains};
use crate::walker::{
    Expr, Reference, SymbolId, Walker, declarations, expressions_pass_ran, type_symbol,
};
use compiler::hir;
use location::Location;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::time::{Duration, Instant};
use types::format::TypeFormatter;
use types::{
    Block, Database, MethodId, MethodLookup, MethodSource, ModuleId, Symbol, TypeArguments,
    TypeEnum, TypeRef,
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
}

impl Unavailable {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Unavailable {
            code: code.to_string(),
            message: message.into(),
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
}

/// The type of the expression at a position.
pub fn type_at(session: &Session, request: &Request) -> Answer<TypeAt> {
    answer(session, request, false, |scope| {
        let Some(expr) = select(&scope.walker.out, scope.at) else {
            return Ok(None);
        };

        Ok(Some(TypeAt {
            typ: typed(expr)?.to_string(),
            range: range(scope.lines, &expr.location),
            kind: expr.kind.to_string(),
        }))
    })
}

/// The members available on the receiver of the call whose name is at a
/// position, e.g. at `foo` in `value.foo`.
pub fn members_at(session: &Session, request: &Request) -> Answer<MembersAt> {
    answer(session, request, false, |scope| {
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
    })
}

/// Where the symbol at a position is declared: a method (for a call, the
/// called method), a variable, a field, a type, trait, type parameter,
/// constant, constructor or module. On a declaration's own name, that
/// declaration.
pub fn definition(session: &Session, request: &Request) -> Answer<Place> {
    answer(session, request, false, |scope| {
        let Some(reference) = symbol_at(
            &scope.walker.references,
            &scope.walker.declarations,
            scope.at,
        ) else {
            return Ok(None);
        };
        let index = Index::new(scope);

        Ok(index
            .declaration(reference.symbol)
            .map(|(file, location)| scope.place(&file, location)))
    })
}

/// Where the type of the expression at a position is declared, without
/// its ownership or type arguments: `ref Option[String]` is `Option`. On a
/// type name (such as in a signature), that type. `None` for types without
/// a declaration: closures, C types and `Never`.
pub fn type_definition(session: &Session, request: &Request) -> Answer<Place> {
    answer(session, request, false, |scope| {
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
        let index = Index::new(scope);

        Ok(index
            .declaration(symbol)
            .map(|(file, location)| scope.place(&file, location)))
    })
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
    answer(session, request, true, |scope| {
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

            if file == scope.file {
                collect(&file, scope.walker);
            } else if scope.in_project(&file) {
                let mut walker = Walker::new(scope.db, module.module_id);

                walker.module(module);
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
    })
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
        guarded(|| diagnose(&target).map(|d| (d, PANIC.with(|p| p.borrow_mut().take()))))
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
                        let text = texts.entry(d.file.clone()).or_insert_with(|| {
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

/// Where each symbol of a check is declared.
pub struct Index {
    declared: HashMap<SymbolId, (PathBuf, Location)>,
}

impl Index {
    /// The declarations of every module checked, and the variables of the
    /// walked module.
    fn new(scope: &Scope) -> Index {
        let mut index = Index::from_modules(scope.db, scope.modules);

        for d in &scope.walker.declarations {
            index.add(&scope.file, d);
        }

        index
    }

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
    let start = Instant::now();
    let prepared = canonical(&request.file).and_then(|file| {
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
        let mut copy = text;

        copy.insert_str(offset, PLACEHOLDER);

        let mut contents: HashMap<PathBuf, String> = request
            .contents
            .iter()
            .filter(|(p, _)| canonical(p).ok() != Some(file.clone()))
            .map(|(p, t)| (p.clone(), t.clone()))
            .collect();

        contents.insert(file.clone(), copy);
        Ok(Request {
            file,
            position: request.position,
            encoding: request.encoding,
            contents,
        })
    });

    match prepared {
        Ok(copy) => {
            let mut answer = members_at(session, &copy);

            answer.elapsed = start.elapsed();
            answer
        }
        Err(e) => Answer {
            result: Err(e),
            elapsed: start.elapsed(),
        },
    }
}

/// The byte offset of a position, or `None` if its line is past the end of
/// the text. A character past the end of its line is the end of the line,
/// and one inside a character is the start of that character.
pub fn byte_offset(text: &str, position: Position, encoding: Encoding) -> Option<usize> {
    let mut start = 0;

    for _ in 0..position.line {
        start += text[start..].find('\n')? + 1;
    }

    let line = &text[start..];
    let line = &line[..line.find('\n').unwrap_or(line.len())];
    let line = line.strip_suffix('\r').unwrap_or(line);
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
struct Scope<'a> {
    db: &'a Database,
    /// Every module checked, including imported ones.
    modules: &'a [hir::Module],
    /// The requested file.
    file: PathBuf,
    module: ModuleId,
    walker: &'a Walker<'a>,
    lines: &'a Lines<'a>,
    at: HirPosition,
    target: &'a Target,
    encoding: Encoding,
    /// The text of files other than the requested one, as read for `place`.
    texts: RefCell<HashMap<PathBuf, String>>,
}

impl Scope<'_> {
    /// A location in any file as a range in the request's encoding.
    fn place(&self, file: &Path, location: Location) -> Place {
        if file == self.file {
            return Place {
                file: file.to_path_buf(),
                range: range(self.lines, &location),
            };
        }

        let mut texts = self.texts.borrow_mut();
        let text = texts.entry(file.to_path_buf()).or_insert_with(|| {
            self.target
                .overlay
                .get(file)
                .cloned()
                .unwrap_or_else(|| std::fs::read_to_string(file).unwrap_or_default())
        });

        Place {
            file: file.to_path_buf(),
            range: range(&Lines::with_encoding(text, self.encoding), &location),
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

type Inspect<'a, T> = dyn FnOnce(&Scope) -> Result<Option<T>, Unavailable> + Send + 'a;

/// Type checks the request's file and passes its walked HIR to `inspect`.
/// With `project` set, it checks the whole project the file is part of
/// (unless the file is outside its `src/` and `test/`), not just the file
/// and what it imports.
fn answer<T: Send>(
    session: &Session,
    request: &Request,
    project: bool,
    inspect: impl FnOnce(&Scope) -> Result<Option<T>, Unavailable> + Send,
) -> Answer<T> {
    let start = Instant::now();
    let inspect: Box<Inspect<T>> = Box::new(inspect);
    let result = prepare(session, request).and_then(|(mut target, text)| {
        let file = target.file.clone().unwrap();

        if project && in_sources(&target.root, &file) {
            target.file = None;
        }

        guarded(move || {
            analyze(
                &target,
                &file,
                &text,
                request.position,
                request.encoding,
                inspect,
            )
        })
        .unwrap_or_else(|message| Err(Unavailable::new("compilerPanic", message)))
    });

    Answer {
        result,
        elapsed: start.elapsed(),
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

fn analyze<T>(
    target: &Target,
    file: &Path,
    text: &str,
    position: Position,
    encoding: Encoding,
    inspect: Box<Inspect<T>>,
) -> Result<Option<T>, Unavailable> {
    let lines = Lines::with_encoding(text, encoding);
    let at = lines
        .to_hir(position)
        .ok_or_else(|| invalid_position(position))?;

    let run = run(target, false, false, |db, modules: &[hir::Module]| {
        if !expressions_pass_ran(modules) {
            return Err(Unavailable::new(
                "declarationErrors",
                "an error in a declaration (e.g. an undefined type in a signature) \
                 stopped the type checker before it typed method bodies",
            ));
        }

        let module = modules
            .iter()
            .find(|m| m.module_id.file(db) == file)
            .ok_or_else(|| Unavailable::new("internal", "the file wasn't type checked"))?;
        let mut walker = Walker::new(db, module.module_id);

        walker.module(module);
        inspect(&Scope {
            db,
            modules,
            file: file.to_path_buf(),
            module: module.module_id,
            walker: &walker,
            lines: &lines,
            at,
            target,
            encoding,
            texts: RefCell::new(HashMap::new()),
        })
    })
    .map_err(|e| Unavailable::new("compilerError", e))?;

    run.inspected.unwrap_or_else(|| {
        Err(Unavailable::new(
            "noHir",
            "the file or a module it imports couldn't be parsed or found, \
             so nothing was type checked",
        ))
    })
}

thread_local! {
    static PANIC: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The stack size for a type check. The compiler is recursive, and the
/// default 2 MiB for spawned threads is less than the main thread's 8 MiB.
const STACK_SIZE: usize = 64 * 1024 * 1024;

/// Runs `f` on its own thread and returns `Err` with the panic's message
/// and location if it panics, so a compiler panic can't take the process
/// down. Nothing outlives `f`: each type check builds its own compiler
/// state, so there's nothing a panic can leave half-updated.
pub fn guarded<R: Send>(f: impl FnOnce() -> R + Send) -> Result<R, String> {
    static HOOK: Once = Once::new();

    HOOK.call_once(|| {
        let default = std::panic::take_hook();

        std::panic::set_hook(Box::new(move |info| {
            PANIC.with(|p| *p.borrow_mut() = Some(info.to_string()));
            default(info);
        }));
    });

    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("check".to_string())
            .stack_size(STACK_SIZE)
            .spawn_scoped(scope, || {
                catch_unwind(AssertUnwindSafe(f)).map_err(|_| {
                    PANIC
                        .with(|p| p.borrow_mut().take())
                        .unwrap_or_else(|| "the compiler panicked".to_string())
                })
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
