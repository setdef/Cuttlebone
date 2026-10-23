//! Cuttlebone, a language server for Inko.
//!
//! For now this is the feasibility spike: type check a project with the
//! upstream compiler, then walk the typed HIR and report each expression's
//! type.
use compiler::compiler::{CompileError, Compiler};
use compiler::config::Config;
use compiler::hir;
use location::Location;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use types::format::format_type;
use types::{
    CallKind, ConstantKind, Database, IdentifierKind, ModuleId, Symbol, ThrowKind, TypeEnum,
    TypeRef,
};

/// One expression in the typed HIR.
#[derive(Clone, Debug)]
pub struct Expr {
    pub file: PathBuf,
    pub location: Location,
    /// The HIR node's variant name, e.g. `Call`.
    pub kind: &'static str,
    /// The formatted type, or `None` if the HIR doesn't record one.
    pub typ: Option<String>,
    pub origin: Origin,
}

/// Where an expression's type comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Origin {
    /// Stored in the HIR node by the type checker, directly or in its
    /// call/field/variable info.
    Stored,
    /// Not stored; computed here by the same rule the type checker uses
    /// (`break`, `next`, `loop`, `try`).
    Derived,
    /// A type or module used as a static receiver (`Foo.new`, `mod.Foo`):
    /// the type checker doesn't store these, so they're looked up again in
    /// the module's symbol table.
    Symbol,
}

impl Expr {
    pub fn status(&self) -> Status {
        match &self.typ {
            None => Status::Missing,
            Some(t) if t.contains("<error>") => Status::Error,
            // An unassigned placeholder without a type parameter formats as
            // `?`, and `<unknown>` is TypeRef::Unknown nested in a type.
            Some(t) if t.contains('?') || t.contains("<unknown>") => Status::Partial,
            Some(_) => Status::Resolved,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Resolved,
    Partial,
    Error,
    Missing,
}

pub struct Checked {
    pub expressions: Vec<Expr>,
    /// Whether type checking and lowering to MIR reported no errors.
    pub ok: bool,
    /// The number of modules type checked, including imported ones.
    pub modules: usize,
    /// Parsing, lowering to HIR and type checking.
    pub type_check: Duration,
    /// The above, plus lowering to MIR (where e.g. moves are checked).
    pub total: Duration,
}

/// What to check.
pub struct Target {
    /// The project directory, containing `src/`.
    pub root: PathBuf,
    /// A single file to check, instead of the whole project.
    pub file: Option<PathBuf>,
    /// Whether to also check the project's `test/` directory.
    pub tests: bool,
    /// The standard library's source directory.
    pub std: PathBuf,
}

impl Target {
    /// Finds the project for a directory or a file in one.
    pub fn new(path: &Path, std: PathBuf) -> Result<Target, String> {
        let path = path
            .canonicalize()
            .map_err(|e| format!("{}: {}", path.display(), e))?;

        if path.is_dir() {
            return Ok(Target {
                root: path,
                file: None,
                tests: true,
                std,
            });
        }

        let root = path
            .ancestors()
            .find(|p| p.join("src").is_dir() && path.starts_with(p.join("src")))
            .or_else(|| {
                path.ancestors()
                    .find(|p| p.join("test").is_dir() && path.starts_with(p.join("test")))
            })
            .map(Path::to_path_buf)
            .unwrap_or_else(|| path.parent().unwrap().to_path_buf());

        Ok(Target {
            root,
            file: Some(path),
            tests: true,
            std,
        })
    }

    fn config(&self) -> Result<Config, String> {
        let mut config = Config::new()?;

        config.std = self.std.clone();
        config.add_source_directory(self.std.clone());
        config.source = self.root.join("src");
        config.tests = self.root.join("test");
        config.build = self.root.join("build");
        config.dependencies = self.root.join("dep");

        if self.tests && config.tests.is_dir() {
            config.add_source_directory(config.tests.clone());
        } else {
            // Compiler::check includes test/ whenever it exists.
            config.tests = self.root.join("\0no-tests");
        }

        Ok(config)
    }
}

/// Type checks the target and collects every expression of its own modules.
pub fn check(target: &Target) -> Result<Checked, String> {
    let mut compiler = Compiler::new(target.config()?);
    let start = Instant::now();
    let mut type_check = Duration::ZERO;
    let mut expressions = Vec::new();
    let mut module_count = 0;
    let res = compiler.check_with(target.file.clone(), |db, modules| {
        type_check = start.elapsed();
        module_count = modules.len();

        for module in modules {
            let file = module.module_id.file(db);

            // Imported modules (std, dependencies) are type checked too, but
            // only the target's own modules are reported.
            let own = match &target.file {
                Some(f) => &file == f,
                None => file.starts_with(&target.root),
            };

            if own {
                let symbols = module.module_id.symbols(db).into_iter().collect();

                Walker {
                    db,
                    file,
                    symbols,
                    out: &mut expressions,
                }
                .module(module);
            }
        }
    });
    let total = start.elapsed();

    let ok = match res {
        Ok(()) => true,
        Err(CompileError::Invalid) => {
            compiler.print_diagnostics();
            false
        }
        Err(CompileError::Internal(e)) => return Err(e),
    };

    expressions.sort_by(|a, b| {
        (&a.file, a.location.line_start, a.location.column_start)
            .cmp(&(&b.file, b.location.line_start, b.location.column_start))
            .then(
                (b.location.line_end, b.location.column_end)
                    .cmp(&(a.location.line_end, a.location.column_end)),
            )
    });
    Ok(Checked {
        expressions,
        ok,
        modules: module_count,
        type_check,
        total,
    })
}

struct Walker<'a> {
    db: &'a Database,
    file: PathBuf,
    /// The module's symbols, for static receivers.
    symbols: HashMap<String, Symbol>,
    out: &'a mut Vec<Expr>,
}

impl Walker<'_> {
    fn module(&mut self, module: &hir::Module) {
        for expr in &module.expressions {
            self.top_level(expr);
        }
    }

