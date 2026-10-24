//! Diagnostics over LSP, with the real binary: what's published, in which
//! encoding, and when (docs/lsp.md, "Diagnostics").
mod common;

use common::client::{Client, Launch, at, uri};
use common::fixture;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn options(extra: Value) -> Value {
    let mut options = json!({ "statusNotifications": true, "diagnosticsDebounceMs": 0 });

    options
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    options
}

/// The diagnostics last published for each URI, taking the
/// notifications received so far.
fn published(client: &mut Client) -> HashMap<String, Vec<Value>> {
    let mut out = HashMap::new();

    client.notifications.retain(|n| {
        if n["method"] == "textDocument/publishDiagnostics" {
            out.insert(
                n["params"]["uri"].as_str().unwrap().to_string(),
                n["params"]["diagnostics"].as_array().unwrap().clone(),
            );
            false
        } else {
            true
        }
    });
    out
}

/// Waits for the next diagnostics check to finish, and returns what it
/// published.
fn next_check(client: &mut Client) -> HashMap<String, Vec<Value>> {
    let status = client.diagnostics_status();

    assert_eq!(status["status"], "fresh", "{}", status);
    published(client)
}

fn codes(list: &[Value]) -> Vec<&str> {
    list.iter().map(|d| d["code"].as_str().unwrap()).collect()
}

fn range(line: u32, start: u32, end: u32) -> Value {
    json!({ "start": { "line": line, "character": start }, "end": { "line": line, "character": end } })
}

#[test]
fn every_file_with_diagnostics_is_published_without_opening_it() {
    let root = fixture("diagnostics/moves");
    let mut client = Client::initialized(&root, options(json!({})));
    let published = next_check(&mut client);
    let main = &published[&uri(&root.join("src/main.inko"))];
    let test = &published[&uri(&root.join("test/test_moves.inko"))];

    assert_eq!(published.len(), 2, "{:?}", published.keys());
    assert_eq!(test.len(), 3);
    assert_eq!(
        test[0],
        json!({
            "range": range(4, 10, 16),
            "severity": 1,
            "code": "moved",
            "source": "cuttlebone",
            "message": "'values' can't be used as it has been moved",
        })
    );
    // Warnings, and the unused ones are tagged as unnecessary.
    assert_eq!(test[1]["severity"], 2);
    assert_eq!(test[1]["code"], "unused-symbol");
    assert_eq!(test[1]["tags"], json!([1]));
    assert!(codes(main).contains(&"invalid-match"));
    assert!(codes(main).contains(&"unreachable"));
}

/// Inko counts columns in grapheme clusters: 👩‍👩‍👦 is one column, eight
/// UTF-16 units and 18 UTF-8 bytes.
#[test]
fn ranges_are_in_the_negotiated_encoding() {
    let root = fixture("diagnostics/syntax");
    let file = root.join("src/main.inko");
    let text = std::fs::read_to_string(&file).unwrap();

    for utf8 in [false, true] {
        let encodings = if utf8 {
            json!(["utf-8"])
        } else {
            json!(["utf-16"])
        };
        let mut client = Client::initialized_with(
            Launch::default(),
            &root,
            options(json!({})),
            json!({ "general": { "positionEncodings": encodings } }),
        );
        let published = next_check(&mut client);
        let list = &published[&uri(&file)];
        let (line, character) = at(&text, "+ )", 2, utf8);

        assert_eq!(list.len(), 1, "{:?}", list);
        assert_eq!(list[0]["range"], range(line, character, character + 1));
        assert_eq!(list[0]["message"], "')' can't be used here");
        assert!(character > 27 && line == 5, "{}", character);
    }
}

