//! A minimal LSP client that drives the real `cuttlebone` binary over stdio.
use super::spec::{capable_client, spec};
use cuttlebone::dump::upstream_std;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use url::Url;

/// How long to wait for any message before failing.
pub const TIMEOUT: Duration = Duration::from_secs(30);

pub struct Client {
    pub child: Child,
    stdin: Option<ChildStdin>,
    messages: Receiver<Value>,
    /// Notifications received while waiting for responses.
    pub notifications: Vec<Value>,
    /// Everything the server wrote to stderr so far.
    pub stderr: Arc<Mutex<String>>,
    next_id: i64,
    /// The method of each request sent, for checking its response.
    methods: HashMap<i64, String>,
    /// The capabilities the client sent in the `initialize` that
    /// succeeded, and those of each `initialize` sent, by id.
    capabilities: Value,
    offered: HashMap<i64, Value>,
}

/// How to start the server.
#[derive(Default)]
pub struct Launch<'a> {
    pub env: Vec<(&'a str, String)>,
    /// Variables the server mustn't inherit from the tests' environment.
    pub unset: Vec<&'a str>,
    pub cwd: Option<PathBuf>,
}

impl Client {
    pub fn start() -> Client {
        Client::launch(Launch::default())
    }

    pub fn launch(launch: Launch) -> Client {
        Client::launch_program(&super::server_bin(), launch)
    }

    /// Starts `program` instead of the server, for tests of this client.
    pub fn launch_program(program: &Path, launch: Launch) -> Client {
        let mut command = Command::new(program);

        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        for (k, v) in &launch.env {
            command.env(k, v);
        }

        for k in &launch.unset {
            command.env_remove(k);
        }

        if let Some(dir) = &launch.cwd {
            command.current_dir(dir);
        }

        let mut child = command.spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let (tx, messages) = channel();
        let log = Arc::new(Mutex::new(String::new()));
        let sink = log.clone();

        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);

