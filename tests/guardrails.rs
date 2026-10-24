//! The standing rules in AGENTS.md that a test can check by reading the
//! repository: the layering of src/, no editor or client named in code, no
//! product named after Inko, and a justification in patches/README.md for
//! every line of the upstream patch.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Each module of the library, and the modules it may use. The core
/// (everything but `worker`, `lsp` and `dump`) never uses `worker` or
/// `lsp`.
const LAYERS: &[(&str, &[&str])] = &[
    ("position", &[]),
    ("walker", &[]),
    ("check", &[]),
    ("edits", &[]),
    ("query", &["position", "walker", "check", "edits"]),
    // Helpers for dump-types and the tests; nothing in the crate uses them.
    ("dump", &["walker", "check"]),
    ("worker", &["position", "walker", "check", "edits", "query"]),
    (
        "lsp",
        &["position", "walker", "check", "edits", "query", "worker"],
    ),
];

/// Editors and clients that code must not name or assume (lowercase, matched
/// as whole words).
const EDITORS: &[&str] = &[
    "parakeet",
    "intellij",
    "jetbrains",
    "lsp4ij",
    "vs code",
    "vscode",
    "visual studio",
    "neovim",
    "nvim",
    "vim",
    "emacs",
    "eglot",
    "helix",
    "sublime",
    "zed",
    "kakoune",
    "lapce",
    "eclipse",
    "xcode",
];

/// Where editors may be named: documentation, the editor setups, and tests
/// (which drive real editors). README.md and AGENTS.md are documentation
/// too.
const EDITORS_ALLOWED_IN: &[&str] = &["docs/", "editors/", "tests/", "README.md", "AGENTS.md"];

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {}", path.display(), e))
}

/// Every `.rs` file under `dir`, recursively.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();

        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The top-level names after each `crate::` in `source`, outside comments:
/// `walker` for `use crate::walker::Expr`, and `a` and `b` for
/// `use crate::{a::X, b}`.
fn crate_paths(source: &str) -> Vec<String> {
    let code: String = source
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let ident = |s: &str| -> String {
        s.trim_start()
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect()
    };
    let mut names = Vec::new();

    for (at, _) in code.match_indices("crate::") {
        let before = code[..at].chars().next_back();

        if before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$') {
            continue;
        }

        let rest = &code[at + "crate::".len()..];

        if let Some(group) = rest.strip_prefix('{') {
            let mut depth = 0;
            let mut start = 0;

            for (i, c) in group.char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' if depth == 0 => {
                        names.push(ident(&group[start..i]));
                        break;
                    }
                    '}' => depth -= 1,
                    ',' if depth == 0 => {
                        names.push(ident(&group[start..i]));
                        start = i + 1;
                    }
                    _ => {}
                }
            }
        } else {
            names.push(ident(rest));
        }
    }

    names.retain(|n| !n.is_empty());
    names
}

#[test]
fn crate_paths_are_read_from_use_and_inline_paths() {
    let source = "use crate::query::{self, Answer};\n\
                  use crate::{VERSION, check::run, walker::{Expr, Origin}};\n\
                  // use crate::lsp;\n\
                  let x = crate::position::Encoding::Utf8;\n\
                  $crate::ignored; mycrate::ignored;";

    assert_eq!(
        crate_paths(source),
        ["query", "VERSION", "check", "walker", "position"]
    );
}

