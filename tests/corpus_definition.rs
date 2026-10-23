//! Go to definition over the whole standard library: every name in std/src
//! that refers to a symbol by id (a call, field, variable, type, constant,
//! constructor, module or import) must select that symbol at its own
//! position, and have a declaration of the same id whose name is in the
//! source at the declared location. Allow-listed exceptions are below.
use compiler::hir;
use cuttlebone::check::{Target, run};
use cuttlebone::dump::upstream_std;
use cuttlebone::position::HirPosition;
use cuttlebone::query::{Index, symbol_at};
use cuttlebone::walker::{SymbolId, Walker};
use location::Location;
use std::collections::BTreeMap;
use std::path::Path;
use unicode_segmentation::UnicodeSegmentation;

/// Symbol kinds whose references may lack a declaration, and why.
const ALLOWED: &[(&str, &str)] = &[];

/// The text at a location, which must be on one line.
fn text_at(text: &str, location: &Location) -> String {
    let line = text
        .split('\n')
        .nth(location.line_start as usize - 1)
        .unwrap_or("");

    line.graphemes(true)
        .skip(location.column_start as usize - 1)
        .take((location.column_end + 1).saturating_sub(location.column_start) as usize)
        .collect()
}

#[test]
fn every_reference_in_std_goes_to_a_declaration_of_its_id() {
    let std = upstream_std();
    let root = std.parent().unwrap();
    let mut target = Target::new(root, std.clone()).unwrap();

    target.tests = false;

    let report = run(&target, false, false, |db, modules| {
        let mut index = Index::from_modules(db, modules);
        let mut walked = Vec::new();
        let mut aliases = std::collections::HashSet::new();

        for module in modules {
            let file = module.module_id.file(db);

            if !file.starts_with(&std) {
                continue;
            }

            for e in &module.expressions {
                if let hir::TopLevelExpression::Import(i) = e {
                    for s in &i.symbols {
                        if s.import_as.name != s.name.name {
                            aliases.insert((file.clone(), s.import_as.name.clone()));
                        }
                    }
                }
            }

            let mut walker = Walker::new(db, module.module_id);

            walker.module(module);

            for d in &walker.declarations {
                index.add(&file, d);
            }

            walked.push((file, walker));
        }

        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        let mut missing: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        let mut wrong = Vec::new();
        let texts = |path: &Path| std::fs::read_to_string(path).unwrap();

        for (file, walker) in &walked {
            let text = texts(file);
            let rel = file.strip_prefix(root).unwrap().display().to_string();

            for r in &walker.references {
                let kind = r.symbol.kind();
                let at = HirPosition {
                    line: r.location.line_start,
                    column: r.location.column_start,
                };
                let here = format!(
                    "{}:{}:{} {:?}",
                    rel,
                    r.location.line_start,
                    r.location.column_start,
                    text_at(&text, &r.location)
                );

                *counts.entry(kind).or_default() += 1;

                // The name as written is the symbol's: a field's with its
                // `@`, `Self` for the type it stands for, `+=` for `+`, or
                // the name a symbol is imported as.
                let written = text_at(&text, &r.location);
                let name = r.symbol.name(db);

                if !matches!(r.symbol, SymbolId::Module(_))
                    && written.trim_start_matches('@') != name
                    && written != "Self"
                    && written != format!("{}=", name)
                    && format!("{}=", written) != name
                    && !aliases.contains(&(file.clone(), written.clone()))
                {
                    wrong.push(format!("{}: refers to {} {:?}", here, kind, name));
                }

                let selected = symbol_at(&walker.references, &walker.declarations, at).unwrap();

                if selected.symbol != r.symbol {
                    wrong.push(format!("{}: selects a {}", here, selected.symbol.kind()));
                    continue;
                }

                let Some((decl_file, decl)) = index.declaration(r.symbol) else {
                    missing.entry(kind).or_default().push(here);
                    continue;
                };

                if let SymbolId::Module(_) = r.symbol {
                    continue;
                }

                let name = r.symbol.name(db);
                let declared = text_at(&texts(&decl_file), &decl);

                if declared.trim_start_matches('@') != name {
                    wrong.push(format!(
                        "{}: declared as {:?} at {}:{}:{}, not {:?}",
                        here,
                        declared,
                        decl_file.display(),
                        decl.line_start,
                        decl.column_start,
                        name
                    ));
                }
            }
        }

        (counts, missing, wrong)
    })
    .unwrap();
    let (counts, missing, wrong) = report.inspected.unwrap();
    let total: usize = counts.values().sum();

    assert!(report.ok, "the standard library has errors");
    assert!(total > 50_000, "only {} references: {:?}", total, counts);

    for kind in [
        "module",
        "type",
        "trait",
        "type parameter",
        "constant",
        "method",
        "field",
        "constructor",
        "variable",
    ] {
        assert!(
            counts.get(kind).is_some_and(|n| *n > 500),
            "too few {} references: {:?}",
            kind,
            counts
        );
    }

    let unexpected: Vec<_> = missing
        .iter()
        .filter(|(k, _)| !ALLOWED.iter().any(|(a, _)| a == *k))
        .flat_map(|(k, v)| {
            std::iter::once(format!(
                "{} {} references without a declaration:",
                v.len(),
                k
            ))
            .chain(v.iter().take(10).cloned())
        })
        .collect();

    assert!(unexpected.is_empty(), "{}", unexpected.join("\n"));
    assert!(
        wrong.is_empty(),
        "{} wrong:\n{}",
        wrong.len(),
        wrong
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );

    for (kind, why) in ALLOWED {
        assert!(
            missing.contains_key(kind),
            "allow-list entry {} ({}) is stale",
            kind,
            why
        );
    }
}
