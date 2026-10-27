//! Checks kept between questions (`query::Cache`, docs/lsp.md "Kept
//! checks"): which questions they answer, that their answers are the ones
//! a new check gives, and that nothing changed since (an edit, a file on
//! disk, a file created or deleted) is ever answered from one.

use crate::common::client::{Client, Launch, at};
use cuttlebone::check::{Fingerprint, Target};
use cuttlebone::dump::upstream_std;
use cuttlebone::edits::{Changes, Fallback};
use cuttlebone::position::Encoding;
use cuttlebone::query::{self, Answer, Cache, Question, Reply, Request, Session, Source};
use cuttlebone::worker::NO_CACHE_VARIABLE;
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const MAIN: &str = "import lib

type async Main {
  fn async main {
    let first = lib.size
    let second = lib.size + 1
    let third = [first, second]
  }

  fn other -> Int {
    lib.size
  }
}
";

const LIB: &str = "fn pub size -> Int {
  42
}
";

/// A project in the tests' temporary directory, with `files` (relative
/// paths and text).
fn project(name: &str, files: &[(&str, &str)]) -> (PathBuf, Session) {
    let root = crate::common::tmp().join(format!("cache-{}", name));
    let _ = std::fs::remove_dir_all(&root);

    for (path, text) in files {
        let path = root.join(path);

        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    let root = root.canonicalize().unwrap();
    let session = Session {
        std: upstream_std(),
        projects: vec![root.clone()],
    };

    (root, session)
}

fn lib_project(name: &str) -> (PathBuf, Session) {
    project(name, &[("src/main.inko", MAIN), ("src/lib.inko", LIB)])
}

/// Asks one question about `file`, whose text is `text`, at `needle` (plus
/// `offset` characters).
fn ask(
    cache: &Cache,
    session: &Session,
    file: &Path,
    text: &str,
    question: Question,
    needle: &str,
    offset: usize,
) -> Answer<Reply> {
    ask_with(
        cache, session, file, text, None, true, question, needle, offset,
    )
}

#[allow(clippy::too_many_arguments)]
fn ask_with(
    cache: &Cache,
    session: &Session,
    file: &Path,
    text: &str,
    fallback: Option<&Fallback>,
    repair: bool,
    question: Question,
    needle: &str,
    offset: usize,
) -> Answer<Reply> {
    let (line, character) = at(text, needle, offset, false);
    let position = cuttlebone::position::Position { line, character };
    let request = Request {
        file: file.to_path_buf(),
        position,
        encoding: Encoding::Utf16,
        contents: HashMap::from([(file.to_path_buf(), text.to_string())]),
    };

    query::ask_with(
        cache,
        session,
        &request,
        fallback,
        repair,
        &[(question, position)],
    )
    .pop()
    .unwrap()
}

fn hover_type(answer: &Answer<Reply>) -> String {
    match &answer.result {
        Ok(Some(Reply::Type(t))) => t.typ.clone(),
        other => panic!("no type: {:?}", other),
    }
}

/// The answer a new check gives: no cache.
fn fresh(
    session: &Session,
    file: &Path,
    text: &str,
    question: Question,
    needle: &str,
    offset: usize,
) -> Answer<Reply> {
    ask(
        &Cache::new(0),
        session,
        file,
        text,
        question,
        needle,
        offset,
    )
}

#[test]
fn questions_about_the_same_text_share_one_check() {
    let (root, session) = lib_project("same-text");
    let main = root.join("src/main.inko");
    let cache = Cache::new(4);
    let questions = [
        (Question::Type, "first", 0),
        (Question::Type, "lib.size + 1", 4),
        (Question::Definition, "lib.size", 4),
        (Question::TypeDefinition, "third", 0),
        (Question::Highlight, "first", 0),
        (
            Question::InlayHints {
                end: cuttlebone::position::Position {
                    line: 20,
                    character: 0,
                },
            },
            "let first",
            0,
        ),
    ];

    for (i, (question, needle, offset)) in questions.iter().enumerate() {
        let answer = ask(&cache, &session, &main, MAIN, *question, needle, *offset);
        let new = fresh(&session, &main, MAIN, *question, needle, *offset);

        assert_eq!(answer.cached, i > 0, "{:?}", question);
        assert!(!new.cached);
        assert_eq!(answer.result, new.result, "{:?}", question);
        assert_eq!(answer.source, new.source);
        assert_eq!(answer.typed, new.typed);
    }

    assert_eq!(cache.len(), 1);
    // References check the whole project: a check of their own.
    let answer = ask(
        &cache,
        &session,
        &main,
        MAIN,
        Question::References {
            include_declaration: true,
        },
        "lib.size",
        4,
    );

    assert!(!answer.cached);
    assert_eq!(cache.len(), 2);
}

#[test]
fn an_edit_is_a_different_check() {
    let (root, session) = lib_project("edit");
    let main = root.join("src/main.inko");
    let cache = Cache::new(4);
    let edited = MAIN.replace("let first = lib.size", "let first = 'text'");

    assert_eq!(
        hover_type(&ask(
            &cache,
            &session,
            &main,
            MAIN,
            Question::Type,
            "first",
            0
        )),
        "Int"
    );

    let answer = ask(&cache, &session, &main, &edited, Question::Type, "first", 0);

    assert!(!answer.cached);
    assert_eq!(hover_type(&answer), "String");

    // Undoing the edit gives the first text again, whose check is kept.
    let answer = ask(&cache, &session, &main, MAIN, Question::Type, "first", 0);

    assert!(answer.cached);
    assert_eq!(hover_type(&answer), "Int");
}

/// An edit outside the editor: a module the open file imports, which isn't
/// open itself, changes on disk, to text of the same length.
#[test]
fn a_file_changed_on_disk_is_never_answered_from_a_kept_check() {
    let (root, session) = lib_project("disk");
    let main = root.join("src/main.inko");
    let lib = root.join("src/lib.inko");
    let cache = Cache::new(4);
    let changed = "fn pub size -> Float {\n1.0}\n";

    assert_eq!(changed.len(), LIB.len());
    assert_eq!(
        hover_type(&ask(
            &cache,
            &session,
            &main,
            MAIN,
            Question::Type,
            "first",
            0
        )),
        "Int"
    );
    assert!(ask(&cache, &session, &main, MAIN, Question::Type, "first", 0).cached);
    std::fs::write(&lib, changed).unwrap();

    let answer = ask(&cache, &session, &main, MAIN, Question::Type, "first", 0);

    assert!(!answer.cached);
    assert_eq!(hover_type(&answer), "Float");
    assert!(ask(&cache, &session, &main, MAIN, Question::Type, "first", 0).cached);

    // Deleting it: the import no longer resolves.
    std::fs::remove_file(&lib).unwrap();

    let answer = ask(&cache, &session, &main, MAIN, Question::Type, "first", 0);

    assert!(!answer.cached);
    assert_eq!(answer.result.unwrap_err().code, "noHir");
}

/// A file created in the project is a module the next check of the whole
/// project finds, which a kept one never had.
#[test]
fn a_file_created_on_disk_is_never_answered_from_a_kept_check() {
    let (root, session) = lib_project("created");
    let main = root.join("src/main.inko");
    let cache = Cache::new(4);
    let references = Question::References {
        include_declaration: false,
    };
    let count = |answer: &Answer<Reply>| match &answer.result {
        Ok(Some(Reply::Places(places))) => places.len(),
        other => panic!("no references: {:?}", other),
    };

    assert_eq!(
        count(&ask(
            &cache, &session, &main, MAIN, references, "lib.size", 4
        )),
        3
    );
    std::fs::create_dir_all(root.join("src/more")).unwrap();
    std::fs::write(
        root.join("src/more/user.inko"),
        "import lib\n\nfn pub twice -> Int {\n  lib.size * 2\n}\n",
    )
    .unwrap();

    let answer = ask(&cache, &session, &main, MAIN, references, "lib.size", 4);

    assert!(!answer.cached);
    assert_eq!(count(&answer), 4);
}

#[test]
fn the_cache_keeps_at_most_its_capacity_and_drops_the_least_recently_used() {
    let (root, session) = lib_project("capacity");
    let main = root.join("src/main.inko");
    let cache = Cache::new(2);
    let texts: Vec<String> = (0..3)
        .map(|i| MAIN.replace("+ 1", &format!("+ {}", i)))
        .collect();

    for text in &texts {
        assert!(!ask(&cache, &session, &main, text, Question::Type, "first", 0).cached);
    }

    // 1 and 2 are kept. Using 1 makes 2 the least recently used, which a
    // new check of 0 pushes out.
    assert_eq!(cache.len(), 2);
    assert!(
        ask(
            &cache,
            &session,
            &main,
            &texts[1],
            Question::Type,
            "first",
            0
        )
        .cached
    );
    assert!(
        !ask(
            &cache,
            &session,
            &main,
            &texts[0],
            Question::Type,
            "first",
            0
        )
        .cached
    );
    assert_eq!(cache.len(), 2);
    assert!(
        ask(
            &cache,
            &session,
            &main,
            &texts[1],
            Question::Type,
            "first",
            0
        )
        .cached
    );
    assert!(
        !ask(
            &cache,
            &session,
            &main,
            &texts[2],
            Question::Type,
            "first",
            0
        )
        .cached
    );

    let none = Cache::new(0);

    assert!(!ask(&none, &session, &main, MAIN, Question::Type, "first", 0).cached);
    assert!(!ask(&none, &session, &main, MAIN, Question::Type, "first", 0).cached);
    assert!(none.is_empty());
}

/// Completion checks a copy with its placeholder, which is asked about
/// once, and would push out a check that answers more.
#[test]
fn completions_copies_are_not_kept() {
    let (root, session) = lib_project("completion");
    let main = root.join("src/main.inko");
    let cache = Cache::new(1);

    ask(&cache, &session, &main, MAIN, Question::Type, "first", 0);

    let answer = ask(
        &cache,
        &session,
        &main,
        MAIN,
        Question::Complete,
        "size + 1",
        0,
    );

    assert!(matches!(answer.result, Ok(Some(Reply::Members(_)))));
    assert!(ask(&cache, &session, &main, MAIN, Question::Type, "first", 0).cached);
}

/// Code that doesn't parse: answers from a repaired copy, and from the last
/// good check, come from kept checks too, with the same source.
#[test]
fn repaired_and_stale_answers_come_from_kept_checks() {
    let (root, session) = lib_project("broken");
    let main = root.join("src/main.inko");
    let broken = MAIN.replace("    lib.size\n", "    lib.size(\n");
    let fallback = Fallback {
        contents: HashMap::from([(main.clone(), MAIN.to_string())]),
        changes: HashMap::from([(main.clone(), Changes::between(MAIN, &broken))]),
        ends: Default::default(),
    };

    for (repair, source) in [(true, Source::Repaired), (false, Source::Stale)] {
        let cache = Cache::new(4);
        let answers: Vec<Answer<Reply>> = [("first", 0), ("lib.size + 1", 4), ("third", 0)]
            .iter()
            .map(|(needle, offset)| {
                let answer = ask_with(
                    &cache,
                    &session,
                    &main,
                    &broken,
                    Some(&fallback),
                    repair,
                    Question::Type,
                    needle,
                    *offset,
                );
                let new = ask_with(
                    &Cache::new(0),
                    &session,
                    &main,
                    &broken,
                    Some(&fallback),
                    repair,
                    Question::Type,
                    needle,
                    *offset,
                );

                assert_eq!(answer.source, source);
                assert_eq!(answer.result, new.result);
                answer
            })
            .collect();

        assert!(!answers[0].cached);
        assert!(answers[1].cached && answers[2].cached);
        assert_eq!(hover_type(&answers[1]), "Int");

        // The documents' own check, which has nothing to answer from, is
        // kept apart, so `cached` must come from the check that answers:
        // here a new one, after a check of other text pushed it out.
        let small = Cache::new(1);
        let asked = |cache: &Cache| {
            ask_with(
                cache,
                &session,
                &main,
                &broken,
                Some(&fallback),
                repair,
                Question::Type,
                "first",
                0,
            )
        };

        let other = MAIN.replace("+ 1", "+ 2");

        asked(&small);
        ask(&small, &session, &main, &other, Question::Type, "first", 0);

        let again = asked(&small);

        assert_eq!(again.source, source);
        assert!(!again.cached, "{:?}", source);
    }
}

/// A file written while a check runs may have been read before the write,
/// so a check is kept only if everything it read last changed before it
/// started.
#[test]
fn a_check_that_read_a_file_changed_while_it_ran_is_not_kept() {
    let (root, _) = lib_project("settled");
    let main = root.join("src/main.inko");
    let lib = root.join("src/lib.inko");
    let target = Target::new(&main, upstream_std()).unwrap();
    let before = SystemTime::now() - Duration::from_secs(10);
    let after = SystemTime::now() + Duration::from_millis(1);

    assert!(Fingerprint::take(&target, [lib.clone()], after).settled());
    assert!(!Fingerprint::take(&target, [lib.clone()], before).settled());
    // A directory counts too: a file was created in it.
    assert!(!Fingerprint::take(&target, [], before).settled());

    let fingerprint = Fingerprint::take(&target, [lib.clone()], after);

    assert!(fingerprint.current());
    std::fs::write(&lib, LIB).unwrap();
    assert!(!fingerprint.current());

    // A tool that sets the modification time back still changes the
    // inode's change time, which nothing can set.
    let file = std::fs::File::options().write(true).open(&lib).unwrap();
    let old = SystemTime::now() - Duration::from_secs(3600);

    file.set_modified(old).unwrap();

    let started = SystemTime::now() - Duration::from_secs(10);
    let fingerprint = Fingerprint::take(&target, [lib.clone()], started);

    assert!(!fingerprint.settled());
    std::fs::write(&lib, LIB.replace("42", "24")).unwrap();
    file.set_modified(old).unwrap();
    assert!(!fingerprint.current());
}

/// A file whose times say it changed after the check started (here, a
/// modification time in the future) may have changed while the check
/// read it, so the check isn't kept.
#[test]
fn a_check_of_a_file_that_may_have_changed_during_it_is_not_kept() {
    let (root, session) = lib_project("future");
    let main = root.join("src/main.inko");
    let cache = Cache::new(4);

    std::fs::File::options()
        .write(true)
        .open(root.join("src/lib.inko"))
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(3600))
        .unwrap();

    assert!(!ask(&cache, &session, &main, MAIN, Question::Type, "first", 0).cached);
    assert!(!ask(&cache, &session, &main, MAIN, Question::Type, "first", 0).cached);
    assert!(cache.is_empty());
}

