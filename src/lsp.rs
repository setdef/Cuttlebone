//! The language server: standard LSP over stdio, documented in docs/lsp.md.
//!
//! One thread runs the loop below. It keeps the open documents, queues
//! hover and completion requests, and runs each query in a worker process
//! (see `worker`) with a timeout, one at a time. Edits and
//! `$/cancelRequest` cancel queries they make pointless, and a query waits
//! until its document has been still for a moment (`debounce`) before it
//! starts.
use crate::position::{Encoding, Position};
use crate::query::{self, Answer, MemberKind, MembersAt, Session, TypeAt, Unavailable};
use crate::worker::{Check, DEADLINE_MARGIN, Finished, Job, Outcome, Query};
use crate::{VERSION, upstream_version};
use crossbeam_channel::{Receiver, Sender, select};
use lsp_server::{
    Connection, ErrorCode, Message, Notification, ProtocolError, Request, RequestId, Response,
    ResponseError,
};
use lsp_types as lsp;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use url::Url;

/// The custom notification that reports each answer's status, if the client
/// asks for it (docs/lsp.md, "Status").
pub const STATUS: &str = "cuttlebone/status";

/// How long `inko --version` and `inko print std` may take.
const INKO_TIMEOUT: Duration = Duration::from_secs(10);

/// Settings a client may pass as `initializationOptions`.
#[derive(Clone, Debug)]
pub struct Options {
    /// The `inko` executable. Defaults to `inko` on `PATH`.
    pub inko: String,
    /// How long one check may run before it's killed.
    pub timeout: Duration,
    /// How long a document must go without edits before a query on it
    /// starts.
    pub debounce: Duration,
    /// Whether to send `cuttlebone/status` notifications.
    pub status: bool,
}

impl Options {
    fn parse(value: Option<&Value>) -> Options {
        let get = |key: &str| value.and_then(|v| v.get(key));
        let millis = |key: &str, default: u64| {
            Duration::from_millis(get(key).and_then(Value::as_u64).unwrap_or(default))
        };

        Options {
            inko: get("inko")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or("inko")
                .to_string(),
            timeout: millis("checkTimeoutMs", 5000),
            debounce: millis("debounceMs", 50),
            status: get("statusNotifications")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }
    }
}

/// Runs a session on `connection` until the client sends `exit`. Returns
/// whether `shutdown` came first, which the exit status reports.
pub fn serve(connection: Connection) -> Result<bool, ProtocolError> {
    let Some(mut server) = handshake(&connection)? else {
        return Ok(false);
    };

    server.run(&connection)
}

/// Answers `initialize` requests until one succeeds, then waits for
/// `initialized`. Returns `None` if the client exits first.
fn handshake(connection: &Connection) -> Result<Option<Server>, ProtocolError> {
    loop {
        let (id, params) = match connection.initialize_start() {
            Ok(v) => v,
            // `initialize_start` refuses an `exit` before `initialize`.
            Err(e) if format!("{:?}", e).contains("\"exit\"") => return Ok(None),
            Err(e) => return Err(e),
        };
        let params: lsp::InitializeParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                let error = ResponseError {
                    code: ErrorCode::InvalidParams as i32,
                    message: format!("invalid initialize params: {}", e),
                    data: None,
                };

                send_error(connection, id, error);
                continue;
            }
        };

        match Server::new(&params) {
            Ok(server) => {
                let result = server.capabilities();

                connection.initialize_finish(id, result)?;
                return Ok(Some(server));
            }
            Err(refusal) => send_error(connection, id, refusal),
        }
    }
}

fn send_error(connection: &Connection, id: RequestId, error: ResponseError) {
    let _ = connection.sender.send(Message::Response(Response {
        id,
        response_result: Err(error),
    }));
}

/// An open document.
struct Document {
    /// `None` for a document that isn't a file, such as an unsaved new one.
    path: Option<PathBuf>,
    text: String,
    /// When it last changed, for the debounce.
    changed: Option<Instant>,
}

