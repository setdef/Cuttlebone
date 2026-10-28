//! The declarations of every module a session's projects can import, from
//! their outlines (`syntax`): the standard library's, each project's own
//! (in its `src/` and `test/`), and its dependencies' (in its `dep/`, as
//! its `inko.pkg` requires them). For workspace symbols, and completion's
//! imports. No type check, so it answers when the projects don't type
//! check.
//!
//! An `Index` is kept between requests: `refresh` reads a file again only
//! if its size or modification time changed, or its open document's text
//! did, and drops files that are gone. Nothing is written anywhere.
use crate::syntax::{self, Declaration, Outline};
use compiler::pkg::manifest::{MANIFEST_FILE, Manifest};
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// What a source directory is to the session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The standard library.
    Std,
    /// A project's `src/`.
    Source(PathBuf),
    /// A project's `test/`, whose modules only its tests can import.
    Test(PathBuf),
    /// A dependency's `src/`, in a project's `dep/`: the project, and the
    /// dependency's name, the first name of every module of it that the
    /// project can import.
    Dependency(PathBuf, String),
}

/// A directory of modules: a module's name is its path in it, without
/// `.inko`, with `.` between the directories (`std/fmt.inko` is
/// `std.fmt`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Root {
    pub dir: PathBuf,
    pub origin: Origin,
}

/// The source directories of a session: std (`std`, `inko print std`'s),
/// and each project's `src/`, `test/` and required dependencies' `src/`,
/// in that order, canonical. Directories that don't exist are left out.
pub fn roots(std: &Path, projects: &[PathBuf]) -> Vec<Root> {
    let mut out = vec![Root {
        dir: std.to_path_buf(),
        origin: Origin::Std,
    }];

    for project in projects {
        out.push(Root {
            dir: project.join("src"),
            origin: Origin::Source(project.clone()),
        });
        out.push(Root {
            dir: project.join("test"),
            origin: Origin::Test(project.clone()),
        });

        // Where the compiler looks for a dependency's modules
        // (`Packages::source_directory`, compiler/src/state.rs:98-135).
        // `Manifest::load` creates a file that isn't there.
        let manifest = project.join(MANIFEST_FILE);

        if manifest.is_file()
            && let Ok(manifest) = Manifest::load(&manifest)
        {
            for dep in manifest.into_dependencies() {
                out.push(Root {
                    dir: project
                        .join("dep")
                        .join(dep.url.directory_name())
                        .join(format!("v{}", dep.version.major))
                        .join("src"),
                    origin: Origin::Dependency(project.clone(), dep.name),
                });
            }
        }
    }

    out.into_iter()
        .filter_map(|r| {
            Some(Root {
                dir: r.dir.canonicalize().ok().filter(|d| d.is_dir())?,
                ..r
            })
        })
        .collect()
}

/// A module in the index.
#[derive(Clone, Debug)]
pub struct Module {
    /// Its name: `std.fmt`, `app.util`.
    pub name: String,
    pub file: PathBuf,
    pub origin: Origin,
    /// Its text: the open document's, or the file's.
    pub text: String,
    /// Where each of its LSP lines starts (`position::line_starts`), for
    /// converting positions without reading the text again.
    pub lines: Vec<usize>,
    /// `None` if no copy of it parses (`syntax::outline`).
    pub outline: Option<Outline>,
}

impl Module {
    /// Whether a module in `importer` (a file of the session) can import
    /// it: std's always; a project's `src/` from that project; a project's
    /// `test/` from that project's tests; a dependency's from the project
    /// that requires it, if its name starts with the dependency's.
    pub fn importable_from(&self, importer: &Path) -> bool {
        match &self.origin {
            Origin::Std => true,
            Origin::Source(project) => importer.starts_with(project) && !in_dep(project, importer),
            Origin::Test(project) => importer.starts_with(project.join("test")),
            Origin::Dependency(project, name) => {
                importer.starts_with(project)
                    && !in_dep(project, importer)
                    && self.name.split('.').next() == Some(name.as_str())
            }
        }
    }
}