/// A check the compiler panicked in has nothing to keep.
#[test]
fn a_check_that_panicked_is_not_kept() {
    let root = crate::common::fixture("panic");
    let session = Session {
        std: upstream_std(),
        projects: vec![root.clone()],
    };
    let main = root.join("src/main.inko");
    let text = std::fs::read_to_string(&main).unwrap();
    let cache = Cache::new(4);

    for _ in 0..2 {
        let answer = ask(
            &cache,
            &session,
            &main,
            &text,
            Question::Type,
            "let value",
            4,
        );

        assert_eq!(answer.result.unwrap_err().code, "compilerPanic");
        assert!(!answer.cached);
    }

    assert!(cache.is_empty());
}

/// `MAIN` with a call in `other` left open, so it doesn't parse. Neither
/// a repaired copy nor `MAIN`'s check answers on that line.
fn unparsed() -> String {
    MAIN.replace("    lib.size\n", "    lib.size(\n")
}

/// `ask`, without a repaired copy: only the documents as they are.
fn ask_as_they_are(cache: &Cache, session: &Session, file: &Path, text: &str) -> Answer<Reply> {
    ask_with(
        cache,
        session,
        file,
        text,
        None,
        false,
        Question::Type,
        "first",
        0,
    )
}

/// Code that doesn't parse has nothing to answer from, and a second
/// question about the same text is a lookup too, with the same answer as
/// a new check.
#[test]
fn a_check_with_nothing_to_answer_from_is_kept() {
    let (root, session) = lib_project("unparsed");
    let main = root.join("src/main.inko");
    let text = unparsed();
    let cache = Cache::new(4);
    let new = ask_as_they_are(&Cache::new(0), &session, &main, &text);

    assert_eq!(new.result.as_ref().unwrap_err().code, "noHir");

    for cached in [false, true] {
        let answer = ask_as_they_are(&cache, &session, &main, &text);

        assert_eq!(answer.result, new.result);
        assert_eq!(answer.cached, cached);
    }

    assert_eq!((cache.len(), cache.failures()), (0, 1));
}

