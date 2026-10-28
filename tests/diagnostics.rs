//! The oracle for diagnostics: for every fixture project in
//! tests/fixtures/diagnostics, and for std, Cuttlebone's diagnostics must be
//! exactly what the installed `inko check --format json` reports, in the
//! same order: file, lines, columns, level, ID and message.
//!
//! This needs the installed `inko` to be the pinned release, and fails if it
//! isn't: the oracle is the point.

use crate::common::fixture;
use cuttlebone::check::upstream_version;
use cuttlebone::dump::upstream_std;
use cuttlebone::position::{Encoding, Position};
use cuttlebone::query::{self, Request, Session};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Every diagnostic ID the compiler has (compiler/src/diagnostics.rs:25-50),
/// except `missing-main`, which only `inko build` reports: `inko check`
/// doesn't define a main module.
const IDS: &[&str] = &[
    "borrow-value-type",
    "duplicate-symbol",
    "invalid-assign",
    "invalid-call",
    "invalid-cast",
    "invalid-const-expr",
    "invalid-field",
    "invalid-file",
    "invalid-implementation",
    "invalid-loop-keyword",
    "invalid-match",
    "invalid-method",
    "invalid-pattern",
    "invalid-symbol",
    "invalid-syntax",
    "invalid-throw",
    "invalid-type",
    "missing-field",
    "missing-trait",
    "moved",
    "unreachable",
    "unused-result",
    "unused-symbol",
];

/// The installed `inko`, which must be the pinned release.
fn inko() -> &'static str {
    let out = Command::new("inko")
        .arg("--version")
        .output()
        .expect("the diagnostics oracle needs `inko` on PATH");
    let version = String::from_utf8_lossy(&out.stdout);

    assert_eq!(
        version.trim(),
        format!("inko {}", upstream_version()),
        "the diagnostics oracle needs the installed inko to be the pinned release"
    );
    "inko"
}

/// The installed standard library, which `inko check` uses.
fn installed_std() -> PathBuf {
    let out = Command::new(inko())
        .args(["print", "std"])
        .output()
        .unwrap();

    PathBuf::from(String::from_utf8_lossy(&out.stdout).trim())
        .canonicalize()
        .unwrap()
}

/// What `inko check --format json` reports for the project in `dir`. It
/// creates and locks a directory in the user's data directory, so it runs
/// with a home directory of its own.
fn inko_check(dir: &Path) -> Vec<Value> {
    let home = crate::common::tmp().join("oracle-home");

    std::fs::create_dir_all(&home).unwrap();

    let out = Command::new(inko())
        .args(["check", "--format", "json"])
        .current_dir(dir)
        .env("HOME", &home)
        .env("XDG_DATA_HOME", home.join("data"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let list: Vec<Value> = serde_json::from_str(stderr.trim())
        .unwrap_or_else(|e| panic!("inko check printed {:?}: {}", stderr, e));

    list.into_iter()
        .map(|mut d| {
            let file = PathBuf::from(d["file"].as_str().unwrap());

            d["file"] = json!(file.canonicalize().unwrap_or(file));
            d
        })
        .collect()
}

/// Cuttlebone's diagnostics for the project in `dir`, in the shape of
/// `inko check`'s.
fn ours(dir: &Path, std: &Path, contents: HashMap<PathBuf, String>) -> Vec<Value> {
    let session = Session {
        std: std.to_path_buf(),
        projects: vec![dir.to_path_buf()],
    };
    let request = Request {
        file: dir.to_path_buf(),
        position: Position {
            line: 0,
            character: 0,
        },
        encoding: Encoding::Utf16,
        contents,
    };
    let answer = query::diagnostics(&session, &request);
    let result = answer.result.unwrap().unwrap();

    assert_eq!(result.incomplete, None);
    result
        .diagnostics
        .into_iter()
        .map(|d| {
            json!({
                "id": d.id,
                "level": if d.error { "error" } else { "warning" },
                "file": d.file,
                "lines": d.lines,
                "columns": d.columns,
                "message": d.message,
            })
        })
        .collect()
}

/// Every `.inko` file under a directory.
fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten() {
        let path = entry.unwrap().path();

        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "inko") {
            out.push(path);
        }
    }
}

