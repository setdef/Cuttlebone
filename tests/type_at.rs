//! typeAt: which expression a position means, and the type it reports.
mod common;

use common::{Project, position_of};
use cuttlebone::position::Position;
use cuttlebone::query::Range;
use cuttlebone::{Target, check, upstream_std};

const MAIN: &str = "src/main.inko";

fn desugar() -> Project {
    Project::new("desugar")
}

fn kind_and_type(p: &Project, needle: &str, offset: u32) -> (&'static str, String) {
    let t = p.type_at(MAIN, None, needle, offset).unwrap().unwrap();

    (t.kind, t.typ)
}

#[test]
fn an_array_literal_is_the_array_not_its_scaffolding() {
    let p = desugar();

    // On `[`: the literal's scope, not `$array`, `with_capacity` or a push.
    assert_eq!(
        kind_and_type(&p, "[10, 20]", 0),
        ("Scope", "Array[Int]".into())
    );
    // On an element: the element, not the `$array.push(element)` call that
    // has the same range.
    assert_eq!(kind_and_type(&p, "[10, 20]", 1), ("Int", "Int".into()));
    assert_eq!(kind_and_type(&p, "[10, 20]", 5), ("Int", "Int".into()));
    // On the `,` between elements: the literal again.
    assert_eq!(
        kind_and_type(&p, "[10, 20]", 3),
        ("Scope", "Array[Int]".into())
    );
}

#[test]
fn an_interpolated_string_is_a_string_and_its_parts_are_themselves() {
    let p = desugar();
    let s = "\"hi ${name}, ${numbers.size} items\"";

    assert_eq!(kind_and_type(&p, s, 0), ("Scope", "String".into()));
    // Literal text: the text, not `$buf.push(text)`.
    assert_eq!(kind_and_type(&p, s, 1), ("String", "String".into()));
    // An interpolated value: the value, not `$buf.push(value.to_string)`
    // or the generated `to_string` call, which share its range.
    assert_eq!(kind_and_type(&p, s, 6), ("IdentifierRef", "String".into()));
    assert_eq!(
        kind_and_type(&p, s, 15),
        ("IdentifierRef", "Array[Int]".into())
    );
    assert_eq!(kind_and_type(&p, s, 23), ("Call", "Int".into()));
}

#[test]
fn a_string_with_only_an_interpolated_value_is_that_value() {
    let p = desugar();

    // `"${name}"` lowers to just `name.to_string`, at `name`'s range.
    assert_eq!(
        kind_and_type(&p, "\"${name}\"", 3),
        ("IdentifierRef", "String".into())
    );
}

#[test]
fn a_for_loop_is_nil_and_its_iterator_and_binding_are_themselves() {
    let p = desugar();
    let f = "for number in numbers.iter {";

    assert_eq!(kind_and_type(&p, f, 0), ("Scope", "Nil".into()));
    // The binding, not the generated `match` around it.
    assert_eq!(kind_and_type(&p, f, 4), ("Binding", "Int".into()));
    // `numbers.iter`, not `$iter`, `into_iter` or `$iter.next`.
    assert_eq!(kind_and_type(&p, f, 22), ("Call", "Iter[Int]".into()));
    assert_eq!(
        kind_and_type(&p, f, 14),
        ("IdentifierRef", "Array[Int]".into())
    );
}

#[test]
fn a_while_loop_condition_is_the_condition_not_the_generated_break() {
    let p = desugar();

    assert_eq!(
        kind_and_type(&p, "while total > 0", 0),
        ("Loop", "Nil".into())
    );
    assert_eq!(
        kind_and_type(&p, "while total > 0", 12),
        ("Call", "Bool".into())
    );
}

#[test]
fn an_if_without_else_is_the_if_not_its_generated_nil() {
    let p = desugar();

    assert_eq!(
        kind_and_type(&p, "if total == 0", 0),
        ("Match", "Nil".into())
    );
}

