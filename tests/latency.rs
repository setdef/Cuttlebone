//! Latency limits, measured from an LSP client's side, and the cost of
//! running each check in a worker process instead of in the server's own
//! thread.
// Tests report the measurements on stderr, which `cargo test` shows.
#![allow(clippy::print_stderr)]
mod common;

use common::client::{Client, Launch, at, uri};
use cuttlebone::dump::upstream_std;
use cuttlebone::position::{Encoding, Position};
use cuttlebone::query::{self, Request, Session};
use cuttlebone::worker::NO_CACHE_VARIABLE;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The latency limit for a hover on a std-sized project.
const LIMIT: Duration = Duration::from_millis(500);

/// How much a worker process may add to a check.
const PROCESS_LIMIT: Duration = Duration::from_millis(50);

const RUNS: usize = 7;

/// With this set, the limits are as they are written: the milestone tier
/// (scripts/check.sh) and the mutations hold every query to them.
/// Without it, as in the check of every commit, each limit is scaled by
/// how loaded the machine is (`load`), so that other processes can't fail
/// a commit (AGENTS.md, "Checks").
const STRICT_VARIABLE: &str = "CUTTLEBONE_LATENCY_STRICT";

/// With `STRICT_VARIABLE`, how this machine compares with the reference
/// machine the limits are written for: its baseline over the reference's
/// (`measure_the_baseline`). scripts/latency.sh measures it and sets it.
const SCALE_VARIABLE: &str = "CUTTLEBONE_LATENCY_SCALE";

/// A latency limit: `base`, or in the check of every commit, `base` times
/// the machine's load. With `STRICT_VARIABLE`, `base` for the reference
/// machine, times `SCALE_VARIABLE` if it's set (`scaled`).
fn limit(base: Duration) -> Duration {
    if std::env::var_os(STRICT_VARIABLE).is_some() {
        let scale = std::env::var(SCALE_VARIABLE)
            .ok()
            .map(|s| s.parse::<f64>().expect("a number"));

        return scaled(base, scale);
    }

    let factor = load();

    if factor > 1.0 {
        eprintln!(
            "limit {:?} scaled by the load, {:.1} per core, to {:?} ({} not set)",
            base,
            factor,
            base.mul_f64(factor),
            STRICT_VARIABLE
        );
    }

    base.mul_f64(factor)
}

/// A strict limit on a machine `scale` times as slow as the reference
/// (`None`: the reference itself). Never less than as written: a machine
/// faster than the reference is held to the limits as written, since
/// part of every answer (processes, pipes, files) is a cost the
/// baseline's speed doesn't say much about.
fn scaled(base: Duration, scale: Option<f64>) -> Duration {
    base.mul_f64(scale.unwrap_or(1.0).max(1.0))
}

/// The reference machine's baseline (`baseline`): an Apple M1 Max, under
/// `cargo test` (docs/history/MILESTONE-14.md). The limits as written are
/// for it.
const REFERENCE_BASELINE: Duration = Duration::from_millis(REFERENCE_BASELINE_MS);
const REFERENCE_BASELINE_MS: u64 = 280;

/// How many times the baseline runs.
const BASELINE_RUNS: usize = 7;

/// The baseline workload: the upstream compiler alone, none of
/// Cuttlebone, type checking all of std (its `src/` and `test/`) as a
/// project, the same work a question about the whole project checks
/// (references across std): a module that imports all of std alone
/// didn't track the runners, which are slower at the whole project's
/// check than one module's suggests. The median of `BASELINE_RUNS`, as
/// the tests take medians. Nothing Cuttlebone changes can make it
/// slower or faster, so a slower Cuttlebone still fails the limits it
/// scales.
fn baseline() -> Duration {
    let std = upstream_std();
    let root = std.parent().unwrap().to_path_buf();
    let times = (0..BASELINE_RUNS)
        .map(|_| {
            let mut config = compiler::config::Config::new().unwrap();

            config.std = std.clone();
            config.add_source_directory(std.clone());
            config.source = root.join("src");
            config.tests = root.join("test");
            config.add_source_directory(root.join("test"));
            config.build = root.join("build");
            config.dependencies = root.join("dep");

            let mut compiler = compiler::compiler::Compiler::new(config);
            let start = Instant::now();
            let checked = compiler.check_with(None, |_, _, _| false).is_ok();

            assert!(checked, "the baseline didn't type check");
            start.elapsed()
        })
        .collect();

    median(times)
}

/// Measures the baseline, and writes it, the reference's and the scale
/// (the first over the second) to the file `CUTTLEBONE_LATENCY_BASELINE`
/// names, for scripts/latency.sh. Ignored but there: it measures, and
/// checks nothing.
#[test]
#[ignore]
fn measure_the_baseline() {
    let measured = baseline();
    let scale = measured.as_secs_f64() / REFERENCE_BASELINE.as_secs_f64();
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let report = format!(
        "baseline {:.1} ms on this machine ({} cores, the median of {} runs)\n\
         reference {:.1} ms (an Apple M1 Max)\n\
         scale {:.3}: every strict limit times this, if it's over 1\n",
        measured.as_secs_f64() * 1000.0,
        cores,
        BASELINE_RUNS,
        REFERENCE_BASELINE.as_secs_f64() * 1000.0,
        scale
    );

    eprint!("{}", report);

    if let Some(path) = std::env::var_os("CUTTLEBONE_LATENCY_BASELINE") {
        std::fs::write(path, report).unwrap();
    }
}

#[test]
fn strict_limits_scale_by_the_baseline() {
    assert_eq!(scaled(LIMIT, None), LIMIT);
    assert_eq!(scaled(LIMIT, Some(1.2)), Duration::from_millis(600));
    assert_eq!(scaled(LIMIT, Some(0.9)), LIMIT);
}