    fn top_level(&mut self, node: &hir::TopLevelExpression) {
        match node {
            hir::TopLevelExpression::Type(n) => {
                for expr in &n.body {
                    match expr {
                        hir::TypeExpression::InstanceMethod(m) => self.body(&m.body),
                        hir::TypeExpression::StaticMethod(m) => self.body(&m.body),
                        hir::TypeExpression::AsyncMethod(m) => self.body(&m.body),
                        hir::TypeExpression::Field(_) | hir::TypeExpression::Constructor(_) => {}
                    }
                }
            }
            hir::TopLevelExpression::Constant(n) => self.constant(&n.value),
            hir::TopLevelExpression::ModuleMethod(n) => self.body(&n.body),
            hir::TopLevelExpression::Trait(n) => {
                for expr in &n.body {
                    match expr {
                        hir::TraitExpression::InstanceMethod(m) => self.body(&m.body),
                        hir::TraitExpression::RequiredMethod(_) => {}
                    }
                }
            }
            hir::TopLevelExpression::Implement(n) => {
                for m in &n.body {
                    self.body(&m.body);
                }
            }
            hir::TopLevelExpression::Reopen(n) => {
                for expr in &n.body {
                    match expr {
                        hir::ReopenTypeExpression::InstanceMethod(m) => self.body(&m.body),
                        hir::ReopenTypeExpression::StaticMethod(m) => self.body(&m.body),
                        hir::ReopenTypeExpression::AsyncMethod(m) => self.body(&m.body),
                    }
                }
            }
            hir::TopLevelExpression::ExternType(_)
            | hir::TopLevelExpression::ExternFunction(_)
            | hir::TopLevelExpression::Import(_)
            | hir::TopLevelExpression::ExternImport(_) => {}
        }
    }

    fn push(&mut self, location: Location, kind: &'static str, typ: Option<TypeRef>) {
        self.push_from(location, kind, typ, Origin::Stored);
    }

    fn push_from(
        &mut self,
        location: Location,
        kind: &'static str,
        typ: Option<TypeRef>,
        origin: Origin,
    ) {
        let typ = typ
            .and_then(|t| self.resolve(t))
            .map(|t| format_type(self.db, t));

        self.out.push(Expr {
            file: self.file.clone(),
            location,
            kind,
            typ,
            origin,
        });
    }

    /// Visits a call's receiver, which may be a type or module rather than a
    /// value.
    fn receiver(&mut self, node: &hir::Expression) {
        let start = self.out.len();

        self.expression(node);

        if self.out.get(start).is_some_and(|e| e.typ.is_none())
            && let Some(typ) = self.static_receiver(node)
        {
            self.out[start].typ = Some(format_type(self.db, typ));
            self.out[start].origin = Origin::Symbol;
        }
    }

