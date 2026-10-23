//! Cuttlebone's type service: reads JSON requests from stdin and writes
//! replies to stdout, one per line (docs/protocol.md).
//!
//! Usage: cuttlebone [--version]
use cuttlebone::server::{PROTOCOL, Server};
use cuttlebone::{VERSION, upstream_version};
use std::process::ExitCode;

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        None => {}
        Some("--version") => {
            println!(
                "cuttlebone {} (Inko {}, protocol {})",
                VERSION,
                upstream_version(),
                PROTOCOL
            );
            return ExitCode::SUCCESS;
        }
        Some(_) => {
            eprintln!("usage: cuttlebone [--version]");
            return ExitCode::from(2);
        }
    }

    let stdin = std::io::stdin().lock();
    let stdout = std::io::stdout().lock();

    match Server::new().serve(stdin, stdout) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cuttlebone: {}", e);
            ExitCode::FAILURE
        }
    }
}
