//! End-to-end tests: launch the real `cuttlebone` binary and drive the
//! protocol (docs/protocol.md) over stdio.
//!
//! The handshake runs `inko --version` and `inko print std`, so these tests
//! use a fake `inko` script that reports the pinned version and the pinned
//! std, and so don't depend on what's installed.
mod common;

use common::{PLACEHOLDER, fixture, position_of};
use cuttlebone::{upstream_std, upstream_version};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

/// How long to wait for any reply before failing.
const TIMEOUT: Duration = Duration::from_secs(30);

/// The latency limit for typeAt on a std-sized project.
const LIMIT: Duration = Duration::from_millis(500);

struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    replies: Receiver<String>,
    next_id: u64,
}

impl Server {
    fn start() -> Server {
        let mut child = Command::new(env!("CARGO_BIN_EXE_cuttlebone"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, replies) = channel();

        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };

                if tx.send(line).is_err() {
                    break;
                }
            }
        });

        Server {
            child,
            stdin: Some(stdin),
            replies,
            next_id: 1,
        }
    }

    /// Starts a server and completes the handshake with the fake `inko`.
    fn initialized(project: &Path) -> Server {
        let mut server = Server::start();
        let reply = server.request(
            "initialize",
            json!({ "inko": fake_inko(upstream_version()), "project": project }),
        );

        assert_eq!(reply["status"], "fresh", "{}", reply);
        server
    }

    fn send_line(&mut self, line: &str) -> Value {
        let stdin = self.stdin.as_mut().unwrap();

        writeln!(stdin, "{}", line).unwrap();
        stdin.flush().unwrap();

        let reply = self
            .replies
            .recv_timeout(TIMEOUT)
            .unwrap_or_else(|e| panic!("no reply to {}: {}", line, e));

        serde_json::from_str(&reply).unwrap_or_else(|e| panic!("{}: {}", reply, e))
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;

        self.next_id += 1;

        let reply =
            self.send_line(&json!({ "id": id, "method": method, "params": params }).to_string());

        assert_eq!(reply["id"], id, "{}", reply);
        assert!(
            reply["status"] == "fresh" || reply["status"] == "unavailable",
            "every reply has a status: {}",
            reply
        );
        reply
    }

    fn at(
        &mut self,
        method: &str,
        file: &Path,
        text: &str,
        needle: &str,
        offset: u32,
        contents: Option<&str>,
    ) -> Value {
        let p = position_of(text, needle, offset);
        let mut params = json!({
            "file": file,
            "position": { "line": p.line, "character": p.character },
        });

        if let Some(contents) = contents {
            params["contents"] = json!({ file.to_str().unwrap(): contents });
        }

        self.request(method, params)
    }

    /// Waits for the process to exit, and returns whether it succeeded.
    fn wait(mut self) -> bool {
        let start = Instant::now();

        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.success();
            }

            assert!(start.elapsed() < TIMEOUT, "the server didn't exit");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A fake `inko` that reports `version`, and the pinned std for
/// `inko print std`.
fn fake_inko(version: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("inko-{}", version));
    let path = dir.join("inko");
    let script = format!(
        "#!/bin/sh\ncase \"$1\" in\n  --version) echo 'inko {}' ;;\n  print) echo '{}' ;;\n  *) exit 1 ;;\nesac\n",
        version,
        upstream_std().display()
    );

    std::fs::create_dir_all(&dir).unwrap();

    // Write to a temporary name and rename, so tests running in parallel
    // never run a half-written script.
    static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dir.join(format!("inko.{}.{}", std::process::id(), n));

    std::fs::write(&tmp, script).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    std::fs::rename(&tmp, &path).unwrap();
    path
}

#[test]
fn the_handshake_reports_both_versions_and_the_standard_library() {
    let mut server = Server::start();
    let reply = server.request(
        "initialize",
        json!({ "protocol": 1, "inko": fake_inko(upstream_version()), "project": fixture("members") }),
    );

    assert_eq!(reply["status"], "fresh");
    assert_eq!(reply["result"]["cuttlebone"], env!("CARGO_PKG_VERSION"));
    assert_eq!(reply["result"]["inko"], "0.21.1");
    assert_eq!(reply["result"]["protocol"], 1);
    assert_eq!(reply["result"]["std"], json!(upstream_std()));
}

