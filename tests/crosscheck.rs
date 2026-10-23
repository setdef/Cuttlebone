//! The types Cuttlebone reconstructs (`Origin::Derived` and
//! `Origin::Symbol`) must agree with what the compiler stores elsewhere for
//! the same values: a call's receiver, a variable's type, or a scope's type.
//! See `CrossCheck` in src/walker.rs.
mod common;

use cuttlebone::walker::CrossCheck;
use cuttlebone::{Origin, Target, check, cross_checks, format_expr, upstream_std};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

fn checks(path: &Path) -> Vec<CrossCheck> {
    let (ok, checks) = cross_checks(&Target::new(path, upstream_std()).unwrap()).unwrap();

    assert!(ok, "{} has errors", path.display());
    checks
}

fn assert_agree(checks: &[CrossCheck]) {
    let wrong: Vec<_> = checks.iter().filter(|c| c.ours != c.compiler).collect();

    assert!(
        wrong.is_empty(),
        "{} of {} reconstructed types disagree with the compiler:\n{:#?}",
        wrong.len(),
        checks.len(),
        &wrong[..wrong.len().min(20)]
    );
}

fn counts(checks: &[CrossCheck]) -> BTreeMap<(&'static str, &'static str), usize> {
    let mut out = BTreeMap::new();

    for c in checks {
        *out.entry((c.kind, c.rule)).or_insert(0) += 1;
    }

    out
}

#[test]
fn the_fixture_cross_checks_every_kind_of_reconstructed_type() {
    let checks = checks(&common::fixture("crosscheck"));
    let counts = counts(&checks);

    assert_agree(&checks);

    // Derived: loop (Nil and Never), break, next and try (in a method
    // returning Result, one returning Option, and a closure whose return
    // type is inferred).
    for kind in ["Loop", "Break", "Next", "Try"] {
        assert!(
            counts.keys().any(|(k, _)| *k == kind),
            "nothing cross-checks {}: {:?}",
            kind,
            counts
        );
    }

    assert_eq!(checks.iter().filter(|c| c.kind == "Try").count(), 3);

    // Symbol: types and modules as receivers of static methods, module
    // methods, constants and type literals.
    let receivers: Vec<_> = checks
        .iter()
        .filter(|c| c.rule == "receiver")
        .map(|c| c.ours.as_str())
        .collect();

    for expected in [
        "Point",
        "geometry",
        "std.fmt",
        "Option[T]",
        "Result[T, E]",
        "Stdout",
    ] {
        assert!(
            receivers.contains(&expected),
            "no receiver {}: {:?}",
            expected,
            receivers
        );
    }
}

#[test]
fn a_break_inside_a_closure_makes_the_enclosing_loop_nil() {
    // The type checker looks for the loop through closure scopes
    // (compiler/src/type_check/expressions.rs:2940-2956), so this loop is Nil, not Never.
    let checks = checks(&common::fixture("crosscheck"));
    let line = std::fs::read_to_string(common::fixture("crosscheck").join("src/main.inko"))
        .unwrap()
        .lines()
        .position(|l| l.contains("let a = loop {"))
        .unwrap() as u32
        + 1;
    let check = checks
        .iter()
        .find(|c| c.kind == "Loop" && c.location.line_start == line)
        .unwrap();

    assert_eq!(
        (check.ours.as_str(), check.compiler.as_str()),
        ("Nil", "Nil")
    );
}

#[test]
fn the_standard_library_agrees_with_the_compiler() {
    let std = upstream_std();
    let target = Target::new(std.parent().unwrap(), upstream_std()).unwrap();
    let checked = check(&target).unwrap();
    let checks = &checked.checks;

    assert!(checked.ok);
    assert_agree(checks);

    // Every type or module used as a receiver is cross-checked. Derived
    // types are where the compiler stores them for a variable or scope too;
    // the rest rely on the rules citing type_check lines in src/walker.rs.
    let key = |l: location::Location, k: &'static str| {
        (l.line_start, l.column_start, l.line_end, l.column_end, k)
    };
    let checked_at: HashSet<_> = checks.iter().map(|c| key(c.location, c.kind)).collect();
    let unchecked: Vec<_> = checked
        .expressions
        .iter()
        .filter(|e| e.origin == Origin::Symbol && !checked_at.contains(&key(e.location, e.kind)))
        .map(|e| format_expr(e, &target.root))
        .collect();

    assert!(
        unchecked.is_empty(),
        "receivers without a cross-check:\n{}",
        unchecked.join("\n")
    );

    let counts = counts(checks);

    assert!(counts[&("Loop", "scope")] > 50, "{:?}", counts);
    assert!(counts[&("Try", "let")] > 50, "{:?}", counts);
}