#[test]
fn modules_use_only_the_layers_below_them() {
    let layers: BTreeMap<_, _> = LAYERS.iter().copied().collect();
    let src = root().join("src");
    let mut files = Vec::new();
    let mut problems = Vec::new();

    rust_files(&src, &mut files);
    files.sort();

    for file in files {
        let relative = file.strip_prefix(&src).unwrap();
        let shown = Path::new("src").join(relative);

        // The binaries sit on top, and lib.rs only declares the modules.
        if relative.starts_with("bin") || relative == Path::new("lib.rs") {
            continue;
        }

        let first = relative.components().next().unwrap().as_os_str();
        let module = Path::new(first).file_stem().unwrap().to_str().unwrap();
        let Some(allowed) = layers.get(module) else {
            problems.push(format!(
                "{}: module `{}` isn't in the LAYERS table in tests/guardrails.rs; add it with the modules it may use",
                shown.display(),
                module
            ));
            continue;
        };

        for name in crate_paths(&read(&file)) {
            if name == module {
                continue;
            }

            if !layers.contains_key(name.as_str()) {
                problems.push(format!(
                    "{}: uses `crate::{}` from the crate root; import it from the module that defines it",
                    shown.display(),
                    name
                ));
            } else if !allowed.contains(&name.as_str()) {
                problems.push(format!(
                    "{}: `{}` uses `{}`, but may only use {:?} (AGENTS.md, Layering)",
                    shown.display(),
                    module,
                    name,
                    allowed
                ));
            }
        }
    }

    // Every module lib.rs declares is in the table, so a new one can't skip
    // it.
    for line in read(&src.join("lib.rs")).lines() {
        let Some(name) = line
            .trim()
            .strip_prefix("pub mod ")
            .or_else(|| line.trim().strip_prefix("mod "))
        else {
            continue;
        };
        let name = name.trim_end_matches(';').trim();

        if !layers.contains_key(name) {
            problems.push(format!(
                "src/lib.rs: module `{}` isn't in the LAYERS table in tests/guardrails.rs",
                name
            ));
        }
    }

    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// Whether `needle` occurs in `haystack` as a whole word.
fn contains_word(haystack: &str, needle: &str) -> bool {
    haystack.match_indices(needle).any(|(at, _)| {
        let word = |c: char| c.is_alphanumeric() || c == '_';
        let before = haystack[..at].chars().next_back();
        let after = haystack[at + needle.len()..].chars().next();

        !before.is_some_and(word) && !after.is_some_and(word)
    })
}

/// The editors named in `text`, ignoring paths into editors/ (such as
/// `/editors/vscode/node_modules` in .gitignore).
fn editors_named(text: &str) -> Vec<&'static str> {
    let text = text.to_lowercase();
    let text: String = text
        .split_inclusive(char::is_whitespace)
        .filter(|w| !w.contains("editors/"))
        .collect();

    EDITORS
        .iter()
        .copied()
        .filter(|name| contains_word(&text, name))
        .collect()
}

#[test]
fn editors_are_matched_as_whole_words() {
    assert_eq!(editors_named("Clients such as VS Code's"), ["vs code"]);
    assert_eq!(editors_named("Bytes, as in Neovim."), ["neovim"]);
    assert_eq!(editors_named("nvim -l check.lua"), ["nvim"]);
    assert!(editors_named("vimrc, recognized, /editors/vscode/node_modules").is_empty());
}

/// The repository's files that git tracks or would track, with their text.
fn repository_files() -> Vec<(String, String)> {
    let output = Command::new("git")
        .args(["ls-files", "--cached", "--others", "--exclude-standard"])
        .current_dir(root())
        .output()
        .expect("git ls-files");

    assert!(output.status.success(), "git ls-files failed");

    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        // A file deleted but not yet staged can't be read.
        .filter_map(|file| {
            let bytes = std::fs::read(root().join(file)).ok()?;

            Some((
                file.to_string(),
                String::from_utf8_lossy(&bytes).into_owned(),
            ))
        })
        .collect()
}

#[test]
fn no_code_names_an_editor_or_client() {
    let mut problems = Vec::new();

    for (file, text) in repository_files() {
        if EDITORS_ALLOWED_IN.iter().any(|p| file.starts_with(p)) {
            continue;
        }

        for (n, line) in text.lines().enumerate() {
            for name in editors_named(line) {
                problems.push(format!(
                    "{}:{}: names {:?}: {}",
                    file,
                    n + 1,
                    name,
                    line.trim()
                ));
            }
        }
    }

    assert!(
        problems.is_empty(),
        "Cuttlebone is a general-purpose LSP server; no code may name or assume an editor or client \
         (only docs/, editors/ and tests/ may):\n{}",
        problems.join("\n")
    );
}

#[test]
fn no_paths_from_one_machine() {
    let mut problems = Vec::new();

    for (file, text) in repository_files() {
        // This file spells out what it looks for.
        if file == "tests/guardrails.rs" {
            continue;
        }

        // Agent instructions are followed literally, so not even `~/`.
        let agents = file == "AGENTS.md" || file.starts_with(".agents/");

        for (n, line) in text.lines().enumerate() {
            if line.contains("/Users/")
                || line.contains("~/Developer")
                || (agents && line.contains("~/"))
            {
                problems.push(format!("{}:{}: {}", file, n + 1, line.trim()));
            }
        }
    }

    assert!(
        problems.is_empty(),
        "use paths relative to the repository; these only work on one machine:\n{}",
        problems.join("\n")
    );
}