#[test]
fn a_negation_is_the_comparison_not_the_generated_false() {
    let p = desugar();

    assert_eq!(
        kind_and_type(&p, "!(total == 1)", 0),
        ("Call", "Bool".into())
    );
}

#[test]
fn await_is_the_awaited_value() {
    let p = desugar();

    assert_eq!(
        kind_and_type(&p, "await answer", 0),
        ("Match", "Int".into())
    );
    // The method itself, not the generated promise argument or future.
    assert_eq!(kind_and_type(&p, "await answer", 6), ("Call", "Nil".into()));
    assert_eq!(
        kind_and_type(&p, "async answer", 0),
        ("Match", "uni Future[Int]".into())
    );
}

#[test]
fn an_inferred_recover_is_the_value_as_written() {
    let p = desugar();

    // The type checker wraps `[7, 8]` in a `recover` because the method
    // returns `uni Array[Int]`.
    assert_eq!(
        kind_and_type(&p, "[7, 8]", 0),
        ("Scope", "Array[Int]".into())
    );
}

#[test]
fn equal_ranges_resolve_to_the_outermost_node() {
    let p = desugar();

    // `total += number` is an assignment containing a `+` call with the same
    // range.
    assert_eq!(
        kind_and_type(&p, "total += number", 6),
        ("AssignVariable", "Nil".into())
    );
}

#[test]
fn a_binding_is_the_variable() {
    let p = desugar();

    assert_eq!(
        kind_and_type(&p, "let numbers", 4),
        ("Binding", "Array[Int]".into())
    );
    assert_eq!(
        kind_and_type(&p, "let mut total", 8),
        ("Binding", "Int".into())
    );
}

#[test]
fn closure_arguments_are_bindings() {
    let p = Project::new("crosscheck");

    assert_eq!(
        p.type_at(MAIN, None, "fn (value) { value + 1 }", 4)
            .unwrap()
            .unwrap()
            .typ,
        "Int"
    );
}

#[test]
fn a_position_outside_every_expression_has_no_result() {
    let p = desugar();

    // In a comment, and on a method's name.
    assert_eq!(p.type_at(MAIN, None, "# One example", 3), Ok(None));
    assert_eq!(p.type_at(MAIN, None, "fn unique", 4), Ok(None));
}

#[test]
fn a_line_past_the_end_is_an_invalid_position() {
    let p = desugar();
    let mut request = p.request(MAIN, None, "fn unique", 0);

    request.position = Position {
        line: 999,
        character: 0,
    };

    let answer = cuttlebone::query::type_at(&p.session, &request);

    assert_eq!(answer.result.unwrap_err().code, "invalidPosition");
}

#[test]
fn every_synthetic_node_shares_its_range_with_a_real_one() {
    // A synthetic node with a range of its own would make that range
    // unanswerable, so each must share it with a node that isn't synthetic
    // or be contained in one.
    let file = common::fixture("desugar").join(MAIN);
    let checked = check(&Target::new(&file, upstream_std()).unwrap()).unwrap();
    let real: Vec<_> = checked
        .expressions
        .iter()
        .filter(|e| !e.synthetic)
        .collect();

    assert!(checked.expressions.iter().any(|e| e.synthetic));

    for e in checked.expressions.iter().filter(|e| e.synthetic) {
        let l = e.location;
        let covered = real.iter().any(|r| {
            let o = r.location;

            (o.line_start, o.column_start) <= (l.line_start, l.column_start)
                && (l.line_end, l.column_end) <= (o.line_end, o.column_end)
        });

        assert!(
            covered,
            "{:?} at {:?} isn't covered by a real node",
            e.kind, l
        );
    }
}

// Positions. The compiler's columns count grapheme clusters (see
// src/position.rs); the protocol's count UTF-16 code units.

fn unicode() -> Project {
    Project::new("unicode")
}

