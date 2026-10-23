//! Unsaved buffers: a request's `contents` replace files' text on disk, for
//! the file asked about and for the modules it imports.
mod common;

use common::Project;
use cuttlebone::query;
use std::path::PathBuf;

const MAIN: &str = "src/main.inko";

#[test]
fn contents_replace_the_file_on_disk() {
    let p = Project::new("members");
    let text = p
        .read(MAIN)
        .replacen("let name = 'héllo'", "let name = 42", 1);

    assert_eq!(
        p.type_at(MAIN, Some(&text), "let name", 4)
            .unwrap()
            .unwrap()
            .typ,
        "Int"
    );
    // The next request, without contents, sees the file on disk again.
    assert_eq!(p.type_of(MAIN, "let name", 4), "String");
}

#[test]
fn contents_of_an_imported_module_change_the_importing_files_types() {
    let p = Project::new("members");
    let shapes = p.read("src/shapes.inko").replace(
        "fn pub area -> Float {",
        "fn pub area -> Int {\n    1\n  }\n\n  fn pub old_area -> Float {",
    );
    let mut request = p.request(MAIN, None, "circle.area", 7);

    request.contents.insert(p.path("src/shapes.inko"), shapes);

    let t = query::type_at(&p.session, &request)
        .result
        .unwrap()
        .unwrap();

    assert_eq!(t.typ, "Int");
    assert_eq!(p.type_of(MAIN, "circle.area", 7), "Float");
}

#[test]
fn contents_for_a_file_that_doesnt_exist_are_refused() {
    let p = Project::new("members");
    let mut request = p.request(MAIN, None, "let name", 4);

    request
        .contents
        .insert(PathBuf::from("/does/not/exist.inko"), String::new());

    let err = query::type_at(&p.session, &request).result.unwrap_err();

    assert_eq!(err.code, "fileNotFound");
}

#[test]
fn a_file_that_doesnt_exist_is_refused() {
    let p = Project::new("members");
    let mut request = p.request(MAIN, None, "let name", 4);

    request.file = p.path("src/missing.inko");

    assert_eq!(
        query::type_at(&p.session, &request)
            .result
            .unwrap_err()
            .code,
        "fileNotFound"
    );
}

#[test]
fn contents_paths_may_be_given_in_any_form_that_names_the_file() {
    // Paths are canonicalized, so `src/../src/main.inko` names the same file.
    let p = Project::new("members");
    let mut request = p.request(MAIN, None, "let name", 4);
    let text = p
        .read(MAIN)
        .replacen("let name = 'héllo'", "let name = 42", 1);

    request
        .contents
        .insert(p.root.join("src/../src/main.inko"), text);

    assert_eq!(
        query::type_at(&p.session, &request)
            .result
            .unwrap()
            .unwrap()
            .typ,
        "Int"
    );
}

#[test]
fn the_projects_manifest_is_read_from_its_root() {
    // The compiler reads inko.pkg from the working directory, so checks run
    // in the project's root. This test's working directory is the
    // repository, which has no inko.pkg.
    let root = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("requires-newer-inko");

    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("inko.pkg"), "require inko 99.0.0\n").unwrap();
    std::fs::write(root.join("src/main.inko"), "let A = 1\n").unwrap();

    let session = query::Session {
        std: cuttlebone::upstream_std(),
        project: Some(root.canonicalize().unwrap()),
    };
    let request = query::Request {
        file: root.join("src/main.inko"),
        position: cuttlebone::position::Position {
            line: 0,
            character: 8,
        },
        contents: Default::default(),
    };
    let err = query::type_at(&session, &request).result.unwrap_err();

    assert_eq!(err.code, "compilerError");
    assert!(
        err.message.contains("requires Inko 99.0.0"),
        "{}",
        err.message
    );
    assert!(!std::env::current_dir().unwrap().starts_with(&root));
}