/// Whether a file is in a project's `dep/`.
fn in_dep(project: &Path, file: &Path) -> bool {
    file.starts_with(project.join("dep"))
}

/// What a file was read as: its size and modification time, or for an
/// open document, its text.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Stamp {
    File(u64, Option<SystemTime>),
    Open(String),
}

impl Stamp {
    fn of(path: &Path, open: Option<&String>) -> Option<Stamp> {
        if let Some(text) = open {
            return Some(Stamp::Open(text.clone()));
        }

        let meta = std::fs::metadata(path).ok()?;

        Some(Stamp::File(meta.len(), meta.modified().ok()))
    }
}

/// The modules of a session's source directories, kept between requests.
#[derive(Default)]
pub struct Index {
    modules: HashMap<PathBuf, (Stamp, Module)>,
    /// How many files the last `refresh` read and parsed.
    pub parsed: usize,
}

impl Index {
    /// Brings the index up to date with the files under `roots`, with
    /// `open` (open documents' text, by path) instead of their files.
    pub fn refresh(&mut self, roots: &[Root], open: &HashMap<PathBuf, String>) {
        let mut seen: HashMap<PathBuf, (Origin, String)> = HashMap::new();

        for root in roots {
            let mut files = Vec::new();

            sources(&root.dir, &mut files);

            for file in files {
                let Ok(relative) = file.strip_prefix(&root.dir) else {
                    continue;
                };
                let name = relative
                    .with_extension("")
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join(".");

                // The first root a file is in wins (a dependency checked out
                // inside another project).
                seen.entry(file)
                    .or_insert_with(|| (root.origin.clone(), name));
            }
        }

        self.modules.retain(|path, _| seen.contains_key(path));
        self.parsed = 0;

        for (file, (origin, name)) in seen {
            let Some(stamp) = Stamp::of(&file, open.get(&file)) else {
                self.modules.remove(&file);
                continue;
            };

            if let Some((kept, module)) = self.modules.get_mut(&file)
                && *kept == stamp
            {
                module.origin = origin;
                module.name = name;
                continue;
            }

            let text = match &stamp {
                Stamp::Open(text) => text.clone(),
                Stamp::File(..) => match std::fs::read_to_string(&file) {
                    Ok(text) => text,
                    Err(_) => continue,
                },
            };
            // The parser has no known panic, but one mustn't take the
            // server down.
            let outline = catch_unwind(AssertUnwindSafe(|| syntax::outline(&text)))
                .ok()
                .flatten();

            self.parsed += 1;
            self.modules.insert(
                file.clone(),
                (
                    stamp,
                    Module {
                        name,
                        file,
                        origin,
                        lines: crate::position::line_starts(&text),
                        text,
                        outline,
                    },
                ),
            );
        }
    }

    /// Every module, sorted by name, then file.
    pub fn modules(&self) -> Vec<&Module> {
        let mut out: Vec<&Module> = self.modules.values().map(|(_, m)| m).collect();

        out.sort_by(|a, b| (&a.name, &a.file).cmp(&(&b.name, &b.file)));
        out
    }

    /// Every declaration that matches `query` (`matches`; an `impl` block
    /// by its header, `impl Area for Shape`), best first (and of equal
    /// matches, the projects', then their dependencies', then std's), at
    /// most `limit`: each with its module and the declarations it's in.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Found<'_>> {
        let mut found: Vec<(Score, Found)> = Vec::new();

        // Every declaration, with what it's in copied only for a match.
        fn walk<'a>(
            module: &'a Module,
            list: &'a [Declaration],
            query: &str,
            parents: &mut Vec<&'a Declaration>,
            found: &mut Vec<(Score, Found<'a>)>,
        ) {
            for declaration in list {
                if let Some(score) = matches(query, &declaration.name) {
                    found.push((
                        score,
                        Found {
                            module,
                            declaration,
                            parents: parents.clone(),
                        },
                    ));
                }

                parents.push(declaration);
                walk(module, &declaration.children, query, parents, found);
                parents.pop();
            }
        }