    /// The type the type checker gives a type or module used as a receiver.
    fn static_receiver(&self, node: &hir::Expression) -> Option<TypeRef> {
        let symbol = match node {
            hir::Expression::ConstantRef(n) if matches!(n.kind, ConstantKind::Unknown) => {
                match &n.source {
                    None => self.symbols.get(&n.name).copied(),
                    Some(src) => self.module_symbol(&src.name, &n.name),
                }
            }
            hir::Expression::IdentifierRef(n) if matches!(n.kind, IdentifierKind::Unknown) => {
                self.symbols.get(&n.name).copied()
            }
            hir::Expression::Call(n) if matches!(n.kind, CallKind::Unknown) => match &n.receiver {
                Some(hir::Expression::IdentifierRef(r)) => {
                    self.module_symbol(&r.name, &n.name.name)
                }
                _ => None,
            },
            _ => None,
        };

        match symbol? {
            Symbol::Type(id) => Some(TypeRef::Owned(TypeEnum::Type(id))),
            Symbol::Module(id) => Some(TypeRef::module(id)),
            _ => None,
        }
    }

    /// Looks up `name` in the module imported as `module`.
    fn module_symbol(&self, module: &str, name: &str) -> Option<Symbol> {
        let Some(Symbol::Module(id)) = self.symbols.get(module) else {
            return None;
        };

        symbol(self.db, *id, name)
    }

    /// Returns `None` for a type the type checker never filled in.
    fn resolve(&self, typ: TypeRef) -> Option<TypeRef> {
        match typ {
            TypeRef::Unknown => None,
            TypeRef::Placeholder(id) => id.value(self.db).and_then(|t| self.resolve(t)),
            t => Some(t),
        }
    }

    fn constant(&mut self, node: &hir::ConstExpression) {
        use hir::ConstExpression as C;

        match node {
            C::Int(n) => self.push(n.location, "ConstInt", Some(n.resolved_type)),
            C::String(n) => self.push(n.location, "ConstString", Some(n.resolved_type)),
            C::Float(n) => self.push(n.location, "ConstFloat", Some(n.resolved_type)),
            C::Binary(n) => {
                self.push(n.location, "ConstBinary", Some(n.resolved_type));
                self.constant(&n.left);
                self.constant(&n.right);
            }
            C::ConstantRef(n) => self.push(n.location, "ConstConstantRef", Some(n.resolved_type)),
            C::Array(n) => {
                self.push(n.location, "ConstArray", Some(n.resolved_type));

                for v in &n.values {
                    self.constant(v);
                }
            }
            C::True(n) => self.push(n.location, "ConstTrue", Some(n.resolved_type)),
            C::False(n) => self.push(n.location, "ConstFalse", Some(n.resolved_type)),
        }
    }

    fn body(&mut self, nodes: &[hir::Expression]) {
        for n in nodes {
            self.expression(n);
        }
    }

    fn arguments(&mut self, nodes: &[hir::Argument]) {
        for arg in nodes {
            match arg {
                hir::Argument::Positional(a) => self.expression(&a.value),
                hir::Argument::Named(a) => self.expression(&a.value),
            }
        }
    }

    fn call_kind(&self, kind: &CallKind) -> Option<TypeRef> {
        match kind {
            CallKind::Unknown => None,
            CallKind::Call(info) => Some(info.returns),
            CallKind::CallClosure(info) => Some(info.returns),
            CallKind::GetField(info) => Some(info.variable_type),
            CallKind::GetConstant(id) => Some(id.value_type(self.db)),
            CallKind::ReadPointer(typ) => Some(*typ),
            CallKind::TypeInstance(info) => Some(info.resolved_type),
            CallKind::SetField(_) | CallKind::WritePointer => Some(TypeRef::nil()),
        }
    }

