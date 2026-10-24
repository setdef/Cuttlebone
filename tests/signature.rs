//! textDocument/signatureHelp over LSP, with the real binary, in UTF-16 and
//! UTF-8.

use crate::common::features::Session;
use serde_json::{Value, json};

/// The signature's label, each parameter's text (read from the label with
/// its offsets), and the active parameter.
fn help(session: &mut Session, needle: &str, offset: usize) -> Value {
    let (id, response) = session.request("textDocument/signatureHelp", needle, offset);

    assert_eq!(
        session.client.status(id).unwrap()["status"],
        "fresh",
        "{}",
        response
    );
    summary(&response)
}

fn summary(response: &Value) -> Value {
    let result = &response["result"];

    if result.is_null() {
        return Value::Null;
    }

    let signature = &result["signatures"][0];
    let label = signature["label"].as_str().unwrap();
    let parameters: Vec<&str> = signature["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let [start, end] =
                [&p["label"][0], &p["label"][1]].map(|v| v.as_u64().unwrap() as usize);

            &label[start..end]
        })
        .collect();

    assert_eq!(result["activeSignature"], 0);
    assert_eq!(result["activeParameter"], signature["activeParameter"]);
    json!({ "label": label, "parameters": parameters, "active": result["activeParameter"] })
}

const ADD: &str = "fn mut add(amount: Int, times: Int)";
const DESCRIBE: &str = "fn static describe(name: String, count: Int, verbose: Bool) -> String";

fn add(active: u64) -> Value {
    json!({ "label": ADD, "parameters": ["amount: Int", "times: Int"], "active": active })
}

fn describe(active: u64) -> Value {
    json!({
        "label": DESCRIBE,
        "parameters": ["name: String", "count: Int", "verbose: Bool"],
        "active": active,
    })
}

#[test]
fn positional_arguments() {
    for utf8 in [false, true] {
        let mut session = Session::new(utf8);

        // In an argument, and between arguments.
        assert_eq!(help(&mut session, "counter.add(total, 2)", 12), add(0));
        assert_eq!(help(&mut session, "counter.add(total, 2)", 17), add(0));
        assert_eq!(help(&mut session, "counter.add(total, 2)", 18), add(1));
        assert_eq!(help(&mut session, "counter.add(total, 2)", 19), add(1));
        // Right before the `)`.
        assert_eq!(help(&mut session, "counter.add(total, 2)", 20), add(1));
        // After non-ASCII text.
        assert_eq!(help(&mut session, "\"日本\", total", 6), describe(1));
    }
}

#[test]
fn named_arguments_are_their_parameters() {
    for utf8 in [false, true] {
        let mut session = Session::new(utf8);

        assert_eq!(help(&mut session, "amount: 1", 8), add(0));
        assert_eq!(help(&mut session, "times: count", 0), add(1));
        assert_eq!(help(&mut session, "times: count", 7), add(1));
        assert_eq!(help(&mut session, "verbose: false", 9), describe(2));

        // In any order.
        let file = session.file.clone();
        let text = session.text.replace(
            "add(amount: 1, times: count)",
            "add(times: count, amount: 1)",
        );

        session.text = text.clone();
        session.client.open(&file, &text);
        assert_eq!(help(&mut session, "times: count", 7), add(1));
        assert_eq!(help(&mut session, "amount: 1", 8), add(0));
    }
}

/// The innermost call whose parentheses contain the position.
#[test]
fn nested_calls() {
    let mut session = Session::new(false);
    let print = json!({
        "label": "fn pub mut print[B: Bytes](bytes: ref String) -> Result[Nil, Error]",
        "parameters": ["bytes: ref String"],
        "active": 0,
    });

    assert_eq!(help(&mut session, "out.print(describe(label", 10), print);
    assert_eq!(
        help(&mut session, "out.print(describe(label", 19),
        describe(0)
    );
    // After the inner call's `)`, the outer call's argument.
    assert_eq!(help(&mut session, "count, true))", 12), print);
}

/// The call's type arguments are filled in: `map` on an `Iter[ref Int]`.
#[test]
fn type_arguments_are_filled_in() {
    let mut session = Session::new(false);

    assert_eq!(
        help(&mut session, "numbers.iter.map(fn (n) {", 17),
        json!({
            "label": "fn pub move map[R](func: fn (ref Int) -> Int) -> Map[ref Int, Self, Int]",
            "parameters": ["func: fn (ref Int) -> Int"],
            "active": 0,
        })
    );
}

/// While typing, the argument list isn't closed yet and the file doesn't
/// parse. The answer is the same as with the `)` there.
#[test]
fn unfinished_argument_lists() {
    for utf8 in [false, true] {
        let mut session = Session::new(utf8);
        let file = session.file.clone();
        let original = session.text.clone();

        session.client.open(&file, &original);

        for (version, (typed, active)) in [
            ("counter.add(", 0),
            ("counter.add(1, ", 1),
            ("counter.add(times: 1, ", 0),
            ("describe(\"日本\", ", 1),
        ]
        .into_iter()
        .enumerate()
        {
            let text = original.replace("    counter.reset\n", &format!("    {}\n", typed));

            session.text = text.clone();
            session
                .client
                .change(&file, version as i32 + 2, json!([{ "text": text }]));

            // The line before makes the needle unique.
            let needle = format!("counter.count = 5\n    {}", typed);
            let found = help(&mut session, &needle, needle.chars().count());

            assert_eq!(found["active"], active, "{}: {}", typed, found);
            assert!(
                found["label"] == ADD || found["label"] == DESCRIBE,
                "{}",
                found
            );
        }

        session.client.close(&file);
    }
}

#[test]
fn null_outside_parentheses_or_without_a_method() {
    let mut session = Session::new(false);
    let file = session.file.clone();

    // On the name, before the `(`, after the `)`, and a call without
    // parentheses.
    for (needle, offset) in [
        ("counter.add(total, 2)", 9),
        ("counter.add(total, 2)", 11),
        ("counter.add(total, 2)", 21),
        ("counter.reset", 10),
        ("# Highlights", 2),
        // Operators are calls, but not written with parentheses.
        ("total = total + 1", 16),
        ("amount * times", 9),
    ] {
        assert_eq!(
            help(&mut session, needle, offset),
            Value::Null,
            "{} {}",
            needle,
            offset
        );
    }

    // A method that doesn't exist.
    let text = session.text.replace("counter.reset", "counter.nope(1)");

    session.text = text.clone();
    session.client.open(&file, &text);
    assert_eq!(help(&mut session, "counter.nope(1)", 13), Value::Null);
}