/// The milestone tier and the mutations set `STRICT_VARIABLE`, and then
/// the limits are exactly as written, however loaded the machine is.
#[test]
fn limits_are_as_written_when_strict() {
    if let (Some(_), Ok(scale)) = (
        std::env::var_os(STRICT_VARIABLE),
        std::env::var(SCALE_VARIABLE),
    ) {
        let scale: f64 = scale.parse().unwrap();

        assert_eq!(limit(LIMIT), LIMIT.mul_f64(scale.max(1.0)));
    } else if std::env::var_os(STRICT_VARIABLE).is_some() {
        assert_eq!(limit(LIMIT), LIMIT);
        assert_eq!(limit(BURST_LIMIT), BURST_LIMIT);
    } else {
        assert!(limit(LIMIT) >= LIMIT);
    }
}

/// The load average over the last minute per core, at least 1: how many
/// times slower than on an idle machine a process here may run, as other
/// processes take their share of the cores.
fn load() -> f64 {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
    let average = std::fs::read_to_string("/proc/loadavg")
        .ok()
        .or_else(|| {
            let out = std::process::Command::new("sysctl")
                .args(["-n", "vm.loadavg"])
                .output()
                .ok()?;

            String::from_utf8(out.stdout).ok()
        })
        .and_then(|text| {
            text.split_whitespace()
                .find_map(|word| word.parse::<f64>().ok())
        })
        .unwrap_or(0.0);

    (average / cores).max(1.0)
}

/// These tests measure queries on their own, as they would be in an editor
/// with nothing else going on. They run in parallel, and each server would
/// also check its whole project for diagnostics, so those are off.
/// `queries_stay_fast_while_diagnostics_are_checked` measures queries
/// with them.
///
/// The tests that load the machine on purpose (a diagnostics check beside
/// every query, a burst of requests) or measure small differences (a
/// worker's cost) run alone (`alone`); the others share it (`shared`).
static LOAD: std::sync::RwLock<()> = std::sync::RwLock::new(());

fn alone() -> std::sync::RwLockWriteGuard<'static, ()> {
    LOAD.write().unwrap_or_else(|e| e.into_inner())
}

fn shared() -> std::sync::RwLockReadGuard<'static, ()> {
    LOAD.read().unwrap_or_else(|e| e.into_inner())
}

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
            unset: vec![],
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
    let root = common::tmp().join("std-sized");
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

    // Tests run at once, and a rewrite would count as a change on disk
    // for each other's kept checks.
    if std::fs::read_to_string(root.join("src/main.inko"))
        .ok()
        .as_ref()
        != Some(&text)
    {
        std::fs::write(root.join("src/main.inko"), &text).unwrap();
    }

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
    let _shared = shared();
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
    let max = limit(LIMIT);
    assert!(median < max, "median {:?} exceeds {:?}", median, max);
}

#[test]
fn hover_in_the_largest_std_module_is_fast_enough() {
    let _shared = shared();
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
    let max = limit(LIMIT);
    assert!(median < max, "median {:?} exceeds {:?}", median, max);
}

/// Completion of a name on a project that imports all of std, with the
/// symbols of every module to import that match it: a check of the
/// document, and the index read before.
#[test]
fn completion_of_names_on_a_std_sized_project_is_fast_enough() {
    let _shared = shared();
    let (root, text) = std_sized_project();
    let file = root.join("src/main.inko");
    let typed = text.replacen("    let doubled", "    Stri\n    let doubled", 1);
    let mut client = uncached(&root, QUERIES_ONLY.clone());

    client.open(&file, &typed);

    let (line, character) = at(&typed, "    Stri\n", 8, false);
    let times: Vec<_> = (0..RUNS)
        .map(|_| {
            let start = Instant::now();
            let (_, response) = client.complete(&file, line, character);
            let items = response["result"]["items"].as_array().unwrap();

            assert!(items.iter().any(|i| i["label"] == "value"));
            assert!(items.iter().any(|i| i.get("additionalTextEdits").is_some()));
            start.elapsed()
        })
        .collect();
    let median = median(times.clone());

    eprintln!(
        "completion of a name on a std-sized project: {:?} (all: {:?})",
        median, times
    );
    let max = limit(LIMIT);
    assert!(median < max, "median {:?} exceeds {:?}", median, max);
}

/// How long document symbols may take for the largest std module: no
/// check, only parsing, and for text that doesn't parse, parsing copies
/// with lines blank until one does.
const SYMBOLS_LIMIT: Duration = Duration::from_millis(150);

#[test]
fn document_symbols_of_the_largest_std_module_are_fast_enough() {
    // Tens of milliseconds: alone, so the other tests' checks don't
    // share the cores.
    let _alone = alone();
    let std = upstream_std();
    // The largest module of std.
    let file = std.join("std/net/http/server.inko");
    let text = std::fs::read_to_string(&file).unwrap();
    let mut client = uncached(std.parent().unwrap(), QUERIES_ONLY.clone());
    let params = json!({ "textDocument": { "uri": uri(&file) } });

    client.open(&file, &text);

    let parsed = median(request_times(
        &mut client,
        "textDocument/documentSymbol",
        params.clone(),
    ));
    // A statement half typed, in the middle of the module.
    let (line, _) = at(&text, "LimitReader.new(file, limit: range.size)", 0, false);

    client.change(
        &file,
        2,
        json!([{
            "range": {
                "start": { "line": line, "character": 0 },
                "end": { "line": line, "character": 0 },
            },
            "text": "    let x = foo(\n",
        }]),
    );

    let repaired = median(request_times(
        &mut client,
        "textDocument/documentSymbol",
        params,
    ));

    eprintln!(
        "document symbols of std.net.http.server: {:?}, with a line that doesn't parse: {:?}",
        parsed, repaired
    );

    let max = limit(SYMBOLS_LIMIT);

    assert!(parsed < max, "median {:?} exceeds {:?}", parsed, max);
    assert!(
        repaired < max,
        "with a line that doesn't parse, median {:?} exceeds {:?}",
        repaired,
        max
    );
}