            while let Some(message) = read_message(&mut reader) {
                if tx.send(message).is_err() {
                    break;
                }
            }
        });

        std::thread::spawn(move || {
            let mut buf = [0; 4096];

            while let Ok(n) = stderr.read(&mut buf) {
                if n == 0 {
                    break;
                }

                sink.lock()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(&buf[..n]));
            }
        });

        Client {
            child,
            stdin: Some(stdin),
            messages,
            notifications: Vec::new(),
            stderr: log,
            next_id: 1,
            methods: HashMap::new(),
            capabilities: json!({}),
            offered: HashMap::new(),
        }
    }

    /// Starts a server and initializes it with the fake `inko`, `root` as
    /// the workspace, and `options` merged into the initialization options.
    pub fn initialized(root: &Path, options: Value) -> Client {
        Client::initialized_with(Launch::default(), root, options, json!({}))
    }

    pub fn initialized_with(
        launch: Launch,
        root: &Path,
        options: Value,
        capabilities: Value,
    ) -> Client {
        let mut client = Client::launch(launch);
        let response = client.initialize(root, options, capabilities);

        assert!(response.get("result").is_some(), "{}", response);
        client.notify("initialized", json!({}));
        client
    }

    /// Sends `initialize` with the capabilities of a client that supports
    /// everything Cuttlebone can use (`spec::capable_client`), with
    /// `capabilities` merged over them, and returns the response.
    pub fn initialize(&mut self, root: &Path, options: Value, capabilities: Value) -> Value {
        let mut all = capable_client();

        merge_deep(&mut all, capabilities);
        self.initialize_exactly(root, options, all)
    }

    /// Sends `initialize` with exactly `capabilities`.
    pub fn initialize_exactly(
        &mut self,
        root: &Path,
        options: Value,
        capabilities: Value,
    ) -> Value {
        let mut init = json!({ "inko": fake_inko(cuttlebone::check::upstream_version()) });

        merge(&mut init, options);
        self.request(
            "initialize",
            json!({
                "processId": null,
                "rootUri": uri(root),
                "capabilities": capabilities,
                "initializationOptions": init,
            }),
        )
    }

    pub fn send(&mut self, message: Value) {
        if let (Some(id), Some(method)) = (message["id"].as_i64(), message["method"].as_str()) {
            self.methods.insert(id, method.to_string());

            if method == "initialize" {
                self.offered
                    .insert(id, message["params"]["capabilities"].clone());
            }
        }

        let body = message.to_string();
        let stdin = self.stdin.as_mut().unwrap();

        write!(stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
        stdin.flush().unwrap();
    }

    pub fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// Sends a request without waiting, and returns its id.
    pub fn send_request(&mut self, method: &str, params: Value) -> i64 {
        let id = self.next_id;

        self.next_id += 1;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        id
    }

    /// Sends a request, and returns its response.
    pub fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.send_request(method, params);

        self.response(id)
    }

    /// Waits for the response to request `id`, keeping notifications (and
    /// requests the server sends, which `spec` checks) that arrive first.
    pub fn response(&mut self, id: i64) -> Value {
        loop {
            let message = self
                .receive(TIMEOUT)
                .unwrap_or_else(|e| panic!("no response to {}: {}", id, e));

            if message.get("id") == Some(&json!(id)) && message.get("method").is_none() {
                return message;
            }

            assert!(
                message.get("id").is_none() || message.get("method").is_some(),
                "an unexpected response: {}",
                message
            );
            self.notifications.push(message);
        }
    }

    /// The next message from the server, checked against the spec
    /// (`spec::Spec::check`): a test fails on any message that doesn't
    /// conform.
    pub fn receive(
        &mut self,
        timeout: Duration,
    ) -> Result<Value, std::sync::mpsc::RecvTimeoutError> {
        let message = self.messages.recv_timeout(timeout)?;
        let method = message["id"]
            .as_i64()
            .and_then(|id| self.methods.get(&id))
            .map(String::as_str);

        if method == Some("initialize")
            && message.get("result").is_some()
            && let Some(offered) = self.offered.get(&message["id"].as_i64().unwrap())
        {
            self.capabilities = offered.clone();
        }

        if let Err(e) = spec().check(&message, method, &self.capabilities) {
            panic!(
                "a message that doesn't conform to LSP 3.18: {}\n{}",
                e, message
            );
        }

        Ok(message)
    }

    /// Waits for a notification that `matches`, among those received so
    /// far or arriving next, and takes it out of `notifications`.
    pub fn wait_for(&mut self, matches: impl Fn(&Value) -> bool) -> Value {
        loop {
            if let Some(i) = self.notifications.iter().position(&matches) {
                return self.notifications.remove(i);
            }

            let message = self
                .receive(TIMEOUT)
                .unwrap_or_else(|e| panic!("no matching notification: {}", e));

            assert!(
                message.get("method").is_some(),
                "an unexpected response: {}",
                message
            );
            self.notifications.push(message);
        }
    }

    /// Waits for the next `cuttlebone/status` notification about a
    /// diagnostics check, and takes it.
    pub fn diagnostics_status(&mut self) -> Value {
        self.wait_for(|n| {
            n["method"] == "cuttlebone/status"
                && n["params"]["method"] == "textDocument/publishDiagnostics"
        })["params"]
            .clone()
    }

    /// The `cuttlebone/status` notification for request `id`.
    pub fn status(&self, id: i64) -> Option<&Value> {
        self.notifications
            .iter()
            .find(|n| n["method"] == "cuttlebone/status" && n["params"]["id"] == id)
            .map(|n| &n["params"])
    }

    pub fn open(&mut self, path: &Path, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": uri(path), "languageId": "inko", "version": 1, "text": text,
            }}),
        );
    }

    pub fn change(&mut self, path: &Path, version: i32, changes: Value) {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": uri(path), "version": version },
                "contentChanges": changes,
            }),
        );
    }

    pub fn close(&mut self, path: &Path) {
        self.notify(
            "textDocument/didClose",
            json!({ "textDocument": { "uri": uri(path) } }),
        );
    }

    /// Sends a hover or completion request at a position, without waiting.
    pub fn send_at(&mut self, method: &str, path: &Path, line: u32, character: u32) -> i64 {
        self.send_request(
            method,
            json!({
                "textDocument": { "uri": uri(path) },
                "position": { "line": line, "character": character },
            }),
        )
    }

    pub fn hover(&mut self, path: &Path, line: u32, character: u32) -> (i64, Value) {
        let id = self.send_at("textDocument/hover", path, line, character);

        (id, self.response(id))
    }

    pub fn complete(&mut self, path: &Path, line: u32, character: u32) -> (i64, Value) {
        let id = self.send_at("textDocument/completion", path, line, character);

        (id, self.response(id))
    }

    /// The worker processes the server is running now.
    pub fn workers(&self) -> Vec<u32> {
        let out = Command::new("pgrep")
            .args(["-P", &self.child.id().to_string()])
            .output()
            .unwrap();

        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .collect()
    }

    /// Closes stdin.
    pub fn close_input(&mut self) {
        self.stdin = None;
    }

    /// Waits for the server to exit, and returns its exit code.
    pub fn wait(&mut self) -> Option<i32> {
        let start = Instant::now();

        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.code();
            }

            assert!(start.elapsed() < TIMEOUT, "the server didn't exit");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_message(reader: &mut impl BufRead) -> Option<Value> {
    let mut length = None;

    loop {
        let mut line = String::new();

        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }

        let line = line.trim_end();

        if line.is_empty() {
            break;
        }

        if let Some(v) = line.strip_prefix("Content-Length: ") {
            length = v.parse().ok();
        }
    }

    let mut body = vec![0; length?];

    reader.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

