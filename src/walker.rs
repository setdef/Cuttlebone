//! Walks a module's typed HIR and records each expression with its type.
//!
//! Most types are stored in the HIR by the type checker. The rest (see
//! `Origin`) are reconstructed here by reapplying the type checker's rules.
//! Each such rule cites the upstream lines it mirrors (paths relative to
//! `.upstream/inko/`; tests/citations.rs fails if they move), so a rebase can
//! diff them. Where the compiler stores something to compare with, the walker
//! records a `CrossCheck` that tests/crosscheck.rs asserts on.
use compiler::hir;
use location::Location;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use types::format::format_type;
use types::{
    CallKind, ConstantId, ConstantKind, ConstantPatternKind, ConstructorId, Database, FieldId,
    IdentifierKind, MethodId, MethodKind, MethodSource, ModuleId, Receiver, Symbol, ThrowKind,
    TraitId, TypeArguments, TypeEnum, TypeId, TypeParameterId, TypeRef, VariableId,
};
use unicode_segmentation::UnicodeSegmentation;

/// One expression (or variable binding) in the typed HIR.
#[derive(Clone, Debug)]
pub struct Expr {
    pub file: PathBuf,
    pub location: Location,
    /// The HIR node's variant name, e.g. `Call`, or `Binding` for a variable
    /// a pattern or closure argument introduces.
    pub kind: &'static str,
    /// The formatted type, or `None` if the HIR doesn't record one.
    pub typ: Option<String>,
    pub origin: Origin,
    /// Whether desugaring introduced this node, rather than it standing for
    /// source code at its location (see docs/lsp.md, "Which expression
    /// a position means").
    pub synthetic: bool,
    /// The unformatted type. It refers into the type database, so it's only
    /// meaningful while that database exists.
    pub raw: Option<TypeRef>,
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

/// A call with an explicit receiver, for completing members after a `.`.
#[derive(Clone, Debug)]
pub struct CallSite {
    /// Where the method name is.
    pub name: Location,
    /// The receiver's index in `Walker::out`.
    pub receiver: usize,
    /// Whether the receiver is `self`, which may call type-private methods
    /// (compiler/src/type_check/expressions.rs:4499).
    pub receiver_is_self: bool,
}

/// A call written with parentheses, for signature help.
#[derive(Clone, Debug)]
pub struct ArgumentList {
    /// The whole call, up to and including its `)`.
    pub location: Location,
    /// Where the method name is.
    pub name: Location,
    /// Where the last type argument is, if there are any (`foo[A, B](...)`).
    pub types: Option<Location>,
    /// The called method and the call's type arguments, if it resolved.
    pub method: Option<(MethodId, TypeArguments)>,
    pub arguments: Vec<ListArgument>,
}

/// An argument in an `ArgumentList`.
#[derive(Clone, Debug)]
pub struct ListArgument {
    pub location: Location,
    /// The name of a named argument (`times` in `times: 2`).
    pub name: Option<String>,
    /// The name of the variable or field (`@x`) that a positional argument
    /// is, if it's only that.
    pub variable: Option<String>,
}

/// A variable whose type isn't written in the source, for inlay hints: one
/// a `let` binds to a name without a type annotation, or a closure's
/// argument without one.
#[derive(Clone, Debug)]
pub struct Untyped {
    /// The variable's name.
    pub name: Location,
    /// The variable's type, if the compiler inferred it.
    pub typ: Option<TypeRef>,
    /// Whether the code says the type anyway: a `let` whose value is a
    /// literal, a cast, a type's constructor (`Point(x: 1)`), or a static
    /// method of a type that returns that type (`Counter.new`,
    /// `Option.Some(1)`).
    pub obvious: bool,
}

/// What a name refers to, as one of the compiler's ids. Two names refer to
/// the same symbol only if their ids are equal, so same-named symbols in
/// different modules stay apart.
#[derive(Clone, Copy, Debug)]
pub enum SymbolId {
    Module(ModuleId),
    Type(TypeId),
    Trait(TraitId),
    TypeParameter(TypeParameterId),
    Constant(ConstantId),
    Method(MethodId),
    Field(FieldId),
    Constructor(ConstructorId),
    Variable(VariableId),
}

impl SymbolId {
    // `ConstructorId` isn't `Hash`, so symbols compare by kind and number.
    fn key(self) -> (u8, usize) {
        match self {
            SymbolId::Module(id) => (0, id.0 as usize),
            SymbolId::Type(id) => (1, id.0 as usize),
            SymbolId::Trait(id) => (2, id.0 as usize),
            SymbolId::TypeParameter(id) => (3, id.0),
            SymbolId::Constant(id) => (4, id.0),
            SymbolId::Method(id) => (5, id.0 as usize),
            SymbolId::Field(id) => (6, id.0),
            SymbolId::Constructor(id) => (7, id.0),
            SymbolId::Variable(id) => (8, id.0),
        }
    }

    /// The symbol a name means when several ids stand for one declaration:
    ///
    /// - An enum constructor's static method (`Option.Some(42)`) is the
    ///   constructor (`define_constructor_method`,
    ///   compiler/src/type_check/methods.rs:1030-1094), which patterns refer
    ///   to directly.
    /// - A default method a type inherits from a trait without redefining
    ///   it is a copy of the trait's method (compiler/src/type_check/methods.rs:1283-1291).
    /// - A type parameter with an `impl ... if T: Trait` bound is a copy of
    ///   the original (`TypeParameterId::original`).
    pub fn canonical(self, db: &Database) -> SymbolId {
        match self {
            SymbolId::Method(id) => {
                if id.kind(db) == MethodKind::Constructor
                    && let TypeEnum::Type(typ) = id.receiver_id(db)
                    && let Some(c) = typ.constructor(db, id.name(db))
                {
                    return SymbolId::Constructor(c);
                }

                match id.source(db) {
                    MethodSource::Inherited(_, original) => {
                        SymbolId::Method(original).canonical(db)
                    }
                    _ => self,
                }
            }
            SymbolId::TypeParameter(id) => match id.original(db) {
                Some(original) if original != id => SymbolId::TypeParameter(original).canonical(db),
                _ => self,
            },
            _ => self,
        }
    }

    /// The symbol's name as declared.
    pub fn name(self, db: &Database) -> String {
        match self {
            SymbolId::Module(id) => id.name(db).to_string(),
            SymbolId::Type(id) => id.name(db).clone(),
            SymbolId::Trait(id) => id.name(db).clone(),
            SymbolId::TypeParameter(id) => id.name(db).clone(),
            SymbolId::Constant(id) => id.name(db).clone(),
            SymbolId::Method(id) => id.name(db).clone(),
            SymbolId::Field(id) => id.name(db).clone(),
            SymbolId::Constructor(id) => id.name(db).clone(),
            SymbolId::Variable(id) => id.name(db).clone(),
        }
    }