/// A hover or completion request waiting to run.
struct Pending {
    id: RequestId,
    method: &'static str,
    query: Query,
    uri: Url,
    position: Position,
    /// When it may start.
    ready: Instant,
}

struct Server {
    options: Options,
    session: Session,
    encoding: Encoding,
    markdown: bool,
    documents: HashMap<Url, Document>,
    queue: VecDeque<Pending>,
    running: Option<(Pending, Check)>,
    checks: u64,
    finished: (Sender<Finished>, Receiver<Finished>),
    shut_down: bool,
}

impl Server {
    /// Checks the toolchain and sets up a session, or explains why not.
    fn new(params: &lsp::InitializeParams) -> Result<Server, ResponseError> {
        let options = Options::parse(params.initialization_options.as_ref());
        let std = find_std(&options.inko)?;
        let encoding = choose_encoding(params);
        let markdown = params
            .capabilities
            .text_document
            .as_ref()
            .and_then(|t| t.hover.as_ref())
            .and_then(|h| h.content_format.as_ref())
            .is_none_or(|formats| formats.contains(&lsp::MarkupKind::Markdown));

        Ok(Server {
            session: Session {
                std,
                projects: project_roots(params),
            },
            options,
            encoding,
            markdown,
            documents: HashMap::new(),
            queue: VecDeque::new(),
            running: None,
            checks: 0,
            finished: crossbeam_channel::unbounded(),
            shut_down: false,
        })
    }

    fn capabilities(&self) -> Value {
        let encoding = match self.encoding {
            Encoding::Utf8 => lsp::PositionEncodingKind::UTF8,
            Encoding::Utf16 => lsp::PositionEncodingKind::UTF16,
        };
        let result = lsp::InitializeResult {
            capabilities: lsp::ServerCapabilities {
                position_encoding: Some(encoding),
                text_document_sync: Some(lsp::TextDocumentSyncCapability::Options(
                    lsp::TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(lsp::TextDocumentSyncKind::INCREMENTAL),
                        save: Some(lsp::TextDocumentSyncSaveOptions::SaveOptions(
                            lsp::SaveOptions {
                                include_text: Some(true),
                            },
                        )),
                        ..Default::default()
                    },
                )),
                hover_provider: Some(lsp::HoverProviderCapability::Simple(true)),
                completion_provider: Some(lsp::CompletionOptions {
                    trigger_characters: Some(vec![".".to_string()]),
                    resolve_provider: Some(false),
                    ..Default::default()
                }),
                ..Default::default()
            },
            server_info: Some(lsp::ServerInfo {
                name: "cuttlebone".to_string(),
                version: Some(VERSION.to_string()),
            }),
        };
        let mut value = serde_json::to_value(result).unwrap();

