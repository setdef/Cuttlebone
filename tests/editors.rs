//! The Neovim smoke check (editors/neovim/check.lua) against this build,
//! through Neovim's own LSP client. Skipped (passing) without `nvim` on
//! PATH. The VS Code check opens a window, so it only runs from
//! editors/check.sh.
// Tests report why the check was skipped on stderr, which `cargo test` shows.
#![allow(clippy::print_stderr)]
mod common;

use common::client::fake_inko;
use std::path::Path;
use std::process::Command;

#[test]
fn neovim_starts_the_server_negotiates_utf8_and_hovers() {
    if Command::new("nvim").arg("--version").output().is_err() {
        eprintln!("skipped: no nvim on PATH");
        return;
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let state = Path::new(env!("CARGO_TARGET_TMPDIR")).join("nvim-state");
    let out = Command::new("nvim")
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
        .env("XDG_STATE_HOME", &state)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(out.status.success(), "{}{}", stdout, stderr);
    assert!(
        stdout.contains("neovim: ok: hover at [3].size shows Array[Int] (positions in utf-8)"),
        "{}",
        stdout
    );
}