    /// The kind's name, e.g. `method`. For messages and tests.
    pub fn kind(self) -> &'static str {
        match self {
            SymbolId::Module(_) => "module",
            SymbolId::Type(_) => "type",
            SymbolId::Trait(_) => "trait",
            SymbolId::TypeParameter(_) => "type parameter",
            SymbolId::Constant(_) => "constant",
            SymbolId::Method(_) => "method",
            SymbolId::Field(_) => "field",
            SymbolId::Constructor(_) => "constructor",
            SymbolId::Variable(_) => "variable",
        }
    }
}

impl PartialEq for SymbolId {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}

impl Eq for SymbolId {}

impl Hash for SymbolId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key().hash(state);
    }
}

/// A name in the source, and the symbol it refers to or declares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reference {
    /// Where the name is.
    pub location: Location,
    /// Already canonical (`SymbolId::canonical`).
    pub symbol: SymbolId,
}

/// The type, trait or type parameter a type is declared by, without its
/// ownership or type arguments: `ref Option[String]` is `Option`. A module
/// used as a value is the module. Closures, C types, pointers and `Never`
/// have no declaration: `Pointer[T]` is built into the compiler, and isn't
/// `T`.
pub fn type_symbol(db: &Database, typ: TypeRef) -> Option<SymbolId> {
    if let TypeRef::Pointer(_) = typ {
        return None;
    }

    let symbol = match typ.as_type_enum(db).ok()? {
        TypeEnum::Type(id) => SymbolId::Type(id),
        TypeEnum::TypeInstance(ins) => SymbolId::Type(ins.instance_of()),
        TypeEnum::Trait(id) => SymbolId::Trait(id),
        TypeEnum::TraitInstance(ins) => SymbolId::Trait(ins.instance_of()),
        TypeEnum::TypeParameter(id)
        | TypeEnum::RigidTypeParameter(id)
        | TypeEnum::AtomicTypeParameter(id) => SymbolId::TypeParameter(id),
        TypeEnum::Module(id) => SymbolId::Module(id),
        TypeEnum::Closure(_) | TypeEnum::Foreign(_) => return None,
    };

    Some(symbol.canonical(db))
}

/// The symbols a module declares, at their names: types, traits, their
/// type parameters, constructors and fields, constants, methods and their
/// arguments. Method bodies aren't visited, so this is cheap enough to run
/// on every module of a check.
pub fn declarations(db: &Database, module: &hir::Module) -> Vec<Reference> {
    let mut out = Vec::new();
    let mut add = |location: Location, symbol: Option<SymbolId>| {
        if let Some(symbol) = symbol {
            out.push(Reference {
                location,
                symbol: symbol.canonical(db),
            });
        }
    };

    for expr in &module.expressions {
        match expr {
            hir::TopLevelExpression::Type(n) => {
                add(n.name.location, n.type_id.map(SymbolId::Type));
                type_parameters(&n.type_parameters, &mut add);

                for e in &n.body {
                    match e {
                        hir::TypeExpression::InstanceMethod(m) => method(
                            db,
                            m.method_id,
                            &m.name,
                            &m.type_parameters,
                            &m.arguments,
                            &mut add,
                        ),
                        hir::TypeExpression::StaticMethod(m) => method(
                            db,
                            m.method_id,
                            &m.name,
                            &m.type_parameters,
                            &m.arguments,
                            &mut add,
                        ),
                        hir::TypeExpression::AsyncMethod(m) => method(
                            db,
                            m.method_id,
                            &m.name,
                            &m.type_parameters,
                            &m.arguments,
                            &mut add,
                        ),
                        hir::TypeExpression::Field(f) => {
                            add(f.name.location, f.field_id.map(SymbolId::Field))
                        }
                        hir::TypeExpression::Constructor(c) => {
                            add(c.name.location, c.constructor_id.map(SymbolId::Constructor))
                        }
                    }
                }
            }
            hir::TopLevelExpression::ExternType(n) => {
                add(n.name.location, n.type_id.map(SymbolId::Type));

                for f in &n.fields {
                    add(f.name.location, f.field_id.map(SymbolId::Field));
                }
            }
            hir::TopLevelExpression::Constant(n) => {
                add(n.name.location, n.constant_id.map(SymbolId::Constant))
            }
            hir::TopLevelExpression::ModuleMethod(m) => method(
                db,
                m.method_id,
                &m.name,
                &m.type_parameters,
                &m.arguments,
                &mut add,
            ),
            hir::TopLevelExpression::ExternFunction(m) => {
                method(db, m.method_id, &m.name, &[], &m.arguments, &mut add)
            }
            hir::TopLevelExpression::Trait(n) => {
                add(n.name.location, n.trait_id.map(SymbolId::Trait));
                type_parameters(&n.type_parameters, &mut add);

                for e in &n.body {
                    match e {
                        hir::TraitExpression::InstanceMethod(m) => method(
                            db,
                            m.method_id,
                            &m.name,
                            &m.type_parameters,
                            &m.arguments,
                            &mut add,
                        ),
                        hir::TraitExpression::RequiredMethod(m) => method(
                            db,
                            m.method_id,
                            &m.name,
                            &m.type_parameters,
                            &m.arguments,
                            &mut add,
                        ),
                    }
                }
            }
            hir::TopLevelExpression::Implement(n) => {
                for m in &n.body {
                    method(
                        db,
                        m.method_id,
                        &m.name,
                        &m.type_parameters,
                        &m.arguments,
                        &mut add,
                    );
                }
            }
            hir::TopLevelExpression::Reopen(n) => {
                for e in &n.body {
                    match e {
                        hir::ReopenTypeExpression::InstanceMethod(m) => method(
                            db,
                            m.method_id,
                            &m.name,
                            &m.type_parameters,
                            &m.arguments,
                            &mut add,
                        ),
                        hir::ReopenTypeExpression::StaticMethod(m) => method(
                            db,
                            m.method_id,
                            &m.name,
                            &m.type_parameters,
                            &m.arguments,
                            &mut add,
                        ),
                        hir::ReopenTypeExpression::AsyncMethod(m) => method(
                            db,
                            m.method_id,
                            &m.name,
                            &m.type_parameters,
                            &m.arguments,
                            &mut add,
                        ),
                    }
                }
            }
            hir::TopLevelExpression::Import(_) | hir::TopLevelExpression::ExternImport(_) => {}
        }
    }

    out
}

fn type_parameters(
    params: &[hir::TypeParameter],
    add: &mut impl FnMut(Location, Option<SymbolId>),
) {
    for p in params {
        add(
            p.name.location,
            p.type_parameter_id.map(SymbolId::TypeParameter),
        );
    }
}

/// A method, its type parameters, and its arguments, which the type checker
/// defines as variables in the same order as the HIR lists them
/// (compiler/src/type_check/methods.rs:198-204, `Arguments::new_argument`).
fn method(
    db: &Database,
    id: Option<MethodId>,
    name: &hir::Identifier,
    params: &[hir::TypeParameter],
    arguments: &[hir::MethodArgument],
    add: &mut impl FnMut(Location, Option<SymbolId>),
) {
    add(name.location, id.map(SymbolId::Method));
    type_parameters(params, add);

    let Some(id) = id else {
        return;
    };
    let variables = id.arguments(db);

    for (arg, var) in arguments.iter().zip(&variables) {
        if arg.name.name == var.name {
            add(arg.name.location, Some(SymbolId::Variable(var.variable)));
        }
    }
}

