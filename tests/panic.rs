//! A compiler panic must not take Cuttlebone down.
mod common;

use common::Project;
use cuttlebone::query::{self, guarded};

const MAIN: &str = "src/main.inko";

#[test]
fn a_panic_in_the_type_checker_is_reported_as_unavailable() {
    let p = Project::new("panic");
    let err = p.type_at(MAIN, None, "let value", 4).unwrap_err();

    assert_eq!(err.code, "compilerPanic");
    assert!(err.message.contains("The type std.map.Map isn't defined"), "{}", err.message);
    // With where it happened.
    assert!(err.message.contains("types/src/lib.rs"), "{}", err.message);

    // And the next request works.
    assert_eq!(Project::new("members").type_of(MAIN, "let name", 4), "String");

    // Including another one that panics.
    assert_eq!(p.type_at(MAIN, None, "let value", 4).unwrap_err().code, "compilerPanic");
}

#[test]
fn a_panic_lowering_to_mir_cant_happen_because_queries_dont_lower_to_mir() {
    // `inko check` panics on this project in MIR lowering (a `break` inside
    // a closure; mir/passes.rs:1836). Types never need MIR, so queries stop
    // before it.
    let p = Project::new("crosscheck");
    let t = p.type_at(MAIN, None, "let a = loop", 8).unwrap().unwrap();

    assert_eq!(t.typ, "Nil");
}

#[test]
fn guarded_returns_the_panic_message_and_location() {
    let err = guarded(|| -> u8 { panic!("boom {}", 42) }).unwrap_err();

    assert!(err.contains("boom 42"), "{}", err);
    assert!(err.contains("tests/panic.rs"), "{}", err);
    assert_eq!(guarded(|| 7), Ok(7));
}

#[test]
fn guarded_survives_deep_recursion_that_would_overflow_a_default_thread() {
    // The compiler is recursive; checks run on a thread with a larger stack
    // than the 2 MiB Rust gives spawned threads.
    fn depth(n: u64) -> u64 {
        let pad = [n; 64];

        if n == 0 { 0 } else { std::hint::black_box(pad)[0] + depth(n - 1) - n + 1 }
    }

    assert_eq!(guarded(|| depth(30_000)), Ok(30_000));
}

#[test]
fn the_query_api_reports_the_time_taken_even_for_a_panic() {
    let p = Project::new("panic");
    let answer = query::type_at(&p.session, &p.request(MAIN, None, "let value", 4));

    assert!(answer.result.is_err());
    assert!(answer.elapsed.as_nanos() > 0);
}
