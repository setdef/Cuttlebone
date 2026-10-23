//! Runs one query in a child process, so the server can time it out and
//! kill it, and so an abort or a crash in the compiler can't take the server
//! down.
//!
//! The server starts `cuttlebone --worker` for each check, writes a `Job` as
//! JSON to its stdin and closes it. The worker writes an `Outcome` as JSON to
//! stdout and exits. Anything on its stderr (such as a panic's message) is
//! passed on to the server's stderr.
use crate::position::Position;
use crate::query::{
    self, Answer, Diagnostics, Highlight, Hint, MembersAt, Place, Request, Session, Signature,
    TypeAt, Unavailable,
};
use crossbeam_channel::Sender;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// The argument that makes `cuttlebone` a worker.
pub const WORKER_FLAG: &str = "--worker";

/// The exit status of a worker that outlived its job's deadline.
pub const DEADLINE_STATUS: i32 = 124;

/// How much longer than the server's timeout a worker may live.
pub const DEADLINE_MARGIN: Duration = Duration::from_secs(1);

/// For testing robustness only: makes a worker hang (`hang`) or abort
/// (`abort`) instead of checking, when the checked file's path ends with
/// the text after the `:`, e.g. `hang:src/slow.inko`. No real input is
/// known to do either, and the server must survive both. Only in builds with
/// the `fault-injection` feature, which only the tests enable.
#[cfg(feature = "fault-injection")]
pub const FAULT_VARIABLE: &str = "CUTTLEBONE_FAULT";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Query {
    /// `query::type_at`.
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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub query: Query,
    pub session: Session,
    pub request: Request,
    /// How long the worker may live before it exits by itself. The server
    /// kills a worker that takes too long, but if the server dies first
    /// (say from SIGKILL), nothing would stop a hung worker.
    pub deadline: Option<Duration>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Outcome {
    Type(Answer<TypeAt>),
    Members(Answer<MembersAt>),
    Place(Answer<Place>),
    Places(Answer<Vec<Place>>),
    Highlights(Answer<Vec<Highlight>>),
    Signature(Answer<Signature>),
    Hints(Answer<Vec<Hint>>),
    Diagnostics(Answer<Diagnostics>),
    /// The worker didn't produce an answer: it timed out, crashed or
    /// couldn't be started.
    Failed(Unavailable),
}

impl Job {
    /// Runs the job in this process.
    pub fn run(&self) -> Outcome {
        match self.query {
            Query::Type => Outcome::Type(query::type_at(&self.session, &self.request)),
            Query::Complete => Outcome::Members(query::complete(&self.session, &self.request)),
            Query::Definition => Outcome::Place(query::definition(&self.session, &self.request)),
            Query::TypeDefinition => {
                Outcome::Place(query::type_definition(&self.session, &self.request))
            }
            Query::References {
                include_declaration,
            } => Outcome::Places(query::references(
                &self.session,
                &self.request,
                include_declaration,
            )),
            Query::Highlight => {
                Outcome::Highlights(query::highlights(&self.session, &self.request))
            }
            Query::SignatureHelp => {
                Outcome::Signature(query::signature_help(&self.session, &self.request))
            }
            Query::InlayHints { end } => {
                Outcome::Hints(query::inlay_hints(&self.session, &self.request, end))
            }
            Query::Diagnostics => {
                Outcome::Diagnostics(query::diagnostics(&self.session, &self.request))
            }
        }
    }
}

/// The worker's `main`: reads a job from stdin, and writes its outcome to
/// stdout.
pub fn serve() -> Result<(), String> {
    let mut input = String::new();

    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| format!("couldn't read the job: {}", e))?;

    let job: Job = serde_json::from_str(&input).map_err(|e| format!("invalid job: {}", e))?;

    if let Some(deadline) = job.deadline {
        std::thread::spawn(move || {
            std::thread::sleep(deadline);
            std::process::exit(DEADLINE_STATUS);
        });
    }

    #[cfg(feature = "fault-injection")]
    inject_fault(&job);

    let outcome = serde_json::to_string(&job.run()).unwrap();
    let mut stdout = std::io::stdout().lock();

    stdout
        .write_all(outcome.as_bytes())
        .and_then(|_| stdout.flush())
        .map_err(|e| format!("couldn't write the outcome: {}", e))
}

#[cfg(feature = "fault-injection")]
fn inject_fault(job: &Job) {
    let Ok(fault) = std::env::var(FAULT_VARIABLE) else {
        return;
    };
    let Some((kind, suffix)) = fault.split_once(':') else {
        return;
    };

    if !job.request.file.to_string_lossy().ends_with(suffix) {
        return;
    }

    match kind {
        "hang" => loop {
            std::thread::sleep(Duration::from_secs(3600));
        },
        "abort" => std::process::abort(),
        _ => {}
    }
}

/// A check running in a worker process.
pub struct Check {
    /// Identifies the `Finished` message this check sends.
    pub id: u64,
    pub started: Instant,
    child: Child,
}

/// What a worker printed, sent when its stdout closes.
pub struct Finished {
    pub id: u64,
    pub stdout: String,
    pub stderr: String,
}

impl Check {
    /// Starts `job` in a new worker process. A thread writes the job and
    /// reads the output, and sends `Finished` to `done` when the worker's
    /// output ends (because it exited, or was killed).
    pub fn start(id: u64, job: &Job, done: Sender<Finished>) -> Result<Check, Unavailable> {
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
        let input = serde_json::to_string(job).unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();

        std::thread::spawn(move || {
            let errors = std::thread::spawn(move || {
                let mut text = String::new();
                let _ = stderr.read_to_string(&mut text);
                text
            });

            // A worker that dies before reading everything closes the pipe;
            // that shows up as missing output below.
            let _ = stdin.write_all(input.as_bytes());
            drop(stdin);

            let mut text = String::new();
            let _ = stdout.read_to_string(&mut text);

            let _ = done.send(Finished {
                id,
                stdout: text,
                stderr: errors.join().unwrap_or_default(),
            });
        });

        Ok(Check {
            id,
            started: Instant::now(),
            child,
        })
    }

    /// The check's outcome, from what its worker printed. Waits for the
    /// worker to exit, which it does right after closing its output.
    pub fn finish(mut self, finished: &Finished) -> Outcome {
        let status = self.child.wait();

        match serde_json::from_str::<Outcome>(&finished.stdout) {
            Ok(outcome) if status.as_ref().is_ok_and(ExitStatus::success) => outcome,
            _ => Outcome::Failed(Unavailable::new(
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
        }
    }

    /// Kills the worker, and waits for it to exit.
    pub fn kill(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
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
