//! Runs queries in a child process, so the server can time them out and
//! kill them, and so an abort or a crash in the compiler can't take the
//! server down.
//!
//! The server starts `cuttlebone --worker`, and writes each `Job` to its
//! stdin as a line of JSON. The worker answers each with a line on stdout:
//! the job's id, a space, and the `Outcome` as JSON. It keeps checks
//! between jobs (`query::Cache`), so questions about the same text cost one
//! check, and exits when its stdin closes. The server sends a worker one job
//! at a time, and kills it to stop a job. Anything on its stderr (such as a
//! panic's message) is passed on to the server's stderr.
use crate::edits::Fallback;
use crate::position::Position;
use crate::query::{
    self, Answer, Cache, Call, Diagnostic, Diagnostics, Fix, Highlight, Hints, HoverAt, ImportFix,
    Item, Lenses, MembersAt, ModuleUse, Place, Question, Reply, Request, Session, Signature,
    Unavailable,
};
use crossbeam_channel::{RecvTimeoutError, Sender};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The argument that makes `cuttlebone` a worker.
pub const WORKER_FLAG: &str = "--worker";

/// The exit status of a worker that outlived its job's deadline.
pub const DEADLINE_STATUS: i32 = 124;

/// How much longer than the server's timeout a worker may spend on a job.
pub const DEADLINE_MARGIN: Duration = Duration::from_secs(1);

/// For testing robustness only: makes a worker hang (`hang`) or abort
/// (`abort`) instead of checking, or check without repaired copies
/// (`norepair`, so answers from older results can be tested), or with the
/// type checker's passes stopping at the first error (`stop`, as after a
/// panic in a later pass, so the answers that need repaired copies or
/// older results then can be tested), when the checked file's path ends
/// with the text after the `:`, e.g. `hang:src/slow.inko`. No real input is
/// known to do either, and the server must survive both. Only in builds with
/// the `fault-injection` feature, which only the tests enable.
#[cfg(feature = "fault-injection")]
pub const FAULT_VARIABLE: &str = "CUTTLEBONE_FAULT";

/// For testing only: with this set (to anything), workers keep no checks
/// between jobs, so every question costs a check of its own, as before
/// `query::Cache`. The tests run in both modes to show that the answers
/// are the same. Only in builds with the `fault-injection` feature.
#[cfg(feature = "fault-injection")]
pub const NO_CACHE_VARIABLE: &str = "CUTTLEBONE_NO_CACHE";