        for module in self.modules() {
            if let Some(outline) = &module.outline {
                walk(
                    module,
                    &outline.declarations,
                    query,
                    &mut Vec::new(),
                    &mut found,
                );
            }
        }

        // The best matches first, and of those, the projects' own, then
        // their dependencies', then std's.
        let rank = |f: &Found| match f.module.origin {
            Origin::Source(_) | Origin::Test(_) => 0,
            Origin::Dependency(..) => 1,
            Origin::Std => 2,
        };

        let order = |(a, x): &(Score, Found), (b, y): &(Score, Found)| {
            a.cmp(b)
                .then_with(|| rank(x).cmp(&rank(y)))
                .then_with(|| x.declaration.name.cmp(&y.declaration.name))
                .then_with(|| x.module.name.cmp(&y.module.name))
                .then_with(|| x.declaration.range.start.cmp(&y.declaration.range.start))
        };

        // Only the best `limit` need sorting.
        if found.len() > limit && limit > 0 {
            found.select_nth_unstable_by(limit - 1, order);
        }

        found.truncate(limit);
        found.sort_by(order);
        found.into_iter().map(|(_, f)| f).collect()
    }
}

/// A declaration `Index::search` found.
pub struct Found<'a> {
    pub module: &'a Module,
    pub declaration: &'a Declaration,
    /// The declarations it's in, outermost first.
    pub parents: Vec<&'a Declaration>,
}

impl Found<'_> {
    /// What it's in: its module's name, and the names of the declarations
    /// around it, with `.` between them (`std.string.String`). An `impl`
    /// block counts as its type.
    pub fn container(&self) -> String {
        let mut out = self.module.name.clone();

        for p in &self.parents {
            out.push('.');
            out.push_str(p.owner());
        }

        out
    }
}

/// Every `.inko` file under `dir`, recursively.
fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();

        match entry.file_type() {
            Ok(t) if t.is_dir() => sources(&path, out),
            Ok(_) if path.extension().is_some_and(|e| e == "inko") => out.push(path),
            _ => {}
        }
    }
}

/// How well a name matches a query: lower is better. Compared as a
/// tuple: how it matches (the whole name, its start, a run of it, or
/// its characters in order), then whether its case differs from the
/// query's, then where the match starts, then how long the name is.
pub type Score = (u8, bool, usize, usize);

/// Whether `name` matches `query` as clients filter symbols: the query's
/// characters appear in the name in the same order, ignoring case (`stou`
/// matches `Stdout`). An empty query matches everything. `None` if it
/// doesn't match.
pub fn matches(query: &str, name: &str) -> Option<Score> {
    if query.is_ascii() && name.is_ascii() {
        return matches_ascii(query, name);
    }

    matches_unicode(query, name)
}

/// `matches`, lowercasing copies of the text.
fn matches_unicode(query: &str, name: &str) -> Option<Score> {
    let lower = |s: &str| s.to_lowercase();
    let (q, n) = (lower(query), lower(name));
    let other_case = !name.contains(query);

    if q.is_empty() {
        return Some((0, false, 0, name.len()));
    }

    if n == q {
        return Some((0, other_case, 0, name.len()));
    }

    if n.starts_with(&q) {
        return Some((1, other_case, 0, name.len()));
    }

    if let Some(at) = n.find(&q) {
        return Some((2, other_case, at, name.len()));
    }

    let mut chars = n.char_indices();
    let mut first = None;

    for c in q.chars() {
        let (at, _) = chars.find(|(_, d)| *d == c)?;

        first.get_or_insert(at);
    }

    Some((3, other_case, first.unwrap_or(0), name.len()))
}

