//! Every expression in the standard library (src/ and test/) must have a
//! type, except for the allow-listed kinds below.
use cuttlebone::{Status, Target, check, format_expr, upstream_std};

/// Expression kinds that may lack a type, and why.
const ALLOWED: &[(&str, Status, &str)] = &[(
    "PointerToMethod",
    Status::Missing,
    // `mut fn_name` takes a pointer to a C-calling-convention method. The
    // type checker types the `Mut` node (Pointer[Uint8]) and records the
    // method in Mut.pointer_to_method, but never types the name itself
    // because it's not a value. Hover can use the method instead.
    "the operand of `mut fn_name` is a method, not a value",
)];

#[test]
fn every_std_expression_has_a_type() {
    let std = upstream_std();
    let root = std.parent().unwrap();
    let target = Target::new(root, std.clone()).unwrap();
    let checked = check(&target).unwrap();

    assert!(checked.ok, "the standard library has errors");
    assert!(
        checked.expressions.len() > 100_000,
        "only {} expressions found; is the walker skipping code?",
        checked.expressions.len()
    );

    let mut used = vec![0; ALLOWED.len()];
    let mut untyped = Vec::new();

    for e in &checked.expressions {
        let status = e.status();

        if status == Status::Resolved {
            continue;
        }

        match ALLOWED.iter().position(|(k, s, _)| *k == e.kind && *s == status)
        {
            Some(i) => used[i] += 1,
            None => untyped.push(format!(
                "{:?}: {}",
                status,
                format_expr(e, &target.root)
            )),
        }
    }

    assert!(
        untyped.is_empty(),
        "{} expressions without a type:\n{}",
        untyped.len(),
        untyped.iter().take(50).cloned().collect::<Vec<_>>().join("\n")
    );

    for ((kind, _, why), n) in ALLOWED.iter().zip(&used) {
        assert!(*n > 0, "allow-list entry {} ({}) is stale", kind, why);
    }
}
