//! Every declaration document symbols report (`syntax::outline`, other
//! than `impl` blocks) is found by workspace symbols (`index::Index`) at
//! the same place, and definition there goes to itself: in every module of
//! std, and of the fixtures with something to declare. Definition is asked
//! as `query::definition` does: the symbol at the name
//! (`query::symbols_at`), and where the check declares it.
use cuttlebone::check::{Target, run};
use cuttlebone::dump::upstream_std;
use cuttlebone::index::{self, Index};
use cuttlebone::position::Lines;
use cuttlebone::query::{Index as Declarations, symbols_at};
use cuttlebone::syntax::{self, Kind};
use cuttlebone::walker::Walker;
use std::collections::HashMap;
use std::path::Path;

/// Checks the modules of `root` under `only`, and returns how many
/// declarations it compared.
fn compare(root: &Path, only: &Path) -> usize {
    let std = upstream_std().canonicalize().unwrap();
    let mut target = Target::new(root, std.clone()).unwrap();
    let mut index = Index::default();

    target.tests = false;
    index.refresh(&index::roots(&std, &[root.to_path_buf()]), &HashMap::new());

    let (count, problems) = run(&target, false, false, |db, modules| {
        let mut declared = Declarations::from_modules(db, modules);
        let mut walked = Vec::new();

        for module in modules {
            let file = module.module_id.file(db);
            let mut walker = Walker::new(db, module.module_id);

            walker.module(module);

            for d in &walker.declarations {
                declared.add(&file, d);
            }

            if file.starts_with(only) {
                walked.push((file, walker));
            }
        }

        let mut count = 0;
        let mut problems = Vec::new();
        // What a search for each name finds: each place, once.
        let mut searched: HashMap<String, Vec<(std::path::PathBuf, usize)>> = HashMap::new();

        for (file, walker) in &walked {
            let text = std::fs::read_to_string(file).unwrap();
            let lines = Lines::new(&text);
            let outline = syntax::parse(&text).unwrap();

            for (d, _) in outline.all() {
                if d.kind == Kind::Implementation {
                    continue;
                }

                count += 1;

                let what = format!("{}: the {} {}", file.display(), d.kind.name(), d.name);
                let found = searched
                    .entry(d.name.clone())
                    .or_insert_with(|| {
                        index
                            .search(&d.name, usize::MAX)
                            .iter()
                            .map(|f| (f.module.file.clone(), f.declaration.name_range.start))
                            .collect()
                    })
                    .contains(&(file.clone(), d.name_range.start));

                if !found {
                    problems.push(format!("{}: not a workspace symbol", what));
                }

                let line = text[..d.name_range.start].matches('\n').count() as u32 + 1;
                let at = lines
                    .to_hir(cuttlebone::position::position_at(
                        &text,
                        d.name_range.start,
                        cuttlebone::position::Encoding::Utf16,
                    ))
                    .unwrap();
                let places: Vec<(std::path::PathBuf, usize, usize)> =
                    symbols_at(&walker.references, &walker.declarations, at)
                        .into_iter()
                        .filter_map(|r| declared.declaration(r.symbol))
                        .filter_map(|(f, l)| {
                            let t = std::fs::read_to_string(&f).ok()?;
                            let l2 = Lines::new(&t);

                            Some((
                                f,
                                l2.offset(l.line_start, l.column_start)?,
                                l2.offset(l.line_end, l.column_end + 1)?,
                            ))
                        })
                        .collect();

                if !places.contains(&(file.clone(), d.name_range.start, d.name_range.end)) {
                    problems.push(format!(
                        "{} (line {}): definition goes to {:?}",
                        what, line, places
                    ));
                }
            }
        }

        (count, problems)
    })
    .unwrap()
    .inspected
    .unwrap();

    assert!(
        problems.is_empty(),
        "{} of {} declarations:\n{}",
        problems.len(),
        count,
        problems
            .iter()
            .take(30)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    count
}

#[test]
fn every_document_symbol_in_std_is_a_workspace_symbol_and_its_own_definition() {
    let std = upstream_std().canonicalize().unwrap();

    assert!(compare(std.parent().unwrap(), &std) > 3000);
}

#[test]
fn every_document_symbol_in_the_fixtures_is_a_workspace_symbol_and_its_own_definition() {
    for name in ["symbols", "features", "navigation", "members"] {
        let root = crate::common::fixture(name);

        assert!(compare(&root, &root.join("src")) > 5, "{}", name);
    }
}