/// Every fixture project, over LSP: the same diagnostics as the oracle in
/// tests/diagnostics.rs, as LSP diagnostics.
#[test]
fn every_fixture_project_publishes_what_the_core_reports() {
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
        let mut client = Client::initialized(&root, options(json!({})));
        let published = next_check(&mut client);
        let session = cuttlebone::query::Session {
            std: cuttlebone::dump::upstream_std(),
            projects: vec![root.clone()],
        };
        let request = cuttlebone::query::Request {
            file: root.clone(),
            position: cuttlebone::position::Position {
                line: 0,
                character: 0,
            },
            encoding: cuttlebone::position::Encoding::Utf16,
            contents: HashMap::new(),
        };
        let core = cuttlebone::query::diagnostics(&session, &request)
            .result
            .unwrap()
            .unwrap()
            .diagnostics;
        let mut expected: HashMap<String, Vec<Value>> = HashMap::new();

        for d in core {
            expected.entry(uri(&d.file)).or_default().push(json!({
                "range": {
                    "start": { "line": d.range.start.line, "character": d.range.start.character },
                    "end": { "line": d.range.end.line, "character": d.range.end.character },
                },
                "severity": if d.error { 1 } else { 2 },
                "code": d.id,
                "message": d.message,
            }));
        }

        let got: HashMap<String, Vec<Value>> = published
            .into_iter()
            .map(|(uri, list)| {
                let list = list
                    .into_iter()
                    .map(|mut d| {
                        let d = d.as_object_mut().unwrap();

                        assert_eq!(d.remove("source"), Some(json!("cuttlebone")));
                        d.remove("tags");
                        Value::Object(d.clone())
                    })
                    .collect();

                (uri, list)
            })
            .collect();

        assert_eq!(got, expected, "{}", name);
    }
}

/// A diagnostic appears when an edit introduces an error, and goes when
/// it's fixed, in the edited file and in another file. A file whose
/// diagnostics are all gone gets an empty list.
#[test]
fn diagnostics_follow_edits_and_clear_when_fixed() {
    let root = fixture("diagnostics/bodies");
    let main = root.join("src/main.inko");
    let helper = root.join("src/helper.inko");
    let text = std::fs::read_to_string(&helper).unwrap();
    let mut client = Client::initialized(&root, options(json!({})));
    let before = next_check(&mut client);

    assert_eq!(codes(&before[&uri(&helper)]), ["invalid-type"]);

    // Fixing the helper clears its list.
    client.open(&helper, &text);
    client.change(
        &helper,
        2,
        json!([{ "text": text.replace("value + \"日本\"", "value + 1") }]),
    );

    let fixed = next_check(&mut client);

    assert_eq!(fixed[&uri(&helper)], Vec::<Value>::new());
    assert!(!fixed[&uri(&main)].is_empty());

    // Breaking it again brings a diagnostic back, at the new text's position.
    let broken = text.replace("value + \"日本\"", "\"日本\".size + undefined_name");

    client.change(&helper, 3, json!([{ "text": broken }]));

    let again = next_check(&mut client);
    let list = &again[&uri(&helper)];
    let (line, character) = at(&broken, "undefined_name", 0, false);

    assert_eq!(codes(list), ["invalid-symbol"]);
    assert_eq!(list[0]["range"], range(line, character, character + 14));

    // Closing it goes back to the text on disk.
    client.close(&helper);
    assert_eq!(
        codes(&next_check(&mut client)[&uri(&helper)]),
        ["invalid-type"]
    );
}

/// A project that type checks without errors or warnings publishes
/// nothing, and a fix that removes a project's last diagnostic clears it.
#[test]
fn a_clean_project_publishes_nothing() {
    let root = fixture("diagnostics/warnings");
    let file = root.join("src/main.inko");
    let text = std::fs::read_to_string(&file).unwrap();
    let mut client = Client::initialized(&root, options(json!({})));

    assert_eq!(next_check(&mut client).len(), 1);

    let clean = text
        .replace("import std.fs.path (Path)\n", "")
        .replace("    let unused = 10\n", "");

    client.open(&file, &clean);

    let published = next_check(&mut client);

    assert_eq!(published.len(), 1);
    assert_eq!(published[&uri(&file)], Vec::<Value>::new());

    // Nothing to clear the next time.
    client.change(&file, 2, json!([{ "text": format!("{}\n", clean) }]));
    assert!(next_check(&mut client).is_empty());
}

/// The debounce: a check starts `diagnosticsDebounceMs` after the last
/// edit, not before.
#[test]
fn diagnostics_wait_for_their_debounce() {
    let root = fixture("diagnostics/warnings");
    let file = root.join("src/main.inko");
    let text = std::fs::read_to_string(&file).unwrap();
    let mut client = Client::initialized(&root, options(json!({ "diagnosticsDebounceMs": 600 })));

    next_check(&mut client);
    client.open(&file, &text);

    let start = Instant::now();

    for version in 2..5 {
        client.change(
            &file,
            version,
            json!([{ "text": format!("{}{}", text, "\n".repeat(version as usize)) }]),
        );
        std::thread::sleep(Duration::from_millis(200));
    }

    let edited = Instant::now();

    client.diagnostics_status();
    assert!(
        edited.elapsed() >= Duration::from_millis(400),
        "{:?}",
        edited.elapsed()
    );
    assert!(start.elapsed() >= Duration::from_millis(1000));
    // The edits came during the debounce, so there was one check.
    assert!(
        !client
            .notifications
            .iter()
            .any(|n| n["params"]["method"] == "textDocument/publishDiagnostics")
    );
}