/// The part of a one-line location that's the trailing `name`, e.g. `Foo`
/// in `mod.Foo`.
fn tail(location: Location, name: &str) -> Location {
    let width = name.graphemes(true).count() as u32;

    if location.line_start != location.line_end || width == 0 || width > location.column_end {
        return location;
    }

    Location::new(
        &(location.line_end..=location.line_end),
        &((location.column_end + 1 - width).max(location.column_start)..=location.column_end),
    )
}

/// A reconstructed type compared with something the compiler did store.
#[derive(Clone, Debug)]
pub struct CrossCheck {
    pub location: Location,
    /// The reconstructed expression's kind, e.g. `Loop`.
    pub kind: &'static str,
    /// What the compiler stored that's compared: `receiver` (a call's
    /// method or module), `let` (a variable's type) or `scope` (a scope's
    /// type, which is its last expression's).
    pub rule: &'static str,
    /// The type Cuttlebone reconstructed.
    pub ours: String,
    /// The type the compiler's stored data implies.
    pub compiler: String,
}

pub struct Walker<'a> {
    pub db: &'a Database,
    pub file: PathBuf,
    /// The module's symbols, for static receivers.
    symbols: HashMap<String, Symbol>,
    pub out: Vec<Expr>,
    pub calls: Vec<CallSite>,
    /// Every call written with parentheses, in the order walked.
    pub argument_lists: Vec<ArgumentList>,
    /// The variables whose types aren't written, in the order walked.
    pub untyped: Vec<Untyped>,
    pub checks: Vec<CrossCheck>,
    /// Every name that refers to a symbol, in the order walked. Names that
    /// desugaring introduced aren't included.
    pub references: Vec<Reference>,
    /// The module's `declarations`, then the variables its patterns and
    /// closures define.
    pub declarations: Vec<Reference>,
    /// Where a reference assigns a new value: `x = 1`, `x := 1`, `x += 1`,
    /// `@x = 1`, `a.x = 1` and so on.
    pub writes: Vec<Location>,
}

impl<'a> Walker<'a> {
    pub fn new(db: &'a Database, module: ModuleId) -> Self {
        Walker {
            db,
            file: module.file(db),
            symbols: module.symbols(db).into_iter().collect(),
            out: Vec::new(),
            calls: Vec::new(),
            argument_lists: Vec::new(),
            untyped: Vec::new(),
            checks: Vec::new(),
            references: Vec::new(),
            declarations: Vec::new(),
            writes: Vec::new(),
        }
    }

    pub fn module(&mut self, module: &hir::Module) {
        self.declarations = declarations(self.db, module);

        for expr in &module.expressions {
            self.top_level(expr);
        }
    }

    fn refer(&mut self, location: Location, symbol: Option<SymbolId>) {
        if let Some(symbol) = symbol {
            self.references.push(Reference {
                location,
                symbol: symbol.canonical(self.db),
            });
        }
    }

    fn declare(&mut self, location: Location, symbol: SymbolId) {
        self.declarations.push(Reference {
            location,
            symbol: symbol.canonical(self.db),
        });
    }

    /// The symbol a type name refers to, and the module it's qualified
    /// with, if any.
    fn type_name(&mut self, node: &hir::TypeName) {
        if let Some(src) = &node.source {
            let module = self.module_named(&src.name);

            self.refer(src.location, module);
        }

        let symbol = type_symbol(self.db, node.resolved_type);

        self.refer(node.name.location, symbol);

        for t in &node.arguments {
            self.type_node(t);
        }
    }

    fn type_node(&mut self, node: &hir::Type) {
        match node {
            hir::Type::Named(n) => self.type_name(n),
            hir::Type::Ref(n) | hir::Type::Mut(n) | hir::Type::Uni(n) | hir::Type::Owned(n) => {
                match &n.type_reference {
                    hir::ReferrableType::Named(t) => self.type_name(t),
                    hir::ReferrableType::Closure(t) => self.closure_type(t),
                    hir::ReferrableType::Tuple(t) => {
                        t.values.iter().for_each(|v| self.type_node(v))
                    }
                }
            }
            hir::Type::Closure(n) => self.closure_type(n),
            hir::Type::Tuple(n) => n.values.iter().for_each(|v| self.type_node(v)),
        }
    }

    fn closure_type(&mut self, node: &hir::ClosureType) {
        node.arguments.iter().for_each(|t| self.type_node(t));

        if let Some(t) = &node.return_type {
            self.type_node(t);
        }
    }

    /// What `name:` in a call refers to: the field it sets when creating an
    /// instance, or the method's argument.
    fn named_argument(&self, kind: &CallKind, name: &str) -> Option<SymbolId> {
        match kind {
            CallKind::TypeInstance(info) => info
                .fields
                .iter()
                .find(|(id, _)| id.name(self.db) == name)
                .map(|(id, _)| SymbolId::Field(*id)),
            CallKind::Call(info) => info
                .id
                .arguments(self.db)
                .into_iter()
                .find(|a| a.name == name)
                .map(|a| SymbolId::Variable(a.variable)),
            _ => None,
        }
    }

    fn module_named(&self, name: &str) -> Option<SymbolId> {
        match self.symbols.get(name) {
            Some(Symbol::Module(id)) => Some(SymbolId::Module(*id)),
            _ => None,
        }
    }

    /// The type names in a method's signature.
    fn signature(
        &mut self,
        params: &[hir::TypeParameter],
        arguments: &[hir::MethodArgument],
        returns: &Option<hir::Type>,
    ) {
        self.type_parameter_requirements(params);

        for arg in arguments {
            self.type_node(&arg.value_type);
        }

        if let Some(t) = returns {
            self.type_node(t);
        }
    }

    fn type_parameter_requirements(&mut self, params: &[hir::TypeParameter]) {
        for p in params {
            p.requirements.iter().for_each(|r| self.type_name(r));
        }
    }

    /// `if T: Trait` bounds on an `impl` or reopened type: `T` is one of the
    /// type's parameters.
    fn bounds(&mut self, typ: Option<TypeId>, bounds: &[hir::TypeBound]) {
        for b in bounds {
            let param = typ.and_then(|t| {
                t.type_parameters(self.db)
                    .into_iter()
                    .find(|p| p.name(self.db) == &b.name.name)
            });

            self.refer(b.name.location, param.map(SymbolId::TypeParameter));
            b.requirements.iter().for_each(|r| self.type_name(r));
        }
    }

    /// Imports: each segment of the module path that names a module, and
    /// each imported symbol (`self` is the module).
    fn import(&mut self, node: &hir::Import) {
        let mut path = Vec::new();
        let mut module = None;

        for segment in &node.source {
            path.push(segment.name.as_str());
            module = self.db.optional_module(&path.join("."));
            self.refer(segment.location, module.map(SymbolId::Module));
        }

        let Some(module) = module else {
            return;
        };

        for symbol in &node.symbols {
            let target = if symbol.name.name == "self" {
                Some(SymbolId::Module(module))
            } else {
                module
                    .symbols(self.db)
                    .into_iter()
                    .find(|(n, _)| n == &symbol.name.name)
                    .and_then(|(_, s)| symbol_id(s))
            };

            self.refer(symbol.name.location, target);

            if symbol.import_as.location != symbol.name.location {
                self.refer(symbol.import_as.location, target);
            }
        }
    }