        // Not a capability: what this server checks with, for clients that
        // want to show it.
        value["capabilities"]["experimental"] = json!({
            "cuttlebone": {
                "inko": upstream_version(),
                "std": self.session.std,
                "statusNotifications": self.options.status,
            }
        });
        value
    }

    fn run(&mut self, connection: &Connection) -> Result<bool, ProtocolError> {
        let finished = self.finished.1.clone();

        loop {
            self.start_next(connection);

            let deadline = self.deadline();
            let timer = match deadline {
                Some(at) => crossbeam_channel::at(at),
                None => crossbeam_channel::never(),
            };

            select! {
                recv(connection.receiver) -> message => {
                    // The client closed stdin without `exit`.
                    let Ok(message) = message else {
                        self.cancel_all(connection);
                        return Ok(false);
                    };

                    if let Some(shut_down) = self.handle(connection, message) {
                        self.cancel_all(connection);
                        return Ok(shut_down);
                    }
                }
                recv(finished) -> done => {
                    if let Ok(done) = done {
                        self.finish(connection, done);
                    }
                }
                recv(timer) -> _ => self.time_out(connection),
            }
        }
    }

    /// Handles one message. Returns `Some` when the client sent `exit`.
    fn handle(&mut self, connection: &Connection, message: Message) -> Option<bool> {
        match message {
            Message::Request(request) => self.request(connection, request),
            Message::Notification(n) if n.method == "exit" => return Some(self.shut_down),
            Message::Notification(n) => self.notification(connection, n),
            Message::Response(_) => {}
        }

        None
    }

    fn request(&mut self, connection: &Connection, request: Request) {
        if self.shut_down {
            return reply_error(
                connection,
                request.id,
                ErrorCode::InvalidRequest,
                "the server is shutting down",
            );
        }

        let (method, query) = match request.method.as_str() {
            "textDocument/hover" => ("textDocument/hover", Query::Type),
            "textDocument/completion" => ("textDocument/completion", Query::Complete),
            "shutdown" => {
                self.shut_down = true;
                self.cancel_all(connection);
                return reply(connection, request.id, Value::Null);
            }
            "initialize" => {
                return reply_error(
                    connection,
                    request.id,
                    ErrorCode::InvalidRequest,
                    "the server is already initialized",
                );
            }
            other => {
                return reply_error(
                    connection,
                    request.id,
                    ErrorCode::MethodNotFound,
                    &format!("unsupported method {:?}", other),
                );
            }
        };
        let params: lsp::TextDocumentPositionParams =
            match serde_json::from_value(request.params.clone()) {
                Ok(p) => p,
                Err(e) => {
                    return reply_error(
                        connection,
                        request.id,
                        ErrorCode::InvalidParams,
                        &e.to_string(),
                    );
                }
            };
        let uri = params.text_document.uri;
        let ready = self
            .documents
            .get(&uri)
            .and_then(|d| d.changed)
            .map_or_else(Instant::now, |at| {
                (at + self.options.debounce).max(Instant::now())
            });

        self.queue.push_back(Pending {
            id: request.id,
            method,
            query,
            uri,
            position: Position {
                line: params.position.line,
                character: params.position.character,
            },
            ready,
        });
    }

    fn notification(&mut self, connection: &Connection, n: Notification) {
        match n.method.as_str() {
            "textDocument/didOpen" => {
                if let Ok(p) = n.extract::<lsp::DidOpenTextDocumentParams>("textDocument/didOpen") {
                    let uri = p.text_document.uri;

                    self.documents.insert(
                        uri.clone(),
                        Document {
                            path: uri.to_file_path().ok(),
                            text: p.text_document.text,
                            changed: None,
                        },
                    );
                }
            }
            "textDocument/didChange" => {
                if let Ok(p) =
                    n.extract::<lsp::DidChangeTextDocumentParams>("textDocument/didChange")
                {
                    let uri = p.text_document.uri;

                    if let Some(doc) = self.documents.get_mut(&uri) {
                        for change in p.content_changes {
                            apply(&mut doc.text, change, self.encoding);
                        }

                        doc.changed = Some(Instant::now());
                    }

                    self.supersede(connection, &uri);
                }
            }
            "textDocument/didSave" => {
                if let Ok(p) = n.extract::<lsp::DidSaveTextDocumentParams>("textDocument/didSave")
                    && let Some(text) = p.text
                    && let Some(doc) = self.documents.get_mut(&p.text_document.uri)
                {
                    doc.text = text;
                }
            }
            "textDocument/didClose" => {
                if let Ok(p) = n.extract::<lsp::DidCloseTextDocumentParams>("textDocument/didClose")
                {
                    self.documents.remove(&p.text_document.uri);
                }
            }
            "$/cancelRequest" => {
                if let Ok(p) = n.extract::<lsp::CancelParams>("$/cancelRequest") {
                    let id = match p.id {
                        lsp::NumberOrString::Number(n) => RequestId::from(n),
                        lsp::NumberOrString::String(s) => RequestId::from(s),
                    };

                    self.cancel(connection, |p| p.id == id, ErrorCode::RequestCanceled);
                }
            }
            _ => {}
        }
    }

    /// Cancels the queries on a document that just changed: their positions
    /// refer to text that's gone.
    fn supersede(&mut self, connection: &Connection, uri: &Url) {
        self.cancel(connection, |p| &p.uri == uri, ErrorCode::ContentModified);
    }

    /// Cancels the queued and running queries `matches` selects, replying
    /// with `code`.
    fn cancel(
        &mut self,
        connection: &Connection,
        matches: impl Fn(&Pending) -> bool,
        code: ErrorCode,
    ) {
        let message = match code {
            ErrorCode::ContentModified => "the document changed",
            _ => "cancelled",
        };

        if self.running.as_ref().is_some_and(|(p, _)| matches(p)) {
            let (pending, check) = self.running.take().unwrap();

            check.kill();
            reply_error(connection, pending.id, code, message);
        }

        let (cancelled, kept): (Vec<_>, Vec<_>) = self.queue.drain(..).partition(|p| matches(p));

        self.queue = kept.into();

        for pending in cancelled {
            reply_error(connection, pending.id, code, message);
        }
    }

    fn cancel_all(&mut self, connection: &Connection) {
        self.cancel(connection, |_| true, ErrorCode::RequestCanceled);
    }

    /// The next time something must happen: a running check's timeout, or
    /// the end of a queued query's debounce.
    fn deadline(&self) -> Option<Instant> {
        match &self.running {
            Some((_, check)) => Some(check.started + self.options.timeout),
            None => self.queue.iter().map(|p| p.ready).min(),
        }
    }

    /// Starts the first query that's ready, if nothing is running. One
    /// that can't start is answered right away.
    fn start_next(&mut self, connection: &Connection) {
        while self.running.is_none() {
            let now = Instant::now();
            let Some(index) = self.queue.iter().position(|p| p.ready <= now) else {
                return;
            };
            let pending = self.queue.remove(index).unwrap();

            self.checks += 1;

            let job = self.job(&pending);

            match job.and_then(|job| Check::start(self.checks, &job, self.finished.0.clone())) {
                Ok(check) => self.running = Some((pending, check)),
                Err(reason) => self.answer(connection, pending, Outcome::Failed(reason)),
            }
        }
    }

    fn job(&self, pending: &Pending) -> Result<Job, Unavailable> {
        let file = self
            .documents
            .get(&pending.uri)
            .map(|d| d.path.clone())
            .unwrap_or_else(|| pending.uri.to_file_path().ok())
            .ok_or_else(|| {
                Unavailable::new(
                    "notAFile",
                    format!("{} isn't a file, so it can't be type checked", pending.uri),
                )
            })?;

        // Every open document that's a file on disk: the compiler only
        // finds modules that exist on disk, and replaces their text.
        let contents = self
            .documents
            .values()
            .filter_map(|d| Some((d.path.clone()?, d.text.clone())))
            .filter(|(path, _)| path.is_file())
            .collect();

        Ok(Job {
            query: pending.query,
            session: self.session.clone(),
            request: query::Request {
                file,
                position: pending.position,
                encoding: self.encoding,
                contents,
            },
            deadline: Some(self.options.timeout + DEADLINE_MARGIN),
        })
    }

    fn finish(&mut self, connection: &Connection, done: Finished) {
        if !self.running.as_ref().is_some_and(|(_, c)| c.id == done.id) {
            // A check that was killed.
            return;
        }

        let (pending, check) = self.running.take().unwrap();

        if !done.stderr.trim().is_empty() {
            log(&format!(
                "check for {}: {}",
                pending.uri,
                done.stderr.trim_end()
            ));
        }

        let outcome = check.finish(&done);

        self.answer(connection, pending, outcome);
    }

    fn time_out(&mut self, connection: &Connection) {
        let Some((_, check)) = &self.running else {
            return;
        };

        if check.started.elapsed() < self.options.timeout {
            return;
        }

        let (pending, check) = self.running.take().unwrap();
        let elapsed = check.started.elapsed();

        check.kill();
        log(&format!(
            "check for {} timed out after {:?}; killed it",
            pending.uri, elapsed
        ));

        let reason = Unavailable::new(
            "timeout",
            format!(
                "the check took longer than {} ms, so it was stopped",
                self.options.timeout.as_millis()
            ),
        );

        self.answer(connection, pending, Outcome::Failed(reason));
    }

    fn answer(&mut self, connection: &Connection, pending: Pending, outcome: Outcome) {
        let (result, status) = match outcome {
            Outcome::Type(answer) => {
                let status = status_of(&answer);

                (self.hover(answer.result), status)
            }
            Outcome::Members(answer) => {
                let status = status_of(&answer);

                (completion(answer.result), status)
            }
            Outcome::Failed(reason) => (Value::Null, (Err(reason), Duration::ZERO)),
        };

        if self.options.status {
            let (state, elapsed) = status;
            let mut params = json!({
                "id": pending.id,
                "method": pending.method,
                "textDocument": { "uri": pending.uri },
                "status": if state.is_ok() { "fresh" } else { "unavailable" },
                "elapsedMs": (elapsed.as_secs_f64() * 10000.0).round() / 10.0,
            });

            if let Err(reason) = state {
                params["reason"] = json!({ "code": reason.code, "message": reason.message });
            }

            let _ = connection
                .sender
                .send(Message::Notification(Notification::new(
                    STATUS.to_string(),
                    params,
                )));
        }

        reply(connection, pending.id, result);
    }

    fn hover(&self, result: Result<Option<TypeAt>, Unavailable>) -> Value {
        let Ok(Some(t)) = result else {
            return Value::Null;
        };
        let contents = if self.markdown {
            lsp::MarkupContent {
                kind: lsp::MarkupKind::Markdown,
                value: format!("```inko\n{}\n```", t.typ),
            }
        } else {
            lsp::MarkupContent {
                kind: lsp::MarkupKind::PlainText,
                value: t.typ,
            }
        };

        serde_json::to_value(lsp::Hover {
            contents: lsp::HoverContents::Markup(contents),
            range: Some(range(t.range)),
        })
        .unwrap()
    }
}