/// What Cuttlebone must report for the project in `dir`: `inko check`'s
/// diagnostics, except where a module doesn't parse or imports one that
/// can't be found, which stops `inko check` before it checks any other.
/// Then the files with errors, and the modules that import them, directly
/// or not, have what `inko check` reports in them, and every other file
/// what it reports for a copy of the project without those files.
fn expected(dir: &Path) -> Vec<Value> {
    let theirs = inko_check(dir);
    let stopped = theirs.iter().any(|d| {
        d["level"] == "error" && (d["id"] == "invalid-syntax" || d["id"] == "invalid-file")
    });

    if !stopped {
        return theirs;
    }

    let dir = dir.canonicalize().unwrap();
    let mut files = Vec::new();

    for sub in ["src", "test"] {
        sources(&dir.join(sub), &mut files);
    }

    // Each module's name, and the modules its `import` lines name.
    let module = |file: &Path| -> String {
        let relative = file
            .strip_prefix(dir.join("src"))
            .or_else(|_| file.strip_prefix(dir.join("test")))
            .unwrap();

        relative
            .with_extension("")
            .to_str()
            .unwrap()
            .replace('/', ".")
    };
    let imports = |file: &Path| -> Vec<String> {
        std::fs::read_to_string(file)
            .unwrap()
            .lines()
            .filter_map(|l| l.strip_prefix("import "))
            .map(|l| l.split([' ', '(']).next().unwrap().to_string())
            .collect()
    };
    let mut left_out: Vec<PathBuf> = theirs
        .iter()
        .filter(|d| d["level"] == "error")
        .map(|d| PathBuf::from(d["file"].as_str().unwrap()))
        .collect();

    loop {
        let names: Vec<String> = left_out.iter().map(|f| module(f)).collect();
        let more: Vec<PathBuf> = files
            .iter()
            .filter(|f| !left_out.contains(f) && imports(f).iter().any(|i| names.contains(i)))
            .cloned()
            .collect();

        if more.is_empty() {
            break;
        }

        left_out.extend(more);
    }

    let copy = crate::common::tmp().join(format!(
        "oracle-without-{}",
        dir.file_name().unwrap().to_str().unwrap()
    ));

    let _ = std::fs::remove_dir_all(&copy);

    for file in files.iter().filter(|f| !left_out.contains(f)) {
        let to = copy.join(file.strip_prefix(&dir).unwrap());

        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(file, to).unwrap();
    }

    let mut out: Vec<Value> = theirs
        .into_iter()
        .filter(|d| left_out.contains(&PathBuf::from(d["file"].as_str().unwrap())))
        .collect();

    if copy.is_dir() {
        let copy = copy.canonicalize().unwrap();

        out.extend(inko_check(&copy).into_iter().map(|mut d| {
            let file = PathBuf::from(d["file"].as_str().unwrap());

            if let Ok(relative) = file.strip_prefix(&copy) {
                d["file"] = json!(dir.join(relative));
            }

            d
        }));
    }

    out
}

fn projects() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(fixture("diagnostics"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();

    dirs.sort();
    dirs
}

#[test]
fn every_fixture_project_matches_inko_check() {
    let std = installed_std();
    let mut ids = BTreeSet::new();
    let mut past = false;

    for dir in projects() {
        let theirs = expected(&dir);

        assert!(!theirs.is_empty(), "{}: no diagnostics", dir.display());
        assert_eq!(
            ours(&dir, &std, HashMap::new()),
            theirs,
            "{}: left is Cuttlebone, right is inko check",
            dir.display()
        );

        for d in theirs {
            past |= d["file"]
                .as_str()
                .unwrap()
                .ends_with("syntax/src/fine.inko");
            ids.insert(d["id"].as_str().unwrap().to_string());
        }
    }

    // A module that imports none that doesn't parse has its diagnostics,
    // which `inko check` of the whole project doesn't report.
    assert!(past);

    // The fixtures produce every diagnostic `inko check` can.
    assert_eq!(ids, IDS.iter().map(|s| s.to_string()).collect());
}

#[test]
fn std_matches_inko_check() {
    let std = installed_std();
    let dir = upstream_std().parent().unwrap().to_path_buf();

    assert!(dir.join("test").is_dir());
    assert_eq!(ours(&dir, &std, HashMap::new()), inko_check(&dir));
}

/// Unsaved text replaces the file on disk: Cuttlebone with an overlay must
/// match `inko check` on a copy with that text saved.
#[test]
fn unsaved_text_matches_inko_check_on_the_saved_text() {
    let std = installed_std();
    let dir = fixture("diagnostics/warnings");
    let copy = crate::common::tmp().join("oracle-unsaved");
    let file = "src/main.inko";
    let text = std::fs::read_to_string(dir.join(file)).unwrap().replace(
        "let unused = 10",
        "let unused = 10\n    let moved = [1]\n    let a = moved\n    let b = moved",
    );

    let _ = std::fs::remove_dir_all(&copy);
    std::fs::create_dir_all(copy.join("src")).unwrap();
    std::fs::write(copy.join(file), &text).unwrap();

    let copy = copy.canonicalize().unwrap();
    let relative = |list: Vec<Value>, root: &Path| -> Vec<Value> {
        list.into_iter()
            .map(|mut d| {
                let path = PathBuf::from(d["file"].as_str().unwrap());

                d["file"] = json!(path.strip_prefix(root).unwrap());
                d
            })
            .collect()
    };
    let theirs = relative(inko_check(&copy), &copy);
    let unsaved = relative(
        ours(&dir, &std, HashMap::from([(dir.join(file), text)])),
        &dir,
    );

    assert!(theirs.iter().any(|d| d["id"] == "moved"), "{:?}", theirs);
    assert_eq!(unsaved, theirs);
}