fn fault(kind: &str, root: &Path) -> Launch<'static> {
    Launch {
        env: vec![("CUTTLEBONE_FAULT", format!("{}:{}", kind, root.display()))],
        cwd: None,
    }
}

/// A diagnostics check doesn't hold up queries, even one that never
/// finishes: they run in a worker of their own.
#[test]
fn a_query_is_not_stuck_behind_a_diagnostics_check() {
    let root = fixture("diagnostics/warnings");
    let file = root.join("src/main.inko");
    let text = std::fs::read_to_string(&file).unwrap();
    let mut client = Client::initialized_with(
        fault("hang", &root),
        &root,
        options(json!({ "checkTimeoutMs": 20000 })),
        json!({}),
    );

    // The check of the project starts at once, and hangs.
    client.open(&file, &text);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(client.workers().len(), 1);

    let (line, character) = at(&text, "unused", 0, false);
    let start = Instant::now();
    let (_, response) = client.hover(&file, line, character);

    assert_eq!(response["result"]["contents"]["value"], "```inko\nInt\n```");
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "{:?}",
        start.elapsed()
    );
    // The diagnostics check is still running, beside the queries' worker,
    // which waits for the next query.
    assert_eq!(client.workers().len(), 2);
}

/// An edit to a project whose check is running cancels the check, and
/// starts another. A check that takes too long is killed, like a query.
#[test]
fn superseded_and_hung_checks_are_killed() {
    let root = fixture("diagnostics/warnings");
    let file = root.join("src/main.inko");
    let text = std::fs::read_to_string(&file).unwrap();
    let mut client = Client::initialized_with(
        fault("hang", &root),
        &root,
        options(json!({ "checkTimeoutMs": 1500 })),
        json!({}),
    );

    std::thread::sleep(Duration::from_millis(200));

    let first = client.workers();

    assert_eq!(first.len(), 1);
    client.open(&file, &text);
    client.change(&file, 2, json!([{ "text": format!("{}\n", text) }]));

    let superseded = client.diagnostics_status();

    assert_eq!(superseded["status"], "unavailable");
    assert_eq!(superseded["reason"]["code"], "superseded");
    std::thread::sleep(Duration::from_millis(200));

    let second = client.workers();

    assert_eq!(second.len(), 1);
    assert_ne!(first, second);

    let timeout = client.diagnostics_status();

    assert_eq!(timeout["reason"]["code"], "timeout");
    assert!(client.workers().is_empty());
}

/// When lowering to MIR panics, which `inko check` doesn't survive, what
/// the compiler reported before the panic is published, and the status
/// says it's incomplete.
#[test]
fn a_panicking_check_publishes_what_it_found_before() {
    let root = fixture("mir_panic");
    let mut client = Client::initialized(&root, options(json!({})));
    let status = client.diagnostics_status();

    assert_eq!(status["status"], "unavailable");
    assert_eq!(status["reason"]["code"], "compilerPanic");
    assert!(
        status["reason"]["message"]
            .as_str()
            .unwrap()
            .contains("passes.rs"),
        "{}",
        status
    );

    let published = published(&mut client);

    assert_eq!(
        codes(&published[&uri(&root.join("src/main.inko"))]),
        ["borrow-value-type"]
    );

    // And the server goes on.
    let (_, response) = client.hover(&root.join("src/main.inko"), 4, 2);

    assert_eq!(response["result"]["contents"]["value"], "```inko\nInt\n```");
}

