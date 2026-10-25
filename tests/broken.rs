//! What Cuttlebone can answer for code with errors: the cases SPIKE.md
//! measured, on tests/fixtures/broken. Each broken version is sent as
//! unsaved contents, so the file on disk stays valid.

use crate::common::{PLACEHOLDER, Project};
use cuttlebone::check::Target;
use cuttlebone::dump::{Status, check, upstream_std};
use cuttlebone::query::{self, Source};
use cuttlebone::walker::Origin;

const MAIN: &str = "src/main.inko";

/// The fixture's text with `from` replaced by `to`.
fn broken(from: &str, to: &str) -> String {
    let text = Project::new("broken").read(MAIN);

    assert!(text.contains(from), "{:?} isn't in the fixture", from);
    text.replacen(from, to, 1)
}

/// Every expression in the fixture (as `text`), with whether it has a type.
fn statuses(text: Option<&str>) -> Vec<(&'static str, Origin, bool)> {
    let file = crate::common::fixture("broken").join(MAIN);
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
fn an_undefined_type_in_a_signature_stops_inko_check_but_not_queries() {
    let text = broken("-> String {", "-> Strin {");
    let all = statuses(Some(&text));

    // As `inko check` runs them (and dump-types), the pass chain stops
    // before method bodies, so the only "types" left are the ones
    // Cuttlebone would reconstruct itself.
    assert!(
        all.iter()
            .filter(|(_, o, _)| *o == Origin::Stored)
            .all(|(_, _, typed)| !typed)
    );
    assert!(
        all.iter()
            .any(|(_, o, typed)| *o != Origin::Stored && *typed)
    );

    // Queries run every pass, and answer in what doesn't use `describe`.
    let p = Project::new("broken");

    for (needle, typ) in [("values.size", "ref Array[Int]"), ("loop {", "Nil")] {
        let found = p.type_at(MAIN, Some(&text), needle, 0).unwrap().unwrap();

        assert_eq!(found.typ, typ, "at {:?}", needle);
    }

    for needle in ["Stdout.new", "'count: "] {
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

/// A request at `needle` (plus `offset`) in navigation's `file`, with
/// `broken`'s text changed from `from` to `to` (each file must contain it).
fn navigation(
    file: &str,
    needle: &str,
    offset: u32,
    broken: &str,
    from: &str,
    to: &str,
) -> (Project, cuttlebone::query::Request) {
    let p = Project::new("navigation");
    let text = p.read(broken);

    assert!(text.contains(from), "{:?} isn't in {}", from, broken);

    let mut request = p.request(file, None, needle, offset);

    request
        .contents
        .insert(p.path(broken), text.replacen(from, to, 1));
    (p, request)
}

#[test]
fn a_declaration_error_in_an_import_leaves_out_only_what_uses_it() {
    let broken = (
        "src/alpha.inko",
        "fn pub message -> String",
        "fn pub message -> Strin",
    );
    let ask = |needle: &str, offset: u32| {
        let (p, request) = navigation(MAIN, needle, offset, broken.0, broken.1, broken.2);

        query::type_at(&p.session, &request)
    };

    // `moved` uses nothing of alpha's.
    let answer = ask("@x + by", 0);

    assert_eq!(answer.result.unwrap().unwrap().typ, "Int");
    assert_eq!(answer.source, Source::Guarded);
    // The check of the documents as they are isn't a good check for later
    // answers in code being edited.
    assert!(!answer.typed);

    // `main` calls `message`, alpha.Error's method with the error.
    let answer = ask("error.message", 0);

    assert_eq!(answer.result.unwrap_err().code, "declarationErrors");
}

#[test]
fn references_are_given_only_where_no_declaration_error_could_hide_one() {
    let references = |file: &str, needle: &str, broken: (&str, &str, &str)| {
        let (p, request) = navigation(file, needle, 0, broken.0, broken.1, broken.2);

        query::references(&p.session, &request, true)
    };
    // An error in a module that imports the file: `total`'s return type.
    let total = ("src/main.inko", "-> Float {", "-> Floa {");
    let answer = references("src/geometry/shapes.inko", "radius * @radius", total);
    let found = answer.result.unwrap().unwrap();

    assert_eq!(found.len(), 4, "{:?}", found);
    assert_eq!(answer.source, Source::Guarded);

    // `total`, with the error, uses `area`, and `main`, which calls `total`,
    // uses `ORIGIN`.
    for needle in ["area -> Float\n}", "ORIGIN = 0"] {
        let answer = references("src/geometry/shapes.inko", needle, total);

        assert_eq!(
            answer.result.unwrap_err().code,
            "declarationErrors",
            "at {:?}",
            needle
        );
    }

    // An error in a module that neither imports the file nor is imported.
    let beta = (
        "src/beta.inko",
        "fn pub message -> String",
        "fn pub message -> Strin",
    );
    let answer = references("src/qualified.inko", "origin -> Int", beta);

    assert_eq!(answer.result.unwrap().unwrap().len(), 1);
    // `main` (which uses `message`) refers to `ORIGIN` too.
    let answer = references("src/qualified.inko", "ORIGIN\n", beta);

    assert_eq!(answer.result.unwrap_err().code, "declarationErrors");
}

#[test]
fn an_operator_used_where_a_declaration_error_matters_has_no_highlights() {
    // `main` calls `describe`, whose return type has the error, and adds
    // `Int`s with `+`.
    let (p, request) = navigation(
        MAIN,
        "@x + by",
        3,
        MAIN,
        "-> Int {\n  match",
        "-> In {\n  match",
    );
    let answer = query::highlights(&p.session, &request);

    assert_eq!(answer.result.unwrap_err().code, "declarationErrors");

    // `moved`'s argument `by` is used only in `moved`, which is safe.
    let (p, request) = navigation(
        MAIN,
        "by: Int) -> Point",
        0,
        MAIN,
        "-> Int {\n  match",
        "-> In {\n  match",
    );
    let found = query::highlights(&p.session, &request)
        .result
        .unwrap()
        .unwrap();

    assert_eq!(found.len(), 2);
}

#[test]
fn a_panic_in_a_pass_after_a_declaration_error_stops_where_inko_check_does() {
    // The compiler requires std.option's `Option` (to insert the
    // prelude), and panics in a later pass if it's renamed.
    let std = upstream_std();
    let file = std.join("std/option.inko");
    let text = std::fs::read_to_string(&file).unwrap();
    let session = query::Session {
        std: std.clone(),
        projects: vec![std.parent().unwrap().to_path_buf()],
    };
    let edited = text.replacen(
        "type pub inline enum Option[T]",
        "type pub inline enum Optio[T]",
        1,
    );
    let request = query::Request {
        file: file.clone(),
        position: crate::common::position_of(&edited, "case Some(T)", 5),
        encoding: cuttlebone::position::Encoding::Utf16,
        contents: [(file, edited)].into_iter().collect(),
    };
    let answer = query::type_at(&session, &request);

    // Checked again, stopping at the error: no method body is typed, as
    // before, so there's no answer, but not a panic.
    assert_eq!(answer.result.unwrap_err().code, "declarationErrors");
}

#[test]
fn every_pass_runs_after_each_kind_of_error_without_a_panic() {
    // Between them, these produce every diagnostic `inko check` can
    // (tests/diagnostics.rs), from each pass that reports any.
    let fixtures = crate::common::fixture("diagnostics");
    let mut checked = 0;

    for entry in std::fs::read_dir(&fixtures).unwrap() {
        let root = entry.unwrap().path();
        let target = Target::new(&root, upstream_std()).unwrap();
        let run = std::panic::catch_unwind(|| {
            cuttlebone::check::run_after_errors(&target, |_, modules, errors| {
                (
                    cuttlebone::walker::expressions_pass_ran(modules),
                    errors.len(),
                )
            })
        })
        .unwrap_or_else(|_| panic!("{} panicked", root.display()))
        .unwrap();

        // A project that doesn't parse has no HIR to type.
        if let Some((typed, errors)) = run.inspected {
            assert!(typed, "{}: no method body was typed", root.display());
            // Running on after an error doesn't make the check succeed.
            assert_eq!(run.ok, errors == 0, "{}", root.display());
            checked += 1;
        }
    }

    assert_eq!(checked, 11);
}

#[test]
fn what_an_error_left_undefined_is_skipped_by_later_passes() {
    let file = crate::common::fixture("broken").join(MAIN);
    let text = std::fs::read_to_string(&file).unwrap();

    // Each panicked in a pass after the error, before the patch skipped
    // it (patches/README.md): an `impl` of an undefined trait (in two
    // passes), a required trait that's undefined, and a method in an
    // `impl` that its trait doesn't have.
    for added in [
        "impl Missing for Main {}\n",
        "trait Shaped: Missing {}\n",
        "trait Sized {}\n\nimpl Sized for Main {\n  fn extra {}\n}\n",
    ] {
        let mut target = Target::new(&file, upstream_std()).unwrap();

        target
            .overlay
            .insert(file.clone(), format!("{}\n{}", text, added));

        let run = std::panic::catch_unwind(|| {
            cuttlebone::check::run_after_errors(&target, |_, modules, errors| {
                (
                    cuttlebone::walker::expressions_pass_ran(modules),
                    errors.len(),
                )
            })
        })
        .unwrap_or_else(|_| panic!("panicked with {:?}", added))
        .unwrap();

        assert_eq!(run.inspected, Some((true, 1)), "with {:?}", added);
    }
}

#[test]
fn what_changed_in_declarations_since_the_last_good_check_counts_too() {
    // A field's type changed since the last good check (no error there:
    // the uses of the field have them), and then a declaration got an
    // error, elsewhere.
    let p = Project::new("navigation");
    let old = p.read(MAIN);
    let retyped = old.replacen("let pub @x: Int", "let pub @x: Float", 1);
    let new = retyped.replacen("-> Int {\n  match", "-> In {\n  match", 1);
    let mut changes = cuttlebone::edits::Changes::none(old.len());

    changes.replace_text(&old, &retyped);
    changes.replace_text(&retyped, &new);

    let fallback = cuttlebone::edits::Fallback {
        contents: [(p.path(MAIN), old.clone())].into_iter().collect(),
        changes: [(p.path(MAIN), changes)].into_iter().collect(),
    };
    let request = p.request(MAIN, Some(&new), "Point(x: x", 0);
    let ask = |fallback| {
        query::ask_with(
            &query::Cache::new(0),
            &p.session,
            &request,
            fallback,
            false,
            &[(query::Question::Type, request.position)],
        )
        .pop()
        .unwrap()
    };

    // `Point.new` mentions `x`, whose type isn't what it was, and may be
    // again once the error is fixed.
    assert_eq!(
        ask(Some(&fallback)).result.unwrap_err().code,
        "declarationErrors"
    );

    // Without the last good check, only the error says what may change.
    let answer = ask(None);

    assert_eq!(answer.source, Source::Guarded);
    assert!(answer.result.unwrap().is_some());
}

#[test]
fn an_error_in_a_body_changes_nothing_else_while_a_declaration_has_one() {
    let p = Project::new("navigation");
    let text = p
        .read(MAIN)
        .replacen("-> Int {\n  match", "-> In {\n  match", 1)
        .replacen("@x + by", "@x + byy", 1);
    let answer = query::type_at(&p.session, &p.request(MAIN, Some(&text), "Point(x: x", 0));

    // `new` is `moved`'s neighbour in `Point`, and mentions `Point`.
    assert_eq!(answer.source, Source::Guarded);
    assert_eq!(answer.result.unwrap().unwrap().typ, "Point");
    // `moved` itself has an expression the check couldn't type.
    let answer = query::type_at(&p.session, &p.request(MAIN, Some(&text), "@x + byy", 0));

    assert_eq!(answer.result.unwrap_err().code, "declarationErrors");
}

/// What a declaration's error affects is decided by the compiler's ids,
/// where names have them: an error in `A.size`'s signature leaves out the
/// units that call `A.size` (or name `A`), or a module's `measure` with an
/// error, but not one that only calls
/// `B.size`, nor `B.size` itself, though they say `size` too. The names
/// in the types of a unit's expressions have no ids, so they still count:
/// a value of type `A`.
#[test]
fn a_declaration_error_affects_its_own_symbols_not_others_with_the_same_name() {
    let root = crate::common::tmp().join("broken-same-name");
    let _ = std::fs::remove_dir_all(&root);
    let util = "type pub A {\n  fn pub size(unit: Missing) -> Int {\n    1\n  }\n}\n\n\
                type pub B {\n  fn pub size(unit: Float) -> Int {\n    2\n  }\n}\n\n\
                fn pub make -> A {\n  A()\n}\n\n\
                fn pub measure(unit: Missing) -> Int {\n  3\n}\n";
    let main = "import util (A, B, make, measure)\n\nfn run(x: B) -> Int {\n  let total = x.size(2.0)\n  total\n}\n\n\
                fn twice(a: A) -> Int {\n  let both = a.size(1)\n  both\n}\n\n\
                fn built {\n  let made = make\n}\n\n\
                fn direct -> Int {\n  let measured = measure(1)\n  measured\n}\n";

    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/util.inko"), util).unwrap();
    std::fs::write(root.join("src/main.inko"), main).unwrap();

    let root = root.canonicalize().unwrap();
    let session = query::Session {
        std: upstream_std(),
        projects: vec![root.clone()],
    };
    let ask = |file: &str, text: &str, needle: &str, offset: u32| {
        let path = root.join(file);
        let request = query::Request {
            file: path.clone(),
            position: crate::common::position_of(text, needle, offset),
            encoding: cuttlebone::position::Encoding::Utf16,
            contents: std::collections::HashMap::new(),
        };

        query::type_at(&session, &request)
    };

    // `run` calls B's `size`, which has no error.
    let answer = ask("src/main.inko", main, "let total", 4);

    assert_eq!(answer.result.unwrap().unwrap().typ, "Int");
    assert_eq!(answer.source, Source::Guarded);
    // So is `B.size` itself.
    let answer = ask("src/util.inko", util, "    2\n", 4);

    assert_eq!(answer.result.unwrap().unwrap().typ, "Int");

    // `twice` calls A's.
    let answer = ask("src/main.inko", main, "let both", 4);

    assert_eq!(answer.result.unwrap_err().code, "declarationErrors");

    // `direct` calls `measure`, a module's method with an error, which
    // only its id says.
    let answer = ask("src/main.inko", main, "let measured", 4);

    assert_eq!(answer.result.unwrap_err().code, "declarationErrors");

    // `built` has a value of type `A`.
    let answer = ask("src/main.inko", main, "let made", 4);

    assert_eq!(answer.result.unwrap_err().code, "declarationErrors");
}
