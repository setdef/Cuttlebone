//! Conformance to LSP 3.18 beyond what every test checks (each message the
//! test client receives goes through `common::spec`): that those checks
//! catch what they should, and what a client with none of the optional
//! capabilities gets.
use crate::common::client::{Client, Launch, at};
use crate::common::fixture;
use crate::common::spec::{capable_client, spec};
use serde_json::{Value, json};
use std::path::Path;

/// A session with a client that sends no capabilities at all.
fn bare(root: &Path, options: Value) -> Client {
    let mut client = Client::launch(Launch::default());
    let response = client.initialize_exactly(root, options, json!({}));

    assert!(response.get("result").is_some(), "{}", response);
    client.notify("initialized", json!({}));
    client
}

/// Every message these sessions get is checked against the client's
/// capabilities too (`spec::Spec::check`), so these check what's there
/// instead.
#[test]
fn hover_and_completion_without_optional_capabilities() {
    let root = fixture("members");
    let file = root.join("src/main.inko");
    let text = std::fs::read_to_string(&file).unwrap();
    let mut client = bare(&root, json!({ "diagnostics": false }));

    client.open(&file, &text);

    let (line, character) = at(&text, "shapes.PI", 0, false);
    let (_, hover) = client.hover(&file, line, character);

    assert_eq!(
        hover["result"]["contents"]["kind"], "plaintext",
        "{}",
        hover
    );

    let (_, completion) = client.complete(&file, line, character + 7);
    let items = completion["result"]["items"].as_array().unwrap();
    let pi = items.iter().find(|i| i["label"] == "PI").unwrap();

    // Value (12), not Constant (21), and no labelDetails.
    assert_eq!(
        pi,
        &json!({ "label": "PI", "kind": 12, "detail": "let pub PI: Float" })
    );
    assert!(items.iter().all(|i| i.get("labelDetails").is_none()));

    // A client that has them gets both.
    let mut client = Client::initialized(&root, json!({ "diagnostics": false }));

    client.open(&file, &text);

    let (_, completion) = client.complete(&file, line, character + 7);
    let items = completion["result"]["items"].as_array().unwrap();

    assert_eq!(
        items.iter().find(|i| i["label"] == "PI").unwrap(),
        &json!({
            "label": "PI",
            "kind": 21,
            "detail": "let pub PI: Float",
            "labelDetails": { "description": "shapes" },
        })
    );
}

#[test]
fn signature_help_without_optional_capabilities() {
    let root = fixture("features");
    let file = root.join("src/main.inko");
    let text = std::fs::read_to_string(&file)
        .unwrap()
        .replace("counter.add(total, 2)", "counter.add(total, 2, 3)");
    let mut client = bare(&root, json!({ "diagnostics": false }));

    client.open(&file, &text);

    let help = |client: &mut Client, offset: usize| {
        let (line, character) = at(&text, "counter.add(total, 2, 3)", offset, false);
        let id = client.send_at("textDocument/signatureHelp", &file, line, character);

        client.response(id)["result"].clone()
    };

    // Parameters as text from the signature, and the active parameter only
    // on the help.
    assert_eq!(
        help(&mut client, 19),
        json!({
            "signatures": [{
                "label": "fn mut add(amount: Int, times: Int)",
                "parameters": [{ "label": "amount: Int" }, { "label": "times: Int" }],
            }],
            "activeSignature": 0,
            "activeParameter": 1,
        })
    );
    // Past the last parameter, none is active, which only a client that
    // supports `null` can be told.
    assert_eq!(
        help(&mut client, 22),
        json!({
            "signatures": [{
                "label": "fn mut add(amount: Int, times: Int)",
                "parameters": [{ "label": "amount: Int" }, { "label": "times: Int" }],
            }],
            "activeSignature": 0,
        })
    );
}