/// A check that didn't parse doesn't say which modules it read, so every
/// source file it could have read counts: a module fixed on disk is
/// checked again.
#[test]
fn a_module_fixed_on_disk_is_never_answered_from_a_kept_failure() {
    let (root, session) = project(
        "fixed-on-disk",
        &[
            ("src/main.inko", MAIN),
            ("src/lib.inko", "fn pub size -> Int {\n"),
        ],
    );
    let main = root.join("src/main.inko");
    let cache = Cache::new(4);
    let answer = ask(&cache, &session, &main, MAIN, Question::Type, "first", 0);

    assert_eq!(answer.result.unwrap_err().code, "noHir");
    assert!(ask(&cache, &session, &main, MAIN, Question::Type, "first", 0).cached);

    // Rewritten in place, which leaves its directory as it was.
    std::fs::write(root.join("src/lib.inko"), LIB).unwrap();

    let answer = ask(&cache, &session, &main, MAIN, Question::Type, "first", 0);

    assert_eq!(hover_type(&answer), "Int");
    assert!(!answer.cached);
}

/// Failures hold no compiler state, so they're kept apart, up to
/// `KEPT_FAILURES`, and never push out a check that has some.
#[test]
fn failures_dont_push_out_checks() {
    let (root, session) = lib_project("failures");
    let main = root.join("src/main.inko");
    let cache = Cache::new(1);

    ask(&cache, &session, &main, MAIN, Question::Type, "first", 0);

    for i in 0..query::KEPT_FAILURES + 2 {
        let text = unparsed().replace("+ 1", &format!("+ {}", i));

        ask_as_they_are(&cache, &session, &main, &text);
    }

    assert_eq!(cache.failures(), query::KEPT_FAILURES);
    assert!(ask(&cache, &session, &main, MAIN, Question::Type, "first", 0).cached);
}

