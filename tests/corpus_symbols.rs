//! Every declaration document symbols report (`syntax::outline`, other
//! than `impl` blocks) is found by workspace symbols (`index::Index`) at
//! the same place, and definition there goes to itself: in every module of
//! std, and of the fixtures with something to declare. Definition is asked
//! as `query::definition` does: the symbol at the name
//! (`query::symbols_at`), and where the check declares it. Whether each is
//! public and static, as `detail` says, is the type database's.
use cuttlebone::check::{Target, run};
use cuttlebone::dump::upstream_std;
use cuttlebone::index::{self, Index};
use cuttlebone::position::Lines;
use cuttlebone::query::{Index as Declarations, symbols_at};
use cuttlebone::syntax::{self, Kind};
use cuttlebone::walker::{SymbolId, Walker};
use std::collections::HashMap;
use std::path::Path;

/// The words that can come between a method's `fn` and its name.
const MODIFIERS: [&str; 7] = ["pub", "static", "mut", "move", "async", "extern", "inline"];

/// Checks the modules of `root` under `only`, and returns how many
/// declarations it compared.
/// How many declarations were compared, and how many of them were traits.
fn compare(root: &Path, only: &Path) -> (usize, usize) {
    let std = upstream_std().canonicalize().unwrap();
    let mut target = Target::new(root, std.clone()).unwrap();
    let mut index = Index::default();

    target.tests = false;
    index.refresh(&index::roots(&std, &[root.to_path_buf()]), &HashMap::new());

    let ((count, traits), problems) = run(&target, false, false, |db, modules| {
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
        let mut traits = 0;
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
                            .filter_map(|f| Some((f.module.file.clone(), f.declaration?.name_range.start)))
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

                // Whether it's public and static, as a client reads them
                // from `detail` (docs/lsp.md, "What a client that indexes
                // declarations gets"), by the type database.
                let symbol = symbols_at(&walker.references, &walker.declarations, at)
                    .into_iter()
                    .map(|r| r.symbol)
                    .next();
                let detail = d.detail();
                let words: Vec<&str> = detail.split_whitespace().collect();
                let public = words.get(1) == Some(&"pub") || (words.get(..2) == Some(&["type", "builtin"][..]) && d.name != "CheckedIntResult");
                let modifiers = words
                    .iter()
                    .skip(1)
                    .take_while(|w| MODIFIERS.contains(w))
                    .collect::<Vec<_>>();
                let flags = match symbol {
                    Some(SymbolId::Method(id)) => Some((
                        id.is_public(db),
                        (d.kind != Kind::ModuleMethod).then(|| id.is_static(db)),
                    )),
                    Some(SymbolId::Type(id)) => Some((id.is_public(db), None)),
                    Some(SymbolId::Trait(id)) => Some((id.is_public(db), None)),
                    Some(SymbolId::Field(id)) => Some((id.is_public(db), None)),
                    Some(SymbolId::Constant(id)) => Some((id.is_public(db), None)),
                    // As public as its type, the declaration it's in.
                    Some(SymbolId::Constructor(_)) => None,
                    _ => {
                        problems.push(format!("{}: no symbol at its name", what));
                        None
                    }
                };

                if let Some((db_public, db_static)) = flags {
                    if d.public != db_public {
                        problems.push(format!(
                            "{}: public {} by the type database, {} by the syntax",
                            what, db_public, d.public
                        ));
                    }

                    if public != db_public {
                        problems.push(format!(
                            "{}: public {} by the type database, {} by {:?}",
                            what, db_public, public, detail
                        ));
                    }

                    if let Some(db_static) = db_static
                        && db_static != modifiers.contains(&&"static")
                    {
                        problems.push(format!(
                            "{}: static {} by the type database, not by {:?}",
                            what, db_static, detail
                        ));
                    }
                }

                // A trait's required methods are those without a body, by
                // the type database: what document symbols mark as default
                // (`has_body`) is what it records as default.
                if d.kind == Kind::Trait {
                    let id = symbols_at(&walker.references, &walker.declarations, at)
                        .into_iter()
                        .find_map(|r| match r.symbol {
                            SymbolId::Trait(id) => Some(id),
                            _ => None,
                        });
                    let names = |ms: Vec<types::MethodId>| -> Vec<String> {
                        let mut out: Vec<String> = ms.iter().map(|m| m.name(db).clone()).collect();

                        out.sort();
                        out
                    };
                    let split = |body: bool| -> Vec<String> {
                        let mut out: Vec<String> = d
                            .children
                            .iter()
                            .filter(|c| c.has_body() == body)
                            .map(|c| c.name.clone())
                            .collect();

                        out.sort();
                        out
                    };

                    match id {
                        Some(id) => {
                            traits += 1;

                            if names(id.required_methods(db)) != split(false)
                                || names(id.default_methods(db)) != split(true)
                            {
                                problems.push(format!(
                                    "{}: required {:?} and default {:?}, but {:?} and {:?} have bodies or not",
                                    what,
                                    names(id.required_methods(db)),
                                    names(id.default_methods(db)),
                                    split(false),
                                    split(true)
                                ));
                            }
                        }
                        None => problems.push(format!("{}: no trait at its name", what)),
                    }
                }

                if !places.contains(&(file.clone(), d.name_range.start, d.name_range.end)) {
                    problems.push(format!(
                        "{} (line {}): definition goes to {:?}",
                        what, line, places
                    ));
                }
            }
        }

        ((count, traits), problems)
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
    (count, traits)
}

#[test]
fn every_document_symbol_in_std_is_a_workspace_symbol_and_its_own_definition() {
    let std = upstream_std().canonicalize().unwrap();

    let (count, traits) = compare(std.parent().unwrap(), &std);

    assert!(count > 3000, "{}", count);
    assert!(traits > 40, "{}", traits);
}

#[test]
fn every_document_symbol_in_the_fixtures_is_a_workspace_symbol_and_its_own_definition() {
    for name in ["symbols", "features", "navigation", "members"] {
        let root = crate::common::fixture(name);

        assert!(compare(&root, &root.join("src")).0 > 5, "{}", name);
    }
}
