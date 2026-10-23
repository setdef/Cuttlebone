//! End-to-end tests: launch the real `cuttlebone` binary and drive it over
//! stdio as an LSP client would (docs/lsp.md).
//!
//! `initialize` runs `inko --version` and `inko print std`, so most tests use
//! a fake `inko` script that reports the pinned version and the pinned std,
//! and so don't depend on what's installed.
// Tests report why a test was skipped on stderr, which `cargo test` shows.
#![allow(clippy::print_stderr)]
mod common;

use common::client::{Client, Launch, at, fake_inko, uri};
use common::fixture;
use cuttlebone::check::upstream_version;
use cuttlebone::worker::FAULT_VARIABLE;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const CONTENT_MODIFIED: i64 = -32801;
const REQUEST_CANCELLED: i64 = -32800;
const SERVER_NOT_INITIALIZED: i64 = -32002;

fn members() -> (PathBuf, PathBuf, String) {
    let root = fixture("members");
    let file = root.join("src/main.inko");
    let text = std::fs::read_to_string(&file).unwrap();

    (root, file, text)
}

/// The type a hover shows, from its Markdown code block.
fn hover_type(response: &Value) -> &str {
    let value = response["result"]["contents"]["value"]
        .as_str()
        .unwrap_or_else(|| panic!("no hover: {}", response));

    assert_eq!(response["result"]["contents"]["kind"], "markdown");
    value
        .strip_prefix("```inko\n")
        .and_then(|v| v.strip_suffix("\n```"))
        .unwrap_or_else(|| panic!("not a code block: {:?}", value))
}

fn labels(response: &Value) -> Vec<&str> {
    response["result"]["items"]
        .as_array()
        .unwrap_or_else(|| panic!("no completion list: {}", response))
        .iter()
        .map(|i| i["label"].as_str().unwrap())
        .collect()
}

fn item<'a>(response: &'a Value, label: &str) -> &'a Value {
    response["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["label"] == label)
        .unwrap_or_else(|| panic!("no {} in {}", label, response))
}

// Tests that count worker processes turn diagnostics off, because a
// diagnostics check runs in a worker of its own (tests/publish.rs tests
// those).
fn status_options() -> Value {
    json!({ "statusNotifications": true })
}

/// A copy of a fixture in a fresh temporary directory, for tests that edit
/// files or need a fault injected into one path.
fn copy_fixture(name: &str, as_name: &str) -> PathBuf {
    let to = Path::new(env!("CARGO_TARGET_TMPDIR")).join(as_name);
    let _ = std::fs::remove_dir_all(&to);

    copy_dir(&fixture(name), &to);
    to.canonicalize().unwrap()
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();

    for entry in std::fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let target = to.join(path.file_name().unwrap());

        if path.is_dir() {
            copy_dir(&path, &target);
        } else {
            std::fs::copy(&path, &target).unwrap();
        }
    }
}

// The handshake.

#[test]
fn initialize_advertises_only_what_works() {
    let mut client = Client::start();
    let response = client.initialize(&fixture("members"), json!({}), json!({}));
    let result = &response["result"];
    let capabilities = result["capabilities"].as_object().unwrap();
    let mut keys: Vec<_> = capabilities.keys().map(String::as_str).collect();

    keys.sort();
    assert_eq!(
        keys,
        [
            "completionProvider",
            "definitionProvider",
            "experimental",
            "hoverProvider",
            "positionEncoding",
            "referencesProvider",
            "textDocumentSync",
            "typeDefinitionProvider",
        ]
    );
    assert_eq!(capabilities["hoverProvider"], true);
    assert_eq!(capabilities["definitionProvider"], true);
    assert_eq!(capabilities["typeDefinitionProvider"], true);
    assert_eq!(capabilities["referencesProvider"], true);
    assert_eq!(
        capabilities["completionProvider"],
        json!({ "triggerCharacters": ["."], "resolveProvider": false })
    );
    assert_eq!(
        capabilities["textDocumentSync"],
        json!({ "openClose": true, "change": 2, "save": { "includeText": true } })
    );
    assert_eq!(capabilities["positionEncoding"], "utf-16");
    assert_eq!(
        capabilities["experimental"]["cuttlebone"],
        json!({
            "inko": "0.21.1",
            "std": cuttlebone::dump::upstream_std(),
            "statusNotifications": false,
        })
    );
    assert_eq!(
        result["serverInfo"],
        json!({ "name": "cuttlebone", "version": env!("CARGO_PKG_VERSION") })
    );
}

