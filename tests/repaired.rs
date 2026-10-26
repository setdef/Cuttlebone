//! Fresh answers in code that doesn't parse, from a repaired copy, over LSP
//! with the real binary (docs/lsp.md, "Answers in code being edited"): the
//! lines in the way are blank in a copy, which is checked instead, and
//! answers hold wherever the blank lines can't change their meaning.

use crate::common::client::{Launch, at};
use crate::common::features::Session;
use serde_json::{Value, json};

/// The response to `method` at `needle` (plus `offset` characters) in
/// `text`, and its status.
fn ask(
    session: &mut Session,
    text: &str,
    method: &str,
    needle: &str,
    offset: usize,
    extra: Value,
) -> (Value, Value) {
    let (line, character) = at(text, needle, offset, session.utf8);
    let mut params = json!({
        "textDocument": { "uri": crate::common::client::uri(&session.file) },
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

fn assert_repaired(status: &Value) {
    assert_eq!(status["status"], "fresh", "{}", status);
    assert_eq!(status["repaired"], true, "{}", status);
}

#[test]
fn an_edit_that_doesnt_parse_leaves_the_rest_fresh() {
    for utf8 in [false, true] {
        let mut session = Session::new(utf8);
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

        // An unfinished statement in `add`, after a good check.
        let (line, _) = at(&text, "    @count += amount", 0, utf8);
        let broken = "    let broken = (\n";
        let edited = text.replacen(
            "    @count += amount",
            &format!("{}    @count += amount", broken),
            1,
        );

        session.client.change(
            &file,
            2,
            json!([{
                "range": {
                    "start": { "line": line, "character": 0 },
                    "end": { "line": line, "character": 0 },
                },
                "text": broken,
            }]),
        );

        let (hover, status) = ask(
            &mut session,
            &edited,
            "textDocument/hover",
            "\"日本\", total",
            6,
            json!({}),
        );
        let (line, start) = at(&edited, "\"日本\", total", 6, utf8);

        assert_repaired(&status);
        assert_eq!(hover["contents"]["value"], "```inko\nInt\n```");
        assert_eq!(
            hover["range"]["start"],
            json!({ "line": line, "character": start })
        );

        // Completion, which never answers from older results, does here.
        let (completion, status) = ask(
            &mut session,
            &edited,
            "textDocument/completion",
            "counter.reset",
            8,
            json!({}),
        );

        assert_repaired(&status);
        assert!(
            completion["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["label"] == "reset"),
            "{}",
            completion
        );

        // References still don't: a blank line may have a use.
        let (references, status) = ask(
            &mut session,
            &edited,
            "textDocument/references",
            "\"日本\", total",
            6,
            json!({ "context": { "includeDeclaration": true } }),
        );

        assert_eq!(references, Value::Null);
        assert_eq!(status["reason"]["code"], "noHir");

        // Nor does the method being edited, after the edit.
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
    }
}

#[test]
fn a_method_being_edited_answers_before_the_edit() {
    let mut session = Session::new(false);
    let file = session.file.clone();
    // Never parsed: there's no older check. The parser reports the
    // unclosed `(` lines later.
    let edited = session.text.replacen(
        "    total = total + 1",
        "    let pending = (\n    total = total + 1",
        1,
    );

    session.client.open(&file, &edited);

    let (hover, status) = ask(
        &mut session,
        &edited,
        "textDocument/hover",
        "numbers.iter.map(fn (n) {",
        0,
        json!({}),
    );

    assert_repaired(&status);
    assert_eq!(hover["contents"]["value"], "```inko\nArray[Int]\n```");

    let (highlights, status) = ask(
        &mut session,
        &edited,
        "textDocument/documentHighlight",
        "let numbers",
        4,
        json!({}),
    );

    assert_repaired(&status);
    assert_eq!(highlights.as_array().unwrap().len(), 4, "{}", highlights);

    let (line, _) = at(&edited, "let doubled", 0, false);
    let (hints, status) = ask(
        &mut session,
        &edited,
        "textDocument/inlayHint",
        "let doubled",
        0,
        json!({ "range": {
            "start": { "line": line, "character": 0 },
            "end": { "line": line + 1, "character": 0 },
        }}),
    );

    assert_repaired(&status);
    assert_eq!(hints[0]["label"], ": Array[Int]");

    // After it, what the blank line declares or changes may be used.
    let (hover, status) = ask(
        &mut session,
        &edited,
        "textDocument/hover",
        "counter.add(total",
        0,
        json!({}),
    );

    assert_eq!(hover, Value::Null);
    assert_eq!(status["reason"]["code"], "noHir");

    // `total` is used after it too, so its highlights may be missing one.
    let (highlights, status) = ask(
        &mut session,
        &edited,
        "textDocument/documentHighlight",
        "let mut total",
        8,
        json!({}),
    );

    assert_eq!(highlights, Value::Null);
    assert_eq!(status["status"], "unavailable");
}

#[test]
fn a_type_the_blank_lines_could_infer_isnt_answered() {
    let mut session = Session::new(false);
    let file = session.file.clone();
    // `items` is an array of a placeholder (shown as `Array[T]`) until the
    // unfinished `push` gives it a type.
    let edited = session.text.replacen(
        "    total = total + 1",
        "    let items = []\n    items.push(1\n    total = total + 1",
        1,
    );

    session.client.open(&file, &edited);

    let (hover, status) = ask(
        &mut session,
        &edited,
        "textDocument/hover",
        "let items",
        4,
        json!({}),
    );

    assert_eq!(hover, Value::Null);
    assert_eq!(status["reason"]["code"], "noHir");
}

#[test]
fn a_changed_declaration_is_blank_in_the_copy() {
    // Checks that stop at the first pass with an error (a fault only test
    // builds have), as after a panic in a later pass: the check of the
    // documents as they are then types no method body.
    let mut session = Session::launched(
        false,
        Launch {
            env: vec![("CUTTLEBONE_FAULT", "stop:.inko".to_string())],
            ..Launch::default()
        },
    );
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

    // A change in `pair`'s body, then an undefined type in `add`'s
    // signature, with no check in between.
    let edited = text.replacen("(1, \"x\")", "(1, \"y\")", 1).replacen(
        "amount: Int, times",
        "amount: Intx, times",
        1,
    );

    let edit = |needle: &str, offset: usize, text: &str| {
        let (line, character) = at(&session.text, needle, offset, false);

        json!({
            "range": {
                "start": { "line": line, "character": character },
                "end": { "line": line, "character": character + 1 },
            },
            "text": text,
        })
    };
    let changes = json!([edit("\"x\"", 1, "y"), edit("Int, times", 2, "tx")]);

    session.client.change(&file, 2, changes);

    // The older check can't answer in `pair`, which changed since. A copy
    // with `add` blank can, with the change.
    let (hover, status) = ask(
        &mut session,
        &edited,
        "textDocument/hover",
        "\"y\"",
        0,
        json!({}),
    );

    assert_repaired(&status);
    assert_eq!(hover["contents"]["value"], "```inko\nString\n```");
}

#[test]
fn the_changed_line_is_blank_first() {
    let mut session = Session::new(false);
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

    // An unfinished expression at the end of `add`, which the parser
    // reports at the `}` after it. Blanking from there would take `add`'s
    // end, and with it `reset`; the changed line alone is enough.
    let (line, _) = at(&text, "    @count += amount", 0, false);
    let unfinished = "    let x = 1 +\n";
    let edited = text.replacen(
        "  }\n\n  fn mut reset",
        &format!("{}  }}\n\n  fn mut reset", unfinished),
        1,
    );

    session.client.change(
        &file,
        2,
        json!([{
            "range": {
                "start": { "line": line + 1, "character": 0 },
                "end": { "line": line + 1, "character": 0 },
            },
            "text": unfinished,
        }]),
    );

    let (hover, status) = ask(
        &mut session,
        &edited,
        "textDocument/hover",
        "@count = 0",
        0,
        json!({}),
    );

    assert_repaired(&status);
    assert_eq!(
        hover["contents"]["value"],
        "```inko\nInt\n```\n\n---\n\n```inko\nlet mut @count: Int\n```\n\nDefined in `main.Counter`"
    );
}

/// An import of a module that doesn't exist stops the check before any
/// type checking (`noHir`), even with no good check to answer from: a
/// project opened with one. The repaired copy has the import blank, so
/// the rest of the file is answered, but not what uses the import's
/// names. The same for such an import in a module the file imports,
/// which isn't open.
#[test]
fn an_import_of_a_module_that_doesnt_exist_is_blank_in_the_copy() {
    let root = crate::common::tmp().join("repaired-missing-module");
    let _ = std::fs::remove_dir_all(&root);
    let main = "import app.util\nimport app.nope\nimport app.nope.deeper (Thing)\n\n\
                type async Main {\n  fn async main {\n    let size = util.size\n  }\n\n  \
                fn uses -> Int {\n    nope.value + 1\n  }\n}\n";
    let util = "import app.missing\n\nfn pub size -> Int {\n  42\n}\n";

    std::fs::create_dir_all(root.join("src/app")).unwrap();
    std::fs::write(root.join("src/main.inko"), main).unwrap();
    std::fs::write(root.join("src/app/util.inko"), util).unwrap();

    let root = root.canonicalize().unwrap();
    let file = root.join("src/main.inko");
    let mut client = crate::common::client::Client::initialized(
        &root,
        json!({ "statusNotifications": true, "diagnostics": false }),
    );
    client.open(&file, main);

    let mut hover = |needle: &str, offset: usize| {
        let (line, character) = at(main, needle, offset, false);
        let (id, response) = client.hover(&file, line, character);

        (
            response["result"].clone(),
            client.status(id).unwrap().clone(),
        )
    };

    let (result, status) = hover("let size", 4);

    assert_repaired(&status);
    assert_eq!(
        result["contents"]["value"], "```inko\nInt\n```",
        "{}",
        status
    );

    // `uses` mentions `nope`, a name in a blank import.
    let (result, status) = hover("nope.value + 1", 11);

    assert_eq!(result, Value::Null);
    assert_eq!(status["reason"]["code"], "noHir", "{}", status);
}
