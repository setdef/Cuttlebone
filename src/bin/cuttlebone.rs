//! Cuttlebone, a language server: speaks LSP on stdin and stdout
//! (docs/lsp.md).
//!
//! Usage: cuttlebone [--stdio | --version]
use cuttlebone::worker::{self, WORKER_FLAG};
use cuttlebone::{VERSION, lsp, upstream_version};
use lsp_server::Connection;
use std::process::ExitCode;

#[allow(clippy::print_stdout, clippy::print_stderr)]
fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        // Clients such as VS Code's pass --stdio; it's the only transport.
        None | Some("--stdio") => {}
        Some("--version") => {
            println!("cuttlebone {} (Inko {})", VERSION, upstream_version());
            return ExitCode::SUCCESS;
        }
        Some(WORKER_FLAG) => {
            return match worker::serve() {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("cuttlebone worker: {}", e);
                    ExitCode::FAILURE
                }
            };
        }
        Some(_) => {
            eprintln!("usage: cuttlebone [--stdio | --version]");
            return ExitCode::from(2);
        }
    }

    let (connection, threads) = Connection::stdio();
    let result = lsp::serve(connection);

    // The reader thread ends after `exit`, or when stdin closes; the writer
    // thread ends once everything queued is written.
    let _ = threads.join();

    match result {
        Ok(true) => ExitCode::SUCCESS,
        // `exit` without `shutdown` first, as LSP specifies.
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("cuttlebone: {}", e);
            ExitCode::FAILURE
        }
    }
}
