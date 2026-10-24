//! Queries enforce the Inko version a project's manifests require, as
//! `inko check` does, but never sync `dep/` (tests/lsp.rs checks that over
//! stdio, with the data directory and working directory a real editor
//! would give).

use cuttlebone::position::{Encoding, Position};
use cuttlebone::query::{self, Request, Session, Unavailable};
use std::path::{Path, PathBuf};

/// A project in a fresh temporary directory, with `manifest` as its
/// inko.pkg, and `dep/` manifests as given.
fn project(name: &str, manifest: &str, deps: &[(&str, &str)]) -> PathBuf {
    let root = crate::common::tmp().join(name);
    let _ = std::fs::remove_dir_all(&root);

    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("inko.pkg"), manifest).unwrap();
    std::fs::write(root.join("src/main.inko"), "let A = 1\n").unwrap();

    for (dir, text) in deps {
        let dir = root.join("dep").join(dir);

        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("inko.pkg"), text).unwrap();
    }

    root.canonicalize().unwrap()
}

fn type_of_a(root: &Path) -> Result<Option<String>, Unavailable> {
    let session = Session {
        std: cuttlebone::dump::upstream_std(),
        projects: vec![root.to_path_buf()],
    };
    let request = Request {
        file: root.join("src/main.inko"),
        position: Position {
            line: 0,
            character: 8,
        },
        encoding: Encoding::Utf16,
        contents: Default::default(),
    };

    query::type_at(&session, &request)
        .result
        .map(|t| t.map(|t| t.typ))
}

#[test]
fn a_project_that_requires_a_newer_inko_is_refused() {
    let root = project("requires-newer", "require inko 99.0.0\n", &[]);
    let err = type_of_a(&root).unwrap_err();

    assert_eq!(err.code, "compilerError");
    assert_eq!(
        err.message,
        "this project requires Inko 99.0.0 or newer, but the current version is 0.21.1"
    );
}

/// Diagnostics refuse it the same way, rather than report errors from a
/// standard library the project doesn't support.
#[test]
fn diagnostics_refuse_a_project_that_requires_a_newer_inko() {
    let root = project("requires-newer-diagnostics", "require inko 99.0.0\n", &[]);
    let session = cuttlebone::query::Session {
        std: cuttlebone::dump::upstream_std(),
        projects: vec![root.clone()],
    };
    let request = cuttlebone::query::Request {
        file: root,
        position: cuttlebone::position::Position {
            line: 0,
            character: 0,
        },
        encoding: cuttlebone::position::Encoding::Utf16,
        contents: Default::default(),
    };
    let err = cuttlebone::query::diagnostics(&session, &request)
        .result
        .unwrap_err();

    assert_eq!(err.code, "compilerError");
    assert!(
        err.message.contains("requires Inko 99.0.0"),
        "{}",
        err.message
    );
}

#[test]
fn a_dependency_that_requires_a_newer_inko_is_refused() {
    let root = project(
        "dependency-requires-newer",
        "",
        &[("abc/v1", "require inko 0.22.0\n")],
    );
    let err = type_of_a(&root).unwrap_err();

    assert!(
        err.message.contains("requires Inko 0.22.0"),
        "{}",
        err.message
    );
}

#[test]
fn a_project_that_requires_this_inko_or_older_is_checked() {
    let root = project(
        "requires-older",
        "require inko 0.21.1\n",
        &[("abc/v1", "require inko 0.1.0\n")],
    );

    assert_eq!(type_of_a(&root), Ok(Some("Int".to_string())));
}

#[test]
fn an_invalid_manifest_is_reported() {
    let root = project("invalid-manifest", "frobnicate\n", &[]);
    let err = type_of_a(&root).unwrap_err();

    assert_eq!(err.code, "compilerError");
    assert!(
        err.message.contains("failed to load the manifest"),
        "{}",
        err.message
    );
    assert!(err.message.contains("inko.pkg"), "{}", err.message);
}

#[test]
fn a_dependency_that_was_never_synced_isnt_synced_by_a_query() {
    // `inko check` would sync dep/ here, because there's no dep/hash. A
    // query must not: the URL doesn't exist, so a sync would fail, and on
    // success it would have created dep/.
    let root = project(
        "never-synced",
        "require file:///does/not/exist/package 1.0.0 0123456789012345678901234567890123456789\n",
        &[],
    );

    assert_eq!(type_of_a(&root), Ok(Some("Int".to_string())));
    assert!(!root.join("dep").exists());
}
