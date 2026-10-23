//! Walks a module's typed HIR and records each expression with its type.
//!
//! Most types are stored in the HIR by the type checker. The rest (see
//! `Origin`) are reconstructed here by reapplying the type checker's rules.
//! Each such rule cites the upstream lines it mirrors (paths relative to
//! `.upstream/inko/compiler/src/`), so a rebase can diff them, and where the
//! compiler stores something to compare with, the walker records a
//! `CrossCheck` that tests/crosscheck.rs asserts on.
use compiler::hir;
use location::Location;
use std::collections::HashMap;
use std::path::PathBuf;
use types::format::format_type;
use types::{
    CallKind, ConstantKind, Database, IdentifierKind, ModuleId, Receiver, Symbol, ThrowKind,
    TypeEnum, TypeRef,
};

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
    /// source code at its location (see docs/protocol.md, "Which expression
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
    /// (type_check/expressions.rs:4499).
    pub receiver_is_self: bool,
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
    pub checks: Vec<CrossCheck>,
}

impl<'a> Walker<'a> {
    pub fn new(db: &'a Database, module: ModuleId) -> Self {
        Walker {
            db,
            file: module.file(db),
            symbols: module.symbols(db).into_iter().collect(),
            out: Vec::new(),
            calls: Vec::new(),
            checks: Vec::new(),
        }
    }

    pub fn module(&mut self, module: &hir::Module) {
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
                && self.out.get(index + 1).is_some_and(|e| e.origin == Origin::Symbol)
            {
                let module = self.out[index + 1].raw.unwrap();

                self.check(index + 1, "receiver", module, TypeRef::module(id.module(self.db)));
            }
        }

        index
    }

    /// The type the type checker gives a type or module used as a receiver.
    ///
    /// Mirrors type_check/expressions.rs: `constant` with `receiver` set
    /// returns `Owned(Type(id))` for a type (:2579, `Some(Symbol::Type(id))
    /// if receiver`), `identifier` returns `TypeRef::module(id)` for a module
    /// (:2714), and `call_with_receiver` returns `Owned(Type(id))` for
    /// `module.Type` used as a receiver (:3812, `Some(Symbol::Type(id)) if
    /// as_receiver`). None of these are stored in the HIR.
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

    /// Records the variables a pattern introduces.
    fn pattern(&mut self, node: &hir::Pattern) {
        use hir::Pattern as P;

        match node {
            P::Identifier(n) => {
                let typ = n.variable_id.map(|id| id.value_type(self.db));
                let synthetic = generated(&n.name.name);

                self.stored(n.name.location, "Binding", typ, synthetic);
            }
            P::Tuple(n) => n.values.iter().for_each(|p| self.pattern(p)),
            P::Constructor(n) => n.values.iter().for_each(|p| self.pattern(p)),
            P::Type(n) => n.values.iter().for_each(|f| self.pattern(&f.pattern)),
            P::Or(n) => n.patterns.iter().for_each(|p| self.pattern(p)),
            P::Array(n) => n.values.iter().for_each(|p| self.pattern(p)),
            P::Constant(_)
            | P::Int(_)
            | P::String(_)
            | P::Wildcard(_)
            | P::True(_)
            | P::False(_) => {}
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
                Receiver::Type | Receiver::Explicit | Receiver::Implicit => {
                    match ours {
                        TypeRef::Owned(TypeEnum::Module(_)) => {
                            TypeRef::module(info.id.module(self.db))
                        }
                        _ => info.id.receiver(self.db),
                    }
                }
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
                self.expression(&n.value);
            }
            E::ReplaceField(n) => {
                self.stored(n.location, "ReplaceField", Some(n.resolved_type), synthetic);
                self.expression(&n.value);
            }
            E::AssignSetter(n) => {
                // A setter assignment's own value is the setter's return
                // value, or nil when it assigns a field directly.
                let typ = self.call_kind(&n.kind);

                self.stored(n.location, "AssignSetter", typ, synthetic);

                let receiver = self.receiver(&n.receiver, false);

                self.check_receiver(receiver, &n.kind);
                self.expression(&n.value);
            }
            E::ReplaceSetter(n) => {
                self.stored(n.location, "ReplaceSetter", Some(n.resolved_type), synthetic);
                self.receiver(&n.receiver, false);
                self.expression(&n.value);
            }
            E::AssignVariable(n) => {
                self.stored(n.location, "AssignVariable", Some(n.resolved_type), synthetic);
                self.expression(&n.value);
            }
            E::ReplaceVariable(n) => {
                self.stored(n.location, "ReplaceVariable", Some(n.resolved_type), synthetic);
                self.expression(&n.value);
            }
            // Not stored: `break_expression` and `next_expression` always
            // return Never (type_check/expressions.rs:2940-2985).
            E::Break(n) => {
                self.push(n.location, "Break", Some(TypeRef::Never), Origin::Derived, synthetic);
            }
            E::Next(n) => {
                self.push(n.location, "Next", Some(TypeRef::Never), Origin::Derived, synthetic);
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
                let same_as_receiver = n.receiver.as_ref().is_some_and(|r| r.location() == n.location);
                self.stored(
                    n.location,
                    "Call",
                    typ,
                    synthetic || generated_receiver || same_as_receiver,
                );

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
                        hir::Argument::Named(a) => &a.value,
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
                // ClosureId, not the HIR (type_check/expressions.rs:2424-2451).
                let args = n.closure_id.map(|id| id.arguments(self.db)).unwrap_or_default();

                for (i, arg) in n.arguments.iter().enumerate() {
                    let typ = args
                        .iter()
                        .find(|a| a.index == i)
                        .map(|a| a.variable.value_type(self.db));

                    self.stored(arg.name.location, "Binding", typ, false);
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

                self.stored(n.location, "ConstantRef", typ, synthetic || generated(&n.name));
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
                self.pattern(&n.pattern);

                let value = self.out.len();

                self.expression(&n.value);

                // `let x = <derived>` gives `x` the value's type
                // (type_check/expressions.rs:1618-1622, then `pattern` binds
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

                self.stored(n.location, "IdentifierRef", typ, synthetic || generated(&n.name));
            }
            E::Loop(n) => {
                // Not stored: `loop_expression` returns Nil if a `break`
                // targets this loop, and Never otherwise
                // (type_check/expressions.rs:2918-2938).
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
                    self.pattern(&case.pattern);

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
                // (type_check/expressions.rs:1335-1342 and :3025-3027).
                let inferred = n.body.len() == 1 && n.body[0].location() == n.location;

                self.stored(n.location, "Recover", Some(n.resolved_type), synthetic || inferred);
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
                    // owned value (type_check/expressions.rs:1371-1380 and
                    // :1594-1605).
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
            }
            E::Try(n) => {
                let typ = self.try_type(n);

                self.push(n.location, "Try", typ, Origin::Derived, synthetic);
                self.expression(&n.expression);
            }
            E::SizeOf(n) => {
                _ = self.stored(n.location, "SizeOf", Some(n.resolved_type), synthetic)
            }
        }
    }

    /// Not stored: `try_expression` (type_check/expressions.rs:4337-4430)
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

/// Whether a name was generated by desugaring. Generated names start with
/// `$` (hir.rs:21-29, and `ARRAY_INTERNAL_NAME` and friends in
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
/// `break_expression` (type_check/expressions.rs:2940-2956) marks the nearest
/// enclosing loop scope. Closures don't stop that search: a closure's scope
/// has the enclosing scope as its parent (:2484-2491), so a `break` inside a
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
/// The pass types every module (type_check/expressions.rs:727-736), and every
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
