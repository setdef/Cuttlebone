//! textDocument/inlayHint over LSP, with the real binary, in UTF-16 and
//! UTF-8. Each rule in docs/lsp.md ("textDocument/inlayHint") has a test.

use crate::common::features::Session;
use serde_json::{Value, json};

/// The hints in the whole document, as (line, the text before the hint on
/// its line, label, kind).
fn hints(session: &mut Session) -> Vec<(u64, String, String, u64)> {
    let end = session.text.lines().count();
    let id = session.client.send_request(
        "textDocument/inlayHint",
        json!({
            "textDocument": { "uri": crate::common::client::uri(&session.file) },
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": end, "character": 0 } },
        }),
    );
    let response = session.client.response(id);

    assert_eq!(
        session.client.status(id).unwrap()["status"],
        "fresh",
        "{}",
        response
    );
    response["result"]
        .as_array()
        .unwrap_or_else(|| panic!("{}", response))
        .iter()
        .map(|h| {
            let position = json!({ "start": h["position"], "end": h["position"] });
            let line = h["position"]["line"].as_u64().unwrap();
            let range = json!({ "start": { "line": line, "character": 0 }, "end": h["position"] });
            let (_, before) = session.slice(&range);
            let _ = position;

            (
                line,
                before.trim_start().to_string(),
                h["label"].as_str().unwrap().to_string(),
                h["kind"].as_u64().unwrap(),
            )
        })
        .collect()
}

fn hint(line: u64, before: &str, label: &str, kind: u64) -> (u64, String, String, u64) {
    (line, before.to_string(), label.to_string(), kind)
}

const TYPE: u64 = 1;
const PARAMETER: u64 = 2;

/// Every hint in the fixture. Lines without one are there to show a rule
/// that leaves them out: see the tests below.
#[test]
fn the_fixtures_hints() {
    for utf8 in [false, true] {
        let mut session = Session::new(utf8);

        assert_eq!(
            hints(&mut session),
            vec![
                hint(31, "let mut counter = Counter.new(", "label:", PARAMETER),
                hint(35, "let numbers", ": Array[Int]", TYPE),
                hint(36, "let doubled", ": Array[Int]", TYPE),
                hint(36, "let doubled = numbers.iter.map(", "func:", PARAMETER),
                hint(36, "let doubled = numbers.iter.map(fn (n", ": Int", TYPE),
                hint(37, "let typed", ": Array[Int]", TYPE),
                hint(37, "let typed = numbers.iter.map(", "func:", PARAMETER),
                hint(38, "let tuple", ": (Int, String)", TYPE),
                hint(43, "counter.add(", "amount:", PARAMETER),
                hint(43, "counter.add(total, ", "times:", PARAMETER),
                hint(47, "let _ = out.print(", "bytes:", PARAMETER),
                hint(47, "let _ = out.print(describe(", "name:", PARAMETER),
                hint(
                    47,
                    "let _ = out.print(describe(label, count, ",
                    "verbose:",
                    PARAMETER
                ),
                hint(48, "let _ = out.print(", "bytes:", PARAMETER),
                hint(48, "let _ = out.print(describe(", "name:", PARAMETER),
                // After non-ASCII text.
                hint(
                    48,
                    "let _ = out.print(describe(\"日本\", ",
                    "count:",
                    PARAMETER
                ),
            ],
            "utf8: {}",
            utf8
        );
    }
}

/// The hints for lines added to `main`, as (the text before the hint, label).
fn hints_for(lines: &[&str]) -> Vec<(String, String)> {
    let mut session = Session::new(false);
    let file = session.file.clone();
    let added: String = lines.iter().map(|l| format!("    {}\n", l)).collect();
    let text = session.text.replace(
        "    counter.reset\n",
        &format!("    counter.reset\n{}", added),
    );

    session.text = text.clone();
    session.client.open(&file, &text);
    hints(&mut session)
        .into_iter()
        .filter(|(line, ..)| (47..47 + lines.len() as u64).contains(line))
        .map(|(_, before, label, _)| (before, label))
        .collect()
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

/// A `let` without a type annotation gets its type, unless the value says
/// it: a literal, a cast, a type's constructor, or a static method of a
/// type that returns that type.
#[test]
fn let_types_unless_the_value_says_them() {
    assert_eq!(
        hints_for(&[
            "let shown = pair",
            "let annotated: (Int, String) = pair",
            "let int = 42",
            "let float = 4.2",
            "let string = 'é'",
            "let yes = true",
            "let nothing = nil",
            "let cast = count as Int",
            "let made = Counter(count: 1, label: \"x\")",
            "let new = Counter.new(label)",
            "let some = Option.Some(1)",
            "let _ = pair",
            "let mut shown_too = count + 1",
        ]),
        pairs(&[
            ("let shown", ": (Int, String)"),
            ("let mut shown_too", ": Int"),
        ])
    );
}

/// A static method that returns another type isn't obvious.
#[test]
fn a_static_method_of_another_type_gets_a_hint() {
    assert_eq!(
        hints_for(&["let parsed = Int.parse('1')"]),
        pairs(&[
            ("let parsed", ": Option[Int]"),
            ("let parsed = Int.parse(", "bytes:"),
        ])
    );
}

/// Closure arguments without a type get one; with one, they don't.
#[test]
fn closure_arguments() {
    assert_eq!(
        hints_for(&[
            "let _ = [1].iter.map(fn (a) { a })",
            "let _ = [1].iter.map(fn (b: Int) { b })",
        ]),
        pairs(&[
            ("let _ = [1].iter.map(", "func:"),
            ("let _ = [1].iter.map(fn (a", ": Int"),
            ("let _ = [1].iter.map(", "func:"),
        ])
    );
}

/// Parameter names: positional arguments only, not for a variable or field
/// named like the parameter, operators, enum constructors, or calls without
/// parentheses.
#[test]
fn parameter_names() {
    assert_eq!(
        hints_for(&[
            "counter.add(count, 1)",
            "counter.add(1, times: 2)",
            "counter.add(amount: 1, times: 2)",
            "let amount = 1",
            "counter.add(amount, count + 1)",
            "let _ = Option.Some(1)",
            "let _ = total + 1",
            "counter.reset",
        ]),
        pairs(&[
            ("counter.add(", "amount:"),
            ("counter.add(count, ", "times:"),
            ("counter.add(", "amount:"),
            ("counter.add(amount, ", "times:"),
        ])
    );
}

/// A variable whose type the compiler couldn't infer gets no hint.
#[test]
fn unknown_types_get_no_hint() {
    assert_eq!(
        hints_for(&["let broken = undefined_name", "let fine = pair"]),
        pairs(&[("let fine", ": (Int, String)")])
    );
}

/// Only the hints in the requested range.
#[test]
fn only_the_requested_range() {
    let mut session = Session::new(true);
    let id = session.client.send_request(
        "textDocument/inlayHint",
        json!({
            "textDocument": { "uri": crate::common::client::uri(&session.file) },
            "range": { "start": { "line": 43, "character": 0 }, "end": { "line": 43, "character": 18 } },
        }),
    );
    let response = session.client.response(id);
    let labels: Vec<&Value> = response["result"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| &h["label"])
        .collect();

    // `times:` is at character 22, past the range's end.
    assert_eq!(labels, [&json!("amount:")]);
    assert_eq!(response["result"][0]["paddingRight"], true);
}
