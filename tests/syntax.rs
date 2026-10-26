//! Outlines from the syntax alone (`syntax::outline`), against two
//! oracles: the compiler's own declarations (`walker::declarations`), over
//! every module of std and the fixtures, for their names and where they
//! are; and the installed `inko doc`, for doc comments.
use cuttlebone::check::{Target, run};
use cuttlebone::dump::upstream_std;
use cuttlebone::position::Lines;
use cuttlebone::syntax::{self, Kind, Outline};
use cuttlebone::walker::{SymbolId, declarations};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Each declaration's name range, and its kind's name.
fn named(outline: &Outline) -> BTreeSet<(usize, usize, &'static str)> {
    outline
        .all()
        .into_iter()
        .filter(|(d, _)| d.kind != Kind::Implementation)
        .map(|(d, _)| {
            let kind = match d.kind {
                Kind::Type | Kind::Enum => "type",
                Kind::Method | Kind::StaticMethod | Kind::ModuleMethod => "method",
                other => other.name(),
            };

            (d.name_range.start, d.name_range.end, kind)
        })
        .collect()
}

/// What the compiler declares in `root`'s modules (types, traits, methods,
/// fields, constructors and constants) against the outlines of their files.
fn compare(root: &Path, only: &Path) -> usize {
    let mut target = Target::new(root, upstream_std()).unwrap();

    target.tests = false;

    let mismatches = run(&target, false, false, |db, modules| {
        let mut out = Vec::new();

        for module in modules {
            let file = module.module_id.file(db);

            if !file.starts_with(only) {
                continue;
            }

            let text = std::fs::read_to_string(&file).unwrap();
            let lines = Lines::new(&text);
            let compiler: BTreeSet<(usize, usize, &'static str)> = declarations(db, module)
                .into_iter()
                .filter_map(|r| {
                    let kind = match r.symbol {
                        SymbolId::Type(_) => "type",
                        SymbolId::Trait(_) => "trait",
                        SymbolId::Constructor(_) => "constructor",
                        SymbolId::Method(_) => "method",
                        SymbolId::Field(_) => "field",
                        SymbolId::Constant(_) => "constant",
                        _ => return None,
                    };
                    let l = r.location;

                    Some((
                        lines.offset(l.line_start, l.column_start)?,
                        lines.offset(l.line_end, l.column_end + 1)?,
                        kind,
                    ))
                })
                .collect();
            let outline =
                syntax::parse(&text).unwrap_or_else(|| panic!("{} doesn't parse", file.display()));

            let ours = named(&outline);

            for missing in compiler.difference(&ours) {
                out.push(format!(
                    "{}: the outline doesn't have the {} {:?}",
                    file.display(),
                    missing.2,
                    &text[missing.0..missing.1]
                ));
            }

            for extra in ours.difference(&compiler) {
                out.push(format!(
                    "{}: the compiler doesn't declare the {} {:?}",
                    file.display(),
                    extra.2,
                    &text[extra.0..extra.1]
                ));
            }

            out.push(String::new());
        }

        out
    })
    .unwrap()
    .inspected
    .unwrap();
    let checked = mismatches.iter().filter(|m| m.is_empty()).count();
    let problems: Vec<&String> = mismatches.iter().filter(|m| !m.is_empty()).collect();

    assert!(
        problems.is_empty(),
        "{} differences:\n{}",
        problems.len(),
        problems
            .iter()
            .take(30)
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
    checked
}

#[test]
fn outlines_have_what_the_compiler_declares_in_std() {
    let std = upstream_std();
    let checked = compare(std.parent().unwrap(), &std);

    assert!(checked > 100, "only {} modules of std", checked);
}

#[test]
fn outlines_have_what_the_compiler_declares_in_the_fixtures() {
    for name in ["features", "navigation", "members"] {
        let root = crate::common::fixture(name);

        assert!(compare(&root, &root.join("src")) > 0, "{}", name);
    }
}

/// The installed `inko`, which must be the pinned release.
fn inko() -> &'static str {
    let out = Command::new("inko")
        .arg("--version")
        .output()
        .expect("the documentation oracle needs `inko` on PATH");

    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("inko {}", cuttlebone::check::upstream_version()),
        "the documentation oracle needs the installed inko to be the pinned release"
    );
    "inko"
}