/// How many checks a worker keeps.
pub fn kept_checks() -> usize {
    #[cfg(feature = "fault-injection")]
    if std::env::var_os(NO_CACHE_VARIABLE).is_some() {
        return 0;
    }

    query::KEPT_CHECKS
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Query {
    /// `query::hover`.
    Type,
    /// `query::complete`.
    Complete,
    /// `query::definition`.
    Definition,
    /// `query::type_definition`.
    TypeDefinition,
    /// `query::references`.
    References { include_declaration: bool },
    /// `query::highlights`.
    Highlight,
    /// `query::signature_help`.
    SignatureHelp,
    /// `query::inlay_hints`, from the request's position to `end`.
    InlayHints { end: Position },
    /// `query::diagnostics`, for the target in the request's `file`.
    Diagnostics,
    /// `query::fixes`, for the diagnostics from the request's position to
    /// `end`, with the job's `imports`.
    Fixes { end: Position },
    /// `Question::Implementation`.
    Implementation,
    /// `Question::Declaration`.
    Declaration,
    /// `Question::CallItem`.
    CallItem,
    /// `Question::IncomingCalls`.
    IncomingCalls,
    /// `Question::OutgoingCalls`.
    OutgoingCalls,
    /// `Question::TypeItem`.
    TypeItem,
    /// `Question::Supertypes`.
    Supertypes,
    /// `Question::Subtypes`.
    Subtypes,
    /// `Question::ModuleUses`, of the request's file and the job's
    /// `files`.
    ModuleUses,
    /// `query::lenses`, of the request's file.
    Lenses,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub query: Query,
    pub session: Session,
    pub request: Request,
    /// The documents of the file's last check that typed method bodies, for
    /// answers from older results.
    #[serde(default)]
    pub fallback: Option<Fallback>,
    /// How long the worker may spend on the job before it exits by
    /// itself. The server kills a worker that takes too long, but if the
    /// server dies first (say from SIGKILL), nothing would stop a hung
    /// worker.
    pub deadline: Option<Duration>,
    /// For `Query::Fixes`: the imports the server offers for names in the
    /// file, from its index.
    #[serde(default)]
    pub imports: Vec<ImportFix>,
    /// For `Query::ModuleUses`: the other files whose modules are asked
    /// about, after the request's.
    #[serde(default)]
    pub files: Vec<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Outcome {
    Hover(Answer<HoverAt>),
    Members(Answer<MembersAt>),
    Place(Answer<Place>),
    Places(Answer<Vec<Place>>),
    Highlights(Answer<Vec<Highlight>>),
    Signature(Answer<Signature>),
    Hints(Answer<Hints>),
    Diagnostics(Answer<Diagnostics>),
    Fixes(Answer<Vec<(Diagnostic, Vec<Fix>)>>),
    Items(Answer<Vec<Item>>),
    Calls(Answer<Vec<Call>>),
    /// Each file's module's uses, in order.
    Uses(Answer<Uses>),
    Lenses(Answer<Lenses>),
    /// The worker didn't produce an answer: it timed out, crashed or
    /// couldn't be started.
    Failed(Unavailable),
}

/// Each file's module's uses (`Question::ModuleUses`).
pub type Uses = Vec<(PathBuf, Vec<ModuleUse>)>;

impl Job {
    /// Runs the job in this process, with the checks `cache` kept.
    pub fn run(&self, cache: &Cache) -> Outcome {
        let question = match self.query {
            Query::Type => Question::Hover,
            Query::Complete => Question::Complete,
            Query::Definition => Question::Definition,
            Query::TypeDefinition => Question::TypeDefinition,
            Query::References {
                include_declaration,
            } => Question::References {
                include_declaration,
            },
            Query::Highlight => Question::Highlight,
            Query::SignatureHelp => Question::SignatureHelp,
            Query::InlayHints { end } => Question::InlayHints { end },
            Query::Diagnostics => {
                return Outcome::Diagnostics(query::diagnostics(&self.session, &self.request));
            }
            Query::Fixes { end } => {
                return Outcome::Fixes(query::fixes(
                    &self.session,
                    &self.request,
                    end,
                    &self.imports,
                ));
            }
            Query::Implementation => Question::Implementation,
            Query::Declaration => Question::Declaration,
            Query::CallItem => Question::CallItem,
            Query::IncomingCalls => Question::IncomingCalls,
            Query::OutgoingCalls => Question::OutgoingCalls,
            Query::TypeItem => Question::TypeItem,
            Query::Supertypes => Question::Supertypes,
            Query::Subtypes => Question::Subtypes,
            Query::ModuleUses => return Outcome::Uses(self.module_uses(cache)),
            Query::Lenses => {
                return Outcome::Lenses(query::lenses(
                    cache,
                    &self.session,
                    &self.request,
                    repair(self),
                ));
            }
        };
        let repair = repair(self);
        let answer = query::ask_with(
            cache,
            &self.session,
            &self.request,
            self.fallback.as_ref(),
            repair,
            &[(question, self.request.position)],
        )
        .pop()
        .unwrap();

        match self.query {
            Query::Type => Outcome::Hover(answer.map(|r| match r {
                Reply::Hover(h) => Some(h),
                _ => None,
            })),
            Query::Complete => Outcome::Members(answer.map(|r| match r {
                Reply::Members(m) => Some(m),
                _ => None,
            })),
            Query::TypeDefinition => Outcome::Place(answer.map(|r| match r {
                Reply::Place(p) => Some(p),
                _ => None,
            })),
            Query::Definition
            | Query::References { .. }
            | Query::Implementation
            | Query::Declaration => Outcome::Places(answer.map(|r| match r {
                Reply::Places(p) => Some(p),
                _ => None,
            })),
            Query::Highlight => Outcome::Highlights(answer.map(|r| match r {
                Reply::Highlights(h) => Some(h),
                _ => None,
            })),
            Query::SignatureHelp => Outcome::Signature(answer.map(|r| match r {
                Reply::Signature(s) => Some(s),
                _ => None,
            })),
            Query::CallItem | Query::TypeItem | Query::Supertypes | Query::Subtypes => {
                Outcome::Items(answer.map(|r| match r {
                    Reply::Items(i) => Some(i),
                    _ => None,
                }))
            }
            Query::IncomingCalls | Query::OutgoingCalls => {
                Outcome::Calls(answer.map(|r| match r {
                    Reply::Calls(c) => Some(c),
                    _ => None,
                }))
            }
            Query::InlayHints { .. }
            | Query::Diagnostics
            | Query::Fixes { .. }
            | Query::ModuleUses
            | Query::Lenses => Outcome::Hints(answer.map(|r| match r {
                Reply::Hints(h) => Some(h),
                _ => None,
            })),
        }
    }
}

/// Whether a job may answer from repaired copies: always, but in tests
/// with the `norepair` fault.
fn repair(job: &Job) -> bool {
    #[cfg(feature = "fault-injection")]
    return fault(job).as_deref() != Some("norepair");
    #[cfg(not(feature = "fault-injection"))]
    {
        let _ = job;
        true
    }
}

impl Job {
    /// The uses of the module of the request's file, and of each of
    /// `files`, from one check of the whole project: the first asks, and
    /// the rest find it kept (`cache`). Every file's, or no answer.
    fn module_uses(&self, cache: &Cache) -> Answer<Uses> {
        let mut all = Vec::new();
        let mut first: Option<Answer<Uses>> = None;

        for file in std::iter::once(&self.request.file).chain(&self.files) {
            let request = Request {
                file: file.clone(),
                ..self.request.clone()
            };
            let answer = query::ask_with(
                cache,
                &self.session,
                &request,
                None,
                false,
                &[(Question::ModuleUses, request.position)],
            )
            .pop()
            .unwrap();
            let answer = answer.map(|r| match r {
                Reply::Uses(u) => Some(vec![(file.clone(), u)]),
                _ => None,
            });

            match &answer.result {
                Ok(Some(uses)) => all.extend(uses.iter().cloned()),
                Ok(None) => {}
                Err(_) => return answer,
            }

            first.get_or_insert(answer);
        }

        let mut answer = first.unwrap();

        answer.result = Ok(Some(all));
        answer
    }
}

impl Outcome {
    /// Whether the check typed method bodies of the documents as they
    /// were sent, so the server may keep them for later answers.
    pub fn typed(&self) -> bool {
        match self {
            Outcome::Hover(a) => a.typed,
            Outcome::Members(a) => a.typed,
            Outcome::Place(a) => a.typed,
            Outcome::Places(a) => a.typed,
            Outcome::Highlights(a) => a.typed,
            Outcome::Signature(a) => a.typed,
            Outcome::Hints(a) => a.typed,
            Outcome::Diagnostics(a) => a.typed,
            Outcome::Fixes(a) => a.typed,
            Outcome::Items(a) => a.typed,
            Outcome::Calls(a) => a.typed,
            Outcome::Uses(a) => a.typed,
            Outcome::Lenses(a) => a.typed,
            Outcome::Failed(_) => false,
        }
    }
}

/// The worker's `main`: answers each job read from stdin, a line of JSON,
/// with a line on stdout, until stdin closes.
pub fn serve() -> Result<(), String> {
    let cache = Cache::new(kept_checks());
    #[cfg(feature = "fault-injection")]
    let cache = match faults().into_iter().find(|(kind, _)| kind == "stop") {
        Some((_, suffix)) => cache.stopping_at_errors(suffix),
        None => cache,
    };
    let deadlines = watchdog();
    let mut stdout = std::io::stdout().lock();

    for line in std::io::stdin().lock().lines() {
        let line = line.map_err(|e| format!("couldn't read a job: {}", e))?;

        if line.trim().is_empty() {
            continue;
        }

        let (id, job) = line
            .split_once(' ')
            .ok_or_else(|| "a job without an id".to_string())?;
        let job: Job = serde_json::from_str(job).map_err(|e| format!("invalid job: {}", e))?;
        let _ = deadlines.send(job.deadline.map(|d| Instant::now() + d));

        #[cfg(feature = "fault-injection")]
        inject_fault(&job);

        let outcome = serde_json::to_string(&job.run(&cache)).unwrap();

        stdout
            .write_all(format!("{} {}\n", id, outcome).as_bytes())
            .and_then(|_| stdout.flush())
            .map_err(|e| format!("couldn't write the outcome: {}", e))?;

        let _ = deadlines.send(None);
    }

    Ok(())
}

/// A thread that exits the process with `DEADLINE_STATUS` if a job is
/// still running at the deadline last sent to it (`None` once it's done).
fn watchdog() -> Sender<Option<Instant>> {
    let (deadlines, receiver) = crossbeam_channel::unbounded::<Option<Instant>>();

    std::thread::spawn(move || {
        let mut deadline: Option<Instant> = None;

        loop {
            let next = match deadline {
                Some(at) => receiver.recv_timeout(at.saturating_duration_since(Instant::now())),
                None => receiver.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };

            match next {
                Ok(at) => deadline = at,
                Err(RecvTimeoutError::Timeout) => std::process::exit(DEADLINE_STATUS),
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    });

    deadlines
}

/// The faults `CUTTLEBONE_FAULT` asks for, separated by commas, as kinds
/// and suffixes.
#[cfg(feature = "fault-injection")]
fn faults() -> Vec<(String, String)> {
    std::env::var(FAULT_VARIABLE)
        .unwrap_or_default()
        .split(',')
        .filter_map(|f| f.split_once(':'))
        .map(|(kind, suffix)| (kind.to_string(), suffix.to_string()))
        .collect()
}

/// The fault `CUTTLEBONE_FAULT` asks for other than `stop`, if its suffix
/// matches the job's file.
#[cfg(feature = "fault-injection")]
fn fault(job: &Job) -> Option<String> {
    let file = job.request.file.to_string_lossy().to_string();

    faults()
        .into_iter()
        .find(|(kind, suffix)| kind != "stop" && file.ends_with(suffix.as_str()))
        .map(|(kind, _)| kind)
}

/// How much slower each job is with the `slow` fault.
#[cfg(feature = "fault-injection")]
const SLOW_FAULT: Duration = Duration::from_millis(300);

#[cfg(feature = "fault-injection")]
fn inject_fault(job: &Job) {
    let Some(kind) = fault(job) else {
        return;
    };

    match kind.as_str() {
        "hang" => loop {
            std::thread::sleep(Duration::from_secs(3600));
        },
        "abort" => std::process::abort(),
        // A deliberately slower Cuttlebone, for the latency limits to
        // fail (.github/workflows/latency.yml, by hand).
        "slow" => std::thread::sleep(SLOW_FAULT),
        _ => {}
    }
}

/// A worker process, which answers jobs one at a time and keeps checks
/// between them.
pub struct Worker {
    child: Child,
    /// `None` once closed, for a worker that answers one job.
    stdin: Option<ChildStdin>,
    /// The id of the job it was last sent, for the `Finished` it sends.
    current: Arc<AtomicU64>,
    /// Whether its output is still open.
    alive: Arc<AtomicBool>,
    /// What it wrote to stderr since the last answer.
    stderr: Arc<Mutex<String>>,
}

/// A job running in a worker.
pub struct Check {
    /// Identifies the `Finished` message this check sends.
    pub id: u64,
    pub started: Instant,
    worker: Worker,
}

/// A line a worker printed, or the end of its output, with what it wrote
/// to stderr since its last answer.
pub struct Finished {
    pub id: u64,
    pub stdout: String,
    pub stderr: String,
}

impl Worker {
    /// Starts a worker process. A thread reads its answers, and sends each
    /// to `done` as a `Finished`, and one more when its output ends
    /// (because it exited, or was killed).
    pub fn start(done: Sender<Finished>) -> Result<Worker, Unavailable> {
        let exe = std::env::current_exe().map_err(|e| {
            Unavailable::new("checkFailed", format!("couldn't find cuttlebone: {}", e))
        })?;
        let mut child = Command::new(exe)
            .arg(WORKER_FLAG)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                Unavailable::new("checkFailed", format!("couldn't start a worker: {}", e))
            })?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let mut errors = child.stderr.take().unwrap();
        let current = Arc::new(AtomicU64::new(0));
        let alive = Arc::new(AtomicBool::new(true));
        let stderr = Arc::new(Mutex::new(String::new()));
        let (job, open, log) = (current.clone(), alive.clone(), stderr.clone());

        std::thread::spawn(move || {
            let collected = log.clone();
            let errors = std::thread::spawn(move || {
                let mut buffer = [0; 4096];

                while let Ok(n) = errors.read(&mut buffer) {
                    if n == 0 {
                        break;
                    }

                    collected
                        .lock()
                        .unwrap()
                        .push_str(&String::from_utf8_lossy(&buffer[..n]));
                }
            });
            let take = || std::mem::take(&mut *log.lock().unwrap());

            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    break;
                };
                let Some((id, outcome)) = line.split_once(' ') else {
                    continue;
                };
                let Ok(id) = id.parse() else {
                    continue;
                };

                if done
                    .send(Finished {
                        id,
                        stdout: outcome.to_string(),
                        stderr: take(),
                    })
                    .is_err()
                {
                    return;
                }
            }

            // In this order, so a job sent from now on either sees that the
            // worker is gone, or gets this message.
            open.store(false, Ordering::SeqCst);

            let _ = errors.join();
            let _ = done.send(Finished {
                id: job.load(Ordering::SeqCst),
                stdout: String::new(),
                stderr: take(),
            });
        });

        Ok(Worker {
            child,
            stdin,
            current,
            alive,
            stderr,
        })
    }

    /// Sends the worker a job. `Err` gives the worker back if it's gone,
    /// so the caller can start another.
    pub fn run(mut self, id: u64, job: &Job) -> Result<Check, Unavailable> {
        self.current.store(id, Ordering::SeqCst);

        let line = format!("{} {}\n", id, serde_json::to_string(job).unwrap());
        let sent = self.alive.load(Ordering::SeqCst)
            && self.stdin.as_mut().is_some_and(|stdin| {
                stdin
                    .write_all(line.as_bytes())
                    .and_then(|_| stdin.flush())
                    .is_ok()
            });

        if !sent {
            self.kill();
            return Err(Unavailable::new(
                "checkFailed",
                "the worker exited before it was sent the job",
            ));
        }

        Ok(Check {
            id,
            started: Instant::now(),
            worker: self,
        })
    }

    /// Kills the worker, and waits for it to exit.
    pub fn kill(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// What the worker wrote to stderr that no answer took.
    pub fn take_stderr(&self) -> String {
        std::mem::take(&mut *self.stderr.lock().unwrap())
    }
}

impl Check {
    /// Starts `job` in a new worker process that answers only it, and
    /// exits afterwards.
    pub fn start(id: u64, job: &Job, done: Sender<Finished>) -> Result<Check, Unavailable> {
        let worker = Worker::start(done)?;
        let mut check = worker.run(id, job)?;

        drop(check.worker.stdin.take());
        Ok(check)
    }

    /// The check's outcome, from what its worker printed, and the worker,
    /// if it can take another job. A worker that answers only one job is
    /// waited for, and must exit successfully.
    pub fn finish(mut self, finished: &Finished) -> (Outcome, Option<Worker>) {
        let parsed = serde_json::from_str::<Outcome>(&finished.stdout);

        if let Ok(outcome) = &parsed
            && self.worker.stdin.is_some()
        {
            return (outcome.clone(), Some(self.worker));
        }

        let status = self.worker.child.wait();

        match parsed {
            Ok(outcome) if status.as_ref().is_ok_and(ExitStatus::success) => (outcome, None),
            _ => (
                Outcome::Failed(Unavailable::new(
                    "checkCrashed",
                    format!(
                        "the check {}{}",
                        match status {
                            Ok(s) => describe(s),
                            Err(e) => format!("couldn't be waited for: {}", e),
                        },
                        last_line(&finished.stderr)
                            .map(|l| format!(": {}", l))
                            .unwrap_or_default()
                    ),
                )),
                None,
            ),
        }
    }

    /// Kills the worker, and waits for it to exit.
    pub fn kill(self) {
        self.worker.kill();
    }
}

fn describe(status: ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;

        if let Some(signal) = status.signal() {
            return format!("was killed by signal {}", signal);
        }
    }

    match status.code() {
        Some(code) => format!("exited with status {}", code),
        None => "exited".to_string(),
    }
}

fn last_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).rfind(|l| !l.is_empty())
}
