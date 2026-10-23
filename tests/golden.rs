//! Golden-file tests: dump-types' output for a few modules must match
//! tests/golden/*.txt. Run with UPDATE_GOLDEN=1 to rewrite the files, then
//! review the diff.
use cuttlebone::{Target, check, format_expr, upstream_std};
use std::path::{Path, PathBuf};

fn golden(name: &str, file: PathBuf) {
    let target = Target::new(&file, upstream_std()).unwrap();
    let checked = check(&target).unwrap();

    assert!(checked.ok, "{} has errors", file.display());

    let mut actual = String::new();

    for e in &checked.expressions {
        actual.push_str(&format_expr(e, &target.root));
        actual.push('\n');
    }

    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{}.txt", name));

    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &actual).unwrap();
        return;
    }

    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("{}: {} (run with UPDATE_GOLDEN=1)", path.display(), e)
    });

    if actual != expected {
        let diff: Vec<_> = expected
            .lines()
            .zip(actual.lines())
            .filter(|(a, b)| a != b)
            .take(10)
            .map(|(a, b)| format!("- {}\n+ {}", a, b))
            .collect();

        panic!(
            "{} differs ({} expected lines, {} actual); first differences:\n{}",
            path.display(),
            expected.lines().count(),
            actual.lines().count(),
            diff.join("\n")
        );
    }
}

fn std_module(path: &str) -> PathBuf {
    upstream_std().join("std").join(path)
}

#[test]
fn std_cmp() {
    golden("std-cmp", std_module("cmp.inko"));
}

#[test]
fn std_option() {
    golden("std-option", std_module("option.inko"));
}

#[test]
fn std_iter() {
    golden("std-iter", std_module("iter.inko"));
}

#[test]
fn std_fs_path() {
    golden("std-fs-path", std_module("fs/path.inko"));
}

#[test]
fn fixture_edges() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/edges/src/main.inko");

    golden("fixture-edges", file);
}