fn status_of<T>(answer: &Answer<T>) -> (Result<(), Unavailable>, Duration) {
    (
        answer.result.as_ref().map(|_| ()).map_err(Clone::clone),
        answer.elapsed,
    )
}

fn completion(result: Result<Option<MembersAt>, Unavailable>) -> Value {
    let Ok(Some(members)) = result else {
        return Value::Null;
    };
    let items = members
        .members
        .into_iter()
        .map(|m| lsp::CompletionItem {
            kind: Some(match m.kind {
                MemberKind::Method => lsp::CompletionItemKind::METHOD,
                MemberKind::Field => lsp::CompletionItemKind::FIELD,
                MemberKind::Constant => lsp::CompletionItemKind::CONSTANT,
                MemberKind::Type => lsp::CompletionItemKind::CLASS,
            }),
            detail: Some(m.signature),
            label_details: Some(lsp::CompletionItemLabelDetails {
                detail: None,
                description: Some(m.module),
            }),
            label: m.name,
            ..Default::default()
        })
        .collect();

    serde_json::to_value(lsp::CompletionList {
        is_incomplete: false,
        items,
    })
    .unwrap()
}

fn range(r: query::Range) -> lsp::Range {
    lsp::Range {
        start: lsp::Position {
            line: r.start.line,
            character: r.start.character,
        },
        end: lsp::Position {
            line: r.end.line,
            character: r.end.character,
        },
    }
}

