//! Prints every expression of an Inko project or file with its type.
//!
//! Usage: dump-types [--std DIR] [--no-tests] [--summary] PATH
use cuttlebone::check::Target;
use cuttlebone::dump::{Status, check, format_expr, upstream_std};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: dump-types [--std DIR] [--no-tests] [--summary] PATH";

fn main() -> ExitCode {
    let mut std = upstream_std();
    let mut tests = true;
    let mut summary = false;
    let mut path = None;
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--std" => match args.next() {
                Some(v) => std = PathBuf::from(v),
                None => return usage(),
            },
            "--no-tests" => tests = false,
            "--summary" => summary = true,
            "-h" | "--help" => {
                println!("{}", USAGE);
                return ExitCode::SUCCESS;
            }
            _ if path.is_none() => path = Some(PathBuf::from(arg)),
            _ => return usage(),
        }
    }

    let Some(path) = path else { return usage() };
    let mut target = match Target::new(&path, std) {
        Ok(t) => t,
        Err(e) => return fail(&e),
    };

    target.tests = tests;

    let checked = match check(&target) {
        Ok(c) => c,
        Err(e) => return fail(&e),
    };

    if summary {
        let total = checked.expressions.len();
        let mut by_status = BTreeMap::new();
        let mut missing_by_kind = BTreeMap::new();

        for e in &checked.expressions {
            *by_status.entry(e.status()).or_insert(0) += 1;

            if e.status() != Status::Resolved {
                *missing_by_kind.entry((e.status(), e.kind)).or_insert(0) += 1;
            }
        }

        let files: std::collections::BTreeSet<_> =
            checked.expressions.iter().map(|e| &e.file).collect();

        println!("files: {}", files.len());
        println!("modules type checked: {}", checked.modules);
        println!("expressions: {}", total);

        for (status, n) in &by_status {
            println!(
                "  {:?}: {} ({:.3}%)",
                status,
                n,
                100.0 * *n as f64 / total.max(1) as f64
            );
        }

        for ((status, kind), n) in &missing_by_kind {
            println!("  {:?} {}: {}", status, kind, n);
        }

        println!("ok: {}", checked.ok);
        println!("type check: {:.3}s", checked.type_check.as_secs_f64());
        println!("type check + MIR: {:.3}s", checked.total.as_secs_f64());
    } else {
        for e in &checked.expressions {
            println!("{}", format_expr(e, &target.root));
        }
    }

    if checked.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn usage() -> ExitCode {
    eprintln!("{}", USAGE);
    ExitCode::from(2)
}

fn fail(message: &str) -> ExitCode {
    eprintln!("dump-types: {}", message);
    ExitCode::FAILURE
}
