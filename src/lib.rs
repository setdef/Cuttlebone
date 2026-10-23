//! Cuttlebone, a language server for Inko.
//!
//! It type checks a project with the upstream compiler (patched to expose
//! the typed HIR; see patches/README.md), walks the typed HIR, and answers
//! questions about the types of expressions.
use compiler::compiler::{CompileError, Compiler};
use compiler::config::Config;
use compiler::hir;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use types::Database;

pub mod position;
pub mod query;
pub mod server;
pub mod walker;

pub use walker::{Expr, Origin};

/// This build's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The upstream release this build links, without the leading `v`.
pub fn upstream_version() -> &'static str {
    include_str!("../UPSTREAM_VERSION")
        .trim()
        .trim_start_matches('v')
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
    /// Source code to use instead of what's on disk, by canonical path.
    pub overlay: HashMap<PathBuf, String>,
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
                overlay: HashMap::new(),
            });
        }

        let root = project_root(&path);

        Ok(Target {
            root,
            file: Some(path),
            tests: true,
            std,
            overlay: HashMap::new(),
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
        config.overlay = self
            .overlay
            .iter()
            .map(|(path, text)| (path.clone(), text.clone().into_bytes()))
            .collect();

        if self.tests && config.tests.is_dir() {
            config.add_source_directory(config.tests.clone());
        } else {
            // Compiler::check includes test/ whenever it exists.
            config.tests = self.root.join("\0no-tests");
        }

        Ok(config)
    }
}

/// The project a file belongs to: the nearest directory whose `src/` or
/// `test/` contains it, or else the file's own directory.
pub fn project_root(file: &Path) -> PathBuf {
    file.ancestors()
        .find(|p| p.join("src").is_dir() && file.starts_with(p.join("src")))
        .or_else(|| {
            file.ancestors()
                .find(|p| p.join("test").is_dir() && file.starts_with(p.join("test")))
        })
        .map(Path::to_path_buf)
        .unwrap_or_else(|| file.parent().unwrap().to_path_buf())
}

/// The outcome of type checking a target.
pub struct Run<R> {
    /// What `inspect` returned, or `None` if it never ran because parsing
    /// or lowering to HIR failed (e.g. a syntax error, or an import of a
    /// module that doesn't exist).
    pub inspected: Option<R>,
    /// Whether the compiler reported no errors.
    pub ok: bool,
    /// The number of modules type checked, including imported ones.
    pub modules: usize,
    /// Parsing, lowering to HIR and type checking.
    pub type_check: Duration,
    /// The above, plus lowering to MIR if `mir` was set.
    pub total: Duration,
}

/// Type checks a target and passes the typed HIR to `inspect`. With `mir`
/// set it also lowers to MIR, where some errors (e.g. moves) are found, but
/// types never depend on that.
pub fn run<R>(
    target: &Target,
    mir: bool,
    print_diagnostics: bool,
    inspect: impl FnOnce(&Database, &[hir::Module]) -> R,
) -> Result<Run<R>, String> {
    let mut compiler = Compiler::new(target.config()?);
    let start = Instant::now();
    let mut type_check = Duration::ZERO;
    let mut modules = 0;
    let mut inspected = None;
    let res = compiler.check_with(target.file.clone(), |db, hir| {
        type_check = start.elapsed();
        modules = hir.len();
        inspected = Some(inspect(db, hir));
        mir
    });
    let total = start.elapsed();

    let ok = match res {
        Ok(()) => true,
        Err(CompileError::Invalid) => false,
        Err(CompileError::Internal(e)) => return Err(e),
    };

    if !ok && print_diagnostics {
        compiler.print_diagnostics();
    }

    Ok(Run {
        inspected,
        ok,
        modules,
        type_check,
        total,
    })
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