#[test]
fn compiler_columns_count_grapheme_clusters() {
    let file = common::fixture("unicode").join(MAIN);
    let checked = check(&Target::new(&file, upstream_std()).unwrap()).unwrap();
    let float = checked
        .expressions
        .iter()
        .find(|e| e.kind == "Float")
        .unwrap();
    let line = std::fs::read_to_string(&file)
        .unwrap()
        .lines()
        .nth(10)
        .unwrap()
        .to_string();
    let before = &line[..line.find("4.5").unwrap()];

    // `    let d = '👩‍👩‍👦' + 4.5`: the family emoji is 1 grapheme cluster, 5
    // code points, 8 UTF-16 units and 18 bytes.
    assert_eq!(float.location.line_start, 11);
    assert_eq!(float.location.column_start, 19);
    assert_eq!(
        unicode_segmentation::UnicodeSegmentation::graphemes(before, true).count() + 1,
        19
    );
    assert_eq!(before.chars().count() + 1, 23);
    assert_eq!(before.len() + 1, 36);
}

#[test]
fn utf16_positions_after_non_ascii_text_find_the_right_expression() {
    let p = unicode();

    assert_eq!(p.type_of(MAIN, "1.to_string", 0), "Int");
    assert_eq!(p.type_of(MAIN, "2.to_string", 0), "Int");
    assert_eq!(p.type_of(MAIN, "true.to_string", 0), "Bool");
    assert_eq!(p.type_of(MAIN, "4.5.to_string", 0), "Float");
    assert_eq!(p.type_of(MAIN, "[3].size", 1), "Int");
}

#[test]
fn ranges_are_utf16_with_exclusive_ends() {
    let p = unicode();
    let text = p.read(MAIN);
    let family = "'\u{1F469}\u{200d}\u{1F469}\u{200d}\u{1F466}'";
    let t = p.type_at(MAIN, None, family, 0).unwrap().unwrap();
    let start = position_of(&text, family, 0);
    let end = Position {
        line: start.line,
        character: start.character + family.encode_utf16().count() as u32,
    };

    assert_eq!(t.typ, "String");
    assert_eq!(t.range, Range { start, end });

    let float = p.type_at(MAIN, None, "4.5", 0).unwrap().unwrap();

    assert_eq!(float.range.start, position_of(&text, "4.5", 0));
    assert_eq!(float.range.end, position_of(&text, "4.5", 3));
}

#[test]
fn a_position_inside_a_surrogate_pair_or_cluster_means_that_cluster() {
    let p = unicode();

    // The second unit of 😀, and the U+0301 after `e`.
    assert_eq!(p.type_of(MAIN, "\u{1F600}\u{1F600}", 1), "String");
    assert_eq!(p.type_of(MAIN, "e\u{301}", 1), "String");
}

#[test]
fn lexer_and_cuttlebone_segment_with_the_same_crate_version() {
    // src/position.rs relies on segmenting graphemes exactly as the lexer
    // does, so Cargo must resolve one version of unicode-segmentation for
    // both.
    let lock = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.lock")).unwrap();
    let versions = lock
        .split("[[package]]")
        .filter(|p| p.contains("name = \"unicode-segmentation\""))
        .count();

    assert_eq!(versions, 1);
}

#[test]
fn a_try_the_type_checker_rejects_is_untyped() {
    // `try` on an Option in a method returning a Result is an error, and the
    // type checker types it as one (compiler/src/type_check/expressions.rs:4407-4414).
    let p = Project::new("crosscheck");
    let text = p
        .read(MAIN)
        .replacen(
            "fn first(values: ref Array[Int]) -> Option[Int] {",
            "fn first(values: ref Array[Int]) -> Result[Int, String] {",
            1,
        )
        .replacen(
            "  Option.Some(value)\n}\n\nfn inferred",
            "  Result.Ok(value)\n}\n\nfn inferred",
            1,
        );
    let needle = "try values.get(0).ok";

    assert_eq!(
        p.type_at(MAIN, Some(&text), needle, 0).unwrap_err().code,
        "untyped"
    );
    assert_eq!(p.type_of(MAIN, needle, 0), "Int");
}
