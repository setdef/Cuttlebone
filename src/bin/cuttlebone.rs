//! Cuttlebone, a language server: speaks LSP on stdin and stdout
//! (docs/lsp.md).
//!
//! Usage: cuttlebone [--stdio] [--clientProcessId=PID] | --version
#![forbid(unsafe_code)]
use cuttlebone::check::{VERSION, upstream_version};
use cuttlebone::lsp::{self, Ended};
use cuttlebone::worker::{self, WORKER_FLAG};
use lsp_server::Connection;
use std::process::ExitCode;

// `--version` and usage errors print before any LSP session starts, and a
// worker's own failure goes to stderr, which the server logs.
#[allow(clippy::print_stdout, clippy::print_stderr)]
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = || {
        eprintln!("usage: cuttlebone [--stdio] [--clientProcessId=PID] | --version");
        ExitCode::from(2)
    };

    match args.first().map(String::as_str) {
        Some("--version") if args.len() == 1 => {
            println!("cuttlebone {} (Inko {})", VERSION, upstream_version());
            return ExitCode::SUCCESS;
        }
        Some(WORKER_FLAG) if args.len() == 1 => {
            return match worker::serve() {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("cuttlebone worker: {}", e);
                    ExitCode::FAILURE
                }
            };
        }
        _ => {}
    }

    // The options LSP recommends for servers: stdio is the only transport,
    // and some clients pass `--stdio`; the client's
    // process id is LSP's `processId`, for clients that don't send it.
    let mut client = None;
    let mut rest = args.iter();

    while let Some(arg) = rest.next() {
        let pid = match arg.as_str() {
            "--stdio" => continue,
            "--clientProcessId" => rest.next().map(String::as_str),
            other => match other.strip_prefix("--clientProcessId=") {
                Some(pid) => Some(pid),
                None => return usage(),
            },
        };

        match pid.and_then(|p| p.parse().ok()) {
            Some(pid) => client = Some(pid),
            None => return usage(),
        }
    }

    let (connection, threads) = Connection::stdio();
    let result = lsp::serve(connection, client);

    match result {
        Ok(Ended::Exit { shut_down }) => {
            // The reader thread ends after `exit`, or when stdin closes;
            // the writer thread ends once everything queued is written.
            let _ = threads.join();

            if shut_down {
                ExitCode::SUCCESS
            } else {
                // `exit` without `shutdown` first, as LSP specifies.
                ExitCode::FAILURE
            }
        }
        // Stdin is still open, so the reader thread would wait forever.
        Ok(Ended::Orphaned) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("cuttlebone: {}", e);
            ExitCode::FAILURE
        }
    }
}