/// Applies one change from `didChange` to a document's text.
fn apply(text: &mut String, change: lsp::TextDocumentContentChangeEvent, encoding: Encoding) {
    let Some(r) = change.range else {
        *text = change.text;
        return;
    };
    let offset = |p: lsp::Position| {
        query::byte_offset(
            text,
            Position {
                line: p.line,
                character: p.character,
            },
            encoding,
        )
        .unwrap_or(text.len())
    };
    let start = offset(r.start);
    let end = offset(r.end).max(start);

    text.replace_range(start..end, &change.text);
}

/// UTF-8 if the client lists it before UTF-16, otherwise UTF-16, which every
/// client supports.
fn choose_encoding(params: &lsp::InitializeParams) -> Encoding {
    let offered = params
        .capabilities
        .general
        .as_ref()
        .and_then(|g| g.position_encodings.as_ref());

    offered
        .into_iter()
        .flatten()
        .find_map(|kind| match kind.as_str() {
            "utf-8" => Some(Encoding::Utf8),
            "utf-16" => Some(Encoding::Utf16),
            _ => None,
        })
        .unwrap_or(Encoding::Utf16)
}

fn project_roots(params: &lsp::InitializeParams) -> Vec<PathBuf> {
    #[allow(deprecated)]
    let uris: Vec<Url> = match (
        &params.workspace_folders,
        &params.root_uri,
        &params.root_path,
    ) {
        (Some(folders), _, _) if !folders.is_empty() => {
            folders.iter().map(|f| f.uri.clone()).collect()
        }
        (_, Some(uri), _) => vec![uri.clone()],
        (_, _, Some(path)) => Url::from_file_path(path).into_iter().collect(),
        _ => Vec::new(),
    };

    uris.iter()
        .filter_map(|u| u.to_file_path().ok())
        .map(|p| p.canonicalize().unwrap_or(p))
        .collect()
}