/// `matches` for ASCII text, without lowercasing copies of it: the same
/// score, by comparing bytes without their case. Workspace symbols match
/// every declaration on each keystroke.
fn matches_ascii(query: &str, name: &str) -> Option<Score> {
    let (q, n) = (query.as_bytes(), name.as_bytes());
    let other_case = !name.contains(query);

    if q.is_empty() {
        return Some((0, false, 0, name.len()));
    }

    if n.eq_ignore_ascii_case(q) {
        return Some((0, other_case, 0, name.len()));
    }

    if n.len() >= q.len() && n[..q.len()].eq_ignore_ascii_case(q) {
        return Some((1, other_case, 0, name.len()));
    }

    if let Some(at) = n.windows(q.len()).position(|w| w.eq_ignore_ascii_case(q)) {
        return Some((2, other_case, at, name.len()));
    }

    let mut chars = n.iter().enumerate();
    let mut first = None;

    for c in q {
        let (at, _) = chars.find(|(_, d)| d.eq_ignore_ascii_case(c))?;

        first.get_or_insert(at);
    }

    Some((3, other_case, first.unwrap_or(0), name.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refresh_reads_only_what_changed() {
        let dir = std::env::temp_dir().join(format!("cuttlebone-index-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        std::fs::create_dir_all(dir.join("src/app")).unwrap();
        std::fs::write(dir.join("src/app/util.inko"), "fn pub help {}\n").unwrap();
        std::fs::write(dir.join("src/main.inko"), "fn main {}\n").unwrap();

        let dir = dir.canonicalize().unwrap();
        let roots = roots(&dir.join("nothing"), std::slice::from_ref(&dir));
        let mut index = Index::default();
        let names = |index: &Index| -> Vec<String> {
            index.modules().iter().map(|m| m.name.clone()).collect()
        };

        index.refresh(&roots, &HashMap::new());
        assert_eq!(index.parsed, 2);
        assert_eq!(names(&index), ["app.util", "main"]);

        index.refresh(&roots, &HashMap::new());
        assert_eq!(index.parsed, 0);

        // An open document's text, then the file's again.
        let open = HashMap::from([(dir.join("src/main.inko"), "fn other {}\n".to_string())]);

        index.refresh(&roots, &open);
        assert_eq!(index.parsed, 1);
        assert_eq!(index.search("other", 10).len(), 1);

        index.refresh(&roots, &HashMap::new());
        assert_eq!(index.parsed, 1);
        assert_eq!(index.search("other", 10).len(), 0);

        std::fs::remove_file(dir.join("src/app/util.inko")).unwrap();
        index.refresh(&roots, &HashMap::new());
        assert_eq!(names(&index), ["main"]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ascii_names_match_as_any_others() {
        let names = [
            "Stdout",
            "stdout",
            "to_string",
            "ToString",
            "STDOUT",
            "s",
            "",
            "OutputStream",
        ];
        let queries = [
            "", "s", "S", "std", "STD", "out", "Out", "ts", "tst", "xyz", "Stdout", "stdoutx",
        ];

        for n in names {
            for q in queries {
                assert_eq!(
                    matches_ascii(q, n),
                    matches_unicode(q, n),
                    "{:?} in {:?}",
                    q,
                    n
                );
            }
        }
    }

    #[test]
    fn names_match_queries_as_clients_filter_them() {
        assert!(matches("stou", "Stdout").is_some());
        assert!(matches("STDOUT", "Stdout").is_some());
        assert!(matches("tuo", "Stdout").is_none());
        assert!(matches("", "anything").is_some());

        // Exactly, then at the start, then a run, then in order.
        let mut names = vec!["map_values", "Map", "to_map", "mmap", "make_a_pie"];

        names.sort_by_key(|n| matches("map", n).unwrap());
        assert_eq!(names, ["Map", "map_values", "mmap", "to_map", "make_a_pie"]);
    }
}
