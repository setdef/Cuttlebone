//! Answers from the last good check over LSP, with the real binary
//! (docs/lsp.md, "Answers in code being edited"): which requests give them,
//! that positions and ranges map through edits in both encodings, and what
//! stays unavailable. Repaired copies answer most of these first
//! (tests/repaired.rs), so these run with them off. tests/replay.rs measures
//! both on a corpus of edits.
mod common;

use common::client::{Client, Launch, at};
use common::features::Session;
use serde_json::{Value, json};

/// A server that never checks repaired copies (a fault only test builds
/// have), so answers come from the last good check.
fn no_repair() -> Launch<'static> {
    Launch {
        env: vec![("CUTTLEBONE_FAULT", "norepair:.inko".to_string())],
        ..Launch::default()
    }
}

fn stale_session(utf8: bool) -> Session {
    Session::launched(utf8, no_repair())
}

/// A change that inserts `text` at the start of the line containing
/// `needle`.
fn insert_before(session: &Session, needle: &str, text: &str) -> Value {
    let (line, _) = at(&session.text, needle, 0, session.utf8);

    json!([{
        "range": {
            "start": { "line": line, "character": 0 },
            "end": { "line": line, "character": 0 },
        },
        "text": text,
    }])
}

/// The response to `method` at `needle` (plus `offset` characters) in the
/// edited text, and its status.
fn ask(
    session: &mut Session,
    edited: &str,
    method: &str,
    needle: &str,
    offset: usize,
    extra: Value,
) -> (Value, Value) {
    let (line, character) = at(edited, needle, offset, session.utf8);
    let mut params = json!({
        "textDocument": { "uri": common::client::uri(&session.file) },
        "position": { "line": line, "character": character },
    });

    for (key, value) in extra.as_object().unwrap() {
        params[key] = value.clone();
    }

    let id = session.client.send_request(method, params);
    let response = session.client.response(id);
    let status = session.client.status(id).unwrap().clone();

    (response["result"].clone(), status)
}

#[test]
fn read_only_requests_answer_from_the_last_good_check() {
    for utf8 in [false, true] {
        let mut session = stale_session(utf8);
        let file = session.file.clone();
        let text = session.text.clone();

        session.client.open(&file, &text);

        // A good check, kept for later.
        let (hover, status) = ask(
            &mut session,
            &text,
            "textDocument/hover",
            "numbers.size",
            0,
            json!({}),
        );

        assert_eq!(status["status"], "fresh");
        assert!(hover.to_string().contains("Array[Int]"), "{}", hover);

        // An unfinished statement in `add`, a line above everything below.
        let broken = "    let broken = (\n";
        let changes = insert_before(&session, "@count += amount", broken);
        let edited = text.replacen(
            "    @count += amount",
            &format!("{}    @count += amount", broken),
            1,
        );

        session.client.change(&file, 2, changes);

        // `total`, after non-ASCII text, in `main`, which doesn't change.
        let (hover, status) = ask(
            &mut session,
            &edited,
            "textDocument/hover",
            "\"日本\", total",
            6,
            json!({}),
        );
        let (line, start) = at(&edited, "\"日本\", total", 6, utf8);

        assert_eq!(status["status"], "stale", "utf8: {}", utf8);
        assert_eq!(hover["contents"]["value"], "```inko\nInt\n```");
        assert_eq!(
            hover["range"]["start"],
            json!({ "line": line, "character": start })
        );
        assert_eq!(
            hover["range"]["end"],
            json!({ "line": line, "character": start + 5 })
        );

        let (definition, status) = ask(
            &mut session,
            &edited,
            "textDocument/definition",
            "\"日本\", total",
            6,
            json!({}),
        );
        let (line, start) = at(&edited, "let mut total", 8, utf8);

        assert_eq!(status["status"], "stale");
        assert_eq!(
            definition["range"]["start"],
            json!({ "line": line, "character": start })
        );

        let (highlights, status) = ask(
            &mut session,
            &edited,
            "textDocument/documentHighlight",
            "\"日本\", total",
            6,
            json!({}),
        );

        assert_eq!(status["status"], "stale");
        assert_eq!(highlights.as_array().unwrap().len(), 7, "{}", highlights);

        let (signature, status) = ask(
            &mut session,
            &edited,
            "textDocument/signatureHelp",
            "add(total",
            4,
            json!({}),
        );

        assert_eq!(status["status"], "stale");
        assert_eq!(
            signature["signatures"][0]["label"],
            "fn mut add(amount: Int, times: Int)"
        );

        let (typ, status) = ask(
            &mut session,
            &edited,
            "textDocument/typeDefinition",
            "counter.reset",
            0,
            json!({}),
        );

        assert_eq!(status["status"], "stale");
        assert!(
            typ["uri"]
                .as_str()
                .unwrap()
                .ends_with("features/src/main.inko"),
            "{}",
            typ
        );

        let (line, _) = at(&edited, "let numbers", 0, utf8);
        let (hints, status) = ask(
            &mut session,
            &edited,
            "textDocument/inlayHint",
            "let numbers",
            0,
            json!({ "range": {
                "start": { "line": line, "character": 0 },
                "end": { "line": line + 1, "character": 0 },
            }}),
        );

        assert_eq!(status["status"], "stale");
        assert_eq!(hints[0]["label"], ": Array[Int]");
        assert_eq!(hints[0]["position"]["line"], line);

        // In the method being edited, and for what may change code: nothing.
        let (hover, status) = ask(
            &mut session,
            &edited,
            "textDocument/hover",
            "amount * times",
            0,
            json!({}),
        );

        assert_eq!(hover, Value::Null);
        assert_eq!(status["status"], "unavailable");
        assert_eq!(status["reason"]["code"], "noHir");

        let (references, status) = ask(
            &mut session,
            &edited,
            "textDocument/references",
            "\"日本\", total",
            6,
            json!({ "context": { "includeDeclaration": true } }),
        );

        assert_eq!(references, Value::Null);
        assert_eq!(status["status"], "unavailable");

        let (completion, status) = ask(
            &mut session,
            &edited,
            "textDocument/completion",
            "counter.reset",
            8,
            json!({}),
        );

        assert_eq!(completion, Value::Null);
        assert_eq!(status["status"], "unavailable");
    }
}