/// How long folding ranges and formatting may take for the largest std
/// module: no check, only its tokens, or parsing and formatting it.
const LAYOUT_LIMIT: Duration = Duration::from_millis(150);

#[test]
fn folding_formatting_and_selection_in_the_largest_std_module_are_fast_enough() {
    // Milliseconds: alone, as document symbols.
    let _alone = alone();
    let std = upstream_std();
    let file = std.join("std/net/http/server.inko");
    let text = std::fs::read_to_string(&file).unwrap();
    let mut client = uncached(std.parent().unwrap(), QUERIES_ONLY.clone());
    let document = json!({ "uri": uri(&file) });

    // Its indentation taken away, for formatting to put back.
    let flat: String = text
        .split_inclusive('\n')
        .map(|l| l.trim_start_matches(' '))
        .collect();

    client.open(&file, &flat);

    let folding = median(request_times(
        &mut client,
        "textDocument/foldingRange",
        json!({ "textDocument": document }),
    ));
    let formatting = median(request_times(
        &mut client,
        "textDocument/formatting",
        json!({ "textDocument": document, "options": { "tabSize": 2, "insertSpaces": true } }),
    ));

    // Ten positions through the file, in one request.
    let lines = flat.lines().count();
    let positions: Vec<serde_json::Value> = (0..10)
        .map(|i| json!({ "line": i * lines / 10, "character": 2 }))
        .collect();
    let selection = median(request_times(
        &mut client,
        "textDocument/selectionRange",
        json!({ "textDocument": document, "positions": positions }),
    ));

    eprintln!(
        "std.net.http.server: folding ranges {:?}, formatting {:?}, selection ranges {:?}",
        folding, formatting, selection
    );

    let max = limit(LAYOUT_LIMIT);

    assert!(
        folding < max,
        "folding: median {:?} exceeds {:?}",
        folding,
        max
    );
    assert!(
        formatting < max,
        "formatting: median {:?} exceeds {:?}",
        formatting,
        max
    );
    assert!(
        selection < max,
        "selection ranges: median {:?} exceeds {:?}",
        selection,
        max
    );
}

/// How long workspace symbols may take while a query is typed, on a
/// project that imports all of std: every module's declarations, read
/// before, matched again.
const WORKSPACE_SYMBOLS_LIMIT: Duration = Duration::from_millis(50);

/// How long the first workspace symbols may take: every module of std and
/// the project read and parsed.
const FIRST_WORKSPACE_SYMBOLS_LIMIT: Duration = Duration::from_millis(500);

#[test]
fn workspace_symbols_are_fast_enough_to_type_into() {
    // Tens of milliseconds: alone, so the other tests' checks don't
    // share the cores.
    let _alone = alone();
    let (root, _) = std_sized_project();
    let mut client = uncached(&root, QUERIES_ONLY.clone());
    let ask = |client: &mut Client, query: &str| {
        let start = Instant::now();
        let response = client.request("workspace/symbol", json!({ "query": query }));

        assert!(
            !response["result"].as_array().unwrap().is_empty(),
            "{}",
            query
        );
        start.elapsed()
    };
    let first = ask(&mut client, "S");
    let typed: Vec<Duration> = [
        "St", "Std", "Stdo", "Stdou", "Stdout", "m", "ma", "map", "map_",
    ]
    .iter()
    .map(|q| ask(&mut client, q))
    .collect();
    let slowest = *typed.iter().max().unwrap();

    eprintln!(
        "workspace symbols on a std-sized project: the first {:?}, then at most {:?} (all: {:?})",
        first, slowest, typed
    );

    let max = limit(FIRST_WORKSPACE_SYMBOLS_LIMIT);

    assert!(
        first < max,
        "the first took {:?}, more than {:?}",
        first,
        max
    );

    let max = limit(WORKSPACE_SYMBOLS_LIMIT);

    assert!(
        slowest < max,
        "the slowest took {:?}, more than {:?}",
        slowest,
        max
    );
}

/// How long every workspace symbol may take, for a client that indexes
/// (an empty query, or partial results): every module's declarations,
/// read before, all of them sent.
const EVERY_WORKSPACE_SYMBOL_LIMIT: Duration = Duration::from_millis(400);

#[test]
fn every_workspace_symbol_is_fast_enough_for_an_index() {
    let _alone = alone();
    let (root, _) = std_sized_project();
    let mut client = uncached(&root, QUERIES_ONLY.clone());

    // The index read first, as by the first query.
    client.request("workspace/symbol", json!({ "query": "S" }));

    let mut all = Vec::new();
    let mut partial = Vec::new();

    for _ in 0..5 {
        let start = Instant::now();
        let response = client.request("workspace/symbol", json!({ "query": "" }));

        all.push(start.elapsed());
        assert!(response["result"].as_array().unwrap().len() > 5000);

        // Partial results: until the response, after every batch.
        let start = Instant::now();
        let response = client.request(
            "workspace/symbol",
            json!({ "query": "", "partialResultToken": 1 }),
        );

        partial.push(start.elapsed());
        assert_eq!(response["result"], json!([]));
        client.notifications.clear();
    }

    all.sort();
    partial.sort();

    let (all, partial) = (all[all.len() / 2], partial[partial.len() / 2]);

    eprintln!(
        "every workspace symbol on a std-sized project: {:?} in a response, {:?} as partial results (medians)",
        all, partial
    );

    let max = limit(EVERY_WORKSPACE_SYMBOL_LIMIT);

    assert!(
        all < max,
        "every symbol: median {:?} exceeds {:?}",
        all,
        max
    );
    assert!(
        partial < max,
        "every symbol as partial results: median {:?} exceeds {:?}",
        partial,
        max
    );
}

#[test]
fn completion_on_a_std_sized_project_is_fast_enough() {
    let _shared = shared();
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
    let max = limit(LIMIT);
    assert!(median < max, "median {:?} exceeds {:?}", median, max);
}

