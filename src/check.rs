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
use std::time::{Duration, Instant, SystemTime};
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
/// `Compiler::prepare` does (compiler/src/compiler.rs:796-811) for `inko
/// check`. `prepare` reads the manifests relative to the working directory
/// (compiler/src/pkg/manifest.rs:11-34), and then syncs `dep/` if `inko.pkg`
/// changed (compiler/src/compiler.rs:813), which can download packages, and
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
    run_with(target, mir, print_diagnostics, false, |db, hir, _| {
        inspect(db, hir)
    })
}

/// `run`, without lowering to MIR, but with every type checking pass run
/// even after an error: an error in a declaration no longer keeps method
/// bodies from being typed (docs/lsp.md, "Errors in declarations").
/// `inspect` also gets the errors reported, since some of the typed HIR
/// may depend on a declaration that has one. Diagnostics then differ
/// from `inko check`'s (more errors in bodies), so they come from
/// `diagnose`, which runs the passes as `inko check` does.
pub fn run_after_errors<R>(
    target: &Target,
    inspect: impl FnOnce(&Database, &[hir::Module], &[Reported]) -> R,
) -> Result<Run<R>, String> {
    run_with(target, false, false, true, inspect)
}

fn run_with<R>(
    target: &Target,
    mir: bool,
    print_diagnostics: bool,
    after_errors: bool,
    inspect: impl FnOnce(&Database, &[hir::Module], &[Reported]) -> R,
) -> Result<Run<R>, String> {
    check_required_version(target)?;

    let mut config = target.config()?;

    config.check_after_errors = after_errors;

    let mut compiler = Compiler::new(config);
    let start = Instant::now();
    let mut type_check = Duration::ZERO;
    let mut modules = 0;
    let mut inspected = None;
    let res = compiler.check_with(target.file.clone(), |db, hir, diagnostics| {
        type_check = start.elapsed();
        modules = hir.len();
        let errors: Vec<Reported> = diagnostics
            .iter()
            .filter(|d| d.is_error())
            .map(|d| reported(d))
            .collect();
        inspected = Some(inspect(db, hir, &errors));
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

fn reported(d: &compiler::diagnostics::Diagnostic) -> Reported {
    Reported {
        file: d.file().clone(),
        error: d.is_error(),
        id: d.id().to_string(),
        message: d.message().to_string(),
        location: *d.location(),
    }
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
        compiler.check_with(target.file.clone(), |db, hir, _| {
            inspect(db, hir);
            true
        })
    }));
    let diagnostics = compiler.diagnostics().map(reported).collect();

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

/// What a file or directory on disk looked like: enough to tell that it
/// changed (`Fingerprint`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
    /// When the inode last changed, which no tool can set back, and its
    /// identity, which a rename over the file changes.
    #[cfg(unix)]
    changed: (i64, i64, u64, u64),
}

impl Stamp {
    fn of(path: &Path) -> Option<Stamp> {
        let meta = std::fs::metadata(path).ok()?;

        Some(Stamp {
            len: meta.len(),
            modified: meta.modified().ok(),
            #[cfg(unix)]
            changed: {
                use std::os::unix::fs::MetadataExt;

                (meta.ctime(), meta.ctime_nsec(), meta.ino(), meta.dev())
            },
        })
    }

    /// The latest time the stamp says the path changed.
    fn latest(&self) -> Option<SystemTime> {
        #[cfg(unix)]
        let changed = {
            let (secs, nanos, _, _) = self.changed;

            u64::try_from(secs)
                .ok()
                .map(|s| SystemTime::UNIX_EPOCH + Duration::new(s, nanos as u32))
        };
        #[cfg(not(unix))]
        let changed: Option<SystemTime> = None;

        changed.into_iter().chain(self.modified).max()
    }
}

/// How long before a check started a file must have last changed for the
/// check to be sure it read that version, on a file system that records
/// times in whole seconds (FAT's are two seconds apart).
const COARSE: Duration = Duration::from_secs(2);

/// The state on disk of everything a check of a target read, or could
/// have read: each module's file, the manifests, and every directory
/// module resolution could find a file in (std, `src/`, `test/` and
/// `dep/`, and the project directory). A file created or removed changes
/// its directory's stamp. A check's results hold for as long as its
/// fingerprint is `current`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fingerprint {
    stamps: Vec<(PathBuf, Option<Stamp>)>,
    /// When the check started.
    started: SystemTime,
}

impl Fingerprint {
    /// The fingerprint of a check of `target` that started at `started`,
    /// and read `files` (the checked modules' files).
    pub fn take(
        target: &Target,
        files: impl IntoIterator<Item = PathBuf>,
        started: SystemTime,
    ) -> Fingerprint {
        let mut paths: Vec<PathBuf> = files.into_iter().collect();

        paths.push(target.root.clone());
        paths.push(target.root.join(MANIFEST_FILE));

        for dir in [
            target.std.clone(),
            target.root.join("src"),
            target.root.join("test"),
            target.root.join("dep"),
        ] {
            directories(&dir, &mut paths);
        }

        paths.sort();
        paths.dedup();

        Fingerprint {
            stamps: paths
                .into_iter()
                .map(|p| {
                    let stamp = Stamp::of(&p);

                    (p, stamp)
                })
                .collect(),
            started,
        }
    }

    /// Whether nothing it covers changed since it was taken.
    pub fn current(&self) -> bool {
        self.stamps.iter().all(|(p, s)| Stamp::of(p) == *s)
    }

    /// Whether every path last changed before the check started, so the
    /// check read what the stamps describe. A file written during the
    /// check (or so shortly before it that the file system's clock can't
    /// tell) may have been read before the write, and its stamp would
    /// then describe text the check never saw.
    pub fn settled(&self) -> bool {
        self.stamps.iter().all(|(_, stamp)| {
            stamp
                .and_then(|s| s.latest())
                .is_none_or(|latest| before(latest, self.started))
        })
    }

    /// How many paths it covers.
    pub fn len(&self) -> usize {
        self.stamps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stamps.is_empty()
    }
}

/// Whether a path that last changed at `changed` changed before `started`.
/// A time in whole seconds may come from a file system that records only
/// those, so it may be up to `COARSE` later than it says.
fn before(changed: SystemTime, started: SystemTime) -> bool {
    let fine = changed
        .duration_since(SystemTime::UNIX_EPOCH)
        .is_ok_and(|d| d.subsec_nanos() != 0);
    let margin = if fine { Duration::ZERO } else { COARSE };

    changed + margin < started
}

/// `dir`, every directory under it, and the manifests in them.
fn directories(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    out.push(dir.to_path_buf());

    for entry in entries.flatten() {
        let path = entry.path();

        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            directories(&path, out);
        } else if path.file_name().is_some_and(|n| n == MANIFEST_FILE) {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_in_whole_seconds_may_be_coarse() {
        let second = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let fine = second + Duration::from_nanos(1);

        assert!(before(fine, fine + Duration::from_millis(1)));
        assert!(!before(fine, fine));
        assert!(!before(second, second + Duration::from_millis(1500)));
        assert!(before(second, second + Duration::from_millis(2001)));
    }
}
