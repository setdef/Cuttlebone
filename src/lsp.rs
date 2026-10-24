//! The language server: standard LSP over stdio, documented in docs/lsp.md.
//!
//! One thread runs the loop below. It keeps the open documents, queues
//! queries (hover, completion, definition, type definition, references,
//! document highlight, signature help and inlay hints),
//! and runs each query in a worker process
//! (see `worker`) with a timeout, one at a time. Edits and
//! `$/cancelRequest` cancel queries they make pointless, and a query waits
//! until its document has been still for a moment (`debounce`) before it
//! starts.
//!
//! Diagnostics have a slot of their own: a full check of a whole project
//! (`query::diagnostics`) in a second worker, after a longer debounce, so a
//! query never waits for one. See `Diagnoser`, and docs/lsp.md,
//! "Diagnostics".
use crate::check::{VERSION, upstream_version};
use crate::edits::Snapshots;
use crate::position::{Encoding, Position, byte_offset};
use crate::query::{
    self, Access, Answer, Diagnostics, HintKind, MemberKind, MembersAt, Place, Session, Source,
    TypeAt, Unavailable,
};
use crate::worker::{Check, DEADLINE_MARGIN, Finished, Job, Outcome, Query, Worker};
use crossbeam_channel::{Receiver, Sender, select};
use lsp_server::{
    Connection, ErrorCode, Message, Notification, ProtocolError, Request, RequestId, Response,
    ResponseError,
};
use lsp_types as lsp;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
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
    /// Whether to publish diagnostics.
    pub diagnostics: bool,
    /// How long a project must go without edits before its diagnostics
    /// are checked.
    pub diagnostics_debounce: Duration,
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
            diagnostics: get("diagnostics").and_then(Value::as_bool).unwrap_or(true),
            diagnostics_debounce: millis("diagnosticsDebounceMs", 300),
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
        let raw = params.clone();
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

        match Server::new(&params, &raw["capabilities"]) {
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

/// A query waiting to run.
struct Pending {
    id: RequestId,
    method: &'static str,
    query: Query,
    uri: Url,
    position: Position,
    /// When it may start.
    ready: Instant,
}

/// Diagnostics, checked and published per target: a project directory, or
/// a file outside any project's `src/` and `test/`
/// (`query::diagnostics_target`). One check runs at a time, in a worker of
/// its own, so queries never wait for it, and it starts only while no query
/// is queued or running.
#[derive(Default)]
struct Diagnoser {
    /// Targets that need a check, and when each may start.
    pending: HashMap<PathBuf, Instant>,
    running: Option<(PathBuf, Check)>,
    /// Every target scheduled so far.
    known: HashSet<PathBuf>,
    /// The files each target last published diagnostics in.
    published: HashMap<PathBuf, HashSet<Url>>,
}

/// What the client's capabilities say it supports, where the spec makes
/// what a server may send depend on it. Read from the capabilities as the
/// client sent them, since lsp-types (for 3.17) drops what 3.18 added.
#[derive(Clone, Copy, Debug, Default)]
struct Support {
    /// Hovers in Markdown: the first format the client lists that
    /// Cuttlebone has, in its order of preference, is Markdown. Plain text
    /// if it lists neither.
    markdown: bool,
    /// `CompletionItem.labelDetails`.
    label_details: bool,
    /// `CompletionItemKind.Constant`, which isn't one of the kinds every
    /// client has (Text to Reference).
    constant_kind: bool,
    /// Parameter labels as offsets into the signature's label.
    label_offsets: bool,
    /// `SignatureInformation.activeParameter`.
    signature_active_parameter: bool,
    /// `activeParameter: null` for no active parameter (3.18).
    no_active_parameter: bool,
    /// The `Unnecessary` diagnostic tag.
    unnecessary_tag: bool,
}

impl Support {
    fn of(capabilities: &Value) -> Support {
        let at = |path: &str| capabilities.pointer(path).unwrap_or(&Value::Null);
        let yes = |path: &str| at(path) == &Value::Bool(true);
        let lists = |path: &str, value: Value| {
            at(path)
                .as_array()
                .is_some_and(|values| values.contains(&value))
        };
        let signature = "/textDocument/signatureHelp/signatureInformation";

        Support {
            markdown: at("/textDocument/hover/contentFormat")
                .as_array()
                .into_iter()
                .flatten()
                .find(|f| *f == "markdown" || *f == "plaintext")
                .is_some_and(|f| f == "markdown"),
            label_details: yes("/textDocument/completion/completionItem/labelDetailsSupport"),
            constant_kind: lists(
                "/textDocument/completion/completionItemKind/valueSet",
                json!(lsp::CompletionItemKind::CONSTANT),
            ),
            label_offsets: yes(&format!(
                "{}/parameterInformation/labelOffsetSupport",
                signature
            )),
            signature_active_parameter: yes(&format!("{}/activeParameterSupport", signature)),
            no_active_parameter: yes("/textDocument/signatureHelp/noActiveParameterSupport"),
            unnecessary_tag: lists(
                "/textDocument/publishDiagnostics/tagSupport/valueSet",
                json!(lsp::DiagnosticTag::UNNECESSARY),
            ),
        }
    }
}

struct Server {
    options: Options,
    session: Session,
    encoding: Encoding,
    support: Support,
    documents: HashMap<Url, Document>,
    queue: VecDeque<Pending>,
    running: Option<(Pending, Check)>,
    /// The queries' worker while it waits for a job, with the checks it
    /// kept (`query::Cache`).
    idle: Option<Worker>,
    diagnoser: Diagnoser,
    /// The documents of each file's last check that typed method bodies,
    /// and the edits since, for answers from older results.
    snapshots: Snapshots,
    checks: u64,
    finished: (Sender<Finished>, Receiver<Finished>),
    shut_down: bool,
}

impl Server {
    /// Checks the toolchain and sets up a session, or explains why not.
    /// `capabilities` are the client's, as it sent them.
    fn new(params: &lsp::InitializeParams, capabilities: &Value) -> Result<Server, ResponseError> {
        let options = Options::parse(params.initialization_options.as_ref());
        let std = find_std(&options.inko)?;
        let encoding = choose_encoding(params);

        Ok(Server {
            session: Session {
                std,
                projects: project_roots(params),
            },
            options,
            encoding,
            support: Support::of(capabilities),
            documents: HashMap::new(),
            queue: VecDeque::new(),
            running: None,
            idle: None,
            diagnoser: Diagnoser::default(),
            snapshots: Snapshots::default(),
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
                definition_provider: Some(lsp::OneOf::Left(true)),
                type_definition_provider: Some(lsp::TypeDefinitionProviderCapability::Simple(true)),
                references_provider: Some(lsp::OneOf::Left(true)),
                document_highlight_provider: Some(lsp::OneOf::Left(true)),
                inlay_hint_provider: Some(lsp::OneOf::Left(true)),
                signature_help_provider: Some(lsp::SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
                    retrigger_characters: None,
                    work_done_progress_options: Default::default(),
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
                "diagnostics": self.options.diagnostics,
            }
        });
        value
    }

    fn run(&mut self, connection: &Connection) -> Result<bool, ProtocolError> {
        let finished = self.finished.1.clone();

        // A workspace folder that's a project gets its diagnostics without
        // any document open.
        for root in self.session.projects.clone() {
            if root.join("src").is_dir() || root.join("test").is_dir() {
                self.schedule(&root, Instant::now(), false);
            }
        }

        loop {
            self.start_next(connection);
            self.start_diagnostics(connection);

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
            "textDocument/definition" => ("textDocument/definition", Query::Definition),
            "textDocument/typeDefinition" => ("textDocument/typeDefinition", Query::TypeDefinition),
            "textDocument/signatureHelp" => ("textDocument/signatureHelp", Query::SignatureHelp),
            "textDocument/inlayHint" => (
                "textDocument/inlayHint",
                // The range's end. Its start is the position below.
                Query::InlayHints {
                    end: Position {
                        line: json_u32(&request.params, "/range/end/line"),
                        character: json_u32(&request.params, "/range/end/character"),
                    },
                },
            ),
            "textDocument/documentHighlight" => {
                ("textDocument/documentHighlight", Query::Highlight)
            }
            "textDocument/references" => (
                "textDocument/references",
                Query::References {
                    // ReferenceParams requires the context, but a client
                    // that leaves it out gets the references alone.
                    include_declaration: request
                        .params
                        .pointer("/context/includeDeclaration")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                },
            ),
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
        let mut params = request.params.clone();

        // An inlay hint request has a range instead of a position.
        if let Some(start) = params.pointer("/range/start").cloned() {
            params["position"] = start;
        }

        let params: lsp::TextDocumentPositionParams = match serde_json::from_value(params) {
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
                    let path = uri.to_file_path().ok();
                    let disk = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok());
                    let unsaved = path.is_some() && disk.as_ref() != Some(&p.text_document.text);

                    if let Some(path) = &path {
                        self.snapshots
                            .opened(path, disk.as_deref(), &p.text_document.text);
                    }

                    self.documents.insert(
                        uri.clone(),
                        Document {
                            path: path.clone(),
                            text: p.text_document.text,
                            changed: None,
                        },
                    );

                    if let Some(path) = path {
                        self.edited(connection, &path, unsaved, false);
                    }
                }
            }
            "textDocument/didChange" => {
                if let Ok(p) =
                    n.extract::<lsp::DidChangeTextDocumentParams>("textDocument/didChange")
                {
                    let uri = p.text_document.uri;

                    if let Some(doc) = self.documents.get_mut(&uri) {
                        for change in p.content_changes {
                            let before = doc.text.clone();
                            let (start, end, len) = apply(&mut doc.text, change, self.encoding);

                            if let Some(path) = &doc.path {
                                self.snapshots.edited(path, &before, start, end, len);
                            }
                        }

                        doc.changed = Some(Instant::now());
                    }

                    self.supersede(connection, &uri);

                    if let Ok(path) = uri.to_file_path() {
                        self.edited(connection, &path, true, true);
                    }
                }
            }
            "textDocument/didSave" => {
                if let Ok(p) = n.extract::<lsp::DidSaveTextDocumentParams>("textDocument/didSave")
                    && let Some(text) = p.text
                    && let Some(doc) = self.documents.get_mut(&p.text_document.uri)
                    && doc.text != text
                {
                    if let Some(path) = &doc.path {
                        self.snapshots.replaced(path, &doc.text, &text);
                    }

                    doc.text = text;

                    if let Some(path) = doc.path.clone() {
                        self.edited(connection, &path, true, true);
                    }
                }
            }
            "textDocument/didClose" => {
                if let Ok(p) = n.extract::<lsp::DidCloseTextDocumentParams>("textDocument/didClose")
                    && let Some(doc) = self.documents.remove(&p.text_document.uri)
                    && let Some(path) = doc.path
                {
                    // The file's text on disk replaces the document's.
                    let disk = std::fs::read_to_string(&path).ok();
                    let unsaved = disk.as_ref() != Some(&doc.text);

                    if let Some(disk) = &disk {
                        self.snapshots.replaced(&path, &doc.text, disk);
                    }

                    self.edited(connection, &path, unsaved, true);
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

        if let Some((pending, check)) = self.running.take_if(|(p, _)| matches(p)) {
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

        if let Some((_, check)) = self.diagnoser.running.take() {
            check.kill();
        }

        if let Some(worker) = self.idle.take() {
            worker.kill();
        }

        self.diagnoser.pending.clear();
    }

    /// Schedules diagnostics for the target of a file that was opened,
    /// changed, saved or closed. `changed` says whether the text the
    /// compiler would read changed. If so, the target waits for the
    /// diagnostics debounce, and a running check of it is superseded:
    /// it's killed, and checks again later. Otherwise a target is only
    /// checked the first time one of its files is opened.
    fn edited(
        &mut self,
        connection: &Connection,
        path: &std::path::Path,
        changed: bool,
        debounce: bool,
    ) {
        let Some(target) = query::diagnostics_target(&self.session, path) else {
            return;
        };

        if !changed && self.diagnoser.known.contains(&target) {
            return;
        }

        let ready = if debounce {
            Instant::now() + self.options.diagnostics_debounce
        } else {
            Instant::now()
        };

        if changed
            && let Some((target, check)) = self.diagnoser.running.take_if(|(t, _)| *t == target)
        {
            check.kill();
            self.diagnostics_status(
                connection,
                &target,
                Err(Unavailable::new(
                    "superseded",
                    "an edit made the check pointless",
                )),
                Duration::ZERO,
            );
        }

        self.schedule(&target, ready, changed);
    }

    /// Queues a diagnostics check of `target` to start at `ready`. A target
    /// already waiting starts at the later of the two times if `later`,
    /// else at the earlier one.
    fn schedule(&mut self, target: &std::path::Path, ready: Instant, later: bool) {
        if !self.options.diagnostics {
            return;
        }

        self.diagnoser.known.insert(target.to_path_buf());
        self.diagnoser
            .pending
            .entry(target.to_path_buf())
            .and_modify(|at| {
                *at = if later {
                    ready.max(*at)
                } else {
                    ready.min(*at)
                }
            })
            .or_insert(ready);
    }

    /// Starts the diagnostics check that's been waiting longest, if its
    /// debounce is over, no diagnostics check is running, and no query is
    /// queued or running: queries come first.
    fn start_diagnostics(&mut self, connection: &Connection) {
        if self.diagnoser.running.is_some() || self.running.is_some() || !self.queue.is_empty() {
            return;
        }

        let now = Instant::now();
        let Some(target) = self
            .diagnoser
            .pending
            .iter()
            .filter(|(_, at)| **at <= now)
            .min_by_key(|(_, at)| **at)
            .map(|(t, _)| t.clone())
        else {
            return;
        };

        self.diagnoser.pending.remove(&target);
        self.checks += 1;

        let job = Job {
            query: Query::Diagnostics,
            session: self.session.clone(),
            request: query::Request {
                file: target.clone(),
                position: Position {
                    line: 0,
                    character: 0,
                },
                encoding: self.encoding,
                contents: self.contents(),
            },
            fallback: None,
            deadline: Some(self.options.timeout + DEADLINE_MARGIN),
        };

        self.snapshots.sent(self.checks, &job.request.contents);

        match Check::start(self.checks, &job, self.finished.0.clone()) {
            Ok(check) => self.diagnoser.running = Some((target, check)),
            Err(reason) => {
                self.diagnostics_status(connection, &target, Err(reason), Duration::ZERO)
            }
        }
    }

    /// Publishes a target's diagnostics, grouped by file, and clears the
    /// files it published in last time that have none now. A check that
    /// didn't finish (it timed out, crashed, or the project can't be
    /// checked) leaves what was published before.
    fn publish(&mut self, connection: &Connection, target: PathBuf, answer: Answer<Diagnostics>) {
        let elapsed = answer.elapsed;
        let diagnostics = match answer.result {
            Ok(Some(d)) => d,
            Ok(None) => Diagnostics {
                diagnostics: Vec::new(),
                incomplete: None,
            },
            Err(reason) => {
                return self.diagnostics_status(connection, &target, Err(reason), elapsed);
            }
        };
        let mut files: BTreeMap<PathBuf, Vec<lsp::Diagnostic>> = BTreeMap::new();

        for d in diagnostics.diagnostics {
            let unnecessary = d.id == "unused-symbol" || d.id == "unreachable";

            files.entry(d.file).or_default().push(lsp::Diagnostic {
                range: range(d.range),
                severity: Some(if d.error {
                    lsp::DiagnosticSeverity::ERROR
                } else {
                    lsp::DiagnosticSeverity::WARNING
                }),
                code: Some(lsp::NumberOrString::String(d.id)),
                source: Some("cuttlebone".to_string()),
                message: d.message,
                tags: (unnecessary && self.support.unnecessary_tag)
                    .then(|| vec![lsp::DiagnosticTag::UNNECESSARY]),
                ..Default::default()
            });
        }

        let mut published = HashSet::new();

        for (file, list) in files {
            if let Ok(uri) = Url::from_file_path(&file) {
                send_diagnostics(connection, &uri, list);
                published.insert(uri);
            }
        }

        let before = self
            .diagnoser
            .published
            .insert(target.clone(), published.clone())
            .unwrap_or_default();

        for uri in before.difference(&published) {
            send_diagnostics(connection, uri, Vec::new());
        }

        let state = match diagnostics.incomplete {
            Some(reason) => Err(reason),
            None => Ok(()),
        };

        self.diagnostics_status(connection, &target, state, elapsed);
    }

    /// The `cuttlebone/status` notification for a diagnostics check.
    fn diagnostics_status(
        &self,
        connection: &Connection,
        target: &std::path::Path,
        state: Result<(), Unavailable>,
        elapsed: Duration,
    ) {
        if let Err(reason) = &state {
            log(&format!(
                "diagnostics for {}: {}: {}",
                target.display(),
                reason.code,
                reason.message
            ));
        }

        if !self.options.status {
            return;
        }

        let mut params = json!({
            "method": "textDocument/publishDiagnostics",
            "target": Url::from_file_path(target).ok(),
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

    /// The next time something must happen: a running check's timeout, or
    /// the end of a queued query's debounce.
    fn deadline(&self) -> Option<Instant> {
        let query = match &self.running {
            Some((_, check)) => Some(check.started + self.options.timeout),
            None => self.queue.iter().map(|p| p.ready).min(),
        };
        let diagnostics = match &self.diagnoser.running {
            Some((_, check)) => Some(check.started + self.options.timeout),
            // Waiting diagnostics can't start before the queries are done,
            // which wake the loop up when they are.
            None if self.running.is_none() && self.queue.is_empty() => {
                self.diagnoser.pending.values().min().copied()
            }
            None => None,
        };

        query.into_iter().chain(diagnostics).min()
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

            if let Ok(job) = &job {
                self.snapshots.sent(self.checks, &job.request.contents);
            }

            match job.and_then(|job| self.send(self.checks, &job)) {
                Ok(check) => self.running = Some((pending, check)),
                Err(reason) => self.answer(connection, pending, Outcome::Failed(reason)),
            }
        }
    }

    /// Sends a query's job to the idle worker, or to a new one if there's
    /// none or it exited.
    fn send(&mut self, id: u64, job: &Job) -> Result<Check, Unavailable> {
        if let Some(worker) = self.idle.take() {
            let stderr = worker.take_stderr();

            if !stderr.trim().is_empty() {
                log(&format!("worker: {}", stderr.trim_end()));
            }

            if let Ok(check) = worker.run(id, job) {
                return Ok(check);
            }
        }

        Worker::start(self.finished.0.clone())?.run(id, job)
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

        Ok(Job {
            query: pending.query,
            session: self.session.clone(),
            fallback: self.snapshots.fallback(
                &file,
                query::diagnostics_target(&self.session, &file).as_deref(),
            ),
            request: query::Request {
                file,
                position: pending.position,
                encoding: self.encoding,
                contents: self.contents(),
            },
            deadline: Some(self.options.timeout + DEADLINE_MARGIN),
        })
    }

    /// Every open document that's a file on disk: the compiler only finds
    /// modules that exist on disk, and replaces their text.
    fn contents(&self) -> HashMap<PathBuf, String> {
        self.documents
            .values()
            .filter_map(|d| Some((d.path.clone()?, d.text.clone())))
            .filter(|(path, _)| path.is_file())
            .collect()
    }

    fn finish(&mut self, connection: &Connection, done: Finished) {
        if let Some((target, check)) = self.diagnoser.running.take_if(|(_, c)| c.id == done.id) {
            if !done.stderr.trim().is_empty() {
                log(&format!(
                    "diagnostics for {}: {}",
                    target.display(),
                    done.stderr.trim_end()
                ));
            }

            let (outcome, _) = check.finish(&done);

            // A check that typed method bodies is the last good one of every
            // open document it checked, and of the target, for documents
            // opened later.
            let checked: Vec<PathBuf> = if outcome.typed() {
                self.documents
                    .values()
                    .filter_map(|d| d.path.clone())
                    .filter(|p| {
                        query::diagnostics_target(&self.session, p).as_ref() == Some(&target)
                    })
                    .chain([target.clone()])
                    .collect()
            } else {
                Vec::new()
            };

            self.snapshots.finished(done.id, &checked);

            return match outcome {
                Outcome::Diagnostics(answer) => self.publish(connection, target, answer),
                Outcome::Failed(reason) => {
                    self.diagnostics_status(connection, &target, Err(reason), Duration::ZERO)
                }
                _ => self.diagnostics_status(
                    connection,
                    &target,
                    Err(Unavailable::new(
                        "internal",
                        "the worker answered another query",
                    )),
                    Duration::ZERO,
                ),
            };
        }

        // A check that was killed isn't running any more.
        let Some((pending, check)) = self.running.take_if(|(_, c)| c.id == done.id) else {
            return;
        };

        if !done.stderr.trim().is_empty() {
            log(&format!(
                "check for {}: {}",
                pending.uri,
                done.stderr.trim_end()
            ));
        }

        let (outcome, worker) = check.finish(&done);

        self.idle = worker;

        let file = self
            .documents
            .get(&pending.uri)
            .and_then(|d| d.path.clone())
            .filter(|_| outcome.typed());

        self.snapshots.finished(done.id, file.as_slice());
        self.answer(connection, pending, outcome);
    }

    fn time_out(&mut self, connection: &Connection) {
        if let Some((target, check)) = self
            .diagnoser
            .running
            .take_if(|(_, c)| c.started.elapsed() >= self.options.timeout)
        {
            check.kill();
            self.diagnostics_status(
                connection,
                &target,
                Err(Unavailable::new(
                    "timeout",
                    format!(
                        "the check took longer than {} ms, so it was stopped",
                        self.options.timeout.as_millis()
                    ),
                )),
                Duration::ZERO,
            );
        }

        let Some((pending, check)) = self
            .running
            .take_if(|(_, c)| c.started.elapsed() >= self.options.timeout)
        else {
            return;
        };
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

                (completion(answer.result, self.support), status)
            }
            Outcome::Place(answer) => {
                let status = status_of(&answer);
                let result = match answer.result {
                    Ok(Some(place)) => location(place),
                    _ => Value::Null,
                };

                (result, status)
            }
            Outcome::Places(answer) => {
                let status = status_of(&answer);
                let result = match answer.result {
                    Ok(Some(places)) => Value::Array(places.into_iter().map(location).collect()),
                    _ => Value::Null,
                };

                (result, status)
            }
            Outcome::Highlights(answer) => {
                let status = status_of(&answer);
                let result = match answer.result {
                    Ok(Some(list)) => serde_json::to_value(
                        list.into_iter()
                            .map(|h| lsp::DocumentHighlight {
                                range: range(h.range),
                                kind: Some(match h.access {
                                    Access::Declaration => lsp::DocumentHighlightKind::TEXT,
                                    Access::Read => lsp::DocumentHighlightKind::READ,
                                    Access::Write => lsp::DocumentHighlightKind::WRITE,
                                }),
                            })
                            .collect::<Vec<_>>(),
                    )
                    .unwrap(),
                    _ => Value::Null,
                };

                (result, status)
            }
            Outcome::Signature(answer) => {
                let status = status_of(&answer);
                let result = match answer.result {
                    Ok(Some(s)) => signature_help(s, self.support, self.encoding),
                    _ => Value::Null,
                };

                (result, status)
            }
            Outcome::Hints(answer) => {
                let status = status_of(&answer);
                let result = match answer.result {
                    Ok(Some(hints)) => serde_json::to_value(
                        hints
                            .into_iter()
                            .map(|h| lsp::InlayHint {
                                position: lsp::Position {
                                    line: h.position.line,
                                    character: h.position.character,
                                },
                                label: lsp::InlayHintLabel::String(h.label),
                                kind: Some(match h.kind {
                                    HintKind::Type => lsp::InlayHintKind::TYPE,
                                    HintKind::Parameter => lsp::InlayHintKind::PARAMETER,
                                }),
                                text_edits: None,
                                tooltip: None,
                                padding_left: None,
                                padding_right: Some(h.kind == HintKind::Parameter),
                                data: None,
                            })
                            .collect::<Vec<_>>(),
                    )
                    .unwrap(),
                    _ => Value::Null,
                };

                (result, status)
            }
            Outcome::Failed(reason) => (
                Value::Null,
                (Err(reason), Duration::ZERO, Source::Fresh, false),
            ),
            Outcome::Diagnostics(_) => (
                Value::Null,
                (
                    Err(Unavailable::new(
                        "internal",
                        "the worker answered another query",
                    )),
                    Duration::ZERO,
                    Source::Fresh,
                    false,
                ),
            ),
        };

        if self.options.status {
            let (state, elapsed, source, cached) = status;
            let mut params = json!({
                "id": pending.id,
                "method": pending.method,
                "textDocument": { "uri": pending.uri },
                "status": match (&state, source) {
                    (Err(_), _) => "unavailable",
                    (Ok(()), Source::Fresh | Source::Repaired) => "fresh",
                    (Ok(()), Source::Stale) => "stale",
                },
                "elapsedMs": (elapsed.as_secs_f64() * 10000.0).round() / 10.0,
            });

            // Fresh, for the documents as they are, except the lines that
            // were blank.
            if state.is_ok() && source == Source::Repaired {
                params["repaired"] = json!(true);
            }

            // No check ran for it: it came from checks kept from earlier
            // requests about the same text.
            if cached {
                params["cached"] = json!(true);
            }

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
        let contents = if self.support.markdown {
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

fn status_of<T>(answer: &Answer<T>) -> (Result<(), Unavailable>, Duration, Source, bool) {
    (
        answer.result.as_ref().map(|_| ()).map_err(Clone::clone),
        answer.elapsed,
        answer.source,
        answer.cached,
    )
}

fn completion(result: Result<Option<MembersAt>, Unavailable>, support: Support) -> Value {
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
                MemberKind::Constant if support.constant_kind => lsp::CompletionItemKind::CONSTANT,
                // The nearest of the kinds every client has.
                MemberKind::Constant => lsp::CompletionItemKind::VALUE,
                MemberKind::Type => lsp::CompletionItemKind::CLASS,
            }),
            detail: Some(m.signature),
            label_details: support
                .label_details
                .then_some(lsp::CompletionItemLabelDetails {
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

/// A signature as `SignatureHelp`, with what the client supports: each
/// parameter's label as offsets into the signature's, or else its text,
/// and the active parameter on the signature too, and `null` when none is
/// (past the last parameter), or else left out. A client that can't take
/// `null` then defaults to the first, as the spec says.
fn signature_help(s: query::Signature, support: Support, encoding: Encoding) -> Value {
    let parameters: Vec<Value> = s
        .parameters
        .iter()
        .map(|(_, [start, end])| {
            if support.label_offsets {
                json!({ "label": [start, end] })
            } else {
                json!({ "label": slice_units(&s.label, *start, *end, encoding) })
            }
        })
        .collect();
    let active = match s.active {
        Some(i) => Some(json!(i)),
        None if support.no_active_parameter => Some(Value::Null),
        None => None,
    };
    let mut signature = json!({ "label": s.label, "parameters": parameters });
    let mut help = json!({ "signatures": [], "activeSignature": 0 });

    if let Some(active) = active {
        if support.signature_active_parameter {
            signature["activeParameter"] = active.clone();
        }

        help["activeParameter"] = active;
    }

    help["signatures"] = json!([signature]);
    help
}

/// The text from `start` to `end` in `encoding`'s code units.
fn slice_units(text: &str, start: u32, end: u32, encoding: Encoding) -> String {
    let mut units = 0;
    let mut out = String::new();

    for c in text.chars() {
        if units >= start && units < end {
            out.push(c);
        }

        units += encoding.len(c.encode_utf8(&mut [0; 4]));
    }

    out
}

/// A place as an LSP `Location`, or `null` for a path that can't be a
/// `file:` URI.
fn location(place: Place) -> Value {
    match Url::from_file_path(&place.file) {
        Ok(uri) => serde_json::to_value(lsp::Location {
            uri,
            range: range(place.range),
        })
        .unwrap(),
        Err(_) => Value::Null,
    }
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

/// Applies one change from `didChange` to a document's text. Returns the
/// bytes it replaced, and how many replaced them.
fn apply(
    text: &mut String,
    change: lsp::TextDocumentContentChangeEvent,
    encoding: Encoding,
) -> (usize, usize, usize) {
    let Some(r) = change.range else {
        let edit = crate::edits::difference(text, &change.text);

        *text = change.text;
        return edit;
    };
    let offset = |p: lsp::Position| {
        byte_offset(
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
    (start, end, change.text.len())
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
    // `rootUri` and `rootPath` are deprecated, but older clients send only
    // them.
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

fn send_diagnostics(connection: &Connection, uri: &Url, diagnostics: Vec<lsp::Diagnostic>) {
    let params = lsp::PublishDiagnosticsParams {
        uri: uri.clone(),
        diagnostics,
        version: None,
    };

    let _ = connection
        .sender
        .send(Message::Notification(Notification::new(
            "textDocument/publishDiagnostics".to_string(),
            params,
        )));
}

/// A number in a request's params, or 0 (then the params are invalid
/// anyway, and parsing them says so).
fn json_u32(params: &Value, pointer: &str) -> u32 {
    params.pointer(pointer).and_then(Value::as_u64).unwrap_or(0) as u32
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

// stderr is the server's log (docs/lsp.md), and this is its one writer.
#[allow(clippy::print_stderr)]
fn log(message: &str) {
    eprintln!("cuttlebone: {}", message);
}