#[test]
fn a_worker_process_costs_little_more_than_checking_in_the_servers_thread() {
    let _alone = alone();
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
        b.saturating_sub(a) < limit(PROCESS_LIMIT),
        "a worker adds {:?}",
        b.saturating_sub(a)
    );
    assert!(
        c < limit(PROCESS_LIMIT),
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
    let _shared = shared();
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
        let max = limit(LIMIT);
        assert!(
            median < max,
            "{}: median {:?} exceeds {:?}",
            method,
            median,
            max
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
    let _shared = shared();
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
        let max = limit(LIMIT);
        assert!(
            median < max,
            "{}: median {:?} exceeds {:?}",
            method,
            median,
            max
        );
    }
}

#[test]
fn references_across_std_are_fast_enough() {
    let _shared = shared();
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
    let max = limit(LIMIT);
    assert!(median < max, "median {:?} exceeds {:?}", median, max);
}

/// How long subtypes and a file's rename may take across std: a check of
/// the whole project, as references, and more than references then (the
/// declarations of every file an implementing type is in, read for its
/// item; every importer's uses of the module). Neither is asked while
/// typing: a client asks for a hierarchy, or before it moves a file.
const WHOLE_PROJECT_LIMIT: Duration = Duration::from_millis(750);

/// Implementations, incoming calls, subtypes and a file's rename each
/// check the whole project, as references do: across all of std.
#[test]
fn implementations_hierarchies_and_renames_across_std_are_fast_enough() {
    let _shared = shared();
    let std = upstream_std();
    let file = std.join("std/string.inko");
    let text = std::fs::read_to_string(&file).unwrap();
    let mut client = uncached(std.parent().unwrap(), QUERIES_ONLY.clone());
    let (line, character) = at(&text, "trait pub ToString", 10, false);
    let item = |client: &mut Client, method: &str| {
        let id = client.send_request(method, position(&file, line, character));

        client.response(id)["result"][0].clone()
    };
    let named = item(&mut client, "textDocument/prepareTypeHierarchy");
    let (line, character) = at(&text, "fn pub to_string -> String\n", 7, false);
    let method = {
        let id = client.send_request(
            "textDocument/prepareCallHierarchy",
            position(&file, line, character),
        );

        client.response(id)["result"][0].clone()
    };
    let (trait_line, trait_character) = at(&text, "trait pub ToString", 10, false);
    let target = std.join("std/strings.inko");
    let asked = [
        (
            "textDocument/implementation",
            position(&file, trait_line, trait_character),
        ),
        ("callHierarchy/incomingCalls", json!({ "item": method })),
        // The same method's references, for comparison: what incoming
        // calls find, without the callers' items.
        ("textDocument/references", position(&file, line, character)),
        ("typeHierarchy/subtypes", json!({ "item": named })),
        (
            "workspace/willRenameFiles",
            json!({ "files": [{ "oldUri": uri(&file), "newUri": uri(&target) }] }),
        ),
    ];

    for (method, params) in asked {
        let times = request_times(&mut client, method, params);
        let median = median(times.clone());

        eprintln!("{} across std: {:?} (all: {:?})", method, median, times);

        let max = match method {
            "typeHierarchy/subtypes" | "workspace/willRenameFiles" => limit(WHOLE_PROJECT_LIMIT),
            _ => limit(LIMIT),
        };

        assert!(
            median < max,
            "{}: median {:?} exceeds {:?}",
            method,
            median,
            max
        );
    }
}

/// How long a document's code lenses may take across all of std past an
/// error in a declaration: every module of the project that names one of
/// the names they're about is looked at for a unit the error makes unsafe
/// mentioning it (`hidden`), and in std, as its own project, that's nearly
/// every module (`String`, `ToString`). A project's own modules are all
/// that count, so this is the worst case.
const PAST_AN_ERROR_LIMIT: Duration = Duration::from_millis(1500);

/// A document's code lenses ask every declaration's implementations and
/// supertypes, and its `impl`s' traits, in one request: one check of the
/// whole project and one of the file, then a lookup each. Across all of
/// std, for std's `string.inko` (12 traits and types, 40 `impl`s), and
/// past an error in a declaration of another module, where each whole
/// project question also looks for the units that could hide a part of
/// it.
#[test]
fn code_lenses_across_std_are_fast_enough() {
    let _shared = shared();
    let std = upstream_std();
    let file = std.join("std/string.inko");
    let mut client = Client::initialized_with(
        Launch {
            env: vec![(NO_CACHE_VARIABLE, "1".to_string())],
            unset: vec![],
            cwd: None,
        },
        std.parent().unwrap(),
        QUERIES_ONLY.clone(),
        json!({ "textDocument": { "codeLens": {} } }),
    );
    let params = json!({ "textDocument": { "uri": uri(&file) } });
    let lenses = client.request("textDocument/codeLens", params.clone());

    assert!(
        lenses["result"].as_array().unwrap().len() > 40,
        "{}",
        lenses
    );

    let other = std.join("std/stdio.inko");
    let broken = std::fs::read_to_string(&other).unwrap().replacen(
        "fn pub static new -> Stdin {",
        "fn pub static new -> Stdinn {",
        1,
    );

    for (what, open) in [("", None), (", past an error", Some(broken))] {
        if let Some(text) = &open {
            client.open(&other, text);
        }

        let times = request_times(&mut client, "textDocument/codeLens", params.clone());
        let median = median(times.clone());

        eprintln!(
            "code lenses across std{}: {:?} (all: {:?})",
            what, median, times
        );

        let max = match open {
            None => limit(WHOLE_PROJECT_LIMIT),
            Some(_) => limit(PAST_AN_ERROR_LIMIT),
        };

        assert!(median < max, "median {:?} exceeds {:?}", median, max);
    }
}

