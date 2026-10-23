//! The questions Cuttlebone answers: the type at a position, and the members
//! available after a `.`.
//!
//! Every answer is computed from a fresh type check of the file and what it
//! imports. Nothing is kept between requests.
use crate::position::{Encoding, HirPosition, Lines, Position, contains};
use crate::walker::{Expr, Walker, expressions_pass_ran};
use crate::{Target, project_root, run};
use compiler::hir;
use location::Location;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
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

/// An answer, and how long it took.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Answer<T> {
    /// `Ok(None)` means the check worked but there's nothing at the position.
    pub result: Result<Option<T>, Unavailable>,
    pub elapsed: Duration,
}

/// The type of the expression at a position.
pub fn type_at(session: &Session, request: &Request) -> Answer<TypeAt> {
    answer(session, request, |_, _, walker, lines, at| {
        let Some(expr) = select(&walker.out, at) else {
            return Ok(None);
        };

        Ok(Some(TypeAt {
            typ: typed(expr)?.to_string(),
            range: range(lines, &expr.location),
            kind: expr.kind.to_string(),
        }))
    })
}

/// The members available on the receiver of the call whose name is at a
/// position, e.g. at `foo` in `value.foo`.
pub fn members_at(session: &Session, request: &Request) -> Answer<MembersAt> {
    answer(session, request, |db, module, walker, lines, at| {
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

type Inspect<'a, T> = dyn FnOnce(&Database, ModuleId, &Walker, &Lines, HirPosition) -> Result<Option<T>, Unavailable>
    + Send
    + 'a;

/// Type checks the request's file and passes its walked HIR to `inspect`.
fn answer<T: Send>(
    session: &Session,
    request: &Request,
    inspect: impl FnOnce(
        &Database,
        ModuleId,
        &Walker,
        &Lines,
        HirPosition,
    ) -> Result<Option<T>, Unavailable>
    + Send,
) -> Answer<T> {
    let start = Instant::now();
    let inspect: Box<Inspect<T>> = Box::new(inspect);
    let result = prepare(session, request).and_then(|(target, text)| {
        guarded(move || analyze(&target, &text, request.position, request.encoding, inspect))
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

    // The innermost project wins, e.g. a dependency checked out inside
    // another project.
    let root = session
        .projects
        .iter()
        .filter(|p| file.starts_with(p.join("src")) || file.starts_with(p.join("test")))
        .max_by_key(|p| p.components().count())
        .cloned()
        .unwrap_or_else(|| project_root(&file));

    let target = Target {
        root,
        file: Some(file),
        tests: true,
        std: session.std.clone(),
        overlay,
    };

    Ok((target, text))
}

fn canonical(path: &Path) -> Result<PathBuf, Unavailable> {
    path.canonicalize()
        .map_err(|e| Unavailable::new("fileNotFound", format!("{}: {}", path.display(), e)))
}

fn analyze<T>(
    target: &Target,
    text: &str,
    position: Position,
    encoding: Encoding,
    inspect: Box<Inspect<T>>,
) -> Result<Option<T>, Unavailable> {
    let lines = Lines::with_encoding(text, encoding);
    let at = lines
        .to_hir(position)
        .ok_or_else(|| invalid_position(position))?;
    let file = target.file.clone().unwrap();

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
        inspect(db, module.module_id, &walker, &lines, at)
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
