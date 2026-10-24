//! Latency limits, measured from an LSP client's side, and the cost of
//! running each check in a worker process instead of in the server's own
//! thread.
// Tests report the measurements on stderr, which `cargo test` shows.
#![allow(clippy::print_stderr)]
mod common;

use common::client::{Client, Launch, at};
use cuttlebone::dump::upstream_std;
use cuttlebone::position::{Encoding, Position};
use cuttlebone::query::{self, Request, Session};
use cuttlebone::worker::NO_CACHE_VARIABLE;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The latency limit for a hover on a std-sized project.
const LIMIT: Duration = Duration::from_millis(500);

/// How much a worker process may add to a check.
const PROCESS_LIMIT: Duration = Duration::from_millis(50);

const RUNS: usize = 7;

/// These tests measure queries on their own, as they would be in an editor
/// with nothing else going on. They run in parallel, and each server would
/// also check its whole project for diagnostics, so those are off.
/// `queries_stay_fast_while_diagnostics_are_checked` measures queries
/// with them.
/// Held by the tests that load the machine on purpose (a diagnostics check
/// beside every query) or measure small differences (a worker's cost), so
/// those two never overlap.
static EXCLUSIVE: std::sync::Mutex<()> = std::sync::Mutex::new(());

static QUERIES_ONLY: std::sync::LazyLock<serde_json::Value> =
    std::sync::LazyLock::new(|| json!({ "diagnostics": false }));

/// A server whose worker keeps no checks, so that asking the same question
/// several times measures a check each time: after an edit, a query's
/// text is always new. `a_burst_of_definitions_costs_one_check` measures
/// what the kept checks save.
fn uncached(root: &Path, options: serde_json::Value) -> Client {
    Client::initialized_with(
        Launch {
            env: vec![(NO_CACHE_VARIABLE, "1".to_string())],
            cwd: None,
        },
        root,
        options,
        json!({}),
    )
}

fn median(mut times: Vec<Duration>) -> Duration {
    times.sort();
    times[times.len() / 2]
}

/// A project whose main module imports every std module that builds on
/// this platform, so one hover type checks all of std/src.
fn std_sized_project() -> (PathBuf, String) {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("std-sized");
    let src = upstream_std();
    let mut modules = Vec::new();
    let mut dirs = vec![src.join("std")];

    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();

            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "inko") {
                let name = path.strip_prefix(&src).unwrap().with_extension("");
                let name = name.to_str().unwrap().replace('/', ".");

                // Linux and FreeBSD modules don't build elsewhere, and vice
                // versa for macOS, so these are platform-specific.
                if !name.starts_with("std.sys.linux") && !name.starts_with("std.sys.freebsd") {
                    modules.push(name);
                }
            }
        }
    }

    modules.sort();
    assert!(modules.len() > 100);

    let mut text = String::new();

    for m in &modules {
        text.push_str(&format!("import {} (self as {})\n", m, m.replace('.', "_")));
    }

    text.push_str(
        "\ntype async Main {\n  fn async main {\n    let value = [1, 2].size\n    \
         let doubled = [value].iter.map(fn (x) { x * 2 }).to_array\n  }\n}\n",
    );
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.inko"), &text).unwrap();
    (root.canonicalize().unwrap(), text)
}

fn hover_times(client: &mut Client, file: &Path, line: u32, character: u32) -> Vec<Duration> {
    (0..RUNS)
        .map(|_| {
            let start = Instant::now();
            let (_, response) = client.hover(file, line, character);

            assert!(response["result"].is_object(), "{}", response);
            start.elapsed()
        })
        .collect()
}

#[test]
fn hover_on_a_std_sized_project_is_fast_enough() {
    let (root, text) = std_sized_project();
    let file = root.join("src/main.inko");
    let mut client = uncached(&root, QUERIES_ONLY.clone());
    let (line, character) = at(&text, "[1, 2].size", 7, false);
    let times = hover_times(&mut client, &file, line, character);
    let median = median(times.clone());

    eprintln!(
        "hover on a std-sized project: {:?} (all: {:?})",
        median, times
    );
    assert!(median < LIMIT, "median {:?} exceeds {:?}", median, LIMIT);
}