/// Declaration, preparing the hierarchies' items, outgoing calls and
/// supertypes check the file and what it imports, as definition does.
#[test]
fn declarations_and_hierarchy_items_on_a_std_sized_project_are_fast_enough() {
    let _shared = shared();
    let (root, text) = std_sized_project();
    let file = root.join("src/main.inko");
    let mut client = uncached(&root, QUERIES_ONLY.clone());
    let (line, character) = at(&text, "[1, 2].size", 7, false);
    let (main_line, main_character) = at(&text, "fn async main", 9, false);
    let main = {
        let id = client.send_request(
            "textDocument/prepareCallHierarchy",
            position(&file, main_line, main_character),
        );

        client.response(id)["result"][0].clone()
    };

    for (method, params) in [
        ("textDocument/declaration", position(&file, line, character)),
        (
            "textDocument/prepareCallHierarchy",
            position(&file, line, character),
        ),
        ("callHierarchy/outgoingCalls", json!({ "item": main })),
    ] {
        let times = request_times(&mut client, method, params);
        let median = median(times.clone());

        eprintln!(
            "{} on a std-sized project: {:?} (all: {:?})",
            method, median, times
        );

        let max = limit(LIMIT);

        assert!(
            median < max,
            "{}: median {:?} exceeds {:?}",
            method,
            median,
            max
        );
    }
}

/// How long a pull of diagnostics may take: a full check, to MIR, of a
/// project that imports all of std.
const PULL_LIMIT: Duration = Duration::from_millis(1000);

#[test]
fn pulling_diagnostics_on_a_std_sized_project_is_fast_enough() {
    let _alone = alone();
    let (root, text) = std_sized_project();
    let file = root.join("src/main.inko");
    let mut client = Client::initialized_with(
        Launch {
            env: vec![(NO_CACHE_VARIABLE, "1".to_string())],
            unset: vec![],
            cwd: None,
        },
        &root,
        json!({ "diagnosticsDebounceMs": 0 }),
        json!({ "textDocument": { "diagnostic": {} } }),
    );
    let mut times = Vec::new();

    client.open(&file, &text);

    for run in 0..RUNS {
        // A new text each time, so each pull waits for a check of it.
        client.change(
            &file,
            run as i32 + 2,
            json!([{ "text": format!("{}\n# {}\n", text, run) }]),
        );

        let start = Instant::now();
        let id = client.send_request(
            "textDocument/diagnostic",
            json!({ "textDocument": { "uri": uri(&file) } }),
        );
        let response = client.response(id);

        times.push(start.elapsed());
        assert_eq!(response["result"]["kind"], "full", "{}", response);
    }

    let median = median(times.clone());

    eprintln!(
        "a document's diagnostics pulled: {:?} (all: {:?})",
        median, times
    );

    let max = limit(PULL_LIMIT);

    assert!(median < max, "median {:?} exceeds {:?}", median, max);
}

/// While a module doesn't parse, a pull checks the project again without
/// it and what imports it: the first check stops before any type checking,
/// so the two cost about one, under the same limit.
#[test]
fn pulling_diagnostics_past_a_module_that_doesnt_parse_is_fast_enough() {
    let _alone = alone();
    let (root, text) = std_sized_project();
    let file = root.join("src/main.inko");
    let typing = root.join("src/typing.inko");

    std::fs::write(&typing, "fn pub typing -> Int {\n  1\n}\n").unwrap();

    let mut client = Client::initialized_with(
        Launch {
            env: vec![(NO_CACHE_VARIABLE, "1".to_string())],
            unset: vec![],
            cwd: None,
        },
        &root,
        json!({ "diagnosticsDebounceMs": 0 }),
        json!({ "textDocument": { "diagnostic": {} } }),
    );
    let mut times = Vec::new();

    client.open(&file, &text);
    client.open(&typing, "fn pub typing -> Int {\n  1\n}\n");

    for run in 0..RUNS {
        // A new text each time, which doesn't parse.
        client.change(
            &typing,
            run as i32 + 2,
            json!([{ "text": format!("fn pub typing -> Int {{\n  {} +\n", run) }]),
        );

        let start = Instant::now();
        let id = client.send_request(
            "textDocument/diagnostic",
            json!({ "textDocument": { "uri": uri(&file) } }),
        );
        let response = client.response(id);

        times.push(start.elapsed());
        assert_eq!(response["result"]["kind"], "full", "{}", response);
    }

    let _ = std::fs::remove_file(&typing);
    let median = median(times.clone());

    eprintln!(
        "a document's diagnostics pulled, past a module that doesn't parse: {:?} (all: {:?})",
        median, times
    );

    let max = limit(PULL_LIMIT);

    assert!(median < max, "median {:?} exceeds {:?}", median, max);
}

/// Queries don't wait for diagnostics, and stay under hover's limit while a
/// diagnostics check of the same std-sized project runs in another worker.
/// Each round edits the document, which starts a full check at once
/// (`diagnosticsDebounceMs: 0`), then sends a query, which starts at once
/// too (`debounceMs: 0`). The check must still be running when the query's
/// answer arrives, so the two overlapped.
#[test]
fn queries_stay_fast_while_diagnostics_are_checked() {
    let _alone = alone();
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
        let max = limit(LIMIT);
        assert!(
            median < max,
            "{}: median {:?} exceeds {:?}",
            method,
            median,
            max
        );
    }
}

