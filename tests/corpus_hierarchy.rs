//! Implementations and the type and call hierarchies over the whole
//! standard library, against the compiler's own data: every implementation
//! the type database records (`TypeId::trait_implementation`,
//! `MethodSource::Implemented`) is found from the trait's (or the trait's
//! method's) declaration, and nothing else; every type's supertypes are
//! the traits it implements, and every trait's its required traits, and
//! its subtypes the traits requiring it and the types implementing it;
//! and every method's outgoing calls are its callees' incoming calls.
use cuttlebone::check::{Target, run};
use cuttlebone::dump::upstream_std;
use cuttlebone::position::{Encoding, Lines, Position};
use cuttlebone::query::{Cache, Index, Question, Reply, Request, Session, ask_with};
use cuttlebone::walker::SymbolId;
use location::Location;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use types::{MethodId, MethodSource, TraitId, TypeEnum, TypeId};

/// A declaration's name: its file, and where it starts.
type Spot = (PathBuf, Position);

/// What the type database says, by each declaration's name.
#[derive(Default)]
struct Expected {
    /// A trait's or a trait's method's implementations.
    implementations: BTreeMap<Spot, BTreeSet<Spot>>,
    supertypes: BTreeMap<Spot, BTreeSet<Spot>>,
    subtypes: BTreeMap<Spot, BTreeSet<Spot>>,
    /// Every method with a body, in std's modules.
    methods: BTreeSet<Spot>,
    /// Implementations whose type or method has no declaration to go to.
    undeclared: usize,
}

fn spot(texts: &mut BTreeMap<PathBuf, String>, file: &Path, location: &Location) -> Spot {
    let text = texts
        .entry(file.to_path_buf())
        .or_insert_with(|| std::fs::read_to_string(file).unwrap());
    let (start, _) = Lines::with_encoding(text, Encoding::Utf16).range(location);

    (file.to_path_buf(), start)
}

fn expected(std: &Path) -> Expected {
    let root = std.parent().unwrap();
    let target = Target::new(root, std.to_path_buf()).unwrap();
    let report = run(&target, false, false, |db, modules| {
        let index = Index::from_modules(db, modules);
        let mut texts = BTreeMap::new();
        let mut out = Expected::default();
        let mut declared = |symbol: SymbolId| {
            index
                .declaration(symbol)
                .map(|(file, location)| spot(&mut texts, &file, &location))
        };

        for t in (0..db.number_of_traits()).map(|i| TraitId(i as u32)) {
            let Some(trait_spot) = declared(SymbolId::Trait(t)) else {
                continue;
            };

            out.implementations.entry(trait_spot.clone()).or_default();
            out.subtypes.entry(trait_spot.clone()).or_default();
            out.supertypes.entry(trait_spot.clone()).or_default();

            for r in t.required_traits(db) {
                if let Some(required) = declared(SymbolId::Trait(r.instance_of())) {
                    out.supertypes
                        .get_mut(&trait_spot)
                        .unwrap()
                        .insert(required.clone());
                    out.subtypes
                        .entry(required)
                        .or_default()
                        .insert(trait_spot.clone());
                }
            }
        }

        for ty in (0..db.number_of_types()).map(|i| TypeId(i as u32)) {
            let type_spot = declared(SymbolId::Type(ty));

            if let Some(s) = &type_spot {
                out.supertypes.entry(s.clone()).or_default();
                out.subtypes.entry(s.clone()).or_default();
            }

            for t in (0..db.number_of_traits()).map(|i| TraitId(i as u32)) {
                if ty.trait_implementation(db, t).is_none()
                    || ty.specialization_source(db).is_some()
                {
                    continue;
                }

                let (Some(trait_spot), Some(type_spot)) =
                    (declared(SymbolId::Trait(t)), type_spot.clone())
                else {
                    out.undeclared += 1;
                    continue;
                };

                out.implementations
                    .entry(trait_spot.clone())
                    .or_default()
                    .insert(type_spot.clone());
                out.subtypes
                    .entry(trait_spot.clone())
                    .or_default()
                    .insert(type_spot.clone());
                out.supertypes
                    .entry(type_spot)
                    .or_default()
                    .insert(trait_spot);
            }
        }

        for m in (0..db.number_of_methods()).map(|i| MethodId(i as u32)) {
            if let MethodSource::Implemented(_, original) = m.source(db) {
                let (Some(of), Some(by)) = (
                    declared(SymbolId::Method(original)),
                    declared(SymbolId::Method(m)),
                ) else {
                    out.undeclared += 1;
                    continue;
                };

                out.implementations.entry(of).or_default().insert(by);
            } else if matches!(
                m.receiver(db).as_type_enum(db),
                Ok(TypeEnum::TraitInstance(_))
            ) && let Some(of) = declared(SymbolId::Method(m))
            {
                out.implementations.entry(of).or_default();
            }
        }

        // Every method with a body, from the HIR.
        for module in modules {
            let file = module.module_id.file(db);

            if !file.starts_with(std) {
                continue;
            }

            let mut walker = cuttlebone::walker::Walker::new(db, module.module_id);

            walker.module(module);

            for (_, id) in &walker.methods {
                if let Some(s) = declared(SymbolId::Method(*id)) {
                    out.methods.insert(s);
                }
            }
        }

        out
    });

    report.unwrap().inspected.unwrap()
}