#[test]
fn hover_in_the_largest_std_module_is_fast_enough() {
    // std.net.http.server imports the most of std (90 modules).
    let std = upstream_std();
    let file = std.join("std/net/http/server.inko");
    let text = std::fs::read_to_string(&file).unwrap();
    let mut client = uncached(std.parent().unwrap(), QUERIES_ONLY.clone());
    let (line, character) = at(&text, "LimitReader.new(file, limit: range.size)", 30, false);
    let times = hover_times(&mut client, &file, line, character);
    let median = median(times.clone());

    eprintln!(
        "hover in std.net.http.server: {:?} (all: {:?})",
        median, times
    );
    assert!(median < LIMIT, "median {:?} exceeds {:?}", median, LIMIT);
}

#[test]
fn completion_on_a_std_sized_project_is_fast_enough() {
    let (root, text) = std_sized_project();
    let file = root.join("src/main.inko");
    let mut client = uncached(&root, QUERIES_ONLY.clone());
    let (line, character) = at(&text, "[1, 2].size", 7, false);
    let times: Vec<_> = (0..RUNS)
        .map(|_| {
            let start = Instant::now();
            let (_, response) = client.complete(&file, line, character);

            assert!(response["result"]["items"].as_array().unwrap().len() > 10);
            start.elapsed()
        })
        .collect();
    let median = median(times.clone());

    eprintln!(
        "completion on a std-sized project: {:?} (all: {:?})",
        median, times
    );
    assert!(median < LIMIT, "median {:?} exceeds {:?}", median, LIMIT);
}

#[test]
fn a_worker_process_costs_little_more_than_checking_in_the_servers_thread() {
    let _exclusive = EXCLUSIVE.lock().unwrap_or_else(|e| e.into_inner());
    let (root, text) = std_sized_project();
    let file = root.join("src/main.inko");
    let (line, character) = at(&text, "[1, 2].size", 7, false);

    // In this process, as milestone 1's server did.
    let session = Session {
        std: upstream_std(),
        projects: vec![root.clone()],
    };
    let request = Request {
        file: file.clone(),
        position: Position { line, character },
        encoding: Encoding::Utf16,
        contents: Default::default(),
    };
    let in_thread: Vec<_> = (0..RUNS)
        .map(|_| {
            let start = Instant::now();

            query::type_at(&session, &request).result.unwrap().unwrap();
            start.elapsed()
        })
        .collect();

    // Through the server, with a worker per check.
    let mut client = uncached(&root, QUERIES_ONLY.clone());
    let lsp = hover_times(&mut client, &file, line, character);

    // A worker that does no type checking: what starting a process costs.
    let missing = root.join("src/missing.inko");
    let spawn: Vec<_> = (0..RUNS)
        .map(|_| {
            let start = Instant::now();
            let (_, response) = client.hover(&missing, 0, 0);

            assert!(response["result"].is_null());
            start.elapsed()
        })
        .collect();
    let (a, b, c) = (median(in_thread), median(lsp), median(spawn));

    eprintln!(
        "std-sized check: in the thread {:?}, over LSP with a worker {:?} (+{:?}); \
         a worker that doesn't check: {:?}",
        a,
        b,
        b.saturating_sub(a),
        c
    );
    assert!(
        b.saturating_sub(a) < PROCESS_LIMIT,
        "a worker adds {:?}",
        b.saturating_sub(a)
    );
    assert!(
        c < PROCESS_LIMIT,
        "a worker that doesn't check takes {:?}",
        c
    );
}

/// The client-side times of `RUNS` requests at a position, each of which
/// must have a result.
fn request_times(client: &mut Client, method: &str, params: serde_json::Value) -> Vec<Duration> {
    (0..RUNS)
        .map(|_| {
            let start = Instant::now();
            let response = client.request(method, params.clone());

            assert!(!response["result"].is_null(), "{}: {}", method, response);
            start.elapsed()
        })
        .collect()
}

/// Each new query of milestone 4, with its params, on the std-sized
/// project: a highlight, a signature in an argument list, and the whole
/// file's inlay hints.
fn assists(file: &Path, text: &str) -> Vec<(&'static str, serde_json::Value)> {
    let (line, character) = at(text, "let value", 4, false);
    let (call_line, call_character) = at(text, "map(fn (x)", 4, false);

    vec![
        (
            "textDocument/documentHighlight",
            position(file, line, character),
        ),
        (
            "textDocument/signatureHelp",
            position(file, call_line, call_character),
        ),
        (
            "textDocument/inlayHint",
            json!({
                "textDocument": { "uri": common::client::uri(file) },
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": text.lines().count(), "character": 0 },
                },
            }),
        ),
    ]
}

