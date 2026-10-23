//! The Neovim smoke check (editors/neovim/check.lua) against this build,
//! through Neovim's own LSP client. Skipped (passing) without `nvim` on
//! PATH. The VS Code check opens a window, so it only runs from
//! editors/check.sh.
// Tests report why the check was skipped on stderr, which `cargo test` shows.
#![allow(clippy::print_stderr)]
mod common;

use common::client::fake_inko;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn neovim_starts_the_server_negotiates_utf8_hovers_goes_to_a_definition_and_shows_diagnostics() {
    if Command::new("nvim").arg("--version").output().is_err() {
        eprintln!("skipped: no nvim on PATH");
        return;
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let state = Path::new(env!("CARGO_TARGET_TMPDIR")).join("nvim-state");
    let mut child = Command::new("nvim")
        .args(["--headless", "--clean", "-c"])
        .arg(format!(
            "luafile {}",
            root.join("editors/neovim/check.lua").display()
        ))
        .env("CUTTLEBONE", env!("CARGO_BIN_EXE_cuttlebone"))
        .env("INKO", fake_inko(cuttlebone::check::upstream_version()))
        .env("FILE", root.join("tests/fixtures/unicode/src/main.inko"))
        // As in editors/check.sh: the wrong encoding lands on `size`.
        .env("NEEDLE", "[3].size")
        .env("OFFSET", "0")
        .env("EXPECTED", "Array[Int]")
        // The wrong encoding is past the end of the line.
        .env("DEFINITION", "'日本' + e")
        .env("DEFINITION_OFFSET", "7")
        .env("DEFINED_LINE", "12")
        .env("DEFINED", "e")
        // An error after non-ASCII text gets a diagnostic covering the
        // name, which goes when the line does.
        .env("BROKEN", "    let g = '日本' + nope")
        .env("BROKEN_AFTER", "12")
        .env("ERROR_CODE", "invalid-symbol")
        .env("ERROR_TEXT", "nope")
        .env("XDG_STATE_HOME", &state)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();

    // Neovim waits at most 20 s for each step itself.
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > Duration::from_secs(120) {
            let _ = child.kill();
            panic!("neovim didn't finish within 120 s");
        }

        std::thread::sleep(Duration::from_millis(50));
    }

    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(out.status.success(), "{}{}", stdout, stderr);
    assert!(
        stdout.contains("neovim: ok: hover at [3].size shows Array[Int] (positions in utf-8)"),
        "{}",
        stdout
    );
    assert!(
        stdout.contains("neovim: ok: definition at '日本' + e is e on line 12"),
        "{}",
        stdout
    );
    assert!(
        stdout.contains(
            "neovim: ok: a invalid-symbol diagnostic covered nope on line 13, and cleared when \
             the line was removed"
        ),
        "{}",
        stdout
    );
}