/// Asks `question` at every spot, a file at a time, with one kept check of
/// the whole of std (`Cache`).
fn answers<'a>(
    session: &Session,
    cache: &Cache,
    question: Question,
    spots: impl Iterator<Item = &'a Spot>,
) -> BTreeMap<Spot, Option<Reply>> {
    let mut by_file: BTreeMap<PathBuf, Vec<Position>> = BTreeMap::new();

    for (file, position) in spots {
        by_file.entry(file.clone()).or_default().push(*position);
    }

    let mut out = BTreeMap::new();

    for (file, positions) in by_file {
        let request = Request {
            file: file.clone(),
            position: positions[0],
            encoding: Encoding::Utf16,
            contents: Default::default(),
        };
        let questions: Vec<(Question, Position)> =
            positions.iter().map(|p| (question, *p)).collect();
        let found = ask_with(cache, session, &request, None, false, &questions);

        for (position, answer) in positions.into_iter().zip(found) {
            let result = answer
                .result
                .unwrap_or_else(|e| panic!("{}:{:?}: {:?}", file.display(), position, e));

            out.insert((file.clone(), position), result);
        }
    }

    out
}

fn places(reply: &Option<Reply>) -> BTreeSet<Spot> {
    match reply {
        Some(Reply::Places(places)) => places
            .iter()
            .map(|p| (p.file.clone(), p.range.start))
            .collect(),
        Some(Reply::Items(items)) => items
            .iter()
            .map(|i| (i.selection.file.clone(), i.selection.range.start))
            .collect(),
        None => BTreeSet::new(),
        other => panic!("{:?}", other),
    }
}

fn compare(
    what: &str,
    expected: &BTreeMap<Spot, BTreeSet<Spot>>,
    got: &BTreeMap<Spot, Option<Reply>>,
) {
    let mut wrong = Vec::new();

    for (at, want) in expected {
        let found = places(&got[at]);

        if &found != want {
            wrong.push(format!(
                "{}:{:?}: missing {:?}, extra {:?}",
                at.0.display(),
                at.1,
                want.difference(&found).collect::<Vec<_>>(),
                found.difference(want).collect::<Vec<_>>()
            ));
        }
    }

    assert!(
        wrong.is_empty(),
        "{} {} of {} wrong:\n{}",
        what,
        wrong.len(),
        expected.len(),
        wrong.join("\n")
    );
}

fn session(std: &Path) -> Session {
    Session {
        std: std.to_path_buf(),
        projects: vec![std.parent().unwrap().to_path_buf()],
    }
}

#[test]
fn every_implementation_the_type_database_records_is_found() {
    let std = upstream_std();
    let expected = expected(&std);
    let session = session(&std);
    let cache = Cache::new(4);
    let got = answers(
        &session,
        &cache,
        Question::Implementation,
        expected.implementations.keys(),
    );
    let found: usize = expected.implementations.values().map(BTreeSet::len).sum();

    assert_eq!(expected.undeclared, 0);
    assert!(
        expected.implementations.len() >= 180 && found >= 1697,
        "{} traits and methods, {} implementations",
        expected.implementations.len(),
        found
    );
    compare("implementations", &expected.implementations, &got);
}

#[test]
fn every_types_and_traits_supertypes_and_subtypes_are_the_databases() {
    let std = upstream_std();
    let expected = expected(&std);
    let session = session(&std);
    let cache = Cache::new(4);
    let supertypes = answers(
        &session,
        &cache,
        Question::Supertypes,
        expected.supertypes.keys(),
    );
    let subtypes = answers(
        &session,
        &cache,
        Question::Subtypes,
        expected.subtypes.keys(),
    );

    assert!(
        expected.supertypes.len() > 400,
        "{}",
        expected.supertypes.len()
    );
    compare("supertypes", &expected.supertypes, &supertypes);
    compare("subtypes", &expected.subtypes, &subtypes);
}