#[test]
fn utf8_is_used_when_the_client_prefers_it() {
    let cases = [
        (json!(["utf-8", "utf-16"]), "utf-8"),
        (json!(["utf-16", "utf-8"]), "utf-16"),
        (json!(["utf-32", "utf-8"]), "utf-8"),
        (json!(["utf-32"]), "utf-16"),
        (json!([]), "utf-16"),
    ];

    for (offered, chosen) in cases {
        let mut client = Client::start();
        let response = client.initialize(
            &fixture("members"),
            json!({}),
            json!({ "general": { "positionEncodings": offered } }),
        );

        assert_eq!(
            response["result"]["capabilities"]["positionEncoding"], chosen,
            "offered {}",
            offered
        );
    }
}

#[test]
fn a_different_inko_version_is_refused_with_a_clear_message() {
    let mut client = Client::start();
    let response = client.initialize(
        &fixture("members"),
        json!({ "inko": fake_inko("0.20.0") }),
        json!({}),
    );
    let error = &response["error"];

    assert!(response.get("result").is_none(), "{}", response);
    assert_eq!(
        error["message"],
        format!(
            "Cuttlebone {} works with Inko 0.21.1 only, but {} is Inko 0.20.0. \
             Install Inko 0.21.1, or use a build of Cuttlebone for Inko 0.20.0.",
            env!("CARGO_PKG_VERSION"),
            fake_inko("0.20.0").display()
        )
    );
    assert_eq!(
        error["data"],
        json!({
            "retry": false,
            "code": "inkoVersionMismatch",
            "cuttlebone": env!("CARGO_PKG_VERSION"),
            "supported": "0.21.1",
            "installed": "0.20.0",
        })
    );

    // Nothing else works until a handshake succeeds.
    let (_, response) = client.hover(&fixture("members").join("src/main.inko"), 0, 0);

    assert_eq!(response["error"]["code"], SERVER_NOT_INITIALIZED);

    // And a later one can.
    let response = client.initialize(&fixture("members"), json!({}), json!({}));

    assert!(response.get("result").is_some(), "{}", response);
}

#[test]
fn a_missing_inko_is_refused() {
    let mut client = Client::start();
    let response = client.initialize(
        &fixture("members"),
        json!({ "inko": "/does/not/exist/inko" }),
        json!({}),
    );

    assert_eq!(response["error"]["data"]["code"], "inkoNotFound");
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("couldn't run `/does/not/exist/inko --version`"),
        "{}",
        response
    );
}

#[test]
fn the_installed_inko_works_when_its_version_matches() {
    // Skipped (passing) unless `inko` on PATH is the pinned version.
    let Ok(out) = std::process::Command::new("inko").arg("--version").output() else {
        eprintln!("skipped: no inko on PATH");
        return;
    };
    let version = String::from_utf8_lossy(&out.stdout)
        .trim()
        .trim_start_matches("inko ")
        .to_string();

    if version != upstream_version() {
        eprintln!(
            "skipped: inko on PATH is {}, not {}",
            version,
            upstream_version()
        );
        return;
    }

    let (root, file, text) = members();
    let mut client = Client::start();
    let response = client.initialize(&root, json!({ "inko": null }), json!({}));

    assert!(response.get("result").is_some(), "{}", response);
    client.notify("initialized", json!({}));

    let (line, character) = at(&text, "name.size", 5, false);
    let (_, response) = client.hover(&file, line, character);

    assert_eq!(hover_type(&response), "Int");
}

// Documents, hover and completion.

