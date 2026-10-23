//! Helpers for dump-types and the tests: every expression of a target with
//! its type, the cross-checks, and the pinned standard library. The
//! language server doesn't use them.
use crate::check::{Target, run};
use crate::walker::{self, Expr, Origin};
use std::path::{Path, PathBuf};
use std::time::Duration;

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
    /// Reconstructed types compared with what the compiler stored.
    pub checks: Vec<walker::CrossCheck>,
    /// Whether type checking and lowering to MIR reported no errors.
    pub ok: bool,
    /// The number of modules type checked, including imported ones.
    pub modules: usize,
    /// Parsing, lowering to HIR and type checking.
    pub type_check: Duration,
    /// The above, plus lowering to MIR.
    pub total: Duration,
}

/// Type checks the target, including lowering to MIR, and collects every
/// expression of its own modules, sorted by location. Prints the compiler's
/// diagnostics to stderr if there are errors.
pub fn check(target: &Target) -> Result<Checked, String> {
    let run = run(target, true, true, |db, modules| {
        let mut expressions = Vec::new();
        let mut checks = Vec::new();

        for module in modules {
            let file = module.module_id.file(db);

            // Imported modules (std, dependencies) are type checked too, but
            // only the target's own modules are reported.
            let own = match &target.file {
                Some(f) => &file == f,
                None => file.starts_with(&target.root),
            };

            if own {
                let mut walker = walker::Walker::new(db, module.module_id);

                walker.module(module);
                expressions.append(&mut walker.out);
                checks.append(&mut walker.checks);
            }
        }

        (expressions, checks)
    })?;

    let (mut expressions, checks) = run.inspected.unwrap_or_default();

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
        checks,
        ok: run.ok,
        modules: run.modules,
        type_check: run.type_check,
        total: run.total,
    })
}

/// Type checks the target, without lowering to MIR, and returns every
/// cross-check of a reconstructed type in its own modules.
pub fn cross_checks(target: &Target) -> Result<(bool, Vec<walker::CrossCheck>), String> {
    let run = run(target, false, false, |db, modules| {
        let mut checks = Vec::new();

        for module in modules {
            let file = module.module_id.file(db);
            let own = match &target.file {
                Some(f) => &file == f,
                None => file.starts_with(&target.root),
            };

            if own {
                let mut walker = walker::Walker::new(db, module.module_id);

                walker.module(module);
                checks.append(&mut walker.checks);
            }
        }

        checks
    })?;

    Ok((run.ok, run.inspected.unwrap_or_default()))
}

/// Formats an expression as `file:line:col-line:col<TAB>kind<TAB>type`, with
/// `file` relative to `root`, then a column for a type that isn't stored in
/// the HIR, and `synthetic` for a node desugaring introduced.
pub fn format_expr(expr: &Expr, root: &Path) -> String {
    let file = expr.file.strip_prefix(root).unwrap_or(&expr.file);
    let l = expr.location;

    let origin = match expr.origin {
        Origin::Stored => "",
        Origin::Derived => "\tderived",
        Origin::Symbol => "\tsymbol",
    };

    format!(
        "{}:{}:{}-{}:{}\t{}\t{}{}{}",
        file.display(),
        l.line_start,
        l.column_start,
        l.line_end,
        l.column_end,
        expr.kind,
        expr.typ.as_deref().unwrap_or("<missing>"),
        origin,
        if expr.synthetic { "\tsynthetic" } else { "" }
    )
}

/// The standard library of the pinned upstream release.
pub fn upstream_std() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(".upstream/inko/std/src")
}
