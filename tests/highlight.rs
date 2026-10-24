//! textDocument/documentHighlight over LSP, with the real binary, in UTF-16
//! and UTF-8.
mod common;

use common::features::Session;
use serde_json::{Value, json};

const TEXT: u64 = 1;
const READ: u64 = 2;
const WRITE: u64 = 3;

/// Each highlight as (line, the text it covers, kind).
fn highlights(session: &mut Session, needle: &str, offset: usize) -> Vec<(u64, String, u64)> {
    let (id, response) = session.request("textDocument/documentHighlight", needle, offset);

    assert_eq!(session.client.status(id).unwrap()["status"], "fresh");
    response["result"]
        .as_array()
        .unwrap_or_else(|| panic!("{}", response))
        .iter()
        .map(|h| {
            let (line, text) = session.slice(&h["range"]);

            (line, text, h["kind"].as_u64().unwrap())
        })
        .collect()
}

fn expect(list: &[(u64, &str, u64)]) -> Vec<(u64, String, u64)> {
    list.iter()
        .map(|(l, t, k)| (*l, t.to_string(), *k))
        .collect()
}

#[test]
fn a_variable_is_written_by_its_binding_and_assignments() {
    for utf8 in [false, true] {
        let mut session = Session::new(utf8);
        // From a use after non-ASCII text, and from the binding: the same.
        let from_use = highlights(&mut session, "\"日本\", total", 6);
        let from_binding = highlights(&mut session, "let mut total", 8);

        assert_eq!(
            from_use,
            expect(&[
                (32, "total", WRITE),
                (41, "total", WRITE),
                (41, "total", READ),
                (42, "total", WRITE),
                (43, "total", READ),
                (48, "total", READ),
                (51, "total", WRITE),
            ]),
            "utf8: {}",
            utf8
        );
        assert_eq!(from_binding, from_use);
    }
}

#[test]
fn a_field_is_declared_read_and_written() {
    for utf8 in [false, true] {
        let mut session = Session::new(utf8);

        assert_eq!(
            highlights(&mut session, "@count = 0", 1),
            expect(&[
                (4, "@count", TEXT),
                (8, "count", READ),
                (12, "@count", WRITE),
                (16, "@count", WRITE),
                (45, "count", WRITE),
            ]),
            "utf8: {}",
            utf8
        );
    }
}

/// Symbols are matched by id: `count` the field, the argument of
/// `describe` and the variable in `main` are three symbols.
#[test]
fn same_named_symbols_are_apart() {
    let mut session = Session::new(false);

    assert_eq!(
        highlights(&mut session, "let count", 4),
        expect(&[
            (34, "count", WRITE),
            (44, "count", READ),
            (47, "count", READ),
            (50, "count", READ),
        ])
    );
    assert_eq!(
        highlights(&mut session, "count: Int, verbose", 0),
        expect(&[(20, "count", WRITE), (21, "count", READ)])
    );
}

#[test]
fn methods_types_and_closure_arguments() {
    let mut session = Session::new(true);

    assert_eq!(
        highlights(&mut session, "counter.add(total", 8),
        expect(&[(11, "add", TEXT), (43, "add", READ), (44, "add", READ)])
    );
    assert_eq!(
        highlights(&mut session, "Counter {", 0),
        expect(&[
            (3, "Counter", TEXT),
            (7, "Counter", READ),
            (8, "Counter", READ),
            (31, "Counter", READ),
        ])
    );
    assert_eq!(
        highlights(&mut session, "fn (n) { n * 2", 4),
        expect(&[(36, "n", WRITE), (36, "n", READ)])
    );
}

/// `null` where there's no symbol: a keyword, a literal, a comment.
#[test]
fn nothing_to_highlight_is_null() {
    let mut session = Session::new(false);

    for (needle, offset) in [("let out", 0), ("\"👩‍👩‍👦\"", 2), ("# Highlights", 3)] {
        let (id, response) = session.request("textDocument/documentHighlight", needle, offset);

        assert_eq!(response["result"], Value::Null, "{}", needle);
        assert_eq!(session.client.status(id).unwrap()["status"], "fresh");
    }
}

/// Broken code: the same reasons as other queries. A syntax error in
/// another method leaves a repaired copy to answer from; one before the
/// variable's uses doesn't.
#[test]
fn a_syntax_error_is_unavailable() {
    let mut session = Session::new(false);
    let file = session.file.clone();
    let broken = format!("{}\nfn broken {{", session.text);

    session.client.open(&file, &broken);

    let (id, response) = session.request("textDocument/documentHighlight", "let mut total", 8);

    assert_eq!(response["result"].as_array().unwrap().len(), 7);
    assert_eq!(session.client.status(id).unwrap()["repaired"], true);

    let broken = session
        .text
        .replacen("    let mut total", "    let x = (\n    let mut total", 1);

    session.client.change(&file, 2, json!([{ "text": broken }]));

    let (line, character) = common::client::at(&broken, "let mut total", 8, false);
    let id = session
        .client
        .send_at("textDocument/documentHighlight", &file, line, character);
    let response = session.client.response(id);

    assert_eq!(response["result"], Value::Null);
    assert_eq!(
        session.client.status(id).unwrap()["reason"]["code"],
        "noHir"
    );
}