/// Copies a directory.
fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();

    for entry in std::fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let dest = to.join(path.file_name().unwrap());

        if path.is_dir() {
            copy(&path, &dest);
        } else {
            std::fs::copy(&path, &dest).unwrap();
        }
    }
}

/// What `inko doc --private --dependencies` writes for a copy of a fixture:
/// every module it documents (the project's and every std module it
/// imports), as JSON. It writes into the project, and creates and locks a
/// directory in the user's data directory, so it runs in a copy, with a
/// home directory of its own.
fn inko_doc(fixture: &str) -> Vec<Value> {
    let dir = crate::common::tmp().join(format!("inko-doc-{}", fixture));
    let home = crate::common::tmp().join("oracle-home");

    let _ = std::fs::remove_dir_all(&dir);
    copy(&crate::common::fixture(fixture), &dir);
    std::fs::create_dir_all(&home).unwrap();

    let out = Command::new(inko())
        .args(["doc", "--private", "--dependencies"])
        .current_dir(&dir)
        .env("HOME", &home)
        .env("XDG_DATA_HOME", home.join("data"))
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    std::fs::read_dir(dir.join("build/docs"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| !p.ends_with("$meta.json"))
        .map(|p| serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap())
        .collect()
}

/// `inko doc`'s documentation, `None` for none.
fn documentation(v: &Value) -> Option<String> {
    Some(v["documentation"].as_str().unwrap().to_string()).filter(|d| !d.is_empty())
}

#[test]
fn doc_comments_are_what_inko_doc_reads() {
    let outlines: std::cell::RefCell<HashMap<PathBuf, Outline>> = Default::default();
    let outline = |file: &str| -> Outline {
        outlines
            .borrow_mut()
            .entry(PathBuf::from(file))
            .or_insert_with(|| syntax::parse(&std::fs::read_to_string(file).unwrap()).unwrap())
            .clone()
    };
    let compared = std::cell::Cell::new(0);
    let wrong = std::cell::RefCell::new(Vec::new());
    let compare = |what: String, expected: Option<String>, ours: Option<Option<String>>| {
        compared.set(compared.get() + 1);

        match ours {
            None => wrong
                .borrow_mut()
                .push(format!("{}: not in the outline", what)),
            Some(ours) if ours != expected => wrong.borrow_mut().push(format!(
                "{}: {:?}, but inko doc reads {:?}",
                what, ours, expected
            )),
            Some(_) => {}
        }
    };

    let modules = inko_doc("navigation");
    // Each trait method's documentation, by name: an implementation without
    // its own is checked against the trait's, if a trait's method of its
    // name has it. Several traits may have a method of the same name, so
    // this takes the one inko doc gives, if any trait has it.
    let mut trait_docs: HashMap<&str, Option<String>> = HashMap::new();

    for module in &modules {
        for t in module["traits"].as_array().unwrap() {
            for m in t["required_methods"]
                .as_array()
                .unwrap()
                .iter()
                .chain(t["default_methods"].as_array().unwrap())
            {
                trait_docs
                    .entry(m["name"].as_str().unwrap())
                    .or_insert_with(|| documentation(m));
            }
        }
    }

    for module in &modules {
        let file = module["file"].as_str().unwrap();
        let top = outline(file);
        let line_of = |text_file: &str, v: &Value| -> u32 {
            let _ = text_file;
            v["location"]["lines"]["start"].as_u64().unwrap() as u32
        };
        // A declaration by its name, in a file, starting on a line (1-based)
        // if that's known.
        let find = |o: &Outline, text: &str, name: &str, line: Option<u32>, kinds: &[Kind]| {
            let lines: Vec<usize> = std::iter::once(0)
                .chain(text.match_indices('\n').map(|(i, _)| i + 1))
                .collect();

            o.all()
                .into_iter()
                .find(|(d, _)| {
                    d.name == name
                        && kinds.contains(&d.kind)
                        && line.is_none_or(|l| {
                            lines.partition_point(|s| *s <= d.range.start) as u32 == l
                        })
                })
                .map(|(d, parents)| {
                    // A trait's method's implementation without a doc
                    // comment of its own has the trait's
                    // (`MethodId::documentation`, types/src/lib.rs:3249-3262),
                    // which hover takes from the type database.
                    let implemented = parents.last().is_some_and(|p| {
                        p.kind == Kind::Implementation && p.name.contains(" for ")
                    });

                    match &d.documentation {
                        None if implemented => trait_docs.get(name).cloned().flatten(),
                        doc => doc.clone(),
                    }
                })
        };
        let text = std::fs::read_to_string(file).unwrap();

        compare(
            format!("module {}", module["name"]),
            documentation(module),
            Some(top.documentation.clone()),
        );

        let methods = |list: &Value| -> Vec<Value> { list.as_array().unwrap().clone() };
        let method = |m: &Value, owner: &str| {
            let f = m["file"].as_str().unwrap();
            let o = outline(f);
            let t = std::fs::read_to_string(f).unwrap();
            let name = m["name"].as_str().unwrap();

            compare(
                format!("{}.{} in {}", owner, name, f),
                documentation(m),
                find(
                    &o,
                    &t,
                    name,
                    Some(line_of(f, m)),
                    &[Kind::Method, Kind::StaticMethod, Kind::ModuleMethod],
                ),
            );
        };

        for m in methods(&module["methods"]) {
            method(&m, "module");
        }

        for c in methods(&module["constants"]) {
            let name = c["name"].as_str().unwrap();

            compare(
                format!("constant {} in {}", name, file),
                documentation(&c),
                find(&top, &text, name, None, &[Kind::Constant]),
            );
        }

        for t in methods(&module["traits"]) {
            let name = t["name"].as_str().unwrap();

            compare(
                format!("trait {} in {}", name, file),
                documentation(&t),
                find(&top, &text, name, None, &[Kind::Trait]),
            );

            for m in methods(&t["required_methods"])
                .into_iter()
                .chain(methods(&t["default_methods"]))
            {
                method(&m, name);
            }
        }

        for t in methods(&module["types"]) {
            let name = t["name"].as_str().unwrap();

            compare(
                format!("type {} in {}", name, file),
                documentation(&t),
                find(&top, &text, name, None, &[Kind::Type, Kind::Enum]),
            );

            for c in methods(&t["constructors"]) {
                let c_name = c["name"].as_str().unwrap();

                compare(
                    format!("{}.{} in {}", name, c_name, file),
                    documentation(&c),
                    find(
                        &top,
                        &text,
                        c_name,
                        Some(line_of(file, &c)),
                        &[Kind::Constructor],
                    ),
                );
            }

            for f in methods(&t["fields"]) {
                let f_name = f["name"].as_str().unwrap();

                // An enum's fields are the compiler's own, at the type.
                if f["location"] == t["location"] {
                    continue;
                }

                compare(
                    format!("{}.@{} in {}", name, f_name, file),
                    documentation(&f),
                    find(&top, &text, f_name, Some(line_of(file, &f)), &[Kind::Field]),
                );
            }

            for m in methods(&t["static_methods"])
                .into_iter()
                .chain(methods(&t["instance_methods"]))
            {
                method(&m, name);
            }
        }
    }

    let (compared, wrong) = (compared.get(), wrong.into_inner());

    assert!(compared > 1000, "only {} compared", compared);
    assert!(
        wrong.is_empty(),
        "{} of {} differ:\n{}",
        wrong.len(),
        compared,
        wrong
            .iter()
            .take(30)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
