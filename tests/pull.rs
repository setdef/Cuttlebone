//! Pull diagnostics (`textDocument/diagnostic`, `workspace/diagnostic`)
//! over LSP: the same diagnostics as the core's (which tests/diagnostics.rs
//! holds to the installed `inko check`), for files open or not, in both
//! encodings, unchanged reports by result id, a workspace pull held until
//! something changes, and no pushes to a client that pulls.
use crate::common::client::{Client, Launch, at, uri};
use crate::common::fixture;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn pulling(root: &Path, utf8: bool, refresh: bool) -> Client {
    Client::initialized_with(
        Launch::default(),
        root,
        json!({ "statusNotifications": true, "diagnosticsDebounceMs": 0 }),
        json!({
            "general": { "positionEncodings": if utf8 { ["utf-8"] } else { ["utf-16"] } },
            "textDocument": { "diagnostic": { "dynamicRegistration": false } },
            "workspace": { "diagnostics": { "refreshSupport": refresh } },
        }),
    )
}

fn result(client: &mut Client, method: &str, params: Value) -> Value {
    let response = client.request(method, params);

    assert!(response.get("error").is_none(), "{}", response);
    response["result"].clone()
}

/// The core's diagnostics for a project, as LSP has them (without
/// `source` and `tags`), by URI.
fn core(root: &Path) -> HashMap<String, Vec<Value>> {
    let session = cuttlebone::query::Session {
        std: cuttlebone::dump::upstream_std(),
        projects: vec![root.to_path_buf()],
    };
    let request = cuttlebone::query::Request {
        file: root.to_path_buf(),
        position: cuttlebone::position::Position {
            line: 0,
            character: 0,
        },
        encoding: cuttlebone::position::Encoding::Utf16,
        contents: HashMap::new(),
    };
    let mut out: HashMap<String, Vec<Value>> = HashMap::new();

    for d in cuttlebone::query::diagnostics(&session, &request)
        .result
        .unwrap()
        .unwrap()
        .diagnostics
    {
        out.entry(uri(&d.file)).or_default().push(json!({
            "range": {
                "start": { "line": d.range.start.line, "character": d.range.start.character },
                "end": { "line": d.range.end.line, "character": d.range.end.character },
            },
            "severity": if d.error { 1 } else { 2 },
            "code": d.id,
            "message": d.message,
        }));
    }

    out
}

fn without_source(items: &Value) -> Vec<Value> {
    items
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            let mut d = d.clone();
            let o = d.as_object_mut().unwrap();

            assert_eq!(o.remove("source"), Some(json!("cuttlebone")));
            o.remove("tags");
            d
        })
        .collect()
}

/// A workspace report's full items with diagnostics, by URI.
fn reported(report: &Value) -> HashMap<String, Vec<Value>> {
    report["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["kind"] == "full" && !i["items"].as_array().unwrap().is_empty())
        .map(|i| {
            (
                i["uri"].as_str().unwrap().to_string(),
                without_source(&i["items"]),
            )
        })
        .collect()
}

#[test]
fn a_workspace_pull_has_what_the_core_reports_for_every_fixture_project() {
    for name in [
        "bodies",
        "constants",
        "duplicates",
        "fields",
        "implementations",
        "methods",
        "moves",
        "reimplemented",
        "signatures",
        "syntax",
        "traits",
        "warnings",
    ] {
        let root = fixture(&format!("diagnostics/{}", name));
        let mut client = pulling(&root, false, false);
        let report = result(
            &mut client,
            "workspace/diagnostic",
            json!({ "previousResultIds": [] }),
        );

        assert_eq!(reported(&report), core(&root), "{}", name);
        // Nothing pushed to a client that pulls.
        assert!(
            !client
                .notifications
                .iter()
                .any(|n| n["method"] == "textDocument/publishDiagnostics"),
            "{}",
            name
        );
    }
}

const TEXT: &str = "fn example -> Int {\n  let unused = 1\n\n  '日本 😀'.size + nope\n}\n";

fn project(name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let root = crate::common::tmp().join(format!("pull-{}", name));
    let _ = std::fs::remove_dir_all(&root);

    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.inko"), TEXT).unwrap();
    std::fs::write(
        root.join("src/other.inko"),
        "fn other -> Int {\n  missing\n}\n",
    )
    .unwrap();

    let root = root.canonicalize().unwrap();

    (
        root.clone(),
        root.join("src/main.inko"),
        root.join("src/other.inko"),
    )
}

fn document(client: &mut Client, file: &Path, previous: Option<&Value>) -> Value {
    let mut params = json!({ "textDocument": { "uri": uri(file) } });

    if let Some(id) = previous {
        params["previousResultId"] = id.clone();
    }

    result(client, "textDocument/diagnostic", params)
}

#[test]
fn a_document_pull_follows_edits_in_both_encodings() {
    for utf8 in [false, true] {
        let (root, main, _) = project(if utf8 { "utf8" } else { "utf16" });
        let mut client = pulling(&root, utf8, false);

        // Not open: the file's.
        let report = document(&mut client, &main, None);
        let (line, start) = at(TEXT, "nope", 0, utf8);

        assert_eq!(report["kind"], "full");
        assert!(
            report["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["code"] == "invalid-symbol"
                    && d["range"]["start"] == json!({ "line": line, "character": start }))
        );

        // The same again: unchanged.
        let again = document(&mut client, &main, Some(&report["resultId"]));

        assert_eq!(
            again,
            json!({ "kind": "unchanged", "resultId": report["resultId"] })
        );

        // Fixed, unsaved: none.
        client.open(&main, "fn example -> Int {\n  1\n}\n");

        let fixed = document(&mut client, &main, Some(&report["resultId"]));

        assert_eq!(fixed["kind"], "full");
        assert_eq!(fixed["items"], json!([]));
        assert_ne!(fixed["resultId"], report["resultId"]);
    }
}

/// Files no one opened are in a workspace pull; one fixed since gets an
/// empty report, and one unchanged an unchanged one.
#[test]
fn a_workspace_pull_has_every_file_and_says_what_changed() {
    let (root, main, other) = project("workspace");
    let mut client = pulling(&root, false, false);
    let first = result(
        &mut client,
        "workspace/diagnostic",
        json!({ "previousResultIds": [] }),
    );
    let ids = |report: &Value| -> HashMap<String, Value> {
        report["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| {
                (
                    i["uri"].as_str().unwrap().to_string(),
                    i["resultId"].clone(),
                )
            })
            .collect()
    };
    let before = ids(&first);

    assert_eq!(
        reported(&first)
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        [uri(&main), uri(&other)].into_iter().collect()
    );

    client.open(&other, "fn other -> Int {\n  1\n}\n");

    let previous: Vec<Value> = before
        .iter()
        .map(|(u, id)| json!({ "uri": u, "value": id }))
        .collect();
    let second = result(
        &mut client,
        "workspace/diagnostic",
        json!({ "previousResultIds": previous }),
    );
    let kinds: HashMap<String, (String, usize)> = second["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["uri"].as_str().unwrap().to_string(),
                (
                    i["kind"].as_str().unwrap().to_string(),
                    i["items"].as_array().map_or(0, Vec::len),
                ),
            )
        })
        .collect();

    assert_eq!(kinds[&uri(&main)], ("unchanged".to_string(), 0));
    assert_eq!(kinds[&uri(&other)], ("full".to_string(), 0));
}

