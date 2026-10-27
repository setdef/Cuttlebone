//! Answers after edits are undone, over LSP: a document changed (with
//! incremental or whole-text changes), questions asked between the
//! changes so that checks are kept and last good checks recorded, and then
//! every change put back, with and without a save between, and with a
//! module on disk changed and restored. References, definitions and
//! document highlights must then be exactly the answers a new server gives
//! for the text as it is. A client's refactoring (a safe delete, say)
//! relies on them.

use crate::common::client::{Client, Launch, uri};
use cuttlebone::worker::NO_CACHE_VARIABLE;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const MAIN: &str = "import lib

fn first -> Int {
  lib.helper(1) + twice(1)
}

fn twice(value: Int) -> Int {
  value * 2
}

fn second -> Int {
  let a = lib.helper(2)

  a + lib.helper(3) + twice(a)
}

type async Main {
  fn async main {
    first + second
  }
}
";

const LIB: &str = "fn pub helper(value: Int) -> Int {
  value + 1
}

fn pub other -> Int {
  helper(0)
}
";

/// The edits made before they're undone: a use removed, the declaration
/// of a function used before and after it renamed, and a line that
/// doesn't parse. Each replaces the first `.0` after the previous ones with
/// `.1`.
const EDITS: &[(&str, &str)] = &[
    ("lib.helper(3)", "0"),
    ("fn twice(", "fn twic("),
    ("  value * 2\n", "  value * 2\n  if value {\n"),
];

/// Where the questions are asked, in the text as it began: the needle and
/// the characters past its start.
const POSITIONS: &[(&str, usize)] = &[
    ("lib.helper(1)", 4),
    ("twice(1)", 0),
    ("fn twice(", 3),
    ("value: Int", 0),
    ("value * 2", 0),
    ("let a", 4),
    ("lib.helper(2)", 4),
    ("lib.helper(3)", 4),
    ("twice(a)", 0),
    ("twice(a)", 6),
    ("first + second", 0),
];

