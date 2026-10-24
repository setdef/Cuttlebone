//! Type checking a project with the upstream compiler, with no side
//! effects, and handing the typed HIR to a caller.
use compiler::compiler::{CompileError, Compiler};
use compiler::config::Config;
use compiler::hir;
use compiler::pkg::manifest::{MANIFEST_FILE, Manifest};
use compiler::pkg::version::Version;
use location::Location;
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use types::Database;

/// This build's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The upstream release this build links, without the leading `v`.
pub fn upstream_version() -> &'static str {
    include_str!("../UPSTREAM_VERSION")
        .trim()
        .trim_start_matches('v')
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

/// Refuses a project that requires a newer Inko than the one linked, as
/// `Compiler::prepare` does (compiler/src/compiler.rs:795-810) for `inko
/// check`. `prepare` reads the manifests relative to the working directory
/// (compiler/src/pkg/manifest.rs:11-34), and then syncs `dep/` if `inko.pkg`
/// changed (compiler/src/compiler.rs:812), which can download packages, and
/// which first creates and locks the packages directory under the user's
/// data directory even when nothing changed (compiler/src/pkg/sync.rs:22-31,
/// compiler/src/pkg/util.rs:11-23). Queries mustn't have side effects, so
/// `check_with` doesn't call `prepare` (see patches/README.md), and this
/// repeats only its version check, with absolute paths.
pub fn check_required_version(target: &Target) -> Result<(), String> {
    let current = Version::parse(upstream_version()).unwrap();
    let mut paths = vec![target.root.join(MANIFEST_FILE)];
    let deps = target.root.join("dep");

    if deps.is_dir() {
        for dep in read_dir_sorted(&deps)? {
            if dep.is_dir() {
                for version in read_dir_sorted(&dep)? {
                    if version.is_dir() {
                        paths.push(version.join(MANIFEST_FILE));
                    }
                }
            }
        }
    }

    for path in paths.into_iter().filter(|p| p.is_file()) {
        let manifest = Manifest::load(&path)
            .map_err(|e| format!("failed to load the manifest '{}': {}", path.display(), e))?;

        if let Some(required) = manifest.minimum_inko_version()
            && required > current
        {
            return Err(format!(
                "this project requires Inko {} or newer, but the current version is {}",
                required, current
            ));
        }
    }

    Ok(())
}

fn read_dir_sorted(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = std::fs::read_dir(dir)
        .and_then(|entries| {
            entries
                .map(|e| e.map(|e| e.path()))
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(|e| format!("failed to read the dependency manifests: {}", e))?;

    paths.sort();
    Ok(paths)
}

/// Type checks a target and passes the typed HIR to `inspect`. With `mir`
/// set it also lowers to MIR, where some errors (e.g. moves) are found, but
/// types never depend on that.
///
/// It has no side effects: unlike `inko check`, it never syncs `dep/` (see
/// `check_required_version`), and it doesn't depend on the working
/// directory.
pub fn run<R>(
    target: &Target,
    mir: bool,
    print_diagnostics: bool,
    inspect: impl FnOnce(&Database, &[hir::Module]) -> R,
) -> Result<Run<R>, String> {
    check_required_version(target)?;

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

/// A diagnostic as the compiler reports it.
#[derive(Clone, Debug)]
pub struct Reported {
    pub file: PathBuf,
    pub error: bool,
    /// The diagnostic's ID, e.g. `invalid-symbol`.
    pub id: String,
    pub message: String,
    /// 1-based lines, and 1-based grapheme cluster columns. The end is
    /// inclusive.
    pub location: Location,
}

/// What a full check reported.
pub struct Diagnosis {
    /// Errors and warnings, in the order the compiler reported them.
    pub diagnostics: Vec<Reported>,
    /// Whether the compiler panicked. `diagnostics` then holds what it
    /// reported before that. The panic's message went to the panic hook.
    pub panicked: bool,
}

/// Checks a target the way `inko check` does (compiler/src/compiler.rs:258-261,
/// inko/src/command/check.rs:62-70), including lowering to MIR, where some
/// errors (e.g. moves) and warnings are found, and returns every diagnostic.
/// Like `run`, it has no side effects. `inspect` sees the typed HIR before
/// MIR, if there is any.
pub fn diagnose(
    target: &Target,
    inspect: impl FnOnce(&Database, &[hir::Module]),
) -> Result<Diagnosis, String> {
    check_required_version(target)?;

    let mut compiler = Compiler::new(target.config()?);
    // The compiler is only read after a panic, to collect what it reported
    // before it.
    let res = catch_unwind(AssertUnwindSafe(|| {
        compiler.check_with(target.file.clone(), |db, hir| {
            inspect(db, hir);
            true
        })
    }));
    let diagnostics = compiler
        .diagnostics()
        .map(|d| Reported {
            file: d.file().clone(),
            error: d.is_error(),
            id: d.id().to_string(),
            message: d.message().to_string(),
            location: *d.location(),
        })
        .collect();

    match res {
        Ok(Err(CompileError::Internal(e))) => Err(e),
        Ok(_) => Ok(Diagnosis {
            diagnostics,
            panicked: false,
        }),
        Err(_) => Ok(Diagnosis {
            diagnostics,
            panicked: true,
        }),
    }
}