/// The standard library's files and dependencies' aren't checked for
/// diagnostics, even when they're open; a file outside any project's `src/`
/// and `test/` is checked on its own, like `inko check file.inko`.
#[test]
fn targets_are_projects_or_lone_files_but_not_std_or_dependencies() {
    let root = fixture("diagnostics/warnings");
    let std = cuttlebone::dump::upstream_std();
    let session = cuttlebone::query::Session {
        std: std.clone(),
        projects: vec![root.clone()],
    };
    let target = |path: &Path| cuttlebone::query::diagnostics_target(&session, path);
    let lone: PathBuf = Path::new(env!("CARGO_TARGET_TMPDIR")).join("lone.inko");
    let dep = Path::new(env!("CARGO_TARGET_TMPDIR")).join("with-dep");
    let dep_file = dep.join("dep/hash/v1/src/thing.inko");

    std::fs::write(&lone, "fn example {\n  let a = 1\n}\n").unwrap();
    std::fs::create_dir_all(dep.join("src")).unwrap();
    std::fs::create_dir_all(dep_file.parent().unwrap()).unwrap();
    std::fs::write(&dep_file, "").unwrap();

    let lone = lone.canonicalize().unwrap();

    assert_eq!(target(&root.join("src/main.inko")), Some(root.clone()));
    assert_eq!(target(&lone), Some(lone.clone()));
    assert_eq!(target(&std.join("std/string.inko")), None);
    assert_eq!(target(&dep_file), None);

    // Over LSP: the lone file's own diagnostics.
    let mut client = Client::initialized(&root, options(json!({})));

    next_check(&mut client);
    client.open(&lone, "fn example {\n  let a = 1\n}\n");

    let status = client.diagnostics_status();

    assert_eq!(status["target"], uri(&lone));
    assert_eq!(
        codes(&published(&mut client)[&uri(&lone)]),
        ["unused-symbol"]
    );

    // Opening a std file checks nothing.
    client.open(&std.join("std/string.inko"), "");

    let (_, response) = client.hover(&root.join("src/main.inko"), 0, 0);

    assert!(response.get("error").is_none());
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !client
            .notifications
            .iter()
            .any(|n| n["method"] == "textDocument/publishDiagnostics"
                || n["params"]["method"] == "textDocument/publishDiagnostics"),
        "{:?}",
        client.notifications
    );
}

/// `"diagnostics": false` turns them off: nothing is checked or published.
#[test]
fn diagnostics_can_be_turned_off() {
    let root = fixture("diagnostics/moves");
    let file = root.join("src/main.inko");
    let mut client = Client::initialized(&root, options(json!({ "diagnostics": false })));

    client.open(&file, &std::fs::read_to_string(&file).unwrap());
    std::thread::sleep(Duration::from_millis(500));

    let (_, response) = client.hover(&file, 0, 0);

    assert!(response.get("error").is_none());
    // Only the queries' worker, waiting for the next query.
    assert_eq!(client.workers().len(), 1);
    assert!(
        !client
            .notifications
            .iter()
            .any(|n| n["method"] == "textDocument/publishDiagnostics"
                || n["params"]["method"] == "textDocument/publishDiagnostics"),
        "{:?}",
        client.notifications
    );
}

/// Queries come first: a diagnostics check that becomes ready while a
/// query runs waits for it. The query hangs until its timeout, so a check
/// that didn't wait would finish long before the query's answer.
#[test]
fn diagnostics_wait_for_running_queries() {
    let root = fixture("diagnostics/bodies");
    let main = root.join("src/main.inko");
    let helper = root.join("src/helper.inko");
    let text = std::fs::read_to_string(&helper).unwrap();
    let launch = Launch {
        env: vec![("CUTTLEBONE_FAULT", "hang:bodies/src/main.inko".to_string())],
        cwd: None,
    };
    let mut client = Client::initialized_with(
        launch,
        &root,
        options(json!({ "checkTimeoutMs": 1000 })),
        json!({}),
    );

    next_check(&mut client);
    client.open(&helper, &text);

    let id = client.send_at("textDocument/hover", &main, 0, 0);

    std::thread::sleep(Duration::from_millis(100));
    client.change(&helper, 2, json!([{ "text": format!("{}\n", text) }]));

    let response = client.response(id);

    assert_eq!(
        client.status(id).unwrap()["reason"]["code"],
        "timeout",
        "{}",
        response
    );
    assert!(
        !client
            .notifications
            .iter()
            .any(|n| n["params"]["method"] == "textDocument/publishDiagnostics"),
        "the check finished before the query"
    );
    assert_eq!(client.diagnostics_status()["status"], "fresh");
}