#[test]
fn highlights_signatures_and_inlay_hints_on_a_std_sized_project_are_fast_enough() {
    let (root, text) = std_sized_project();
    let file = root.join("src/main.inko");
    let mut client = uncached(&root, QUERIES_ONLY.clone());

    for (method, params) in assists(&file, &text) {
        let times = request_times(&mut client, method, params);
        let median = median(times.clone());

        eprintln!(
            "{} on a std-sized project: {:?} (all: {:?})",
            method, median, times
        );
        assert!(
            median < LIMIT,
            "{}: median {:?} exceeds {:?}",
            method,
            median,
            LIMIT
        );
    }
}

fn position(file: &Path, line: u32, character: u32) -> serde_json::Value {
    json!({
        "textDocument": { "uri": common::client::uri(file) },
        "position": { "line": line, "character": character },
        "context": { "includeDeclaration": true },
    })
}

#[test]
fn definition_and_type_definition_on_a_std_sized_project_are_fast_enough() {
    let (root, text) = std_sized_project();
    let file = root.join("src/main.inko");
    let mut client = uncached(&root, QUERIES_ONLY.clone());
    let (line, character) = at(&text, "[1, 2].size", 7, false);

    for method in ["textDocument/definition", "textDocument/typeDefinition"] {
        let times = request_times(&mut client, method, position(&file, line, character));
        let median = median(times.clone());

        eprintln!(
            "{} on a std-sized project: {:?} (all: {:?})",
            method, median, times
        );
        assert!(
            median < LIMIT,
            "{}: median {:?} exceeds {:?}",
            method,
            median,
            LIMIT
        );
    }
}

#[test]
fn references_across_std_are_fast_enough() {
    // std as the project: references check all of std/src and std/test
    // (hundreds of modules), not only what one file imports.
    let std = upstream_std();
    let file = std.join("std/net/http/server.inko");
    let text = std::fs::read_to_string(&file).unwrap();
    let mut client = uncached(std.parent().unwrap(), QUERIES_ONLY.clone());
    let (line, character) = at(&text, "LimitReader.new(file, limit: range.size)", 0, false);
    let times = request_times(
        &mut client,
        "textDocument/references",
        position(&file, line, character),
    );
    let median = median(times.clone());

    eprintln!("references across std: {:?} (all: {:?})", median, times);
    assert!(median < LIMIT, "median {:?} exceeds {:?}", median, LIMIT);
}

/// Queries don't wait for diagnostics, and stay under hover's limit while a
/// diagnostics check of the same std-sized project runs in another worker.
/// Each round edits the document, which starts a full check at once
/// (`diagnosticsDebounceMs: 0`), then sends a query, which starts at once
/// too (`debounceMs: 0`). The check must still be running when the query's
/// answer arrives, so the two overlapped.
#[test]
fn queries_stay_fast_while_diagnostics_are_checked() {
    let _exclusive = EXCLUSIVE.lock().unwrap_or_else(|e| e.into_inner());
    let (root, text) = std_sized_project();
    let file = root.join("src/main.inko");
    let mut client = uncached(
        &root,
        json!({ "diagnosticsDebounceMs": 0, "debounceMs": 0, "statusNotifications": true }),
    );
    let (line, character) = at(&text, "[1, 2].size", 7, false);
    let mut queries = vec![
        ("textDocument/hover", position(&file, line, character)),
        ("textDocument/definition", position(&file, line, character)),
        (
            "textDocument/completion",
            position(&file, line, character + 1),
        ),
    ];

    queries.extend(assists(&file, &text));
    client.open(&file, &text);
    // The check that opening the project starts.
    assert_eq!(client.diagnostics_status()["status"], "fresh");

    let mut version = 1;

    for (method, params) in queries {
        let mut times = Vec::new();
        let mut checks = Vec::new();

        for _ in 0..RUNS {
            // The same text, so the queries' positions stay right: the
            // check runs all the same.
            version += 1;
            client.change(&file, version, json!([{ "text": text }]));

            let start = Instant::now();
            let response = client.request(method, params.clone());

            times.push(start.elapsed());
            assert!(!response["result"].is_null(), "{}: {}", method, response);
            assert!(
                !client
                    .notifications
                    .iter()
                    .any(|n| n["params"]["method"] == "textDocument/publishDiagnostics"),
                "{}: the diagnostics check finished before the answer",
                method
            );

            let status = client.diagnostics_status();

            assert_eq!(status["status"], "fresh", "{}", status);
            checks.push(status["elapsedMs"].as_f64().unwrap());
        }

        let median = median(times.clone());

        eprintln!(
            "{} during a diagnostics check: {:?} (all: {:?}); the checks took {:?} ms",
            method, median, times, checks
        );
        assert!(
            median < LIMIT,
            "{}: median {:?} exceeds {:?}",
            method,
            median,
            LIMIT
        );
    }
}