const METHODS: &[&str] = &[
    "textDocument/references",
    "textDocument/definition",
    "textDocument/documentHighlight",
];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Change {
    /// Each edit as a range, and each undo as the range it made.
    Incremental,
    /// Each edit and undo as the whole text.
    Full,
    /// Each edit as a range, and every undo in one `didChange`, a range
    /// each, in order.
    Batched,
    /// Each edit as a range, and the undo by closing the document without
    /// saving and opening it again with the text as it began.
    Reopened,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Save {
    No,
    /// The edited text is saved (with `text` in `didSave`), then the edits
    /// undone without saving: the file on disk isn't the document.
    Between,
    /// Saved after the edits and again after the undo, without `text`.
    Both,
}

/// The 0-based line and character of byte `offset` of `text`.
fn position(text: &str, offset: usize, utf8: bool) -> Value {
    let before = &text[..offset];
    let line = before.matches('\n').count();
    let column = &before[before.rfind('\n').map_or(0, |i| i + 1)..];
    let character = if utf8 {
        column.len()
    } else {
        column.encode_utf16().count()
    };

    json!({ "line": line, "character": character })
}

/// A copy of the project in the tests' temporary directory.
fn project(name: &str) -> PathBuf {
    let root = crate::common::tmp().join(format!("undo-{}", name));
    let _ = std::fs::remove_dir_all(&root);

    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.inko"), MAIN).unwrap();
    std::fs::write(root.join("src/lib.inko"), LIB).unwrap();
    root.canonicalize().unwrap()
}

/// A server with kept checks (whatever the tests' switch says: an undo
/// returns to text a kept check checked), status notifications, and no
/// diagnostics.
fn server(root: &Path, utf8: bool) -> Client {
    let encodings = if utf8 {
        json!(["utf-8"])
    } else {
        json!(["utf-16"])
    };

    Client::initialized_with(
        Launch {
            unset: vec![NO_CACHE_VARIABLE],
            ..Launch::default()
        },
        root,
        json!({ "statusNotifications": true, "diagnostics": false }),
        json!({ "general": { "positionEncodings": encodings } }),
    )
}

/// The answers to every question at every position in `text` (main's
/// text as it began), with each result and its status.
fn answers(client: &mut Client, main: &Path, text: &str, utf8: bool) -> Vec<(String, Value)> {
    let mut out = Vec::new();

    for (needle, offset) in POSITIONS {
        let Some(at) = text.find(needle) else {
            continue;
        };
        let at = at + offset;

        for method in METHODS {
            let mut params = json!({
                "textDocument": { "uri": uri(main) },
                "position": position(text, at, utf8),
            });

            if *method == "textDocument/references" {
                params["context"] = json!({ "includeDeclaration": true });
            }

            let id = client.send_request(method, params);
            let response = client.response(id);
            let status = client.status(id).cloned().unwrap_or(Value::Null);
            let mut result = response["result"].clone();

            // Locations in any order are the same answer.
            if let Some(list) = result.as_array_mut() {
                list.sort_by_key(|v| v.to_string());
            }

            out.push((
                format!("{} at {:?}+{}", method, needle, offset),
                json!({
                    "result": result,
                    "status": status["status"],
                    "repaired": status.get("repaired"),
                    "reason": status.pointer("/reason/code"),
                }),
            ));
        }
    }

    out
}

/// Sends the change from `old` to `new` as the client would: the range
/// between their common start and end, or the whole text.
fn send(
    client: &mut Client,
    main: &Path,
    version: &mut i32,
    old: &str,
    new: &str,
    change: Change,
    utf8: bool,
) {
    *version += 1;

    let changes = match change {
        Change::Full => json!([{ "text": new }]),
        Change::Incremental | Change::Batched | Change::Reopened => {
            let (start, end, len) = cuttlebone::edits::difference(old, new);

            json!([{
                "range": {
                    "start": position(old, start, utf8),
                    "end": position(old, end, utf8),
                },
                "text": &new[start..start + len],
            }])
        }
    };

    client.change(main, *version, changes);
}

fn save(client: &mut Client, main: &Path, version: i32, text: Option<&str>) {
    let mut params = json!({ "textDocument": { "uri": uri(main), "version": version } });

    if let Some(text) = text {
        std::fs::write(main, text).unwrap();
        params["text"] = json!(text);
    }

    client.notify("textDocument/didSave", params);
}

/// What a new server answers for main's text and lib's on disk, each
/// state asked once.
struct Oracle {
    root: PathBuf,
    utf8: bool,
    answers: HashMap<(String, String), Vec<(String, Value)>>,
}

impl Oracle {
    fn answers(&mut self, text: &str) -> &[(String, Value)] {
        let main = self.root.join("src/main.inko");
        let lib = std::fs::read_to_string(self.root.join("src/lib.inko")).unwrap();
        let (root, utf8) = (self.root.clone(), self.utf8);

        self.answers
            .entry((text.to_string(), lib))
            .or_insert_with(|| {
                let mut fresh = server(&root, utf8);

                fresh.open(&main, text);
                answers(&mut fresh, &main, text, utf8)
            })
    }
}

/// Whether an answer came from the documents as they are, with no lines
/// blank: one any server gives for the same text and files.
fn plain(answer: &Value) -> bool {
    answer["status"] == "fresh" && answer["repaired"].is_null()
}

/// Asks every question about `text`, and compares each answer either
/// server gives from the documents as they are with the other's.
fn compare(client: &mut Client, oracle: &mut Oracle, main: &Path, text: &str, step: &str) {
    let got = answers(client, main, text, oracle.utf8);
    let want = oracle.answers(text);

    assert_eq!(got.len(), want.len());

    for ((question, got), (_, want)) in got.iter().zip(want) {
        if plain(got) || plain(want) {
            assert_eq!(got, want, "{}, {}", question, step);
        }
    }
}

/// Edits main (and lib on disk, if `disk`), asking every question after
/// each edit, then undoes it all, asking again after each undo. Every
/// answer from the documents as they are must be a new server's, and at
/// the end, every answer must be.
fn undo(name: &str, change: Change, saved: Save, disk: bool) {
    for utf8 in [false, true] {
        let root = project(&format!("{}-{}", name, if utf8 { "utf8" } else { "utf16" }));
        let main = root.join("src/main.inko");
        let lib = root.join("src/lib.inko");
        let mut client = server(&root, utf8);
        let mut oracle = Oracle {
            root: root.clone(),
            utf8,
            answers: HashMap::new(),
        };
        let mut version = 1;
        let mut texts = vec![MAIN.to_string()];
        let case = format!(
            "{}, {:?}, saved {:?}, disk {}",
            if utf8 { "UTF-8" } else { "UTF-16" },
            change,
            saved,
            disk
        );
        let mut check = |client: &mut Client, text: &str, step: String| {
            compare(
                client,
                &mut oracle,
                &main,
                text,
                &format!("{} ({})", step, case),
            );
        };

        client.open(&main, MAIN);
        check(&mut client, MAIN, "opened".to_string());

        for (from, to) in EDITS {
            let old = texts.last().unwrap().clone();
            let new = old.replacen(from, to, 1);

            assert_ne!(old, new);
            send(&mut client, &main, &mut version, &old, &new, change, utf8);
            check(&mut client, &new, format!("{:?} edited", from));
            texts.push(new);

            if disk && texts.len() == 2 {
                // A module that isn't open changed outside the editor: its
                // function's uses here go nowhere.
                std::fs::write(&lib, LIB.replace("helper", "helped")).unwrap();
                check(
                    &mut client,
                    texts.last().unwrap(),
                    "lib changed".to_string(),
                );
            }
        }

        let edited = texts.last().unwrap().clone();

        match saved {
            Save::No => {}
            Save::Between => save(&mut client, &main, version, Some(&edited)),
            Save::Both => {
                std::fs::write(&main, &edited).unwrap();
                save(&mut client, &main, version, None);
            }
        }

        check(&mut client, &edited, "saved".to_string());

        if change == Change::Batched {
            let mut ranges = Vec::new();

            while texts.len() > 1 {
                let old = texts.pop().unwrap();
                let new = texts.last().unwrap();
                let (start, end, len) = cuttlebone::edits::difference(&old, new);

                ranges.push(json!({
                    "range": {
                        "start": position(&old, start, utf8),
                        "end": position(&old, end, utf8),
                    },
                    "text": &new[start..start + len],
                }));
            }

            version += 1;
            client.change(&main, version, json!(ranges));
        }

        if change == Change::Reopened {
            client.close(&main);
            // Its file's text, as it was saved or not.
            let disk = std::fs::read_to_string(&main).unwrap();

            check(&mut client, &disk, "closed".to_string());
            texts.truncate(1);
            version = 1;
            client.open(&main, MAIN);
        }

        if disk && texts.len() == 1 {
            std::fs::write(&lib, LIB).unwrap();
        }

        // Undone in the reverse order, a question after each.
        while texts.len() > 1 {
            let old = texts.pop().unwrap();
            let new = texts.last().unwrap().clone();

            send(&mut client, &main, &mut version, &old, &new, change, utf8);

            if disk && texts.len() == 2 {
                std::fs::write(&lib, LIB).unwrap();
            }

            check(&mut client, &new, format!("{} undone", texts.len()));
        }

        if saved == Save::Both {
            std::fs::write(&main, MAIN).unwrap();
            save(&mut client, &main, version, None);
        }

        let got = answers(&mut client, &main, MAIN, utf8);
        let want = oracle.answers(MAIN).to_vec();

        assert_eq!(got.len(), want.len());

        for ((question, got), (_, want)) in got.iter().zip(&want) {
            assert!(plain(want), "{}: {}", question, want);
            assert_eq!(got, want, "{} ({})", question, case);
        }

        // The questions with a list have one.
        let references = &want[0].1["result"];

        assert_eq!(
            references.as_array().map(Vec::len),
            Some(5),
            "lib.helper's references: {}",
            references
        );
        assert_eq!(want[3].1["result"].as_array().map(Vec::len), Some(3));
    }
}

#[test]
fn undone_incremental_changes_answer_for_the_text_as_it_is() {
    undo("incremental", Change::Incremental, Save::No, false);
}

#[test]
fn undone_whole_text_changes_answer_for_the_text_as_it_is() {
    undo("full", Change::Full, Save::No, false);
}

#[test]
fn undone_changes_after_a_save_answer_for_the_text_as_it_is() {
    undo("saved", Change::Incremental, Save::Between, false);
    undo("saved-full", Change::Full, Save::Between, false);
}

#[test]
fn undone_changes_saved_both_ways_answer_for_the_text_as_it_is() {
    undo("saved-both", Change::Incremental, Save::Both, false);
}

#[test]
fn undone_changes_with_a_module_changed_on_disk_and_restored_answer_for_the_text_as_it_is() {
    undo("disk", Change::Incremental, Save::No, true);
    undo("disk-saved", Change::Full, Save::Between, true);
}

#[test]
fn undone_changes_in_one_notification_answer_for_the_text_as_it_is() {
    undo("batched", Change::Batched, Save::No, false);
    undo("batched-disk", Change::Batched, Save::Between, true);
}

#[test]
fn a_document_closed_unsaved_and_opened_again_answers_for_the_text_as_it_is() {
    undo("reopened", Change::Reopened, Save::No, false);
    undo("reopened-saved", Change::Reopened, Save::Between, true);
}

/// A module edited while open, closed without saving, and then changed on
/// disk (or opened again with other text), while the last good check's
/// documents still have its text then: answers from that check must map
/// through the change on disk too, never pair the edits recorded until
/// the close with another text. Parakeet's sweep found 0.12.0's checks
/// panicking in `units::widen` here.
#[test]
fn a_module_closed_and_changed_on_disk_is_mapped_from_its_text_on_disk() {
    for reopened in [false, true] {
        for shape in ["longer", "shorter", "reordered"] {
            let shorter = shape;
            let name = format!("closed-{}-{}", reopened, shorter);
            let name = name.as_str();
            let root = project(name);
            let main = root.join("src/main.inko");
            let lib = root.join("src/lib.inko");
            // Without repaired copies, answers come from the last good check,
            // which maps every changed file's text, lib's too (a fault only
            // tests have).
            let mut client = Client::initialized_with(
                Launch {
                    env: vec![("CUTTLEBONE_FAULT", "norepair:.inko".to_string())],
                    unset: vec![NO_CACHE_VARIABLE],
                    ..Launch::default()
                },
                &root,
                json!({ "statusNotifications": true, "diagnostics": false }),
                json!({ "general": { "positionEncodings": ["utf-16"] } }),
            );
            let mut oracle = Oracle {
                root: root.clone(),
                utf8: false,
                answers: HashMap::new(),
            };

            client.open(&main, MAIN);
            client.open(&lib, LIB);
            // A good check of both, kept as the last good one.
            answers(&mut client, &main, MAIN, false);

            // lib edited, then closed without saving.
            client.change(
                &lib,
                2,
                json!([{ "text": LIB.replace("value + 1", "value + 2") }]),
            );
            answers(&mut client, &main, MAIN, false);
            client.close(&lib);

            // lib on disk gains a function at its start, so every offset of
            // its text then moves, or gets shorter than where the edits
            // recorded until the close were.
            // Or keeps its length, its two functions swapped, which only
            // where the edits ended can tell.
            let (helper, other) = LIB.split_at(LIB.find("fn pub other").unwrap());
            let disk = match shape {
                "shorter" => "fn pub helper(value: Int) -> Int {\n  0\n}\n".to_string(),
                "reordered" => format!("{}\n{}", other, helper.trim_end_matches('\n')) + "\n",
                _ => format!("fn pub first -> Int {{\n  0\n}}\n\n{}", LIB),
            };

            std::fs::write(&lib, &disk).unwrap();

            if reopened {
                client.open(&lib, &disk);
            }

            // main stops parsing: answers come from repaired copies or the
            // last good check, which map lib's changes.
            // main stops parsing, or has an error in a declaration: answers
            // come from the last good check, or past the error, and either
            // maps lib's changes.
            for (version, broken) in [
                MAIN.replace("fn second -> Int {", "fn second -> Int {\n  if true {"),
                MAIN.replace("fn first -> Int {", "fn first -> Nope {"),
            ]
            .into_iter()
            .enumerate()
            {
                client.change(&main, version as i32 + 2, json!([{ "text": broken }]));

                for (question, got) in answers(&mut client, &main, &broken, false) {
                    assert!(
                        !matches!(
                            got["reason"].as_str(),
                            Some("checkCrashed" | "compilerPanic")
                        ),
                        "{}: {}",
                        question,
                        got
                    );

                    // A definition in lib is at `helper` in lib as it is.
                    for place in got["result"].as_array().into_iter().flatten() {
                        if place["uri"] == json!(uri(&lib)) && question.contains("definition") {
                            let r = &place["range"];
                            let line = disk
                                .lines()
                                .nth(r["start"]["line"].as_u64().unwrap() as usize)
                                .unwrap();
                            let (from, to) = (
                                r["start"]["character"].as_u64().unwrap() as usize,
                                r["end"]["character"].as_u64().unwrap() as usize,
                            );

                            assert_eq!(&line[from..to], "helper", "{}: {}", question, got);
                        }
                    }
                }
            }

            let stderr = client.stderr.lock().unwrap().clone();

            assert!(!stderr.contains("panicked"), "{}: {}", name, stderr);

            // And once main parses again, every answer is a new server's.
            client.change(&main, 4, json!([{ "text": MAIN }]));
            compare(&mut client, &mut oracle, &main, MAIN, name);
        }
    }
}