#[test]
fn diagnostics_without_tag_support_have_no_tags() {
    let root = fixture("diagnostics/moves");
    let mut client = bare(&root, json!({ "diagnosticsDebounceMs": 0 }));
    let unused = client.wait_for(|n| {
        n["method"] == "textDocument/publishDiagnostics"
            && n["params"]["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["code"] == "unused-symbol")
    });

    for d in unused["params"]["diagnostics"].as_array().unwrap() {
        assert!(d.get("tags").is_none(), "{}", d);
    }
}

/// The capabilities the tests send are the spec's: a capability in the
/// wrong place would test what no client sends.
#[test]
fn the_tests_capabilities_conform() {
    spec()
        .conforms("ClientCapabilities", &capable_client())
        .unwrap();
}

#[test]
fn the_checks_catch_what_they_check() {
    let spec = spec();
    let full = capable_client();
    let ok = |m: Value, method: &str| spec.check(&m, Some(method), &full);
    let hover = |contents: Value| serde_json::json!({ "jsonrpc": "2.0", "id": 1, "result": { "contents": contents } });

    assert!(
        ok(
            hover(serde_json::json!({ "kind": "markdown", "value": "x" })),
            "textDocument/hover"
        )
        .is_ok()
    );
    // A property the type doesn't have, a missing one, a wrong type.
    assert!(
        ok(
            hover(serde_json::json!({ "kind": "markdown", "value": "x", "extra": 1 })),
            "textDocument/hover"
        )
        .is_err()
    );
    assert!(
        ok(
            hover(serde_json::json!({ "kind": "markdown" })),
            "textDocument/hover"
        )
        .is_err()
    );
    assert!(
        ok(
            hover(serde_json::json!({ "kind": "html", "value": "x" })),
            "textDocument/hover"
        )
        .is_err()
    );
    // Markdown to a client that doesn't list it.
    assert!(
        spec.check(
            &hover(serde_json::json!({ "kind": "markdown", "value": "x" })),
            Some("textDocument/hover"),
            &serde_json::json!({}),
        )
        .is_err()
    );

    let signature = |active: Value, label: Value| {
        serde_json::json!({ "jsonrpc": "2.0", "id": 1, "result": {
            "signatures": [{ "label": "fn f(a: Int)", "parameters": [{ "label": label }], "activeParameter": active }],
            "activeSignature": 0,
            "activeParameter": active,
        }})
    };
    let bare = serde_json::json!({});

    assert!(
        ok(
            signature(Value::Null, serde_json::json!([5, 11])),
            "textDocument/signatureHelp"
        )
        .is_ok()
    );
    assert!(
        spec.check(
            &signature(Value::Null, serde_json::json!("a: Int")),
            Some("textDocument/signatureHelp"),
            &bare
        )
        .is_err()
    );
    assert!(
        spec.check(
            &signature(serde_json::json!(0), serde_json::json!([5, 11])),
            Some("textDocument/signatureHelp"),
            &bare
        )
        .is_err()
    );

    let diagnostics = serde_json::json!({ "jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {
        "uri": "file:///a.inko",
        "diagnostics": [{ "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } }, "message": "m", "tags": [1] }],
    }});

    assert!(spec.check(&diagnostics, None, &full).is_ok());
    assert!(spec.check(&diagnostics, None, &bare).is_err());
    // Responses need a result or an error, and errors their code.
    assert!(
        ok(
            serde_json::json!({ "jsonrpc": "2.0", "id": 1 }),
            "textDocument/hover"
        )
        .is_err()
    );
    assert!(
        ok(
            serde_json::json!({ "jsonrpc": "2.0", "id": 1, "error": { "message": "m" } }),
            "textDocument/hover"
        )
        .is_err()
    );
    assert!(
        ok(
            serde_json::json!({ "jsonrpc": "2.0", "id": 1, "result": null }),
            "textDocument/hover"
        )
        .is_ok()
    );
}

/// The test client fails a test on a message that doesn't conform: here
/// from a fake server that answers a hover with a property `Hover` doesn't
/// have.
#[test]
#[should_panic(expected = "doesn't conform to LSP 3.18")]
fn the_client_fails_on_a_message_that_does_not_conform() {
    let body = r#"{"jsonrpc":"2.0","id":1,"result":{"contents":"x","extra":1}}"#;
    let dir = crate::common::tmp().join("nonconforming-server");
    let program = dir.join("server");

    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\nprintf 'Content-Length: {}\\r\\n\\r\\n%s' '{}'\nsleep 5\n",
            body.len(),
            body
        ),
    )
    .unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let mut client = Client::launch_program(&program, Launch::default());
    let id = client.send_request(
        "textDocument/hover",
        json!({ "textDocument": { "uri": "file:///a.inko" }, "position": { "line": 0, "character": 0 } }),
    );

    client.response(id);
}