/// Code that doesn't parse: a hover answered from a repaired copy (two
/// checks), and the worst case, a hover after the edit, where neither the
/// repaired copy nor the last good check may answer (three checks, and no
/// answer).
#[test]
fn answers_in_code_being_edited_are_fast_enough() {
    let (root, text) = std_sized_project();
    let file = root.join("src/main.inko");
    let mut client = uncached(&root, QUERIES_ONLY.clone());

    client.open(&file, &text);

    let (line, character) = at(&text, "[1, 2].size", 7, false);

    // The last good check.
    client.hover(&file, line, character);

    let edited = text.replacen("    let doubled", "    let broken = (\n    let doubled", 1);

    client.change(&file, 2, json!([{ "text": edited }]));

    for (name, needle, offset, answered) in [
        ("a repaired copy", "[1, 2].size", 7, true),
        ("no answer, after three checks", "[value].iter", 1, false),
    ] {
        let (line, character) = at(&edited, needle, offset, false);
        let times: Vec<Duration> = (0..RUNS)
            .map(|_| {
                let start = Instant::now();
                let (_, response) = client.hover(&file, line, character);

                assert_eq!(response["result"].is_object(), answered, "{}", response);
                start.elapsed()
            })
            .collect();
        let median = median(times.clone());

        eprintln!(
            "hover in code being edited, {}: {:?} (all: {:?})",
            name, median, times
        );
        assert!(median < LIMIT, "median {:?} exceeds {:?}", median, LIMIT);
    }
}

/// How long a burst of `BURST` definition requests about unchanged text
/// may take, from the first request sent to the last answer, on the
/// std-sized project: one check (about 0.1 s), and a lookup for each.
const BURST_LIMIT: Duration = Duration::from_secs(3);

const BURST: usize = 1000;

/// Every position where a name starts in `text`.
fn name_starts(text: &str) -> Vec<(u32, u32)> {
    let mut out = Vec::new();

    for (line, l) in text.lines().enumerate() {
        let mut previous = ' ';

        for (character, c) in l.char_indices() {
            if (c.is_alphanumeric() || c == '_') && !(previous.is_alphanumeric() || previous == '_')
            {
                out.push((line as u32, character as u32));
            }

            previous = c;
        }
    }

    out
}

/// A client asking about every occurrence of a name in a search, as a
/// client's find-usages may: many requests at once, about text that
/// doesn't change. They cost one check, and a lookup each in what it
/// kept.
#[test]
fn a_burst_of_definitions_costs_one_check() {
    let (root, text) = std_sized_project();
    let file = root.join("src/main.inko");
    let mut client = Client::initialized(
        &root,
        json!({ "diagnostics": false, "statusNotifications": true }),
    );
    let positions = name_starts(&text);

    client.open(&file, &text);

    let start = Instant::now();
    let ids: Vec<i64> = positions
        .iter()
        .cycle()
        .take(BURST)
        .map(|(line, character)| {
            client.send_request(
                "textDocument/definition",
                position(&file, *line, *character),
            )
        })
        .collect();
    let mut found = 0;

    for id in &ids {
        let response = client.response(*id);

        assert!(response.get("error").is_none(), "{}", response);
        found += usize::from(response["result"].is_object());
    }

    let elapsed = start.elapsed();
    let checks = ids
        .iter()
        .filter(|id| {
            let status = client.status(**id).unwrap();

            assert_eq!(status["status"], "fresh", "{}", status);
            status.get("cached").is_none()
        })
        .count();

    eprintln!(
        "{} definitions at once on a std-sized project: {:?}, {} with a check, {} with a location",
        BURST, elapsed, checks, found
    );
    assert_eq!(checks, 1);
    // The import's modules (`import` itself and the aliases' declarations
    // have none), and the names in `main`.
    assert!(found > BURST / 3, "{}", found);
    assert!(
        elapsed < BURST_LIMIT,
        "{:?} exceeds {:?}",
        elapsed,
        BURST_LIMIT
    );
}