/// `from` merged into `into`, recursively for objects.
pub fn merge_deep(into: &mut Value, from: Value) {
    match (into, from) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in b {
                match a.get_mut(&k) {
                    Some(existing) => merge_deep(existing, v),
                    None => {
                        a.insert(k, v);
                    }
                }
            }
        }
        (into, from) => *into = from,
    }
}

fn merge(into: &mut Value, from: Value) {
    if let (Some(a), Value::Object(b)) = (into.as_object_mut(), from) {
        a.extend(b);
    }
}

pub fn uri(path: &Path) -> String {
    Url::from_file_path(path).unwrap().to_string()
}

/// A fake `inko` that reports `version`, and the pinned std for
/// `inko print std`, so tests don't depend on what's installed.
pub fn fake_inko(version: &str) -> PathBuf {
    fake_inko_with_std(version, &upstream_std(), &format!("inko-{}", version))
}

/// A fake `inko` in a directory named `name`, that reports `version` and
/// `std` as its standard library.
pub fn fake_inko_with_std(version: &str, std: &Path, name: &str) -> PathBuf {
    let dir = super::tmp().join(name);
    let path = dir.join("inko");
    let script = format!(
        "#!/bin/sh\ncase \"$1\" in\n  --version) echo 'inko {}' ;;\n  print) echo '{}' ;;\n  *) exit 1 ;;\nesac\n",
        version,
        std.display()
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

/// The 0-based line and the offset in `encoding`'s units of the first
/// occurrence of `needle` in `text`, plus `offset` characters (not units).
pub fn at(text: &str, needle: &str, offset: usize, utf8: bool) -> (u32, u32) {
    let byte = text
        .find(needle)
        .unwrap_or_else(|| panic!("{:?} isn't in the text", needle));
    let byte = byte
        + text[byte..]
            .chars()
            .take(offset)
            .map(char::len_utf8)
            .sum::<usize>();
    let before = &text[..byte];
    let line = before.matches('\n').count() as u32;
    let start = before.rfind('\n').map_or(0, |i| i + 1);
    let column = &before[start..];
    let units = if utf8 {
        column.len()
    } else {
        column.encode_utf16().count()
    };

    (line, units as u32)
}