    fn top_level(&mut self, node: &hir::TopLevelExpression) {
        match node {
            hir::TopLevelExpression::Type(n) => {
                self.type_parameter_requirements(&n.type_parameters);

                for expr in &n.body {
                    match expr {
                        hir::TypeExpression::InstanceMethod(m) => {
                            self.signature(&m.type_parameters, &m.arguments, &m.return_type);
                            self.body(&m.body);
                        }
                        hir::TypeExpression::StaticMethod(m) => {
                            self.signature(&m.type_parameters, &m.arguments, &m.return_type);
                            self.body(&m.body);
                        }
                        hir::TypeExpression::AsyncMethod(m) => {
                            self.signature(&m.type_parameters, &m.arguments, &m.return_type);
                            self.body(&m.body);
                        }
                        hir::TypeExpression::Field(f) => self.type_node(&f.value_type),
                        hir::TypeExpression::Constructor(c) => {
                            c.members.iter().for_each(|t| self.type_node(t))
                        }
                    }
                }
            }
            hir::TopLevelExpression::Constant(n) => self.constant(&n.value),
            hir::TopLevelExpression::ModuleMethod(n) => {
                self.signature(&n.type_parameters, &n.arguments, &n.return_type);
                self.body(&n.body);
            }
            hir::TopLevelExpression::Trait(n) => {
                self.type_parameter_requirements(&n.type_parameters);
                n.requirements.iter().for_each(|r| self.type_name(r));

                for expr in &n.body {
                    match expr {
                        hir::TraitExpression::InstanceMethod(m) => {
                            self.signature(&m.type_parameters, &m.arguments, &m.return_type);
                            self.body(&m.body);
                        }
                        hir::TraitExpression::RequiredMethod(m) => {
                            self.signature(&m.type_parameters, &m.arguments, &m.return_type);
                        }
                    }
                }
            }
            hir::TopLevelExpression::Implement(n) => {
                let typ = n.type_instance.map(|i| i.instance_of());

                self.type_name(&n.trait_name);
                self.refer(n.type_name.location, typ.map(SymbolId::Type));
                self.bounds(typ, &n.bounds);

                for m in &n.body {
                    self.signature(&m.type_parameters, &m.arguments, &m.return_type);
                    self.body(&m.body);
                }
            }
            hir::TopLevelExpression::Reopen(n) => {
                self.refer(n.type_name.location, n.type_id.map(SymbolId::Type));
                self.bounds(n.type_id, &n.bounds);

                for expr in &n.body {
                    match expr {
                        hir::ReopenTypeExpression::InstanceMethod(m) => {
                            self.signature(&m.type_parameters, &m.arguments, &m.return_type);
                            self.body(&m.body);
                        }
                        hir::ReopenTypeExpression::StaticMethod(m) => {
                            self.signature(&m.type_parameters, &m.arguments, &m.return_type);
                            self.body(&m.body);
                        }
                        hir::ReopenTypeExpression::AsyncMethod(m) => {
                            self.signature(&m.type_parameters, &m.arguments, &m.return_type);
                            self.body(&m.body);
                        }
                    }
                }
            }
            hir::TopLevelExpression::ExternType(n) => {
                n.fields.iter().for_each(|f| self.type_node(&f.value_type))
            }
            hir::TopLevelExpression::ExternFunction(n) => {
                self.signature(&[], &n.arguments, &n.return_type)
            }
            hir::TopLevelExpression::Import(n) => self.import(n),
            hir::TopLevelExpression::ExternImport(_) => {}
        }
    }

    fn push(
        &mut self,
        location: Location,
        kind: &'static str,
        typ: Option<TypeRef>,
        origin: Origin,
        synthetic: bool,
    ) -> usize {
        let raw = typ.and_then(|t| self.resolve(t));
        let typ = raw.map(|t| format_type(self.db, t));

        self.out.push(Expr {
            file: self.file.clone(),
            location,
            kind,
            typ,
            origin,
            synthetic,
            raw,
        });
        self.out.len() - 1
    }

    fn stored(
        &mut self,
        location: Location,
        kind: &'static str,
        typ: Option<TypeRef>,
        synthetic: bool,
    ) -> usize {
        self.push(location, kind, typ, Origin::Stored, synthetic)
    }

    fn check(&mut self, index: usize, rule: &'static str, ours: TypeRef, compiler: TypeRef) {
        self.checks.push(CrossCheck {
            location: self.out[index].location,
            kind: self.out[index].kind,
            rule,
            ours: format_type(self.db, ours),
            compiler: format_type(self.db, compiler),
        });
    }

    /// Visits a call's receiver, which may be a type or module rather than a
    /// value. Returns the receiver's index in `out`.
    fn receiver(&mut self, node: &hir::Expression, synthetic: bool) -> usize {
        let index = self.out.len();

        self.expr(node, synthetic);

        if self.out[index].typ.is_none()
            && let Some(typ) = self.static_receiver(node)
        {
            self.out[index].typ = Some(format_type(self.db, typ));
            self.out[index].raw = Some(typ);
            self.out[index].origin = Origin::Symbol;

            // In `module.Type.method`, `module` is the receiver of
            // `module.Type`, and must be the module defining the type.
            if let (hir::Expression::Call(_), TypeRef::Owned(TypeEnum::Type(id))) = (node, typ)
                && self
                    .out
                    .get(index + 1)
                    .is_some_and(|e| e.origin == Origin::Symbol)
            {
                let module = self.out[index + 1].raw.unwrap();

                self.check(
                    index + 1,
                    "receiver",
                    module,
                    TypeRef::module(id.module(self.db)),
                );
            }
        }

        index
    }

    /// The type the type checker gives a type or module used as a receiver.
    ///
    /// Mirrors three places in the type checker, none of which store the
    /// type in the HIR:
    ///
    /// - `constant` with `receiver` set returns `Owned(Type(id))` for a type
    ///   (compiler/src/type_check/expressions.rs:2579).
    /// - `identifier` returns `TypeRef::module(id)` for a module
    ///   (compiler/src/type_check/expressions.rs:2714).
    /// - `call_with_receiver` returns `Owned(Type(id))` for `module.Type`
    ///   used as a receiver (compiler/src/type_check/expressions.rs:3826).
    fn static_receiver(&self, node: &hir::Expression) -> Option<TypeRef> {
        match self.static_lookup(node)? {
            Symbol::Type(id) => Some(TypeRef::Owned(TypeEnum::Type(id))),
            Symbol::Module(id) => Some(TypeRef::module(id)),
            _ => None,
        }
    }

    /// The type or module a name with no stored kind refers to, as
    /// `static_receiver` finds it.
    fn static_symbol(&self, node: &hir::Expression) -> Option<SymbolId> {
        match self.static_lookup(node)? {
            Symbol::Type(id) => Some(SymbolId::Type(id)),
            Symbol::Module(id) => Some(SymbolId::Module(id)),
            _ => None,
        }
    }