/// Every method's outgoing calls are exactly the incoming calls the
/// methods it calls have from it, ranges and all, for a fifth of std's
/// methods (each incoming call looks through every module).
#[test]
fn outgoing_calls_are_their_callees_incoming_calls() {
    let std = upstream_std();
    let expected = expected(&std);
    let session = session(&std);
    let cache = Cache::new(4);
    let sample: Vec<&Spot> = expected.methods.iter().step_by(5).collect();
    let outgoing = answers(
        &session,
        &cache,
        Question::OutgoingCalls,
        sample.iter().copied(),
    );
    // caller -> callee -> ranges, from both directions.
    let mut from_outgoing: BTreeSet<(Spot, Spot, Vec<Position>)> = BTreeSet::new();
    let mut callees: BTreeSet<Spot> = BTreeSet::new();

    for (caller, reply) in &outgoing {
        let Some(Reply::Calls(calls)) = reply else {
            panic!("{:?}: {:?}", caller, reply);
        };

        for call in calls {
            let callee = (
                call.item.selection.file.clone(),
                call.item.selection.range.start,
            );

            if callee.0.starts_with(&std) {
                callees.insert(callee.clone());
            }

            from_outgoing.insert((
                caller.clone(),
                callee,
                call.ranges.iter().map(|p| p.range.start).collect(),
            ));
        }
    }

    let incoming = answers(&session, &cache, Question::IncomingCalls, callees.iter());
    let asked: BTreeSet<&Spot> = sample.iter().copied().collect();
    let mut from_incoming: BTreeSet<(Spot, Spot, Vec<Position>)> = BTreeSet::new();

    for (callee, reply) in &incoming {
        let Some(Reply::Calls(calls)) = reply else {
            panic!("{:?}: {:?}", callee, reply);
        };

        for call in calls {
            let caller = (
                call.item.selection.file.clone(),
                call.item.selection.range.start,
            );

            if asked.contains(&caller) {
                from_incoming.insert((
                    caller,
                    callee.clone(),
                    call.ranges.iter().map(|p| p.range.start).collect(),
                ));
            }
        }
    }

    let from_outgoing: BTreeSet<_> = from_outgoing
        .into_iter()
        .filter(|(_, callee, _)| callees.contains(callee))
        .collect();

    assert!(from_outgoing.len() >= 1370, "{}", from_outgoing.len());
    assert_eq!(
        from_outgoing
            .difference(&from_incoming)
            .take(10)
            .collect::<Vec<_>>(),
        Vec::<&(Spot, Spot, Vec<Position>)>::new(),
        "outgoing without incoming"
    );
    assert_eq!(
        from_incoming
            .difference(&from_outgoing)
            .take(10)
            .collect::<Vec<_>>(),
        Vec::<&(Spot, Spot, Vec<Position>)>::new(),
        "incoming without outgoing"
    );
}

/// A method's call hierarchy item, taken from the type database without
/// parsing its file, has the whole declaration the syntax has, and the
/// type or trait it's in, as hover's `Defined in` does: every method with
/// a body in std.
#[test]
fn every_methods_item_is_its_declaration_as_the_syntax_has_it() {
    let std = upstream_std();
    let expected = expected(&std);
    let session = session(&std);
    let cache = Cache::new(4);
    let items = answers(
        &session,
        &cache,
        Question::CallItem,
        expected.methods.iter(),
    );
    let mut wrong = Vec::new();

    for ((file, position), reply) in &items {
        let Some(Reply::Items(items)) = reply else {
            panic!("{}:{:?}: {:?}", file.display(), position, reply);
        };
        let text = std::fs::read_to_string(file).unwrap();
        let outline = cuttlebone::syntax::parse(&text).unwrap();
        let offset = cuttlebone::position::byte_offset(&text, *position, Encoding::Utf16).unwrap();
        let (d, parents) = outline.named_at(offset).unwrap();
        let start = cuttlebone::position::position_at(&text, d.range.start, Encoding::Utf16);
        let end = cuttlebone::position::position_at(&text, d.range.end, Encoding::Utf16);
        let owner: Vec<&str> = parents.iter().map(|p| p.owner()).collect();
        let item = &items[0];

        if (item.place.range.start, item.place.range.end) != (start, end)
            || !item.detail.ends_with(&owner.join("."))
        {
            wrong.push(format!(
                "{}:{:?}: {:?} {} against {:?} {:?}",
                file.display(),
                position,
                item.place.range,
                item.detail,
                (start, end),
                owner
            ));
        }
    }

    assert!(items.len() >= 2963, "{}", items.len());
    assert!(
        wrong.is_empty(),
        "{} of {}:\n{}",
        wrong.len(),
        items.len(),
        wrong[..wrong.len().min(10)].join("\n")
    );
}