/// A workspace pull with nothing new is held until an edit changes a
/// report, as the spec allows (a long poll), and a cancel ends it.
#[test]
fn a_workspace_pull_with_nothing_new_waits_for_a_change() {
    let (root, _, other) = project("held");
    let mut client = pulling(&root, false, false);
    let first = result(
        &mut client,
        "workspace/diagnostic",
        json!({ "previousResultIds": [] }),
    );
    let previous: Vec<Value> = first["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| json!({ "uri": i["uri"], "value": i["resultId"] }))
        .collect();
    let held = client.send_request(
        "workspace/diagnostic",
        json!({ "previousResultIds": previous }),
    );

    // Nothing new: no answer yet.
    while let Ok(message) = client.receive(Duration::from_millis(500)) {
        assert_ne!(message["id"], json!(held), "{}", message);
        client.notifications.push(message);
    }

    client.open(&other, "fn other -> Int {\n  1\n}\n");

    let answer = client.response(held);
    let changed: Vec<&Value> = answer["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["kind"] == "full")
        .collect();

    assert_eq!(changed.len(), 1, "{}", answer);
    assert_eq!(changed[0]["uri"], json!(uri(&other)));

    // Held again, and cancelled.
    let previous: Vec<Value> = answer["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| json!({ "uri": i["uri"], "value": i["resultId"] }))
        .collect();
    let held = client.send_request(
        "workspace/diagnostic",
        json!({ "previousResultIds": previous }),
    );

    client.notify("$/cancelRequest", json!({ "id": held }));
    assert_eq!(client.response(held)["error"]["code"], -32800);
}

#[test]
fn only_clients_that_pull_are_offered_it() {
    let (root, main, _) = project("offered");
    let mut client = Client::start();
    let pushed = client.initialize(&root, json!({}), json!({}));

    assert_eq!(
        pushed["result"]["capabilities"].get("diagnosticProvider"),
        None
    );

    let mut client = Client::start();
    let pulled = client.initialize(
        &root,
        json!({}),
        json!({ "textDocument": { "diagnostic": {} } }),
    );

    assert_eq!(
        pulled["result"]["capabilities"]["diagnosticProvider"],
        json!({ "identifier": "cuttlebone", "interFileDependencies": true, "workspaceDiagnostics": true })
    );

    // A client that pulls, with diagnostics turned off, still gets them
    // when it asks, and none pushed.
    let mut client = Client::initialized_with(
        Launch::default(),
        &root,
        json!({ "diagnostics": false }),
        json!({ "textDocument": { "diagnostic": {} } }),
    );
    let report = document(&mut client, &main, None);

    assert!(!report["items"].as_array().unwrap().is_empty());
}

/// A check that doesn't finish (here, one that hangs until it's killed at
/// the timeout) fails the pulls waiting for it, without asking for them
/// again.
#[test]
fn a_pull_whose_check_fails_gets_an_error() {
    let (root, main, _) = project("hung");
    let mut client = Client::initialized_with(
        Launch {
            env: vec![("CUTTLEBONE_FAULT", format!("hang:{}", root.display()))],
            unset: vec![],
            cwd: None,
        },
        &root,
        json!({ "checkTimeoutMs": 500 }),
        json!({ "textDocument": { "diagnostic": {} } }),
    );
    let response = client.request(
        "textDocument/diagnostic",
        json!({ "textDocument": { "uri": uri(&main) } }),
    );

    assert_eq!(response["error"]["code"], -32803, "{}", response);
    assert_eq!(
        response["error"]["data"],
        json!({ "retriggerRequest": false })
    );
}
