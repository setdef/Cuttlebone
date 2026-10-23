//! What Cuttlebone can answer for code with errors: the cases SPIKE.md
//! measured, on tests/fixtures/broken. Each broken version is sent as
//! unsaved contents, so the file on disk stays valid.
mod common;

use common::{PLACEHOLDER, Project};
use cuttlebone::{Origin, Status, Target, check, upstream_std};

const MAIN: &str = "src/main.inko";

/// The fixture's text with `from` replaced by `to`.
fn broken(from: &str, to: &str) -> String {
    let text = Project::new("broken").read(MAIN);

    assert!(text.contains(from), "{:?} isn't in the fixture", from);
    text.replacen(from, to, 1)
}

/// Every expression in the fixture (as `text`), with whether it has a type.
fn statuses(text: Option<&str>) -> Vec<(&'static str, Origin, bool)> {
    let file = common::fixture("broken").join(MAIN);
    let mut target = Target::new(&file, upstream_std()).unwrap();

    if let Some(text) = text {
        target.overlay.insert(file, text.to_string());
    }

    check(&target)
        .unwrap()
        .expressions
        .iter()
        .map(|e| (e.kind, e.origin, e.status() == Status::Resolved))
        .collect()
}

#[test]
fn without_errors_every_expression_has_a_type() {
    let all = statuses(None);

    assert_eq!(all.len(), 72);
    assert!(all.iter().all(|(_, _, typed)| *typed));

    let p = Project::new("broken");

    assert_eq!(p.type_of(MAIN, "values.size", 0), "ref Array[Int]");
}

#[test]
fn an_unknown_method_in_a_body_only_loses_that_call() {
    let text = broken("${values.size}", "${values.length}");
    let all = statuses(Some(&text));
    let untyped: Vec<_> = all.iter().filter(|(_, _, typed)| !typed).collect();

    // The call itself, and the `to_string` that interpolation adds around
    // it. The `$buf.push(...)` around that still returns Nil.
    assert_eq!(untyped.len(), 2, "{:?}", untyped);

    let p = Project::new("broken");

    assert_eq!(
        p.type_at(MAIN, Some(&text), "values.length", 7)
            .unwrap_err()
            .code,
        "untyped"
    );
    // Its receiver, and the rest of the file, still have types.
    assert_eq!(
        p.type_at(MAIN, Some(&text), "values.length", 0)
            .unwrap()
            .unwrap()
            .typ,
        "ref Array[Int]"
    );
    assert_eq!(
        p.type_at(MAIN, Some(&text), "sum += ", 4)
            .unwrap()
            .unwrap()
            .kind,
        "AssignVariable"
    );
}

#[test]
fn an_undefined_type_in_a_signature_leaves_no_body_typed() {
    let text = broken("-> String {", "-> Strin {");
    let all = statuses(Some(&text));

    // The pass chain stops before method bodies, so the only "types" left
    // are the ones Cuttlebone would reconstruct itself, which is why
    // queries refuse to answer in this state.
    assert!(
        all.iter()
            .filter(|(_, o, _)| *o == Origin::Stored)
            .all(|(_, _, typed)| !typed)
    );
    assert!(
        all.iter()
            .any(|(_, o, typed)| *o != Origin::Stored && *typed)
    );

    let p = Project::new("broken");

    for needle in ["values.size", "break", "Stdout.new", "loop {"] {
        let err = p.type_at(MAIN, Some(&text), needle, 0).unwrap_err();

        assert_eq!(err.code, "declarationErrors", "at {:?}", needle);
    }
}

#[test]
fn a_syntax_error_leaves_nothing_typed() {
    let text = broken("let mut i = 0", "let mut i = )");

    assert!(statuses(Some(&text)).is_empty());

    let p = Project::new("broken");
    let err = p.type_at(MAIN, Some(&text), "values.size", 0).unwrap_err();

    assert_eq!(err.code, "noHir");
}

#[test]
fn a_syntax_error_in_an_imported_module_leaves_nothing_typed() {
    let p = Project::new("members");
    let mut request = p.request("src/main.inko", None, "name.size", 0);

    request
        .contents
        .insert(p.path("src/shapes.inko"), "fn pub broken(".to_string());

    let err = cuttlebone::query::type_at(&p.session, &request)
        .result
        .unwrap_err();

    assert_eq!(err.code, "noHir");
}

#[test]
fn the_completion_placeholder_only_loses_its_own_call() {
    let placeholder = format!("values.{}", PLACEHOLDER);
    let text = broken("${values.size}", &format!("${{{}}}", placeholder));
    let all = statuses(Some(&text));

    assert_eq!(all.iter().filter(|(_, _, typed)| !typed).count(), 2);

    let p = Project::new("broken");
    let m = p
        .members_at(MAIN, "${values.size}", &format!("${{{}}}", placeholder))
        .unwrap()
        .unwrap();

    assert_eq!(m.receiver, "ref Array[Int]");
    assert!(m.members.iter().any(|m| m.name == "size"));
}