/// An answer with no result is `cached` only if every check it took was
/// kept: the documents as they are, and then a repaired copy, or the last
/// good check. Here a check of other text pushes out the second between
/// the questions, so it's checked again.
#[test]
fn an_answer_with_no_result_is_cached_only_if_no_check_ran_for_it() {
    let (root, session) = lib_project("no-result");
    let main = root.join("src/main.inko");
    let text = unparsed();
    let fallback = Fallback {
        contents: HashMap::from([(main.clone(), MAIN.to_string())]),
        changes: HashMap::from([(main.clone(), Changes::between(MAIN, &text))]),
        ends: Default::default(),
    };

    // A repaired copy (the open call's line blank), or the last good check.
    for (fallback, repair) in [(None, true), (Some(&fallback), false)] {
        let cache = Cache::new(1);
        let asked = || {
            ask_with(
                &cache,
                &session,
                &main,
                &text,
                fallback,
                repair,
                Question::Type,
                "lib.size(",
                4,
            )
        };
        let first = asked();

        assert_eq!(first.result.unwrap_err().code, "noHir");
        assert!(!first.cached);
        assert!(asked().cached);

        let other = MAIN.replace("+ 1", "+ 2");

        ask(&cache, &session, &main, &other, Question::Type, "first", 0);

        let again = asked();

        assert_eq!(again.result.unwrap_err().code, "noHir");
        assert!(!again.cached, "repair: {}", repair);
    }
}