    fn expression(&mut self, node: &hir::Expression) {
        use hir::Expression as E;

        match node {
            E::And(n) => {
                self.push(n.location, "And", Some(n.resolved_type));
                self.expression(&n.left);
                self.expression(&n.right);
            }
            E::Or(n) => {
                self.push(n.location, "Or", Some(n.resolved_type));
                self.expression(&n.left);
                self.expression(&n.right);
            }
            E::AssignField(n) => {
                self.push(n.location, "AssignField", Some(n.resolved_type));
                self.expression(&n.value);
            }
            E::ReplaceField(n) => {
                self.push(n.location, "ReplaceField", Some(n.resolved_type));
                self.expression(&n.value);
            }
            E::AssignSetter(n) => {
                // A setter assignment's own value is the setter's return
                // value, or nil when it assigns a field directly.
                let typ = self.call_kind(&n.kind);

                self.push(n.location, "AssignSetter", typ);
                self.receiver(&n.receiver);
                self.expression(&n.value);
            }
            E::ReplaceSetter(n) => {
                self.push(n.location, "ReplaceSetter", Some(n.resolved_type));
                self.receiver(&n.receiver);
                self.expression(&n.value);
            }
            E::AssignVariable(n) => {
                self.push(n.location, "AssignVariable", Some(n.resolved_type));
                self.expression(&n.value);
            }
            E::ReplaceVariable(n) => {
                self.push(n.location, "ReplaceVariable", Some(n.resolved_type));
                self.expression(&n.value);
            }
            // The type checker types `break` and `next` as Never without
            // storing it.
            E::Break(n) => {
                self.push_from(n.location, "Break", Some(TypeRef::Never), Origin::Derived)
            }
            E::Next(n) => self.push_from(n.location, "Next", Some(TypeRef::Never), Origin::Derived),
            E::BuiltinCall(n) => {
                let typ = n.info.as_ref().map(|i| i.returns);

                self.push(n.location, "BuiltinCall", typ);
                self.body(&n.arguments);
            }
            E::Call(n) => {
                let typ = self.call_kind(&n.kind);

                self.push(n.location, "Call", typ);

                if let Some(r) = &n.receiver {
                    self.receiver(r);
                }

                self.arguments(&n.arguments);
            }
            E::Closure(n) => {
                self.push(n.location, "Closure", Some(n.resolved_type));
                self.body(&n.body);
            }
            E::ConstantRef(n) => {
                // A constant that's really a method call (e.g. `Foo()`
                // without a receiver) records the call instead.
                let typ = match &n.kind {
                    ConstantKind::Method(info) => Some(info.returns),
                    _ => Some(n.resolved_type),
                };

                self.push(n.location, "ConstantRef", typ);
            }
            E::DefineVariable(n) => {
                self.push(n.location, "DefineVariable", Some(n.resolved_type));
                self.expression(&n.value);

                if let Some(e) = &n.else_body {
                    self.expression(e);
                }
            }
            E::False(n) => self.push(n.location, "False", Some(n.resolved_type)),
            E::True(n) => self.push(n.location, "True", Some(n.resolved_type)),
            E::Nil(n) => self.push(n.location, "Nil", Some(n.resolved_type)),
            E::FieldRef(n) => {
                let typ = n.info.as_ref().map(|i| i.variable_type);

                self.push(n.location, "FieldRef", typ);
            }
            E::Float(n) => self.push(n.location, "Float", Some(n.resolved_type)),
            E::Int(n) => self.push(n.location, "Int", Some(n.resolved_type)),
            E::String(n) => self.push(n.location, "String", Some(n.resolved_type)),
            E::IdentifierRef(n) => {
                let typ = match &n.kind {
                    IdentifierKind::Unknown => None,
                    IdentifierKind::Variable(id) => Some(id.value_type(self.db)),
                    IdentifierKind::Method(info) => Some(info.returns),
                };

                self.push(n.location, "IdentifierRef", typ);
            }
            E::Loop(n) => {
                // Not stored: a loop is nil if something breaks out of it,
                // and Never otherwise.
                let typ = if breaks(&n.body) {
                    TypeRef::nil()
                } else {
                    TypeRef::Never
                };

                self.push_from(n.location, "Loop", Some(typ), Origin::Derived);
                self.body(&n.body);
            }
            E::Match(n) => {
                self.push(n.location, "Match", Some(n.resolved_type));
                self.expression(&n.expression);

                for case in &n.cases {
                    if let Some(g) = &case.guard {
                        self.expression(g);
                    }

                    self.body(&case.body);
                }
            }
            E::Mut(n) => {
                self.push(n.location, "Mut", Some(n.resolved_type));

                match &n.value {
                    // `mut fn_name` takes a pointer to a C function: the
                    // name is a method, not a value, so it has no type of its
                    // own. The method is in `pointer_to_method`.
                    hir::Expression::IdentifierRef(v) if n.pointer_to_method.is_some() => {
                        self.push(v.location, "PointerToMethod", None)
                    }
                    v => self.expression(v),
                }
            }
            E::Ref(n) => {
                self.push(n.location, "Ref", Some(n.resolved_type));
                self.expression(&n.value);
            }
            E::Recover(n) => {
                self.push(n.location, "Recover", Some(n.resolved_type));
                self.body(&n.body);
            }
            E::Return(n) => {
                self.push(n.location, "Return", Some(n.resolved_type));

                if let Some(v) = &n.value {
                    self.expression(v);
                }
            }
            E::Scope(n) => {
                self.push(n.location, "Scope", Some(n.resolved_type));
                self.body(&n.body);
            }
            E::SelfObject(n) => self.push(n.location, "SelfObject", Some(n.resolved_type)),
            E::Throw(n) => {
                self.push(n.location, "Throw", Some(n.resolved_type));
                self.expression(&n.value);
            }
            E::Tuple(n) => {
                self.push(n.location, "Tuple", Some(n.resolved_type));
                self.body(&n.values);
            }
            E::TypeCast(n) => {
                self.push(n.location, "TypeCast", Some(n.resolved_type));
                self.expression(&n.value);
            }
            E::Try(n) => {
                // Not stored: `try` evaluates to the Option's or Result's
                // inner value, which `kind` records.
                let typ = match n.kind {
                    ThrowKind::Option(t) | ThrowKind::Result(t, _) => Some(t),
                    ThrowKind::Unknown | ThrowKind::Infer(_) => None,
                };

                self.push_from(n.location, "Try", typ, Origin::Derived);
                self.expression(&n.expression);
            }
            E::SizeOf(n) => self.push(n.location, "SizeOf", Some(n.resolved_type)),
        }
    }
}