#[test]
fn a_session_opens_edits_hovers_completes_and_closes() {
    let (root, file, text) = members();
    let mut client = Client::initialized(&root, json!({}));

    // A file that isn't open is read from disk.
    let (line, character) = at(&text, "name.size", 5, false);
    let (_, response) = client.hover(&file, line, character);

    assert_eq!(hover_type(&response), "Int");
    assert_eq!(
        response["result"]["range"],
        json!({
            "start": { "line": line, "character": character - 5 },
            "end": { "line": line, "character": character + 4 },
        })
    );

    client.open(&file, &text);

    // An incremental edit: `'héllo'` becomes `42`.
    let (line, start) = at(&text, "'héllo'", 0, false);

    client.change(
        &file,
        2,
        json!([{
            "range": {
                "start": { "line": line, "character": start },
                "end": { "line": line, "character": start + 7 },
            },
            "text": "42",
        }]),
    );

    let (_, response) = client.hover(&file, line, start);

    assert_eq!(hover_type(&response), "Int");

    // Typing `name.` on a new line, then asking for completion after the
    // `.` with nothing after it, as an editor does.
    let edited = text.replacen("'héllo'", "42", 1);
    let (line, end) = at(&edited, "let total = counter.count + size", 32, false);

    client.change(
        &file,
        3,
        json!([{
            "range": {
                "start": { "line": line, "character": end },
                "end": { "line": line, "character": end },
            },
            "text": "\n    counter.",
        }]),
    );

    let (_, response) = client.complete(&file, line + 1, 12);
    let names = labels(&response);

    assert_eq!(response["result"]["isIncomplete"], false);
    assert!(names.contains(&"count"), "{:?}", names);
    assert!(names.contains(&"label"), "{:?}", names);
    assert_eq!(
        item(&response, "count"),
        &json!({
            "label": "count",
            "kind": 2,
            "detail": "fn pub count -> Int",
            "labelDetails": { "description": "main" },
        })
    );
    assert_eq!(
        item(&response, "label"),
        &json!({
            "label": "label",
            "kind": 5,
            "detail": "let pub @label: String",
            "labelDetails": { "description": "main" },
        })
    );

    // With a prefix typed, the same members: the client filters them.
    client.change(
        &file,
        4,
        json!([{
            "range": {
                "start": { "line": line + 1, "character": 12 },
                "end": { "line": line + 1, "character": 12 },
            },
            "text": "co",
        }]),
    );

    let (_, response) = client.complete(&file, line + 1, 14);

    assert!(labels(&response).contains(&"label"));

    // Closing the document goes back to the file on disk.
    client.close(&file);

    let (line, character) = at(&text, "let name", 4, false);
    let (_, response) = client.hover(&file, line, character);

    assert_eq!(hover_type(&response), "String");
}

#[test]
fn a_full_document_change_replaces_the_text() {
    let (root, file, text) = members();
    let mut client = Client::initialized(&root, json!({}));

    client.open(&file, &text);
    client.change(
        &file,
        2,
        json!([{ "text": text.replacen("'héllo'", "4.5", 1) }]),
    );

    let (line, character) = at(&text, "let name", 4, false);
    let (_, response) = client.hover(&file, line, character);

    assert_eq!(hover_type(&response), "Float");
}

#[test]
fn several_changes_in_one_notification_apply_in_order() {
    let (root, file, text) = members();
    let mut client = Client::initialized(&root, json!({}));
    let (line, start) = at(&text, "'héllo'", 0, false);
    let range = |a: u32, b: u32| json!({ "start": { "line": line, "character": a }, "end": { "line": line, "character": b } });

    client.open(&file, &text);
    // 'héllo' -> 1 -> 1.5: the second range is in the first's result.
    client.change(
        &file,
        2,
        json!([
            { "range": range(start, start + 7), "text": "1" },
            { "range": range(start + 1, start + 1), "text": ".5" },
        ]),
    );

    let (_, response) = client.hover(&file, line, start);

    assert_eq!(hover_type(&response), "Float");
}

#[test]
fn unsaved_changes_to_an_imported_module_are_seen_by_its_importers() {
    let (root, file, text) = members();
    let shapes = root.join("src/shapes.inko");
    let shapes_text = std::fs::read_to_string(&shapes).unwrap();
    let mut client = Client::initialized(&root, json!({}));

    client.open(
        &shapes,
        &shapes_text.replace(
            "fn pub area -> Float {",
            "fn pub area -> Int {\n    1\n  }\n\n  fn pub old_area -> Float {",
        ),
    );

    let (line, character) = at(&text, "circle.area", 7, false);
    let (_, response) = client.hover(&file, line, character);

    assert_eq!(hover_type(&response), "Int");
}

#[test]
fn saving_with_text_updates_the_document() {
    let (root, file, text) = members();
    let mut client = Client::initialized(&root, json!({}));

    client.open(&file, &text);
    client.notify(
        "textDocument/didSave",
        json!({
            "textDocument": { "uri": uri(&file) },
            "text": text.replacen("'héllo'", "42", 1),
        }),
    );

    let (line, character) = at(&text, "let name", 4, false);
    let (_, response) = client.hover(&file, line, character);

    assert_eq!(hover_type(&response), "Int");
}

#[test]
fn nothing_at_a_position_is_a_null_result() {
    let (root, file, text) = members();
    let mut client = Client::initialized(&root, status_options());
    let (line, _) = at(&text, "# Receivers", 0, false);
    let (id, response) = client.hover(&file, line, 3);

    assert_eq!(response["result"], Value::Null);
    assert_eq!(client.status(id).unwrap()["status"], "fresh");

    // Completion where nothing has members, e.g. in a comment.
    let (id, response) = client.complete(&file, line, 3);

    assert_eq!(response["result"], Value::Null);
    assert_eq!(client.status(id).unwrap()["status"], "fresh");
}