#[test]
fn the_handshake_refuses_a_different_inko_version() {
    let mut server = Server::start();
    let reply = server.request("initialize", json!({ "inko": fake_inko("0.20.0") }));

    assert_eq!(reply["status"], "unavailable");
    assert_eq!(reply["reason"]["code"], "inkoVersionMismatch");
    assert_eq!(reply["reason"]["installed"], "0.20.0");
    assert_eq!(reply["reason"]["supported"], "0.21.1");
    assert_eq!(reply["reason"]["cuttlebone"], env!("CARGO_PKG_VERSION"));

    // Nothing else works until a handshake succeeds.
    let file = fixture("members").join("src/main.inko");
    let reply = server.request(
        "typeAt",
        json!({ "file": file, "position": { "line": 0, "character": 0 } }),
    );

    assert_eq!(reply["reason"]["code"], "notInitialized");
}

#[test]
fn the_handshake_reports_a_missing_inko() {
    let mut server = Server::start();
    let reply = server.request("initialize", json!({ "inko": "/does/not/exist/inko" }));

    assert_eq!(reply["reason"]["code"], "inkoNotFound");
}

#[test]
fn the_handshake_refuses_another_protocol_version() {
    let mut server = Server::start();
    let reply = server.request(
        "initialize",
        json!({ "protocol": 2, "inko": fake_inko(upstream_version()) }),
    );

    assert_eq!(reply["reason"]["code"], "protocolMismatch");
    assert_eq!(reply["reason"]["protocol"], 1);
}

#[test]
fn type_at_and_members_at_over_stdio() {
    let root = fixture("members");
    let file = root.join("src/main.inko");
    let text = std::fs::read_to_string(&file).unwrap();
    let mut server = Server::initialized(&root);

    let reply = server.at("typeAt", &file, &text, "name.size", 5, None);

    assert_eq!(reply["status"], "fresh", "{}", reply);
    assert_eq!(reply["result"]["type"], "Int");
    assert_eq!(reply["result"]["kind"], "Call");

    let start = position_of(&text, "name.size", 0);

    assert_eq!(
        reply["result"]["range"],
        json!({
            "start": { "line": start.line, "character": start.character },
            "end": { "line": start.line, "character": start.character + 9 },
        })
    );
    assert!(reply["elapsedMs"].as_f64().unwrap() > 0.0);

    // Completion: the placeholder in an unsaved copy of the buffer.
    let after = format!("name.{}", PLACEHOLDER);
    let unsaved = text.replacen("name.size", &after, 1);
    let reply = server.at("membersAt", &file, &unsaved, &after, 5, Some(&unsaved));

    assert_eq!(reply["status"], "fresh", "{}", reply);
    assert_eq!(reply["result"]["receiver"]["type"], "String");

    let size = reply["result"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "size")
        .unwrap();

    assert_eq!(
        size,
        &json!({
            "name": "size",
            "kind": "method",
            "signature": "fn pub inline size -> Int",
            "type": "Int",
            "module": "std.string",
        })
    );

    // Nothing at the position is fresh, with a null result.
    let reply = server.at("typeAt", &file, &text, "# Receivers", 0, None);

    assert_eq!(reply["status"], "fresh");
    assert_eq!(reply["result"], Value::Null);
}

#[test]
fn unanswerable_requests_are_unavailable_with_a_reason() {
    let root = fixture("broken");
    let file = root.join("src/main.inko");
    let text = std::fs::read_to_string(&file).unwrap();
    let mut server = Server::initialized(&root);
    let broken = text.replacen("let mut i = 0", "let mut i = )", 1);
    let reply = server.at("typeAt", &file, &broken, "values.size", 0, Some(&broken));

    assert_eq!(reply["status"], "unavailable");
    assert_eq!(reply["reason"]["code"], "noHir");
    assert!(reply["reason"]["message"].as_str().unwrap().len() > 10);
    assert!(reply.get("result").is_none());
}

#[test]
fn a_compiler_panic_doesnt_stop_the_server() {
    let root = fixture("panic");
    let file = root.join("src/main.inko");
    let text = std::fs::read_to_string(&file).unwrap();
    let mut server = Server::initialized(&root);

    for _ in 0..2 {
        let reply = server.at("typeAt", &file, &text, "let value", 4, None);

        assert_eq!(reply["status"], "unavailable");
        assert_eq!(reply["reason"]["code"], "compilerPanic");
    }

    let other = fixture("members").join("src/main.inko");
    let text = std::fs::read_to_string(&other).unwrap();
    let reply = server.at("typeAt", &other, &text, "let name", 4, None);

    assert_eq!(reply["result"]["type"], "String", "{}", reply);
}

