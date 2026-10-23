//! Latency limits, measured from an LSP client's side, and the cost of
//! running each check in a worker process instead of in the server's own
//! thread.
// Tests report the measurements on stderr, which `cargo test` shows.
#![allow(clippy::print_stderr)]
mod common;

use common::client::{Client, at};
use cuttlebone::dump::upstream_std;
use cuttlebone::position::{Encoding, Position};
use cuttlebone::query::{self, Request, Session};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The latency limit for a hover on a std-sized project.
const LIMIT: Duration = Duration::from_millis(500);

/// How much a worker process may add to a check.
const PROCESS_LIMIT: Duration = Duration::from_millis(50);

const RUNS: usize = 7;

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

    text.push_str("\ntype async Main {\n  fn async main {\n    let value = [1, 2].size\n  }\n}\n");
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
    let mut client = Client::initialized(&root, json!({}));
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
    let mut client = Client::initialized(std.parent().unwrap(), json!({}));
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
    let mut client = Client::initialized(&root, json!({}));
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
    let mut client = Client::initialized(&root, json!({}));
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