/// Runs `inko --version` and `inko print std`, and refuses an Inko other
/// than the one this build links.
fn find_std(inko: &str) -> Result<PathBuf, ResponseError> {
    let supported = upstream_version();
    let refuse = |code: &str, message: String, installed: Option<&str>| ResponseError {
        code: ErrorCode::RequestFailed as i32,
        message,
        data: Some(json!({
            "retry": false,
            "code": code,
            "cuttlebone": VERSION,
            "supported": supported,
            "installed": installed,
        })),
    };
    let installed = inko_output(inko, &["--version"])
        .map_err(|e| refuse("inkoNotFound", e, None))?
        .trim()
        .trim_start_matches("inko")
        .trim()
        .to_string();

    if installed != supported {
        return Err(refuse(
            "inkoVersionMismatch",
            format!(
                "Cuttlebone {} works with Inko {} only, but {} is Inko {}. \
                 Install Inko {}, or use a build of Cuttlebone for Inko {}.",
                VERSION, supported, inko, installed, supported, installed
            ),
            Some(&installed),
        ));
    }

    let std = inko_output(inko, &["print", "std"])
        .map(|out| PathBuf::from(out.trim()))
        .map_err(|e| refuse("inkoNotFound", e, Some(&installed)))?;

    if !std.is_dir() {
        return Err(refuse(
            "inkoNotFound",
            format!(
                "{} prints {} as its standard library, which isn't a directory",
                inko,
                std.display()
            ),
            Some(&installed),
        ));
    }

    Ok(std)
}

/// Runs `inko` with a timeout, and returns its stdout.
fn inko_output(inko: &str, args: &[&str]) -> Result<String, String> {
    let command = format!("`{} {}`", inko, args.join(" "));
    let mut child = Command::new(inko)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't run {}: {}", command, e))?;
    let start = Instant::now();

    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < INKO_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(5))
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{} didn't finish within {} s",
                    command,
                    INKO_TIMEOUT.as_secs()
                ));
            }
            Err(e) => return Err(format!("couldn't run {}: {}", command, e)),
        }
    }

    let out = child
        .wait_with_output()
        .map_err(|e| format!("couldn't run {}: {}", command, e))?;

    if !out.status.success() {
        return Err(format!(
            "{} failed: {}",
            command,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn reply(connection: &Connection, id: RequestId, result: Value) {
    let _ = connection.sender.send(Message::Response(Response {
        id,
        response_result: Ok(result),
    }));
}

fn reply_error(connection: &Connection, id: RequestId, code: ErrorCode, message: &str) {
    send_error(
        connection,
        id,
        ResponseError {
            code: code as i32,
            message: message.to_string(),
            data: None,
        },
    );
}

#[allow(clippy::print_stderr)]
fn log(message: &str) {
    eprintln!("cuttlebone: {}", message);
}