#[test]
fn malformed_requests_get_a_reply_and_the_server_keeps_going() {
    let mut server = Server::initialized(&fixture("members"));

    let reply = server.send_line("{not json");

    assert_eq!(reply["id"], Value::Null);
    assert_eq!(reply["status"], "unavailable");
    assert_eq!(reply["reason"]["code"], "invalidRequest");

    let reply = server.send_line("[1, 2]");

    assert_eq!(reply["reason"]["code"], "invalidRequest");

    let reply = server.request("frobnicate", json!({}));

    assert_eq!(reply["reason"]["code"], "unknownMethod");

    let reply = server.request("typeAt", json!({ "file": 3 }));

    assert_eq!(reply["reason"]["code"], "invalidRequest");

    let reply = server.request(
        "typeAt",
        json!({ "file": "/x.inko", "position": { "line": 0 } }),
    );

    assert_eq!(reply["reason"]["code"], "invalidRequest");

    let reply = server.request("shutdown", Value::Null);

    assert_eq!(reply["status"], "fresh");
    assert!(server.wait());
}

#[test]
fn the_server_exits_when_its_input_ends() {
    let mut server = Server::initialized(&fixture("members"));

    server.stdin = None;
    assert!(server.wait());
}

/// A project whose main module imports every std module that builds on
/// this platform, so one typeAt type checks all of std/src.
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

#[test]
fn type_at_on_a_std_sized_project_is_fast_enough() {
    let (root, text) = std_sized_project();
    let file = root.join("src/main.inko");
    let mut server = Server::initialized(&root);
    let mut times = Vec::new();

    for _ in 0..5 {
        let start = Instant::now();
        let reply = server.at("typeAt", &file, &text, "[1, 2].size", 7, None);
        let elapsed = start.elapsed();

        assert_eq!(reply["result"]["type"], "Int", "{}", reply);
        times.push(elapsed);
    }

    times.sort();

    // The median, measured from the client's side, so it includes the
    // protocol's overhead.
    let median = times[times.len() / 2];

    eprintln!(
        "typeAt on a std-sized project: {:?} (all: {:?})",
        median, times
    );
    assert!(
        median < LIMIT,
        "median {:?} exceeds {:?}: {:?}",
        median,
        LIMIT,
        times
    );
}

#[test]
fn type_at_in_the_largest_std_module_is_fast_enough() {
    // std.net.http.server imports the most of std (90 modules).
    let std = upstream_std();
    let file = std.join("std/net/http/server.inko");
    let text = std::fs::read_to_string(&file).unwrap();
    let mut server = Server::initialized(std.parent().unwrap());
    let mut times = Vec::new();

    for _ in 0..5 {
        let start = Instant::now();
        let reply = server.at("typeAt", &file, &text, "import std.", 0, None);

        assert_eq!(reply["status"], "fresh", "{}", reply);
        times.push(start.elapsed());
    }

    times.sort();

    let median = times[times.len() / 2];

    eprintln!(
        "typeAt in std.net.http.server: {:?} (all: {:?})",
        median, times
    );
    assert!(median < LIMIT, "median {:?} exceeds {:?}", median, LIMIT);
}

#[test]
fn the_installed_inko_works_when_its_version_matches() {
    // Skipped (passing) unless `inko` on PATH is the pinned version.
    let Ok(out) = Command::new("inko").arg("--version").output() else {
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

    let root = fixture("members");
    let mut server = Server::start();
    let reply = server.request("initialize", json!({ "project": root }));

    assert_eq!(reply["status"], "fresh", "{}", reply);

    let file = root.join("src/main.inko");
    let text = std::fs::read_to_string(&file).unwrap();
    let reply = server.at("typeAt", &file, &text, "name.size", 5, None);

    assert_eq!(reply["result"]["type"], "Int", "{}", reply);
}

#[test]
fn a_failed_handshake_after_a_successful_one_leaves_the_server_uninitialized() {
    let root = fixture("members");
    let file = root.join("src/main.inko");
    let mut server = Server::initialized(&root);
    let reply = server.request("initialize", json!({ "inko": fake_inko("0.20.0") }));

    assert_eq!(reply["reason"]["code"], "inkoVersionMismatch");

    let reply = server.request(
        "typeAt",
        json!({ "file": file, "position": { "line": 0, "character": 0 } }),
    );

    assert_eq!(reply["reason"]["code"], "notInitialized");

    // And a later successful one works again.
    let reply = server.request(
        "initialize",
        json!({ "inko": fake_inko(upstream_version()) }),
    );

    assert_eq!(reply["status"], "fresh");
}