/// Code that doesn't parse: a hover answered from a repaired copy (two
/// checks), and the worst case, a hover after the edit, where neither the
/// repaired copy nor the last good check may answer (three checks, and no
/// answer).
#[test]
fn answers_in_code_being_edited_are_fast_enough() {
    let _shared = shared();
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
        let max = limit(LIMIT);
        assert!(median < max, "median {:?} exceeds {:?}", median, max);
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
    // It loads the machine: the server and the worker are busy throughout.
    let _alone = alone();
    let (root, text) = std_sized_project();
    let file = root.join("src/main.inko");
    // Kept checks, even when every test runs without them.
    let mut client = Client::initialized_with(
        Launch {
            env: vec![],
            unset: vec![NO_CACHE_VARIABLE],
            cwd: None,
        },
        &root,
        json!({ "diagnostics": false, "statusNotifications": true }),
        json!({}),
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
    let max = limit(BURST_LIMIT);
    assert!(elapsed < max, "{:?} exceeds {:?}", elapsed, max);
}

/// How long a person typing at about 7 characters a second takes between
/// keystrokes.
const KEYSTROKE: Duration = Duration::from_millis(143);

/// Text typed into a file, one character per keystroke: `typed` inserted
/// after `after`, or, with `retype`, the line containing `after` deleted
/// and typed again.
struct Typing {
    name: &'static str,
    root: PathBuf,
    file: PathBuf,
    after: &'static str,
    typed: &'static str,
    retype: bool,
}

fn typings() -> Vec<Typing> {
    let std = upstream_std();
    let features = common::fixture("features");

    vec![
        Typing {
            name: "std json: a new method",
            root: std.parent().unwrap().to_path_buf(),
            file: std.join("std/json.inko"),
            after: "    Query(Option.Some(self))\n  }\n",
            typed: "\n  fn pub size_hint -> Int {\n    let text = to_string\n    \
                    let words = text.split(',').to_array\n    words.size + text.size\n  }\n",
            retype: false,
        },
        Typing {
            name: "std json: a line in a method",
            root: std.parent().unwrap().to_path_buf(),
            file: std.join("std/json.inko"),
            after: "Generator.new(DEFAULT_PRETTY_INDENT).generate(self)",
            typed: "",
            retype: true,
        },
        Typing {
            name: "features: a line in a method",
            root: features.clone(),
            file: features.join("src/main.inko"),
            after: "counter.add(amount: 1, times: count)",
            typed: "",
            retype: true,
        },
    ]
}

/// One request of a typing replay, as the client saw it.
struct Asked {
    method: &'static str,
    sent: Instant,
    /// The time to its response, and whether it was an error (cancelled).
    answered: Option<(Duration, bool)>,
}

/// What a typing replay measured.
#[derive(Default, Debug)]
struct Typed {
    requests: usize,
    cancelled: usize,
    /// Answers with a result or `null`, and of those, the ones from a check
    /// run for them (not `cached`), and their time in the worker.
    answered: usize,
    checks: usize,
    check_ms: Vec<f64>,
    /// Answers whose reason is `noHir`, and of those, the `cached` ones.
    no_hir: usize,
    no_hir_cached: usize,
    /// Every answer's time in the worker.
    worker_ms: f64,
    /// The client's time from request to response, for answered requests.
    latency: Vec<Duration>,
    /// Query worker processes started.
    workers: usize,
    /// Cancelled checks killed because a query waited too long for them.
    killed: usize,
    /// How many answers had each method, status, reason and source.
    kinds: std::collections::BTreeMap<String, usize>,
    elapsed: Duration,
}

/// Opens the file, then types into it at `KEYSTROKE`s, as an editor would
/// ask: after each keystroke, document highlights and the definition at the
/// cursor, the inlay hints of the 50 lines around it, and completion after
/// a `.`.
/// Each edit first cancels the requests the client hasn't had an answer to,
/// as a client that cancels what an edit made outdated does.
fn replay_typing(client: &mut Client, typing: &Typing) -> Typed {
    let text = std::fs::read_to_string(&typing.file).unwrap();
    let at = text.find(typing.after).unwrap();
    let (mut offset, typed) = if typing.retype {
        let start = at + (typing.after.len() - typing.after.trim_start().len());

        (start, typing.after.trim_start().to_string())
    } else {
        (at + typing.after.len(), typing.typed.to_string())
    };
    let mut now = text.clone();
    let file = &typing.file;
    let mut asked: HashMap<i64, Asked> = HashMap::new();
    let mut statuses: HashMap<i64, serde_json::Value> = HashMap::new();
    let mut pids: HashSet<u32> = HashSet::new();
    let mut version = 1;
    let position = |text: &str, offset: usize| {
        let before = &text[..offset];
        let line = before.matches('\n').count() as u32;
        let character = (offset - before.rfind('\n').map_or(0, |i| i + 1)) as u32;

        (line, character)
    };

    client.open(file, &text);

    // The last good check, as after opening the file in an editor.
    let (line, character) = position(&text, offset);
    let (id, _) = client.hover(file, line, character);

    statuses.extend(client.status(id).map(|s| (id, s.clone())));
    pids.extend(client.workers());

    let mut edits: Vec<(usize, usize, String)> = Vec::new();

    if typing.retype {
        edits.push((offset, offset + typed.len(), String::new()));
    }

    for (i, c) in typed.char_indices() {
        edits.push((offset + i, offset + i, c.to_string()));
    }

    let start = Instant::now();
    let mut next = start;
    let receive = |client: &mut Client,
                   asked: &mut HashMap<i64, Asked>,
                   statuses: &mut HashMap<i64, serde_json::Value>,
                   until: Instant| {
        loop {
            let left = until.saturating_duration_since(Instant::now());
            let Ok(message) = client.receive(left) else {
                return;
            };

            if message["method"] == "cuttlebone/status" {
                if let Some(id) = message["params"]["id"].as_i64() {
                    statuses.insert(id, message["params"].clone());
                }
            } else if let Some(id) = message["id"].as_i64()
                && let Some(a) = asked.get_mut(&id)
            {
                a.answered = Some((a.sent.elapsed(), message.get("error").is_some()));
            }

            if left.is_zero() {
                return;
            }
        }
    };

    for (from, to, insert) in edits {
        next += KEYSTROKE;
        receive(client, &mut asked, &mut statuses, next);

        // Before the cancels, which may stop it.
        pids.extend(client.workers());

        // The edit makes every request not answered yet outdated.
        let outdated: Vec<i64> = asked
            .iter()
            .filter(|(_, a)| a.answered.is_none())
            .map(|(id, _)| *id)
            .collect();

        for id in outdated {
            client.notify("$/cancelRequest", json!({ "id": id }));
        }

        let (start_line, start_character) = position(&now, from);
        let (end_line, end_character) = position(&now, to);

        now.replace_range(from..to, &insert);
        version += 1;
        client.change(
            file,
            version,
            json!([{
                "range": {
                    "start": { "line": start_line, "character": start_character },
                    "end": { "line": end_line, "character": end_character },
                },
                "text": insert,
            }]),
        );
        offset = from + insert.len();

        let (line, character) = position(&now, offset);
        let mut send = |client: &mut Client, method: &'static str, params: serde_json::Value| {
            let id = client.send_request(method, params);

            asked.insert(
                id,
                Asked {
                    method,
                    sent: Instant::now(),
                    answered: None,
                },
            );
        };

        send(
            client,
            "textDocument/documentHighlight",
            json!({
                "textDocument": { "uri": common::client::uri(file) },
                "position": { "line": line, "character": character.saturating_sub(1) },
            }),
        );
        // As a client that looks up the definition where it highlights,
        // to have it ready.
        send(
            client,
            "textDocument/definition",
            json!({
                "textDocument": { "uri": common::client::uri(file) },
                "position": { "line": line, "character": character.saturating_sub(1) },
            }),
        );
        send(
            client,
            "textDocument/inlayHint",
            json!({
                "textDocument": { "uri": common::client::uri(file) },
                "range": {
                    "start": { "line": line.saturating_sub(25), "character": 0 },
                    "end": { "line": line + 25, "character": 0 },
                },
            }),
        );

        if insert == "." {
            send(
                client,
                "textDocument/completion",
                json!({
                    "textDocument": { "uri": common::client::uri(file) },
                    "position": { "line": line, "character": character },
                }),
            );
        }
    }

    // Everything sent gets its answer.
    let end = Instant::now() + common::client::TIMEOUT;

    while asked.values().any(|a| a.answered.is_none()) && Instant::now() < end {
        receive(
            client,
            &mut asked,
            &mut statuses,
            Instant::now() + Duration::from_millis(100),
        );
    }

    let elapsed = start.elapsed();

    if std::env::var_os("CUTTLEBONE_TYPING_TIMELINE").is_some() {
        let mut ids: Vec<&i64> = asked.keys().collect();

        ids.sort();

        for id in ids.iter().take(60) {
            let a = &asked[id];

            eprintln!(
                "{} {} sent {:?} answer {:?} {}",
                id,
                a.method,
                a.sent.duration_since(start),
                a.answered,
                statuses.get(id).map(|s| s.to_string()).unwrap_or_default()
            );
        }
    }

    pids.extend(client.workers());
    // It ends with code that type checks.
    let (line, character) = position(&now, offset.saturating_sub(2));
    let (id, _) = client.hover(file, line, character);

    assert_eq!(
        client.status(id).unwrap()["status"],
        "fresh",
        "{}: {}",
        typing.name,
        client.status(id).unwrap()
    );

    if typing.retype {
        assert_eq!(now, text, "{}", typing.name);
    }

    client.close(file);

    let mut out = Typed {
        killed: client
            .stderr
            .lock()
            .unwrap()
            .matches("a cancelled check ran on")
            .count(),
        requests: asked.len(),
        workers: pids.len(),
        elapsed,
        ..Typed::default()
    };

    for (id, a) in &asked {
        let Some((time, error)) = a.answered else {
            panic!("{}: no answer to {} {}", typing.name, a.method, id);
        };

        if error {
            out.cancelled += 1;
            continue;
        }

        let status = statuses.get(id).unwrap();
        let ms = status["elapsedMs"].as_f64().unwrap();

        *out.kinds
            .entry(format!(
                "{} {} {}{}{}",
                a.method.trim_start_matches("textDocument/"),
                status["status"].as_str().unwrap(),
                status["reason"]["code"].as_str().unwrap_or(""),
                if status.get("repaired").is_some() {
                    " repaired"
                } else {
                    ""
                },
                if status.get("cached").is_some() {
                    " cached"
                } else {
                    ""
                },
            ))
            .or_default() += 1;

        out.answered += 1;
        out.latency.push(time);
        out.worker_ms += ms;

        if status.get("cached").is_none() && ms > 0.0 {
            out.checks += 1;
            out.check_ms.push(ms);
        }

        if status["reason"]["code"] == "noHir" {
            out.no_hir += 1;
            out.no_hir_cached += usize::from(status.get("cached").is_some());
        }
    }

    out
}