/// The `name = "..."` values in a Cargo.toml section, and its keys if
/// `keys` is set.
fn toml_names(toml: &str, section: &str, keys: bool) -> Vec<String> {
    let mut names = Vec::new();
    let mut inside = false;

    for line in toml.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line == section;
            continue;
        }

        if !inside || line.starts_with('#') || !line.contains('=') {
            continue;
        }

        let (key, value) = line.split_once('=').unwrap();

        if keys {
            names.push(key.trim().to_string());
        } else if key.trim() == "name" {
            names.push(value.trim().trim_matches('"').to_string());
        }
    }

    names
}

#[test]
fn nothing_is_named_after_inko() {
    let toml = read(&root().join("Cargo.toml"));
    let mut names = toml_names(&toml, "[package]", false);

    names.extend(toml_names(&toml, "[[bin]]", false));
    names.extend(toml_names(&toml, "[features]", true));

    let package = read(&root().join("editors/vscode/package.json"));

    for key in ["\"name\"", "\"displayName\"", "\"publisher\""] {
        let line = package
            .lines()
            .find(|l| l.trim_start().starts_with(key))
            .unwrap();

        names.push(line.split(':').nth(1).unwrap().trim().to_string());
    }

    for entry in std::fs::read_dir(root().join("editors/neovim/lsp")).unwrap() {
        names.push(entry.unwrap().file_name().to_string_lossy().into_owned());
    }

    assert!(names.contains(&"cuttlebone".to_string()), "{:?}", names);

    let named: Vec<_> = names
        .iter()
        .filter(|n| n.to_lowercase().contains("inko"))
        .collect();

    assert!(
        named.is_empty(),
        "never name anything after Inko (trademark): {:?}",
        named
    );
}

/// Lines added and removed in each file of patches/upstream.patch.
fn patch_counts() -> BTreeMap<String, (usize, usize)> {
    let mut counts = BTreeMap::new();
    let mut file = None;

    for line in read(&root().join("patches/upstream.patch")).lines() {
        if let Some(rest) = line.strip_prefix("diff --git a/") {
            let name = rest.split(" b/").next().unwrap().to_string();

            counts.insert(name.clone(), (0, 0));
            file = Some(name);
        } else if line.starts_with("+++") || line.starts_with("---") {
            continue;
        } else if let Some(f) = &file {
            let entry = counts.get_mut(f).unwrap();

            if line.starts_with('+') {
                entry.0 += 1;
            } else if line.starts_with('-') {
                entry.1 += 1;
            }
        }
    }

    counts
}

/// The lines added and removed in each file, from the table in
/// patches/README.md (`+16 -1`).
fn justified_counts() -> BTreeMap<String, (usize, usize)> {
    let mut counts = BTreeMap::new();

    for line in read(&root().join("patches/README.md")).lines() {
        let cells: Vec<_> = line.split('|').map(str::trim).collect();

        if cells.len() < 4 || !cells[1].contains("/src/") {
            continue;
        }

        let (mut added, mut removed) = (0, 0);

        for part in cells[2].split_whitespace() {
            let number = |s: &str| -> usize {
                s.parse()
                    .unwrap_or_else(|_| panic!("patches/README.md: can't read {:?}", cells[2]))
            };

            match part.split_at(1) {
                ("+", n) => added += number(n),
                ("-", n) => removed += number(n),
                _ => panic!("patches/README.md: can't read {:?}", cells[2]),
            }
        }

        counts.insert(cells[1].to_string(), (added, removed));
    }

    counts
}

#[test]
fn patches_readme_accounts_for_every_line_of_the_patch() {
    let patch = patch_counts();
    let justified = justified_counts();

    assert!(!patch.is_empty());
    assert_eq!(
        patch, justified,
        "patches/README.md's table must list each file in patches/upstream.patch with its \
         lines (as `+added -removed`), and justify each line below it. \
         Left: (added, removed) in the patch. Right: in the README."
    );
}