#[test]
fn a_changed_declaration_leaves_out_what_mentions_it() {
    let mut session = stale_session(false);
    let file = session.file.clone();
    let text = session.text.clone();

    session.client.open(&file, &text);
    ask(
        &mut session,
        &text,
        "textDocument/hover",
        "numbers.size",
        0,
        json!({}),
    );

    // `add`'s argument's type becomes undefined: a declaration error.
    let (line, character) = at(&text, "Int, times", 0, false);
    let edited = text.replacen("amount: Int, times", "amount: Intx, times", 1);

    session.client.change(
        &file,
        2,
        json!([{
            "range": {
                "start": { "line": line, "character": character + 3 },
                "end": { "line": line, "character": character + 3 },
            },
            "text": "x",
        }]),
    );

    // `main` calls `add`.
    let (hover, status) = ask(
        &mut session,
        &edited,
        "textDocument/hover",
        "\"日本\", total",
        6,
        json!({}),
    );

    assert_eq!(hover, Value::Null);
    assert_eq!(status["reason"]["code"], "declarationErrors");

    let (signature, status) = ask(
        &mut session,
        &edited,
        "textDocument/signatureHelp",
        "add(total",
        4,
        json!({}),
    );

    assert_eq!(signature, Value::Null);
    assert_eq!(status["reason"]["code"], "declarationErrors");

    // `describe` doesn't.
    let (hover, status) = ask(
        &mut session,
        &edited,
        "textDocument/hover",
        "verbose {",
        0,
        json!({}),
    );

    assert_eq!(status["status"], "stale");
    assert_eq!(hover["contents"]["value"], "```inko\nBool\n```");
}

#[test]
fn a_diagnostics_check_is_a_good_check_too() {
    let session = stale_session(false);
    let (file, text) = (session.file.clone(), session.text.clone());
    let root = file.parent().unwrap().parent().unwrap().to_path_buf();
    let mut client = Client::initialized_with(
        no_repair(),
        &root,
        json!({ "statusNotifications": true }),
        json!({}),
    );

    client.open(&file, &text);
    assert_eq!(client.diagnostics_status()["status"], "fresh");

    // A whole new text, as clients without incremental sync send it.
    let edited = text.replacen("  fn mut reset {", "  fn broken(\n\n  fn mut reset {", 1);

    client.change(&file, 2, json!([{ "text": edited }]));

    let (line, character) = at(&edited, "verbose {", 0, false);
    let (id, hover) = client.hover(&file, line, character);

    assert_eq!(client.status(id).unwrap()["status"], "stale");
    assert_eq!(hover["result"]["contents"]["value"], "```inko\nBool\n```");
}

#[test]
fn without_a_good_check_there_is_nothing_older() {
    let mut session = stale_session(false);
    let file = session.file.clone();
    let edited = session
        .text
        .replacen("  fn mut reset {", "  fn broken(\n\n  fn mut reset {", 1);

    session.client.open(&file, &edited);

    let (hover, status) = ask(
        &mut session,
        &edited,
        "textDocument/hover",
        "verbose {",
        0,
        json!({}),
    );

    assert_eq!(hover, Value::Null);
    assert_eq!(status["reason"]["code"], "noHir");
}