fn median_ms(mut values: Vec<f64>) -> f64 {
    if values.is_empty() {
        return 0.0;
    }

    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

/// Measures typing (`replay_typing`) in every `typings`, and prints it.
/// Set `CUTTLEBONE_TYPING_REPLAY` to run it (it takes about a minute).
#[test]
fn typing_replay() {
    if std::env::var_os("CUTTLEBONE_TYPING_REPLAY").is_none() {
        return;
    }

    let _alone = alone();

    for typing in typings() {
        let mut client = Client::initialized_with(
            Launch {
                env: vec![],
                unset: vec![NO_CACHE_VARIABLE],
                cwd: None,
            },
            &typing.root,
            json!({ "diagnostics": false, "statusNotifications": true }),
            json!({}),
        );
        let t = replay_typing(&mut client, &typing);
        let mut latency = t.latency.clone();

        latency.sort();
        eprintln!(
            "{}: {} requests in {:.1} s, {} cancelled, {} answered; {} checks (median {:.1} ms), \
             {:.2} s in the worker; noHir {} ({} cached); latency median {:?}, p90 {:?}; {} workers, {} killed after the grace",
            typing.name,
            t.requests,
            t.elapsed.as_secs_f64(),
            t.cancelled,
            t.answered,
            t.checks,
            median_ms(t.check_ms.clone()),
            t.worker_ms / 1000.0,
            t.no_hir,
            t.no_hir_cached,
            median(latency.clone()),
            latency[latency.len() * 9 / 10],
            t.workers,
            t.killed,
        );
        eprintln!("  {:?}", t.kinds);
    }
}

/// A server that keeps checks, even when every test runs without them.
fn kept(root: &Path, options: serde_json::Value) -> Client {
    Client::initialized_with(
        Launch {
            env: vec![],
            unset: vec![NO_CACHE_VARIABLE],
            cwd: None,
        },
        root,
        options,
        json!({}),
    )
}

/// The worker's time for request `id`, from its status.
fn worker_time(client: &Client, id: i64) -> Duration {
    let status = client.status(id).unwrap();

    Duration::from_secs_f64(status["elapsedMs"].as_f64().unwrap() / 1000.0)
}

/// Cancelling a running check leaves its worker, and the checks it kept,
/// so the next question costs what it would have without the cancel. In
/// code being typed that doesn't parse, each question uses a check kept
/// from before: here the repaired copy is the last good text at every
/// edit. Killing the worker, as cancelling did before milestone 9, made
/// the next question check that again. Each round makes a new edit that
/// doesn't parse and asks a hover; in half of them, a hover about the edit
/// before is cancelled while it's checked first.
#[test]
fn a_question_after_a_cancelled_one_is_no_slower() {
    let _alone = alone();
    let (root, text) = std_sized_project();
    let file = root.join("src/main.inko");
    let mut client = kept(
        &root,
        json!({ "diagnostics": false, "debounceMs": 0, "statusNotifications": true }),
    );
    let (line, character) = at(&text, "[1, 2].size", 7, false);

    client.open(&file, &text);
    client.hover(&file, line, character);

    let mut version = 1;
    // Each a new text that doesn't parse, whose repaired copy is `text`.
    let mut edit = |client: &mut Client, i: usize| {
        version += 1;
        client.change(
            &file,
            version,
            json!([{ "text": text.replacen(
                "    let doubled",
                &format!("    let broken{} = (\n    let doubled", i),
                1,
            ) }]),
        );
    };
    let (mut plain, mut after) = (Vec::new(), Vec::new());

    for i in 0..2 * RUNS {
        let cancel = i % 2 == 1;

        if cancel {
            // The server starts the check before it reads the cancel.
            edit(&mut client, 2 * i);
            let id = client.send_at("textDocument/hover", &file, line, character);

            client.notify("$/cancelRequest", json!({ "id": id }));
            assert_eq!(client.response(id)["error"]["code"], -32800);
        }

        edit(&mut client, 2 * i + 1);

        let start = Instant::now();
        let (id, response) = client.hover(&file, line, character);
        let time = start.elapsed();

        assert!(response["result"].is_object(), "{}", response);
        assert_eq!(client.status(id).unwrap()["repaired"], true);
        (if cancel { &mut after } else { &mut plain }).push(time);
    }

    let (plain_median, after_median) = (median(plain.clone()), median(after.clone()));

    eprintln!(
        "a question in code that doesn't parse: {:?} (all: {:?}); after a cancelled one: {:?} (all: {:?})",
        plain_median, plain, after_median, after
    );
    assert!(
        after_median < plain_median.mul_f64(AFTER_A_CANCEL),
        "{:?} after a cancel, {:?} without",
        after_median,
        plain_median
    );
}

/// How much longer than another question the one after a cancelled one
/// may take (`a_question_after_a_cancelled_one_is_no_slower`): it waits for
/// the cancelled check to finish, about one question's time, and is then
/// answered from what that kept. Measured on an Apple M1 Max: 2.0 times as
/// long; 4.5 times before milestone 9, when the worker and its checks were
/// killed. The limit leaves a quarter for noise.
const AFTER_A_CANCEL: f64 = 2.5;

/// How long a second question about code that doesn't parse may take in
/// the worker, as a share of the first, where nothing answers: it's
/// lookups in the checks the first kept
/// (`a_second_question_in_code_that_doesnt_parse_is_a_lookup`). Measured
/// on an Apple M1 Max: 9.7 ms, 0.32 of the first's 30 ms. Before milestone
/// 9 it was a check again, as long as the first.
const UNPARSED_LOOKUP: f64 = 0.6;

/// In code that doesn't parse, a question that nothing can answer (the
/// documents as they are, a repaired copy, the last good check) is a
/// lookup the second time, as one with an answer is.
#[test]
fn a_second_question_in_code_that_doesnt_parse_is_a_lookup() {
    let _shared = shared();
    let (root, text) = std_sized_project();
    let file = root.join("src/main.inko");
    let mut client = kept(
        &root,
        json!({ "diagnostics": false, "statusNotifications": true }),
    );
    let (line, character) = at(&text, "[1, 2].size", 7, false);

    client.open(&file, &text);
    client.hover(&file, line, character);

    let mut first = Vec::new();
    let mut second = Vec::new();
    let mut statuses = Vec::new();

    for i in 0..RUNS {
        let edited = text.replacen(
            "    let doubled",
            &format!("    let broken{} = (\n    let doubled", i),
            1,
        );
        let (line, character) = at(&edited, "broken", 0, false);

        client.change(&file, 2 + i as i32, json!([{ "text": edited }]));

        for times in [&mut first, &mut second] {
            let (id, response) = client.hover(&file, line, character);
            let status = client.status(id).unwrap();

            assert!(response["result"].is_null(), "{}", response);
            assert_eq!(status["reason"]["code"], "noHir", "{}", status);
            times.push(worker_time(&client, id));
        }

        statuses.push(client.notifications.last().unwrap()["params"].clone());
    }

    let (first_median, second_median) = (median(first.clone()), median(second.clone()));

    eprintln!(
        "a question in code that doesn't parse, with no answer: {:?} (all: {:?}); \
         the second: {:?} (all: {:?})",
        first_median, first, second_median, second
    );

    for status in statuses {
        assert_eq!(status["cached"], true, "{}", status);
    }
    assert!(
        second_median < first_median.mul_f64(UNPARSED_LOOKUP),
        "the second: {:?}, the first: {:?}",
        second_median,
        first_median
    );
}