    fn static_lookup(&self, node: &hir::Expression) -> Option<Symbol> {
        match node {
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
        }
    }

    /// Looks up `name` in the module imported as `module`.
    fn module_symbol(&self, module: &str, name: &str) -> Option<Symbol> {
        let Some(Symbol::Module(id)) = self.symbols.get(module) else {
            return None;
        };

        id.symbols(self.db)
            .into_iter()
            .find(|(n, _)| n == name)
            .map(|(_, s)| s)
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
            C::Int(n) => _ = self.stored(n.location, "ConstInt", Some(n.resolved_type), false),
            C::String(n) => {
                _ = self.stored(n.location, "ConstString", Some(n.resolved_type), false)
            }
            C::Float(n) => _ = self.stored(n.location, "ConstFloat", Some(n.resolved_type), false),
            C::Binary(n) => {
                self.stored(n.location, "ConstBinary", Some(n.resolved_type), false);
                self.constant(&n.left);
                self.constant(&n.right);
            }
            C::ConstantRef(n) => {
                _ = self.stored(n.location, "ConstConstantRef", Some(n.resolved_type), false)
            }
            C::Array(n) => {
                self.stored(n.location, "ConstArray", Some(n.resolved_type), false);

                for v in &n.values {
                    self.constant(v);
                }
            }
            C::True(n) => _ = self.stored(n.location, "ConstTrue", Some(n.resolved_type), false),
            C::False(n) => _ = self.stored(n.location, "ConstFalse", Some(n.resolved_type), false),
        }
    }

    fn body(&mut self, nodes: &[hir::Expression]) {
        for n in nodes {
            self.expression(n);
        }
    }

    /// Records the variables a pattern introduces, and what it matches
    /// against. In a `synthetic` pattern (a `for` loop's `Some(value)`),
    /// only the variables are written in the source.
    fn pattern(&mut self, node: &hir::Pattern, synthetic: bool) {
        use hir::Pattern as P;

        match node {
            P::Identifier(n) => {
                let typ = n.variable_id.map(|id| id.value_type(self.db));
                let synthetic = generated(&n.name.name);

                self.stored(n.name.location, "Binding", typ, synthetic);

                if let Some(id) = n.variable_id
                    && !synthetic
                {
                    self.declare(n.name.location, SymbolId::Variable(id));
                }

                if let Some(t) = &n.value_type {
                    self.type_node(t);
                }
            }
            P::Tuple(n) => n.values.iter().for_each(|p| self.pattern(p, synthetic)),
            P::Constructor(n) => {
                if !synthetic {
                    self.refer(n.name.location, n.constructor_id.map(SymbolId::Constructor));
                }

                n.values.iter().for_each(|p| self.pattern(p, synthetic))
            }
            P::Type(n) => n.values.iter().for_each(|f| {
                if !synthetic {
                    self.refer(f.field.location, f.field_id.map(SymbolId::Field));
                }

                self.pattern(&f.pattern, synthetic)
            }),
            P::Or(n) => n.patterns.iter().for_each(|p| self.pattern(p, synthetic)),
            P::Array(n) => n.values.iter().for_each(|p| self.pattern(p, synthetic)),
            P::Constant(_) if synthetic => {}
            P::Constant(n) => {
                let symbol = match n.kind {
                    ConstantPatternKind::Constructor(id) => Some(SymbolId::Constructor(id)),
                    ConstantPatternKind::String(id) | ConstantPatternKind::Int(id) => {
                        Some(SymbolId::Constant(id))
                    }
                    ConstantPatternKind::Unknown => None,
                };

                if let Some(src) = &n.source {
                    let module = self.module_named(&src.name);

                    self.refer(src.location, module);
                }

                self.refer(tail(n.location, &n.name), symbol);
            }
            P::Int(_) | P::String(_) | P::Wildcard(_) | P::True(_) | P::False(_) => {}
        }
    }

    /// Whether an expression says its own type: a literal, a cast, a type's
    /// constructor, or a static method of a type that returns that type.
    fn obvious(&self, node: &hir::Expression) -> bool {
        use hir::Expression as E;

        match node {
            E::Int(_)
            | E::Float(_)
            | E::String(_)
            | E::True(_)
            | E::False(_)
            | E::Nil(_)
            | E::TypeCast(_) => true,
            E::Call(n) => match &n.kind {
                CallKind::TypeInstance(_) => true,
                CallKind::Call(info) => n.receiver.as_ref().is_some_and(|r| {
                    let receiver = self.static_symbol(r);

                    receiver.is_some() && receiver == type_symbol(self.db, info.returns)
                }),
                _ => false,
            },
            _ => false,
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

    /// Compares a receiver whose type was looked up again (`Origin::Symbol`)
    /// with what the call it's the receiver of recorded.
    fn check_receiver(&mut self, receiver: usize, kind: &CallKind) {
        let expr = &self.out[receiver];

        if expr.origin != Origin::Symbol {
            return;
        }

        let ours = expr.raw.unwrap();

        let compiler = match kind {
            // A static method (or constructor) records `Receiver::Type`, and
            // its receiver is the type itself; a module method's receiver is
            // the module (types/src/lib.rs, `Receiver::with_module`).
            CallKind::Call(info) => match info.receiver {
                Receiver::Type | Receiver::Explicit | Receiver::Implicit => match ours {
                    TypeRef::Owned(TypeEnum::Module(_)) => TypeRef::module(info.id.module(self.db)),
                    _ => info.id.receiver(self.db),
                },
                Receiver::Extern => TypeRef::module(info.id.module(self.db)),
            },
            // `module.CONSTANT`
            CallKind::GetConstant(id) => TypeRef::module(id.module(self.db)),
            // `module.Type(field: value)`
            CallKind::TypeInstance(info) => TypeRef::module(info.type_id.module(self.db)),
            _ => return,
        };

        self.check(receiver, "receiver", ours, compiler);
    }

    /// Compares a derived type with the type of a variable or scope that the
    /// compiler stored for the same value.
    fn check_value(&mut self, index: usize, rule: &'static str, compiler: TypeRef) {
        let expr = &self.out[index];

        if expr.origin != Origin::Derived {
            return;
        }

        if let (Some(ours), Some(compiler)) = (expr.raw, self.resolve(compiler)) {
            self.check(index, rule, ours, compiler);
        }
    }

    pub fn expression(&mut self, node: &hir::Expression) {
        self.expr(node, false);
    }

    /// Records `node` and what it contains. `synthetic` says whether the
    /// parent already knows that desugaring introduced `node`; the rules
    /// that only depend on `node` itself are applied here.
    fn expr(&mut self, node: &hir::Expression, synthetic: bool) {
        use hir::Expression as E;

        match node {
            E::And(n) => {
                self.stored(n.location, "And", Some(n.resolved_type), synthetic);
                self.expression(&n.left);
                self.expression(&n.right);
            }
            E::Or(n) => {
                self.stored(n.location, "Or", Some(n.resolved_type), synthetic);
                self.expression(&n.left);
                self.expression(&n.right);
            }
            E::AssignField(n) => {
                self.stored(n.location, "AssignField", Some(n.resolved_type), synthetic);
                self.refer(n.field.location, n.field_id.map(SymbolId::Field));
                self.writes.push(n.field.location);
                self.expression(&n.value);
            }
            E::ReplaceField(n) => {
                self.stored(n.location, "ReplaceField", Some(n.resolved_type), synthetic);
                self.refer(n.field.location, n.field_id.map(SymbolId::Field));
                self.writes.push(n.field.location);
                self.expression(&n.value);
            }
            E::AssignSetter(n) => {
                // A setter assignment's own value is the setter's return
                // value, or nil when it assigns a field directly.
                let typ = self.call_kind(&n.kind);

                self.stored(n.location, "AssignSetter", typ, synthetic);
                self.refer(n.name.location, call_symbol(&n.kind));
                self.writes.push(n.name.location);

                let receiver = self.receiver(&n.receiver, false);

                self.check_receiver(receiver, &n.kind);
                self.expression(&n.value);
            }
            E::ReplaceSetter(n) => {
                self.stored(
                    n.location,
                    "ReplaceSetter",
                    Some(n.resolved_type),
                    synthetic,
                );
                self.refer(n.name.location, n.field_id.map(SymbolId::Field));
                self.writes.push(n.name.location);
                self.receiver(&n.receiver, false);
                self.expression(&n.value);
            }
            E::AssignVariable(n) => {
                self.stored(
                    n.location,
                    "AssignVariable",
                    Some(n.resolved_type),
                    synthetic,
                );

                if !generated(&n.variable.name) {
                    self.refer(n.variable.location, n.variable_id.map(SymbolId::Variable));
                    self.writes.push(n.variable.location);
                }

                self.expression(&n.value);
            }
            E::ReplaceVariable(n) => {
                self.stored(
                    n.location,
                    "ReplaceVariable",
                    Some(n.resolved_type),
                    synthetic,
                );

                if !generated(&n.variable.name) {
                    self.refer(n.variable.location, n.variable_id.map(SymbolId::Variable));
                    self.writes.push(n.variable.location);
                }

                self.expression(&n.value);
            }
            // Not stored: `break_expression` and `next_expression` always
            // return Never (compiler/src/type_check/expressions.rs:2940-2985).
            E::Break(n) => {
                self.push(
                    n.location,
                    "Break",
                    Some(TypeRef::Never),
                    Origin::Derived,
                    synthetic,
                );
            }
            E::Next(n) => {
                self.push(
                    n.location,
                    "Next",
                    Some(TypeRef::Never),
                    Origin::Derived,
                    synthetic,
                );
            }
            E::BuiltinCall(n) => {
                let typ = n.info.as_ref().map(|i| i.returns);

                self.stored(n.location, "BuiltinCall", typ, synthetic);
                self.body(&n.arguments);
            }
            E::Call(n) => {
                let typ = self.call_kind(&n.kind);
                // `$buf.push(...)`, `$array.push(...)`, `$iter.next` and so
                // on: a call on a generated variable or type is generated.
                let generated_receiver = n.receiver.as_ref().is_some_and(names_generated);
                // `iterator.into_iter` in a `for` loop and `value.to_string`
                // in string interpolation reuse the receiver's location. A
                // call written in the source always also spans its name.
                let same_as_receiver = n
                    .receiver
                    .as_ref()
                    .is_some_and(|r| r.location() == n.location);
                // `!x` becomes `x == false`, with the `false` at the location
                // of the whole expression, and so is the `==`'s name.
                let negation = n.arguments.iter().any(|a| {
                    matches!(a, hir::Argument::Positional(p) if matches!(&p.value, E::False(f) if f.location == n.location))
                });
                let synthetic = synthetic || generated_receiver || same_as_receiver;

                self.stored(n.location, "Call", typ, synthetic);

                // Binary operators (`a + b`) are calls with parentheses too,
                // but not written with them.
                let operator = !n
                    .name
                    .name
                    .starts_with(|c: char| c.is_alphabetic() || c == '_');

                if n.parens && !synthetic && !operator {
                    self.argument_lists.push(ArgumentList {
                        location: n.location,
                        name: n.name.location,
                        types: n.type_arguments.last().map(|t| t.location()),
                        method: match &n.kind {
                            CallKind::Call(info) => Some((info.id, info.type_arguments.clone())),
                            _ => None,
                        },
                        arguments: n
                            .arguments
                            .iter()
                            .map(|a| match a {
                                hir::Argument::Positional(p) => ListArgument {
                                    location: p.value.location(),
                                    name: None,
                                    variable: match &p.value {
                                        E::IdentifierRef(v) => Some(v.name.clone()),
                                        E::FieldRef(f) => Some(f.name.clone()),
                                        _ => None,
                                    },
                                },
                                hir::Argument::Named(n) => ListArgument {
                                    location: n.location,
                                    name: Some(n.name.name.clone()),
                                    variable: None,
                                },
                            })
                            .collect(),
                    });
                }

                if !synthetic && !negation {
                    let symbol = match n.kind {
                        CallKind::Unknown => self.static_symbol(node),
                        _ => call_symbol(&n.kind),
                    };

                    self.refer(n.name.location, symbol);
                }

                n.type_arguments.iter().for_each(|t| self.type_node(t));

                if let Some(r) = &n.receiver {
                    let receiver = self.receiver(r, false);

                    self.check_receiver(receiver, &n.kind);
                    self.calls.push(CallSite {
                        name: n.name.location,
                        receiver,
                        receiver_is_self: r.is_self(),
                    });
                }

                // The capacity in `$Array.with_capacity(n)` is generated, but
                // the values in `$array.push(value)` are written in the
                // source.
                let generated_arguments = matches!(
                    &n.receiver,
                    Some(E::ConstantRef(c)) if generated(&c.name)
                );

                for arg in &n.arguments {
                    let value = match arg {
                        hir::Argument::Positional(a) => &a.value,
                        hir::Argument::Named(a) => {
                            if !synthetic {
                                let symbol = self.named_argument(&n.kind, &a.name.name);

                                self.refer(a.name.location, symbol);
                            }

                            &a.value
                        }
                    };
                    // `not x` becomes `x == false`, with the `false` at the
                    // location of the whole expression.
                    let not = matches!(value, E::False(f) if f.location == n.location);

                    self.expr(value, generated_arguments || not);
                }
            }
            E::Closure(n) => {
                self.stored(n.location, "Closure", Some(n.resolved_type), synthetic);

                // The type checker records a closure's arguments in its
                // ClosureId, not the HIR (compiler/src/type_check/expressions.rs:2424-2451).
                let args = n
                    .closure_id
                    .map(|id| id.arguments(self.db))
                    .unwrap_or_default();

                for (i, arg) in n.arguments.iter().enumerate() {
                    let typ = args
                        .iter()
                        .find(|a| a.index == i)
                        .map(|a| a.variable.value_type(self.db));

                    self.stored(arg.name.location, "Binding", typ, false);

                    if arg.value_type.is_none() {
                        self.untyped.push(Untyped {
                            name: arg.name.location,
                            typ,
                            obvious: false,
                        });
                    }

                    if let Some(a) = args.iter().find(|a| a.index == i) {
                        self.declare(arg.name.location, SymbolId::Variable(a.variable));
                    }

                    if let Some(t) = &arg.value_type {
                        self.type_node(t);
                    }
                }

                if let Some(t) = &n.return_type {
                    self.type_node(t);
                }

                self.body(&n.body);
            }
            E::ConstantRef(n) => {
                // A constant that's really a method call (e.g. `Foo()`
                // without a receiver) records the call instead.
                let typ = match &n.kind {
                    ConstantKind::Method(info) => Some(info.returns),
                    _ => Some(n.resolved_type),
                };
                let synthetic = synthetic || generated(&n.name);

                self.stored(n.location, "ConstantRef", typ, synthetic);

                if !synthetic {
                    let symbol = match &n.kind {
                        ConstantKind::Unknown => self.static_symbol(node),
                        ConstantKind::Constant(id) => Some(SymbolId::Constant(*id)),
                        ConstantKind::Method(info) => Some(SymbolId::Method(info.id)),
                    };

                    if let Some(src) = &n.source {
                        let module = self.module_named(&src.name);

                        self.refer(src.location, module);
                    }

                    self.refer(tail(n.location, &n.name), symbol);
                }
            }
            E::DefineVariable(n) => {
                let generated_var =
                    matches!(&n.pattern, hir::Pattern::Identifier(p) if generated(&p.name.name));

                self.stored(
                    n.location,
                    "DefineVariable",
                    Some(n.resolved_type),
                    synthetic || generated_var,
                );
                self.pattern(&n.pattern, synthetic);

                if let hir::Pattern::Identifier(p) = &n.pattern
                    && p.value_type.is_none()
                    && !synthetic
                    && !generated_var
                {
                    self.untyped.push(Untyped {
                        name: p.name.location,
                        typ: p.variable_id.map(|id| id.value_type(self.db)),
                        obvious: self.obvious(&n.value),
                    });
                }

                let value = self.out.len();

                self.expression(&n.value);

                // `let x = <derived>` gives `x` the value's type
                // (compiler/src/type_check/expressions.rs:1618-1622, then `pattern` binds
                // an identifier to `input_type` when it has no annotation).
                if let hir::Pattern::Identifier(p) = &n.pattern
                    && p.value_type.is_none()
                    && let Some(id) = p.variable_id
                {
                    self.check_value(value, "let", id.value_type(self.db));
                }

                if let Some(e) = &n.else_body {
                    self.expression(e);
                }
            }
            E::False(n) => _ = self.stored(n.location, "False", Some(n.resolved_type), synthetic),
            E::True(n) => _ = self.stored(n.location, "True", Some(n.resolved_type), synthetic),
            E::Nil(n) => _ = self.stored(n.location, "Nil", Some(n.resolved_type), synthetic),
            E::FieldRef(n) => {
                let typ = n.info.as_ref().map(|i| i.variable_type);

                self.stored(n.location, "FieldRef", typ, synthetic);
                self.refer(n.location, n.info.as_ref().map(|i| SymbolId::Field(i.id)));
            }
            E::Float(n) => _ = self.stored(n.location, "Float", Some(n.resolved_type), synthetic),
            E::Int(n) => _ = self.stored(n.location, "Int", Some(n.resolved_type), synthetic),
            E::String(n) => _ = self.stored(n.location, "String", Some(n.resolved_type), synthetic),
            E::IdentifierRef(n) => {
                let typ = match &n.kind {
                    IdentifierKind::Unknown => None,
                    IdentifierKind::Variable(id) => Some(id.value_type(self.db)),
                    IdentifierKind::Method(info) => Some(info.returns),
                };
                let synthetic = synthetic || generated(&n.name);

                self.stored(n.location, "IdentifierRef", typ, synthetic);

                if !synthetic {
                    let symbol = match &n.kind {
                        IdentifierKind::Unknown => self.static_symbol(node),
                        IdentifierKind::Variable(id) => Some(SymbolId::Variable(*id)),
                        IdentifierKind::Method(info) => Some(SymbolId::Method(info.id)),
                    };

                    self.refer(n.location, symbol);
                }
            }
            E::Loop(n) => {
                // Not stored: `loop_expression` returns Nil if a `break`
                // targets this loop, and Never otherwise
                // (compiler/src/type_check/expressions.rs:2918-2938).
                let typ = if breaks(&n.body) {
                    TypeRef::nil()
                } else {
                    TypeRef::Never
                };

                self.push(n.location, "Loop", Some(typ), Origin::Derived, synthetic);

                for e in &n.body {
                    // `while` and `for` loops wrap their body in a `match`
                    // at the loop's location.
                    let desugared = matches!(e, E::Match(m) if m.location == n.location);

                    self.expr(e, desugared);
                }
            }
            E::Match(n) => {
                self.stored(n.location, "Match", Some(n.resolved_type), synthetic);
                self.expression(&n.expression);

                for case in &n.cases {
                    self.pattern(&case.pattern, synthetic);

                    if let Some(g) = &case.guard {
                        self.expression(g);
                    }

                    for e in &case.body {
                        let generated = match e {
                            // The `break` that ends a `while` or `for` loop,
                            // in its last (wildcard) case.
                            E::Break(_) => {
                                synthetic
                                    && case.body.len() == 1
                                    && matches!(case.pattern, hir::Pattern::Wildcard(_))
                            }
                            // The `nil` of an `if` without an `else`, at the
                            // location of the whole `if`.
                            E::Nil(v) => v.location == n.location,
                            _ => false,
                        };

                        self.expr(e, generated);
                    }
                }
            }
            E::Mut(n) => {
                self.stored(n.location, "Mut", Some(n.resolved_type), synthetic);

                match &n.value {
                    // `mut fn_name` takes a pointer to a C function: the
                    // name is a method, not a value, so it has no type of its
                    // own. The method is in `pointer_to_method`.
                    hir::Expression::IdentifierRef(v) if n.pointer_to_method.is_some() => {
                        self.stored(v.location, "PointerToMethod", None, false);
                        self.refer(v.location, n.pointer_to_method.map(SymbolId::Method));
                    }
                    v => self.expression(v),
                }
            }
            E::Ref(n) => {
                self.stored(n.location, "Ref", Some(n.resolved_type), synthetic);
                self.expression(&n.value);
            }
            E::Recover(n) => {
                // The type checker wraps a returned value in a `recover` at
                // the value's own location when the return type is unique
                // (compiler/src/type_check/expressions.rs:1335-1342 for a
                // body's last value, compiler/src/type_check/expressions.rs:3025-3027
                // for `return`).
                let inferred = n.body.len() == 1 && n.body[0].location() == n.location;

                self.stored(
                    n.location,
                    "Recover",
                    Some(n.resolved_type),
                    synthetic || inferred,
                );
                self.body(&n.body);
            }
            E::Return(n) => {
                self.stored(n.location, "Return", Some(n.resolved_type), synthetic);

                if let Some(v) = &n.value {
                    self.expression(v);
                }
            }
            E::Scope(n) => {
                self.stored(n.location, "Scope", Some(n.resolved_type), synthetic);

                for (i, e) in n.body.iter().enumerate() {
                    // A `for` loop is a scope containing the iterator's
                    // variable and a loop, both at the scope's location.
                    let desugared = matches!(e, E::Loop(l) if l.location == n.location);
                    let index = self.out.len();

                    self.expr(e, desugared);

                    // A scope's type is its last expression's type, as an
                    // owned value (`last_expression_type`,
                    // compiler/src/type_check/expressions.rs:1371-1380, and
                    // `scope`, compiler/src/type_check/expressions.rs:1594-1605).
                    if i + 1 == n.body.len()
                        && let Some(ours) = self.out[index].raw
                        && self.out[index].origin == Origin::Derived
                    {
                        let ours = ours.value_type_as_owned(self.db);

                        if let Some(compiler) = self.resolve(n.resolved_type) {
                            self.check(index, "scope", ours, compiler);
                        }
                    }
                }
            }
            E::SelfObject(n) => {
                _ = self.stored(n.location, "SelfObject", Some(n.resolved_type), synthetic)
            }
            E::Throw(n) => {
                self.stored(n.location, "Throw", Some(n.resolved_type), synthetic);
                self.expression(&n.value);
            }
            E::Tuple(n) => {
                self.stored(n.location, "Tuple", Some(n.resolved_type), synthetic);
                self.body(&n.values);
            }
            E::TypeCast(n) => {
                self.stored(n.location, "TypeCast", Some(n.resolved_type), synthetic);
                self.expression(&n.value);
                self.type_node(&n.cast_to);
            }
            E::Try(n) => {
                let typ = self.try_type(n);

                self.push(n.location, "Try", typ, Origin::Derived, synthetic);
                self.expression(&n.expression);
            }
            E::SizeOf(n) => {
                self.stored(n.location, "SizeOf", Some(n.resolved_type), synthetic);
                self.type_node(&n.argument);
            }
        }
    }

    /// Not stored: `try_expression` (compiler/src/type_check/expressions.rs:4337-4426)
    /// stores the operand's `ThrowKind` in `kind` and the surrounding
    /// method's return type in `return_type`, then returns the Option's or
    /// Result's inner value for the combinations it accepts and an error for
    /// the rest.
    ///
    /// One difference: for a Result inside a method returning a Result, the
    /// type checker also checks that the error types are compatible
    /// (`TypeChecker::check_return`, with the method's `self` type), and
    /// types the `try` as an error if they aren't. This returns the inner
    /// value either way. That only matters for code with an invalid-throw
    /// error.
    fn try_type(&self, node: &hir::Try) -> Option<TypeRef> {
        let ret = node.return_type.throw_kind(self.db);

        match (node.kind, ret) {
            (ThrowKind::Option(some), ThrowKind::Option(_) | ThrowKind::Infer(_)) => Some(some),
            (ThrowKind::Result(ok, _), ThrowKind::Result(_, _) | ThrowKind::Infer(_)) => Some(ok),
            _ => None,
        }
    }
}

/// The `SymbolId` of a module's symbol.
fn symbol_id(symbol: Symbol) -> Option<SymbolId> {
    Some(match symbol {
        Symbol::Type(id) => SymbolId::Type(id),
        Symbol::Trait(id) => SymbolId::Trait(id),
        Symbol::Module(id) => SymbolId::Module(id),
        Symbol::TypeParameter(id) => SymbolId::TypeParameter(id),
        Symbol::Constant(id) => SymbolId::Constant(id),
        Symbol::Method(id) => SymbolId::Method(id),
    })
}

/// The symbol a call's name refers to.
fn call_symbol(kind: &CallKind) -> Option<SymbolId> {
    match kind {
        CallKind::Call(info) => Some(SymbolId::Method(info.id)),
        CallKind::GetField(info) | CallKind::SetField(info) => Some(SymbolId::Field(info.id)),
        CallKind::GetConstant(id) => Some(SymbolId::Constant(*id)),
        CallKind::TypeInstance(info) => Some(SymbolId::Type(info.type_id)),
        CallKind::Unknown
        | CallKind::CallClosure(_)
        | CallKind::ReadPointer(_)
        | CallKind::WritePointer => None,
    }
}

/// Whether a name was generated by desugaring. Generated names start with
/// `$` (compiler/src/hir.rs:21-29, and `ARRAY_INTERNAL_NAME` and friends in
/// types/src/lib.rs), which isn't valid in source code.
fn generated(name: &str) -> bool {
    name.starts_with('$')
}

/// Whether an expression refers to a generated variable or type.
fn names_generated(node: &hir::Expression) -> bool {
    match node {
        hir::Expression::IdentifierRef(n) => generated(&n.name),
        hir::Expression::ConstantRef(n) => generated(&n.name),
        _ => false,
    }
}

/// Whether a `break` in this loop body targets the loop itself.
///
/// `break_expression` (compiler/src/type_check/expressions.rs:2940-2968)
/// marks the nearest enclosing loop scope. Closures don't stop that search:
/// a closure's scope has the enclosing scope as its parent
/// (compiler/src/type_check/expressions.rs:2484-2491), so a `break` inside a
/// closure inside a loop makes the loop Nil. Only a nested loop stops it.
fn breaks(body: &[hir::Expression]) -> bool {
    body.iter().any(breaks_in)
}

fn breaks_in(node: &hir::Expression) -> bool {
    use hir::Expression as E;

    match node {
        E::Break(_) => true,
        E::Loop(_) => false,
        E::Closure(n) => breaks(&n.body),
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

/// Whether the type checker's `Expressions` pass ran. It's the last pass
/// before unused imports are checked, and `check_types` stops at the first
/// pass with errors (compiler.rs, `check_types`), so after an error in a
/// declaration no method body is typed.
///
/// The pass types every module (compiler/src/type_check/expressions.rs:727-736), and every
/// check includes the standard library's prelude, so if it ran, some literal
/// in some method body has a type.
pub fn expressions_pass_ran(modules: &[hir::Module]) -> bool {
    modules.iter().any(|m| {
        m.expressions.iter().any(|e| match e {
            hir::TopLevelExpression::ModuleMethod(n) => typed_literal(&n.body),
            hir::TopLevelExpression::Type(n) => n.body.iter().any(|e| match e {
                hir::TypeExpression::InstanceMethod(m) => typed_literal(&m.body),
                hir::TypeExpression::StaticMethod(m) => typed_literal(&m.body),
                _ => false,
            }),
            _ => false,
        })
    })
}

fn typed_literal(body: &[hir::Expression]) -> bool {
    body.iter().any(|e| match e {
        hir::Expression::Int(n) => n.resolved_type != TypeRef::Unknown,
        hir::Expression::String(n) => n.resolved_type != TypeRef::Unknown,
        hir::Expression::True(n) => n.resolved_type != TypeRef::Unknown,
        hir::Expression::False(n) => n.resolved_type != TypeRef::Unknown,
        _ => false,
    })
}