#[test]
fn a_client_without_markdown_gets_plain_text() {
    let (root, file, text) = members();
    let capabilities = json!({ "textDocument": { "hover": { "contentFormat": ["plaintext"] } } });
    let mut client = Client::initialized_with(Launch::default(), &root, json!({}), capabilities);
    let (line, character) = at(&text, "name.size", 5, false);
    let (_, response) = client.hover(&file, line, character);

    assert_eq!(
        response["result"]["contents"],
        json!({ "kind": "plaintext", "value": "Int" })
    );
}

#[test]
fn a_document_that_isnt_a_file_has_no_answer() {
    let (root, _, _) = members();
    let mut client = Client::initialized(&root, status_options());
    let id = client.send_request(
        "textDocument/hover",
        json!({
            "textDocument": { "uri": "untitled:Untitled-1" },
            "position": { "line": 0, "character": 0 },
        }),
    );
    let response = client.response(id);

    assert_eq!(response["result"], Value::Null);
    assert_eq!(client.status(id).unwrap()["reason"]["code"], "notAFile");
}

#[test]
fn workspace_folders_are_project_roots() {
    // The workspace folder's inko.pkg (and dep/) apply to a file in a nested
    // src/ directory. Without the folder as a root, the nearest directory
    // with a src/ would be the project, with no manifest.
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("nested-roots");
    let _ = std::fs::remove_dir_all(&root);

    std::fs::create_dir_all(root.join("src/inner/src")).unwrap();
    std::fs::write(root.join("inko.pkg"), "require inko 99.0.0\n").unwrap();

    let text = "fn example {\n  let a = 1\n}\n";
    let file = root.join("src/inner/src/user.inko");

    std::fs::write(&file, text).unwrap();

    let root = root.canonicalize().unwrap();
    let file = file.canonicalize().unwrap();
    let (line, character) = at(text, "let a", 4, false);

    for (folders, refused) in [
        (vec![fixture("members"), root.clone()], true),
        (vec![fixture("members")], false),
    ] {
        let mut client = Client::launch(Launch::default());
        let folders: Vec<_> = folders
            .iter()
            .map(|f| json!({ "uri": uri(f), "name": "folder" }))
            .collect();
        let response = client.request(
            "initialize",
            json!({
                "processId": null,
                "rootUri": null,
                "workspaceFolders": folders,
                "capabilities": {},
                "initializationOptions": {
                    "inko": fake_inko(upstream_version()),
                    "statusNotifications": true,
                },
            }),
        );

        assert!(response.get("result").is_some(), "{}", response);
        client.notify("initialized", json!({}));

        let (id, response) = client.hover(&file, line, character);

        if refused {
            assert_eq!(response["result"], Value::Null);
            assert!(
                client.status(id).unwrap()["reason"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("requires Inko 99.0.0")
            );
        } else {
            assert_eq!(hover_type(&response), "Int");
        }
    }
}

// Positions.

#[test]
fn non_ascii_positions_and_ranges_in_utf16() {
    non_ascii(false);
}

#[test]
fn non_ascii_positions_and_ranges_in_utf8() {
    non_ascii(true);
}

fn non_ascii(utf8: bool) {
    let root = fixture("unicode");
    let file = root.join("src/main.inko");
    let text = std::fs::read_to_string(&file).unwrap();
    let offered = if utf8 { "utf-8" } else { "utf-16" };
    let mut client = Client::initialized_with(
        Launch::default(),
        &root,
        json!({}),
        json!({ "general": { "positionEncodings": [offered] } }),
    );

    for (needle, offset, typ, length) in [
        // After é (2 bytes, 1 UTF-16 unit).
        ("1.to_string", 2, "String", 11),
        // After two 😀 (4 bytes, 2 units each).
        ("2.to_string", 2, "String", 11),
        // After e + U+0301 isn't in the fixture; after a ZWJ family (18
        // bytes, 8 units, 1 cluster).
        ("4.5.to_string", 4, "String", 13),
        // After CJK (3 bytes, 1 unit each).
        ("[3].size", 4, "Int", 8),
        // The string itself, whose range covers the non-ASCII text.
        ("'😀😀'", 1, "String", 0),
    ] {
        let (line, character) = at(&text, needle, offset, utf8);
        let (_, response) = client.hover(&file, line, character);

        assert_eq!(hover_type(&response), typ, "at {}", needle);

        let (_, start) = at(&text, needle, 0, utf8);
        let end = if length > 0 {
            start + length
        } else {
            at(&text, needle, needle.chars().count(), utf8).1
        };

        assert_eq!(
            response["result"]["range"],
            json!({
                "start": { "line": line, "character": start },
                "end": { "line": line, "character": end },
            }),
            "range of {} in {}",
            needle,
            offered
        );
    }

    // An incremental edit after non-ASCII text: `2` becomes `2.5`, so
    // `2.5.to_string` is still a String but `2.5` is a Float.
    client.open(&file, &text);

    let (line, character) = at(&text, "2.to_string", 1, utf8);

    client.change(
        &file,
        2,
        json!([{
            "range": {
                "start": { "line": line, "character": character },
                "end": { "line": line, "character": character },
            },
            "text": ".5",
        }]),
    );

    let (_, response) = client.hover(&file, line, character - 1);

    assert_eq!(hover_type(&response), "Float", "{}", offered);
}

// Status.

#[test]
fn status_notifications_report_why_there_is_no_answer() {
    let root = fixture("broken");
    let file = root.join("src/main.inko");
    let text = std::fs::read_to_string(&file).unwrap();
    let mut client = Client::initialized(&root, status_options());
    let (line, character) = at(&text, "values.size", 0, false);

    client.open(&file, &text);

    // Fresh.
    let (id, response) = client.hover(&file, line, character);
    let status = client.status(id).unwrap().clone();

    assert!(response["result"].is_object());
    assert_eq!(status["status"], "fresh");
    assert_eq!(status["method"], "textDocument/hover");
    assert_eq!(status["textDocument"]["uri"], uri(&file));
    assert!(status["elapsedMs"].as_f64().unwrap() > 0.0, "{}", status);
    assert!(status.get("reason").is_none());

    // A syntax error: nothing was type checked.
    client.change(
        &file,
        2,
        json!([{ "text": text.replacen("let mut i = 0", "let mut i = )", 1) }]),
    );

    let (id, response) = client.hover(&file, line, character);

    assert_eq!(response["result"], Value::Null);
    assert_eq!(client.status(id).unwrap()["status"], "unavailable");
    assert_eq!(client.status(id).unwrap()["reason"]["code"], "noHir");

    let (id, response) = client.complete(&file, line, character + 7);

    assert_eq!(response["result"], Value::Null);
    assert_eq!(
        client.status(id).unwrap()["method"],
        "textDocument/completion"
    );
    assert_eq!(client.status(id).unwrap()["reason"]["code"], "noHir");

    // An error in a declaration.
    let broken = text.replacen("-> Int", "-> Intt", 1);

    assert_ne!(broken, text);
    client.change(&file, 3, json!([{ "text": broken }]));

    let (id, response) = client.hover(&file, line, character);

    assert_eq!(response["result"], Value::Null);
    assert_eq!(
        client.status(id).unwrap()["reason"]["code"],
        "declarationErrors"
    );
    assert!(
        client.status(id).unwrap()["reason"]["message"]
            .as_str()
            .unwrap()
            .contains("declaration")
    );
}

#[test]
fn without_opting_in_there_are_no_status_notifications() {
    let (root, file, text) = members();
    let mut client = Client::initialized(&root, json!({}));
    let (line, character) = at(&text, "name.size", 5, false);

    client.hover(&file, line, character);
    client.complete(&file, line, character);
    client.hover(&file, 9999, 0);

    assert!(
        client.notifications.is_empty(),
        "{:?}",
        client.notifications
    );
}

// Robustness.

#[test]
fn a_compiler_panic_is_unavailable_and_the_server_keeps_going() {
    let root = fixture("panic");
    let file = root.join("src/main.inko");
    let text = std::fs::read_to_string(&file).unwrap();
    let mut client = Client::initialized(&root, status_options());
    let (line, character) = at(&text, "let value", 4, false);

    for _ in 0..2 {
        let (id, response) = client.hover(&file, line, character);

        assert_eq!(response["result"], Value::Null);

        let reason = &client.status(id).unwrap()["reason"];

        assert_eq!(reason["code"], "compilerPanic");
        assert!(
            reason["message"]
                .as_str()
                .unwrap()
                .contains("The type std.map.Map isn't defined"),
            "{}",
            reason
        );
    }

    let (_, other, text) = members();
    let (line, character) = at(&text, "let name", 4, false);
    let (_, response) = client.hover(&other, line, character);

    assert_eq!(hover_type(&response), "String");
}

#[test]
fn a_hung_check_times_out_is_killed_and_the_server_keeps_going() {
    let root = copy_fixture("members", "fault-hang");
    let main = root.join("src/main.inko");
    let shapes = root.join("src/shapes.inko");
    let text = std::fs::read_to_string(&main).unwrap();
    let launch = Launch {
        env: vec![(
            FAULT_VARIABLE,
            "hang:fault-hang/src/shapes.inko".to_string(),
        )],
        cwd: None,
    };
    let mut client = Client::initialized_with(
        launch,
        &root,
        json!({ "statusNotifications": true, "checkTimeoutMs": 500, "diagnostics": false }),
        json!({}),
    );
    let start = Instant::now();
    let (id, response) = client.hover(&shapes, 0, 0);
    let elapsed = start.elapsed();

    assert_eq!(response["result"], Value::Null);
    assert_eq!(client.status(id).unwrap()["reason"]["code"], "timeout");
    assert_eq!(
        client.status(id).unwrap()["reason"]["message"],
        "the check took longer than 500 ms, so it was stopped"
    );
    assert!(
        elapsed >= Duration::from_millis(500) && elapsed < Duration::from_secs(5),
        "{:?}",
        elapsed
    );
    assert!(
        client.workers().is_empty(),
        "the hung worker is still running"
    );
    assert!(client.stderr.lock().unwrap().contains("timed out"));

    let (line, character) = at(&text, "name.size", 5, false);
    let (_, response) = client.hover(&main, line, character);

    assert_eq!(hover_type(&response), "Int");
}

#[test]
fn a_hung_worker_exits_by_itself_if_the_server_dies() {
    let root = copy_fixture("members", "fault-orphan");
    let shapes = root.join("src/shapes.inko");
    let launch = Launch {
        env: vec![(
            FAULT_VARIABLE,
            "hang:fault-orphan/src/shapes.inko".to_string(),
        )],
        cwd: None,
    };
    let mut client = Client::initialized_with(
        launch,
        &root,
        json!({ "checkTimeoutMs": 500, "diagnostics": false }),
        json!({}),
    );

    client.send_at("textDocument/hover", &shapes, 0, 0);
    wait_for_a_worker(&client);

    let worker = client.workers()[0];

    // SIGKILL: the server gets no chance to kill its worker.
    client.child.kill().unwrap();
    client.child.wait().unwrap();

    // The worker's own deadline is the timeout plus a second.
    let start = Instant::now();

    while alive(worker) {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "the orphaned worker {} is still running",
            worker
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}

#[test]
fn an_aborting_check_is_unavailable_and_the_server_keeps_going() {
    let root = copy_fixture("members", "fault-abort");
    let main = root.join("src/main.inko");
    let shapes = root.join("src/shapes.inko");
    let text = std::fs::read_to_string(&main).unwrap();
    let launch = Launch {
        env: vec![(
            FAULT_VARIABLE,
            "abort:fault-abort/src/shapes.inko".to_string(),
        )],
        cwd: None,
    };
    let mut client = Client::initialized_with(launch, &root, status_options(), json!({}));

    for _ in 0..2 {
        let (id, response) = client.hover(&shapes, 0, 0);

        assert_eq!(response["result"], Value::Null);

        let reason = &client.status(id).unwrap()["reason"];

        assert_eq!(reason["code"], "checkCrashed");
        // SIGABRT.
        assert!(
            reason["message"]
                .as_str()
                .unwrap()
                .starts_with("the check was killed by signal 6"),
            "{}",
            reason
        );
    }

    let (line, character) = at(&text, "name.size", 5, false);
    let (_, response) = client.hover(&main, line, character);

    assert_eq!(hover_type(&response), "Int");
}

#[test]
fn cancelling_a_running_check_kills_it() {
    let root = copy_fixture("members", "fault-cancel");
    let main = root.join("src/main.inko");
    let shapes = root.join("src/shapes.inko");
    let text = std::fs::read_to_string(&main).unwrap();
    let launch = Launch {
        env: vec![(
            FAULT_VARIABLE,
            "hang:fault-cancel/src/shapes.inko".to_string(),
        )],
        cwd: None,
    };
    let mut client =
        Client::initialized_with(launch, &root, json!({ "diagnostics": false }), json!({}));
    let id = client.send_at("textDocument/hover", &shapes, 0, 0);

    wait_for_a_worker(&client);
    client.notify("$/cancelRequest", json!({ "id": id }));

    let response = client.response(id);

    assert_eq!(response["error"]["code"], REQUEST_CANCELLED, "{}", response);
    assert!(client.workers().is_empty());

    let (line, character) = at(&text, "name.size", 5, false);
    let (_, response) = client.hover(&main, line, character);

    assert_eq!(hover_type(&response), "Int");
}

#[test]
fn cancelling_a_queued_request_answers_it_without_a_check() {
    let root = copy_fixture("members", "fault-cancel-queued");
    let main = root.join("src/main.inko");
    let shapes = root.join("src/shapes.inko");
    let launch = Launch {
        env: vec![(
            FAULT_VARIABLE,
            "hang:fault-cancel-queued/src/shapes.inko".to_string(),
        )],
        cwd: None,
    };
    let mut client = Client::initialized_with(
        launch,
        &root,
        json!({ "checkTimeoutMs": 2000, "diagnostics": false }),
        json!({}),
    );
    // The first hangs, so the second waits behind it.
    let first = client.send_at("textDocument/hover", &shapes, 0, 0);
    let second = client.send_at("textDocument/hover", &main, 0, 0);

    wait_for_a_worker(&client);
    client.notify("$/cancelRequest", json!({ "id": second }));

    let start = Instant::now();
    let response = client.response(second);

    assert_eq!(response["error"]["code"], REQUEST_CANCELLED);
    assert!(start.elapsed() < Duration::from_millis(1000));
    // The first is still running, until it times out.
    assert_eq!(client.workers().len(), 1);
    assert_eq!(client.response(first)["result"], Value::Null);
}

#[test]
fn an_edit_cancels_a_running_check_on_that_document() {
    let root = copy_fixture("members", "fault-edit");
    let shapes = root.join("src/shapes.inko");
    let shapes_text = std::fs::read_to_string(&shapes).unwrap();
    let main = root.join("src/main.inko");
    let text = std::fs::read_to_string(&main).unwrap();
    let launch = Launch {
        env: vec![(
            FAULT_VARIABLE,
            "hang:fault-edit/src/shapes.inko".to_string(),
        )],
        cwd: None,
    };
    let mut client =
        Client::initialized_with(launch, &root, json!({ "diagnostics": false }), json!({}));

    client.open(&shapes, &shapes_text);
    client.open(&main, &text);

    // A check on another document isn't cancelled by the edit.
    let id = client.send_at("textDocument/hover", &shapes, 0, 0);

    wait_for_a_worker(&client);
    client.change(&shapes, 2, json!([{ "text": shapes_text.clone() + "\n" }]));

    let response = client.response(id);

    assert_eq!(response["error"]["code"], CONTENT_MODIFIED, "{}", response);
    assert!(client.workers().is_empty());

    let (line, character) = at(&text, "name.size", 5, false);
    let id = client.send_at("textDocument/hover", &main, line, character);

    client.change(&shapes, 3, json!([{ "text": shapes_text }]));
    assert_eq!(hover_type(&client.response(id)), "Int");
}

#[test]
fn checks_wait_until_typing_pauses() {
    let (root, file, text) = members();
    let mut client = Client::initialized(&root, json!({ "debounceMs": 400, "diagnostics": false }));
    let (line, character) = at(&text, "name.size", 5, false);

    client.open(&file, &text);

    // No edit yet: no wait.
    let start = Instant::now();

    client.hover(&file, line, character);
    assert!(
        start.elapsed() < Duration::from_millis(400),
        "{:?}",
        start.elapsed()
    );

    // Right after an edit: at least the debounce.
    client.change(&file, 2, json!([{ "text": text.clone() }]));

    let start = Instant::now();
    let (_, response) = client.hover(&file, line, character);

    assert_eq!(hover_type(&response), "Int");
    assert!(
        start.elapsed() >= Duration::from_millis(400),
        "{:?}",
        start.elapsed()
    );

    // An edit during the wait supersedes the request before any check
    // starts.
    client.change(&file, 3, json!([{ "text": text.clone() }]));

    let start = Instant::now();
    let id = client.send_at("textDocument/hover", &file, line, character);

    std::thread::sleep(Duration::from_millis(100));
    assert!(client.workers().is_empty());
    client.change(&file, 4, json!([{ "text": text.clone() }]));

    let response = client.response(id);

    assert_eq!(response["error"]["code"], CONTENT_MODIFIED);
    assert!(
        start.elapsed() < Duration::from_millis(400),
        "{:?}",
        start.elapsed()
    );
}

fn wait_for_a_worker(client: &Client) {
    let start = Instant::now();

    while client.workers().is_empty() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "no worker started"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

// Side effects.

#[test]
fn queries_never_sync_dependencies_or_touch_the_data_directory() {
    // `inko check` here would create and lock the packages directory in the
    // data directory, then try to sync dep/ (there's no dep/hash), cloning
    // a URL that doesn't exist. The server runs in the project's directory,
    // as editors often start it, with a fresh home directory.
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("no-side-effects");
    let home = Path::new(env!("CARGO_TARGET_TMPDIR")).join("no-side-effects-home");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&home);

    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        root.join("inko.pkg"),
        "require file:///does/not/exist/package 1.0.0 0123456789012345678901234567890123456789\n",
    )
    .unwrap();

    let text = "fn example {\n  let a = 1\n}\n";
    let file = root.join("src/main.inko");

    std::fs::write(&file, text).unwrap();

    let root = root.canonicalize().unwrap();
    let launch = Launch {
        env: vec![
            ("HOME", home.display().to_string()),
            ("XDG_DATA_HOME", home.join("data").display().to_string()),
        ],
        cwd: Some(root.clone()),
    };
    let mut client = Client::initialized_with(launch, &root, status_options(), json!({}));
    let (line, character) = at(text, "let a", 4, false);
    let (id, response) = client.hover(&root.join("src/main.inko"), line, character);

    assert_eq!(client.status(id).unwrap()["status"], "fresh");
    assert_eq!(hover_type(&response), "Int");

    let (_, response) = client.complete(&root.join("src/main.inko"), line, character);

    assert!(response.get("error").is_none());

    // Diagnostics run a full check, including MIR, in the same way.
    let status = client.diagnostics_status();

    assert_eq!(status["status"], "fresh", "{}", status);
    assert!(client.notifications.iter().any(|n| {
        n["method"] == "textDocument/publishDiagnostics"
            && n["params"]["diagnostics"][0]["code"] == "unused-symbol"
    }));
    assert!(!root.join("dep").exists());
    assert!(!root.join("build").exists());

    let created: Vec<_> = std::fs::read_dir(&home).unwrap().collect();

    assert!(created.is_empty(), "{:?}", created);
}

// The protocol.

#[test]
fn shutdown_then_exit_exits_with_status_0() {
    let (root, file, _) = members();
    let mut client = Client::initialized(&root, json!({}));

    assert_eq!(
        client.request("shutdown", Value::Null)["result"],
        Value::Null
    );

    let (_, response) = client.hover(&file, 0, 0);

    assert_eq!(response["error"]["code"], -32600);
    client.notify("exit", Value::Null);
    assert_eq!(client.wait(), Some(0));
}

#[test]
fn exit_without_shutdown_exits_with_status_1() {
    let (root, _, _) = members();
    let mut client = Client::initialized(&root, json!({}));

    client.notify("exit", Value::Null);
    assert_eq!(client.wait(), Some(1));

    // Also before initializing.
    let mut client = Client::start();

    client.notify("exit", Value::Null);
    assert_eq!(client.wait(), Some(1));
}

#[test]
fn the_server_exits_when_its_input_ends() {
    let (root, _, _) = members();
    let mut client = Client::initialized(&root, json!({}));

    client.close_input();
    assert_eq!(client.wait(), Some(1));
}

#[test]
fn shutdown_kills_a_running_check() {
    let root = copy_fixture("members", "fault-shutdown");
    let shapes = root.join("src/shapes.inko");
    let launch = Launch {
        env: vec![(
            FAULT_VARIABLE,
            "hang:fault-shutdown/src/shapes.inko".to_string(),
        )],
        cwd: None,
    };
    let mut client =
        Client::initialized_with(launch, &root, json!({ "diagnostics": false }), json!({}));
    let id = client.send_at("textDocument/hover", &shapes, 0, 0);

    wait_for_a_worker(&client);

    let shutdown = client.send_request("shutdown", Value::Null);

    assert_eq!(client.response(id)["error"]["code"], REQUEST_CANCELLED);
    assert_eq!(client.response(shutdown)["result"], Value::Null);
    assert!(client.workers().is_empty());
}

#[test]
fn unknown_methods_and_invalid_params_get_errors_and_the_server_keeps_going() {
    let (root, file, text) = members();
    let mut client = Client::initialized(&root, json!({}));

    assert_eq!(
        client.request("textDocument/implementation", json!({}))["error"]["code"],
        -32601
    );
    assert_eq!(
        client.request("textDocument/hover", json!({ "textDocument": 3 }))["error"]["code"],
        -32602
    );
    assert_eq!(
        client.request("initialize", json!({ "capabilities": {} }))["error"]["code"],
        -32600
    );

    // Unknown notifications are ignored.
    client.notify(
        "workspace/didChangeConfiguration",
        json!({ "settings": {} }),
    );

    let (line, character) = at(&text, "name.size", 5, false);
    let (_, response) = client.hover(&file, line, character);

    assert_eq!(hover_type(&response), "Int");
}

#[test]
fn a_position_past_the_end_of_the_file_has_no_answer() {
    let (root, file, _) = members();
    let mut client = Client::initialized(&root, status_options());
    let (id, response) = client.hover(&file, 9999, 0);

    assert_eq!(response["result"], Value::Null);
    assert_eq!(
        client.status(id).unwrap()["reason"]["code"],
        "invalidPosition"
    );

    let (id, response) = client.complete(&file, 9999, 0);

    assert_eq!(response["result"], Value::Null);
    assert_eq!(
        client.status(id).unwrap()["reason"]["code"],
        "invalidPosition"
    );
}