// Over LSP.

/// A server whose worker keeps checks, even when every test runs without
/// them (`CUTTLEBONE_NO_CACHE`): these tests are about kept checks.
fn with_kept_checks(root: &Path) -> Client {
    Client::initialized_with(
        Launch {
            env: vec![],
            unset: vec![NO_CACHE_VARIABLE],
            cwd: None,
        },
        root,
        json!({ "statusNotifications": true, "diagnostics": false }),
        json!({}),
    )
}

#[test]
fn kept_answers_are_marked_cached_over_lsp() {
    let (root, _) = lib_project("lsp");
    let main = root.join("src/main.inko");
    let mut client = with_kept_checks(&root);
    let (line, character) = at(MAIN, "first", 0, false);

    client.open(&main, MAIN);

    let (first, response) = client.hover(&main, line, character);

    assert_eq!(response["result"]["contents"]["value"], "```inko\nInt\n```");
    assert_eq!(client.status(first).unwrap().get("cached"), None);

    let (second, again) = client.hover(&main, line, character);
    let status = client.status(second).unwrap();

    assert_eq!(again["result"], response["result"]);
    assert_eq!(status["status"], "fresh");
    assert_eq!(status["cached"], true);
}

/// An edit outside the editor (a `git checkout`, a formatter) of a module
/// that isn't open, with no `workspace/didChangeWatchedFiles`.
#[test]
fn a_file_changed_outside_the_editor_is_never_answered_from_a_kept_check_over_lsp() {
    let (root, _) = lib_project("lsp-disk");
    let main = root.join("src/main.inko");
    let mut client = with_kept_checks(&root);
    let (line, character) = at(MAIN, "first", 0, false);

    client.open(&main, MAIN);
    client.hover(&main, line, character);

    let (id, _) = client.hover(&main, line, character);

    assert_eq!(client.status(id).unwrap()["cached"], true);
    std::fs::write(
        root.join("src/lib.inko"),
        "fn pub size -> String {\n  'a'\n}\n",
    )
    .unwrap();

    let (id, response) = client.hover(&main, line, character);

    assert_eq!(
        response["result"]["contents"]["value"],
        "```inko\nString\n```"
    );
    assert_eq!(client.status(id).unwrap().get("cached"), None);
}