#[test]
fn hints_in_the_method_being_edited_are_left_out() {
    let mut session = stale_session(false);
    let file = session.file.clone();
    let text = session.text.clone();

    session.client.open(&file, &text);
    ask(
        &mut session,
        &text,
        "textDocument/hover",
        "numbers.size",
        0,
        json!({}),
    );

    // `main` has every hint.
    let changes = insert_before(&session, "    match Option.Some", "    let broken = (\n");

    session.client.change(&file, 2, changes);

    let (hints, status) = ask(
        &mut session,
        &text,
        "textDocument/inlayHint",
        "type Counter",
        0,
        json!({ "range": {
            "start": { "line": 0, "character": 0 },
            "end": { "line": 99, "character": 0 },
        }}),
    );

    assert_eq!(status["status"], "stale");
    assert_eq!(hints, json!([]));
}

#[test]
fn a_copy_with_a_paren_is_not_a_good_check() {
    let mut session = stale_session(false);
    let file = session.file.clone();
    let text = session.text.clone();

    session.client.open(&file, &text);
    ask(
        &mut session,
        &text,
        "textDocument/hover",
        "numbers.size",
        0,
        json!({}),
    );

    let typed = "    counter.add(\n";
    let changes = insert_before(&session, "    counter.reset", typed);
    let edited = text.replacen(
        "    counter.reset",
        &format!("{}    counter.reset", typed),
        1,
    );

    session.client.change(&file, 2, changes);

    // Checked with a `)` inserted, which parses: fresh, but not the
    // documents as they are.
    let (signature, status) = ask(
        &mut session,
        &edited,
        "textDocument/signatureHelp",
        "counter.add(\n",
        12,
        json!({}),
    );

    assert_eq!(status["status"], "fresh");
    assert_eq!(
        signature["signatures"][0]["label"],
        "fn mut add(amount: Int, times: Int)"
    );

    // So the last good check is still the one before.
    let (hover, status) = ask(
        &mut session,
        &edited,
        "textDocument/hover",
        "verbose {",
        0,
        json!({}),
    );

    assert_eq!(status["status"], "stale");
    assert_eq!(hover["contents"]["value"], "```inko\nBool\n```");
}

#[test]
fn unsaved_text_when_opened_counts_as_an_edit() {
    let session = stale_session(false);
    let (file, text) = (session.file.clone(), session.text.clone());
    let root = file.parent().unwrap().parent().unwrap().to_path_buf();
    let mut client = Client::initialized_with(
        no_repair(),
        &root,
        json!({ "statusNotifications": true }),
        json!({}),
    );

    // The check of the project at start reads the file from disk.
    assert_eq!(client.diagnostics_status()["status"], "fresh");

    // Then it's opened with unsaved text that doesn't parse, a line longer
    // above `describe`.
    let unsaved = text.replacen(
        "    @count += amount",
        "    let broken = (\n    @count += amount",
        1,
    );

    client.open(&file, &unsaved);

    let (line, character) = at(&unsaved, "verbose {", 0, false);
    let (id, hover) = client.hover(&file, line, character);

    assert_eq!(client.status(id).unwrap()["status"], "stale");
    assert_eq!(hover["result"]["contents"]["value"], "```inko\nBool\n```");
    assert_eq!(
        hover["result"]["range"]["start"],
        json!({ "line": line, "character": character })
    );
}

#[test]
fn definitions_into_an_edited_import_are_mapped() {
    let root = common::fixture("navigation");
    let main = root.join("src/main.inko");
    let alpha = root.join("src/alpha.inko");
    let main_text = std::fs::read_to_string(&main).unwrap();
    let alpha_text = std::fs::read_to_string(&alpha).unwrap();
    let mut client = Client::initialized_with(
        no_repair(),
        &root,
        json!({ "statusNotifications": true, "diagnostics": false }),
        json!({}),
    );

    client.open(&main, &main_text);
    client.open(&alpha, &alpha_text);

    let (line, character) = at(&main_text, "Error.new('a')", 6, false);
    let (id, _) = client.hover(&main, line, character);

    assert_eq!(client.status(id).unwrap()["status"], "fresh");

    // A line at the top of alpha, and an unfinished statement in a method
    // body that `main` doesn't call.
    let (body, _) = at(&alpha_text, "    @text", 0, false);

    client.change(
        &alpha,
        2,
        json!([
            {
                "range": { "start": { "line": body, "character": 0 }, "end": { "line": body, "character": 0 } },
                "text": "    let broken = (\n",
            },
            {
                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } },
                "text": "\n",
            },
        ]),
    );

    let id = client.send_at("textDocument/definition", &main, line, character);
    let definition = client.response(id);
    let (new_line, new_character) = at(&alpha_text, "new(text", 0, false);

    assert_eq!(client.status(id).unwrap()["status"], "stale");
    assert!(
        definition["result"]["uri"]
            .as_str()
            .unwrap()
            .ends_with("alpha.inko")
    );
    assert_eq!(
        definition["result"]["range"]["start"],
        json!({ "line": new_line + 1, "character": new_character })
    );
}