fn symbol(db: &Database, module: ModuleId, name: &str) -> Option<Symbol> {
    module
        .symbols(db)
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, s)| s)
}

/// Whether a `break` in this loop body targets the loop itself.
fn breaks(body: &[hir::Expression]) -> bool {
    body.iter().any(breaks_in)
}

fn breaks_in(node: &hir::Expression) -> bool {
    use hir::Expression as E;

    match node {
        E::Break(_) => true,
        // A break in a nested loop targets that loop, and closures can't
        // break out of the loop they're defined in.
        E::Loop(_) | E::Closure(_) => false,
        E::And(n) => breaks_in(&n.left) || breaks_in(&n.right),
        E::Or(n) => breaks_in(&n.left) || breaks_in(&n.right),
        E::AssignField(n) => breaks_in(&n.value),
        E::ReplaceField(n) => breaks_in(&n.value),
        E::AssignSetter(n) => breaks_in(&n.receiver) || breaks_in(&n.value),
        E::ReplaceSetter(n) => breaks_in(&n.receiver) || breaks_in(&n.value),
        E::AssignVariable(n) => breaks_in(&n.value),
        E::ReplaceVariable(n) => breaks_in(&n.value),
        E::BuiltinCall(n) => breaks(&n.arguments),
        E::Call(n) => {
            n.receiver.as_ref().is_some_and(breaks_in)
                || n.arguments.iter().any(|a| match a {
                    hir::Argument::Positional(a) => breaks_in(&a.value),
                    hir::Argument::Named(a) => breaks_in(&a.value),
                })
        }
        E::DefineVariable(n) => breaks_in(&n.value) || n.else_body.as_ref().is_some_and(breaks_in),
        E::Match(n) => {
            breaks_in(&n.expression)
                || n.cases
                    .iter()
                    .any(|c| c.guard.as_ref().is_some_and(breaks_in) || breaks(&c.body))
        }
        E::Mut(n) => breaks_in(&n.value),
        E::Ref(n) => breaks_in(&n.value),
        E::Recover(n) => breaks(&n.body),
        E::Return(n) => n.value.as_ref().is_some_and(breaks_in),
        E::Scope(n) => breaks(&n.body),
        E::Throw(n) => breaks_in(&n.value),
        E::Tuple(n) => breaks(&n.values),
        E::TypeCast(n) => breaks_in(&n.value),
        E::Try(n) => breaks_in(&n.expression),
        E::ConstantRef(_)
        | E::False(_)
        | E::True(_)
        | E::Nil(_)
        | E::FieldRef(_)
        | E::Float(_)
        | E::Int(_)
        | E::String(_)
        | E::IdentifierRef(_)
        | E::Next(_)
        | E::SelfObject(_)
        | E::SizeOf(_) => false,
    }
}

/// Formats an expression as `file:line:col-line:col<TAB>kind<TAB>type`, with
/// `file` relative to `root`, and a fourth column for a type that isn't
/// stored in the HIR.
pub fn format_expr(expr: &Expr, root: &Path) -> String {
    let file = expr.file.strip_prefix(root).unwrap_or(&expr.file);
    let l = expr.location;

    let origin = match expr.origin {
        Origin::Stored => "",
        Origin::Derived => "\tderived",
        Origin::Symbol => "\tsymbol",
    };

    format!(
        "{}:{}:{}-{}:{}\t{}\t{}{}",
        file.display(),
        l.line_start,
        l.column_start,
        l.line_end,
        l.column_end,
        expr.kind,
        expr.typ.as_deref().unwrap_or("<missing>"),
        origin
    )
}

/// The standard library of the pinned upstream release.
pub fn upstream_std() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(".upstream/inko/std/src")
}