/// A save writes the open document's text to its file, which the check
/// didn't read (it read the document), so it keeps the checks of that
/// text: written in place, or to a new file renamed over the old one, as
/// editors that save safely do. The file must still exist, though, and a
/// change to a file that isn't open still counts.
#[test]
fn a_save_keeps_the_checks_of_the_text_it_saved() {
    let (root, _) = lib_project("lsp-save");
    let main = root.join("src/main.inko");
    let edited = MAIN.replace("let third = [first, second]", "let third = [second, first]");
    let mut client = with_kept_checks(&root);
    let (line, character) = at(&edited, "first", 0, false);
    let mut version = 1;
    let cached = |client: &mut Client| {
        let (id, response) = client.hover(&main, line, character);

        assert_eq!(response["result"]["contents"]["value"], "```inko\nInt\n```");
        client.status(id).unwrap().get("cached") == Some(&json!(true))
    };

    client.open(&main, MAIN);
    client.change(&main, 2, json!([{ "text": edited }]));
    assert!(!cached(&mut client));
    assert!(cached(&mut client));

    for safely in [false, true, false] {
        if safely {
            let temporary = root.join("src/.main.inko.tmp");

            std::fs::write(&temporary, &edited).unwrap();
            std::fs::rename(&temporary, &main).unwrap();
        } else {
            std::fs::write(&main, &edited).unwrap();
        }

        version += 1;
        client.notify(
            "textDocument/didSave",
            json!({ "textDocument": { "uri": crate::common::client::uri(&main), "version": version }, "text": edited }),
        );
        assert!(cached(&mut client), "saved safely: {}", safely);
    }

    // A module that isn't open, changed on disk, still counts.
    std::fs::write(root.join("src/lib.inko"), LIB.replace("42", "24")).unwrap();
    assert!(!cached(&mut client));
    assert!(cached(&mut client));

    // So does a file created where the compiler finds modules.
    std::fs::write(root.join("src/other.inko"), "").unwrap();
    assert!(!cached(&mut client));
}

/// The tests' switch for running every test without kept checks, to show
/// that the answers are the same.
#[test]
fn the_tests_can_turn_kept_checks_off() {
    let (root, _) = lib_project("lsp-off");
    let main = root.join("src/main.inko");
    let mut client = Client::initialized_with(
        Launch {
            env: vec![(NO_CACHE_VARIABLE, "1".to_string())],
            unset: vec![],
            cwd: None,
        },
        &root,
        json!({ "statusNotifications": true, "diagnostics": false }),
        json!({}),
    );
    let (line, character) = at(MAIN, "first", 0, false);

    client.open(&main, MAIN);

    for _ in 0..2 {
        let (id, response) = client.hover(&main, line, character);

        assert_eq!(response["result"]["contents"]["value"], "```inko\nInt\n```");
        assert_eq!(client.status(id).unwrap().get("cached"), None);
    }
}

/// `shutdown` stops the worker that waits with its kept checks.
#[test]
fn shutdown_stops_the_waiting_worker() {
    let (root, _) = lib_project("lsp-shutdown");
    let main = root.join("src/main.inko");
    let mut client = Client::initialized(&root, json!({ "diagnostics": false }));
    let (line, character) = at(MAIN, "first", 0, false);

    client.open(&main, MAIN);
    client.hover(&main, line, character);
    assert_eq!(client.workers().len(), 1);
    client.request("shutdown", serde_json::Value::Null);
    assert!(client.workers().is_empty());
}

/// A worker that died while it waited (killed from outside, say) is
/// replaced by a new one for the next query.
#[test]
fn a_waiting_worker_that_died_is_replaced() {
    let (root, _) = lib_project("lsp-died");
    let main = root.join("src/main.inko");
    let mut client = with_kept_checks(&root);
    let (line, character) = at(MAIN, "first", 0, false);

    client.open(&main, MAIN);
    client.hover(&main, line, character);

    let workers = client.workers();

    assert_eq!(workers.len(), 1);
    std::process::Command::new("kill")
        .args(["-9", &workers[0].to_string()])
        .status()
        .unwrap();
    std::thread::sleep(Duration::from_millis(100));

    let start = std::time::Instant::now();
    let (id, response) = client.hover(&main, line, character);

    assert_eq!(response["result"]["contents"]["value"], "```inko\nInt\n```");
    assert_eq!(client.status(id).unwrap()["status"], "fresh");
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
    assert_ne!(client.workers(), workers);
}
