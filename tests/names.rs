//! Completion of names over LSP, in both encodings: variables and
//! arguments in scope (and nothing out of it), what `self` has, fields
//! after `@`, the module's own symbols, imported ones and the prelude's,
//! types in a signature, symbols to import with the edit that imports
//! them (nothing private, nothing a module can't import), modules and
//! symbols in an `import`, and what it answers in code being edited.
use crate::common::client::{Client, Launch, at};
use crate::symbols::project_with_a_dependency;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn session(root: &Path, utf8: bool) -> Client {
    let encodings = if utf8 {
        json!(["utf-8"])
    } else {
        json!(["utf-16"])
    };

    Client::initialized_with(
        Launch::default(),
        root,
        json!({ "statusNotifications": true, "diagnostics": false, "debounceMs": 0 }),
        json!({ "general": { "positionEncodings": encodings } }),
    )
}

struct Edited {
    client: Client,
    file: PathBuf,
    utf8: bool,
}

impl Edited {
    /// Opens `file` with `text`, and completes at the end of the first
    /// `needle` in it: its result, and its status.
    fn complete(&mut self, text: &str, needle: &str) -> (Value, Value) {
        self.client.open(&self.file, text);

        // A needle may end with the line's end, which isn't typed.
        let typed = needle.trim_end_matches('\n').chars().count();
        let (line, character) = at(text, needle, typed, self.utf8);
        let (id, response) = self.client.complete(&self.file, line, character);
        let status = self.client.status(id).unwrap().clone();

        self.client.close(&self.file);
        (response["result"].clone(), status)
    }
}

/// Each item as `kind label | detail`, sorted by `sortText`.
#[track_caller]
fn items(result: &Value) -> Vec<String> {
    let mut list: Vec<&Value> = result["items"].as_array().unwrap().iter().collect();

    list.sort_by_key(|i| i["sortText"].as_str().unwrap_or("").to_string());
    list.iter()
        .map(|i| {
            format!(
                "{} {} | {}",
                i["kind"],
                i["label"].as_str().unwrap(),
                i["detail"].as_str().unwrap()
            )
        })
        .collect()
}

#[track_caller]
fn labels(result: &Value) -> Vec<String> {
    result["items"]
        .as_array()
        .unwrap_or_else(|| panic!("no items: {}", result))
        .iter()
        .map(|i| i["label"].as_str().unwrap().to_string())
        .collect()
}

fn item<'a>(result: &'a Value, label: &str) -> &'a Value {
    result["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["label"] == label)
        .unwrap_or_else(|| panic!("no {} in {:?}", label, labels(result)))
}

/// `text` with an item's `additionalTextEdits` applied.
fn apply(text: &str, item: &Value, utf8: bool) -> String {
    let mut out = text.to_string();

    for edit in item["additionalTextEdits"].as_array().unwrap().iter().rev() {
        let byte = |p: &Value| {
            cuttlebone::position::byte_offset(
                &out,
                cuttlebone::position::Position {
                    line: p["line"].as_u64().unwrap() as u32,
                    character: p["character"].as_u64().unwrap() as u32,
                },
                if utf8 {
                    cuttlebone::position::Encoding::Utf8
                } else {
                    cuttlebone::position::Encoding::Utf16
                },
            )
            .unwrap()
        };
        let (start, end) = (byte(&edit["range"]["start"]), byte(&edit["range"]["end"]));

        out.replace_range(start..end, edit["newText"].as_str().unwrap());
    }

    out
}

fn edited(name: &str, utf8: bool) -> (Edited, String) {
    let root = project_with_a_dependency(&format!("names-{}-{}", name, utf8));
    let file = root.join("src/main.inko");
    let text = std::fs::read_to_string(&file).unwrap();

    (
        Edited {
            client: session(&root, utf8),
            file,
            utf8,
        },
        text,
    )
}

#[test]
fn names_in_scope_and_nothing_else() {
    for utf8 in [false, true] {
        let (mut e, text) = edited("scope", utf8);

        // In a method: its argument, what `self` has, what the module
        // binds. Not another method's variables, not fields by name.
        let typed = text.replacen(
            "    @count += amount\n",
            "    @count += amount\n    am\n",
            1,
        );
        let (result, status) = e.complete(&typed, "    am\n");

        let shown = items(&result);

        assert_eq!(status["status"], "fresh", "{}", status);
        assert_eq!(result["isIncomplete"], true);
        assert_eq!(shown[0], "6 amount | let amount: Int");
        assert!(
            shown.contains(&"2 add | fn pub mut add(amount: Int)".to_string()),
            "{:?}",
            shown
        );
        assert!(
            shown.contains(&"2 same | fn pub same(other: ref Counter) -> Bool".to_string()),
            "{:?}",
            shown
        );

        let names = labels(&result);

        for present in [
            "Counter", "MAX", "NAMES", "describe", "total", "Shape", "Area", "Stdout", "Int",
            "Option", "panic",
        ] {
            assert!(
                names.contains(&present.to_string()),
                "{} isn't in {:?}",
                present,
                names
            );
        }

        for absent in ["counter", "out", "label", "count", "new", "main", "Circle"] {
            assert!(
                !names.contains(&absent.to_string()),
                "{} is in {:?}",
                absent,
                names
            );
        }

        // In `main`, before a `let`: the ones before it only, and `self`'s
        // methods aren't `Counter`'s.
        let typed = text.replacen(
            "    counter.add(NAMES.size)\n",
            "    co\n    counter.add(NAMES.size)\n",
            1,
        );
        let (result, _) = e.complete(&typed, "    co\n");
        let names = labels(&result);

        assert!(
            names.contains(&"counter".to_string()) && names.contains(&"out".to_string()),
            "{:?}",
            names
        );
        assert!(
            !names.contains(&"add".to_string()) && !names.contains(&"amount".to_string()),
            "{:?}",
            names
        );

        let typed = text.replacen(
            "    let out = Stdout.new\n",
            "    ou\n    let out = Stdout.new\n",
            1,
        );
        let (result, _) = e.complete(&typed, "    ou\n");

        assert!(!labels(&result).contains(&"out".to_string()));
        assert!(!labels(&result).contains(&"counter".to_string()));

        // A closure's argument, and a `match` case's binding, only in them.
        let typed = text.replacen(
            "    let _ = total(Shape.Square)\n",
            "    let _ = [1].iter.map(fn (item) { it })\n    match Option.Some(1) {\n      case Some(found) -> fo\n      case _ -> {}\n    }\n    ite\n",
            1,
        );
        let (result, _) = e.complete(&typed, "{ it");

        assert!(labels(&result).contains(&"item".to_string()));

        let (result, _) = e.complete(&typed, "-> fo");

        assert!(labels(&result).contains(&"found".to_string()));

        let (result, _) = e.complete(&typed, "    ite\n");

        assert!(!labels(&result).contains(&"item".to_string()));
        assert!(!labels(&result).contains(&"found".to_string()));

        // After `@`: the type's fields, all there are.
        let typed = text.replacen("    @count += amount\n", "    @count += amount\n    @\n", 1);
        let (result, _) = e.complete(&typed, "    @\n");

        assert_eq!(result["isIncomplete"], false);
        assert_eq!(
            items(&result),
            [
                "5 count | let @count: Int",
                "5 label | let pub @label: String"
            ]
        );

        // In a comment, nothing.
        let (result, _) = e.complete(&text, "# How many");

        assert_eq!(result, Value::Null);
    }
}

#[test]
fn types_in_a_signature() {
    for utf8 in [false, true] {
        let (mut e, text) = edited("types", utf8);
        let typed = text.replacen("fn total(shape: ref Shape)", "fn total(shape: ref Sha)", 1);
        let (result, status) = e.complete(&typed, "ref Sha");
        let names = labels(&result);

        assert_eq!(status["status"], "fresh", "{}", status);

        for present in ["Shape", "Counter", "Area", "Int", "Option"] {
            assert!(
                names.contains(&present.to_string()),
                "{} isn't in {:?}",
                present,
                names
            );
        }

        for absent in ["describe", "MAX", "shape", "total"] {
            assert!(
                !names.contains(&absent.to_string()),
                "{} is in {:?}",
                absent,
                names
            );
        }
    }
}

/// Public symbols of modules not imported, matching what's typed, with the
/// edit that imports them: a new import in order, or into the module's
/// list. Not private ones, not a test module's from `src/`, not what's
/// imported already.
#[test]
fn symbols_to_import() {
    for utf8 in [false, true] {
        let (mut e, text) = edited("imports", utf8);
        let typed = text.replacen(
            "    counter.add(NAMES.size)\n",
            "    StringBu\n    counter.add(NAMES.size)\n",
            1,
        );
        let (result, _) = e.complete(&typed, "    StringBu\n");
        let buffer = item(&result, "StringBuffer");

        assert_eq!(buffer["labelDetails"]["description"], "std.string");
        assert!(buffer["sortText"].as_str().unwrap().starts_with('3'));
        assert_eq!(
            apply(&typed, buffer, utf8)
                .lines()
                .take(4)
                .collect::<Vec<_>>(),
            [
                "# Every kind of declaration, for document and workspace symbols.",
                "import shapes (Area, Shape)",
                "import std.stdio (Stdout)",
                "import std.string (StringBuffer)",
            ]
        );

        // Into the module's list.
        let typed = text.replacen(
            "    counter.add(NAMES.size)\n",
            "    Stde\n    counter.add(NAMES.size)\n",
            1,
        );
        let (result, _) = e.complete(&typed, "    Stde\n");

        assert_eq!(
            apply(&typed, item(&result, "Stderr"), utf8).lines().nth(2),
            Some("import std.stdio (Stderr, Stdout)")
        );

        // The project's, into its import's list, and not a private one.
        let typed = text.replacen(
            "    counter.add(NAMES.size)\n",
            "    shape_\n    counter.add(NAMES.size)\n",
            1,
        );
        let (result, _) = e.complete(&typed, "    shape_\n");

        assert_eq!(
            apply(&typed, item(&result, "shape_count"), utf8)
                .lines()
                .nth(1),
            Some("import shapes (Area, Shape, shape_count)")
        );
        assert!(!labels(&result).contains(&"shape_helper".to_string()));

        // Nothing typed yet: nothing to import.
        let typed = text.replacen(
            "    let out = Stdout.new\n",
            "    let out = Stdout.new\n    \n",
            1,
        );
        let (result, _) = e.complete(&typed, "Stdout.new\n    ");

        assert!(labels(&result).contains(&"out".to_string()));
        assert!(
            result["items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|i| i.get("additionalTextEdits").is_none()),
            "{}",
            result
        );

        // A dependency's.
        let typed = text.replacen(
            "    counter.add(NAMES.size)\n",
            "    are\n    counter.add(NAMES.size)\n",
            1,
        );
        let (result, _) = e.complete(&typed, "    are\n");

        assert_eq!(
            apply(&typed, item(&result, "area"), utf8).lines().nth(1),
            Some("import geo (area)")
        );

        // Not a test module's, nor what's bound already, nor private.
        let typed = text.replacen(
            "    counter.add(NAMES.size)\n",
            "    test_cou\n    counter.add(NAMES.size)\n",
            1,
        );
        let (result, _) = e.complete(&typed, "    test_cou\n");

        assert!(!labels(&result).contains(&"test_counter_counts".to_string()));

        let typed = text.replacen(
            "    counter.add(NAMES.size)\n",
            "    Stdou\n    counter.add(NAMES.size)\n",
            1,
        );
        let (result, _) = e.complete(&typed, "    Stdou\n");
        let stdout: Vec<&Value> = result["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|i| i["label"] == "Stdout")
            .collect();

        assert_eq!(stdout.len(), 1, "{:?}", stdout);
        assert!(stdout[0].get("additionalTextEdits").is_none());

        // std.string's private `StringBuffer.bytes`-like helpers aren't
        // offered: only public module-level declarations are.
        for i in result["items"].as_array().unwrap() {
            if let Some(module) = i["labelDetails"]["description"].as_str()
                && i.get("additionalTextEdits").is_some()
            {
                let file = cuttlebone::dump::upstream_std()
                    .join(module.replace('.', "/"))
                    .with_extension("inko");

                if let Ok(source) = std::fs::read_to_string(&file) {
                    let outline = cuttlebone::syntax::parse(&source).unwrap();

                    assert!(
                        outline
                            .declarations
                            .iter()
                            .any(|d| d.name == i["label"].as_str().unwrap() && d.public),
                        "{}",
                        i
                    );
                }
            }
        }
    }
}

#[test]
fn modules_and_symbols_in_an_import() {
    for utf8 in [false, true] {
        let (mut e, text) = edited("import-lines", utf8);
        let typed = text.replacen(
            "import std.stdio (Stdout)\n",
            "import std.stdio (Stdout)\nimport std.fm\n",
            1,
        );
        let (result, status) = e.complete(&typed, "import std.fm");

        assert_eq!(status["status"], "fresh");
        assert!(labels(&result).contains(&"fmt".to_string()));
        assert!(labels(&result).contains(&"net".to_string()));

        let typed = text.replacen(
            "import std.stdio (Stdout)\n",
            "import std.stdio (Stdout)\nimport \n",
            1,
        );
        let (result, _) = e.complete(&typed, "\nimport ");
        let names = labels(&result);

        for present in ["std", "shapes", "geo", "uses_geo"] {
            assert!(
                names.contains(&present.to_string()),
                "{} isn't in {:?}",
                present,
                names
            );
        }

        for absent in ["main", "test_main"] {
            assert!(
                !names.contains(&absent.to_string()),
                "{} is in {:?}",
                absent,
                names
            );
        }

        let typed = text.replacen(
            "import std.stdio (Stdout)\n",
            "import std.stdio (Stdout, Std)\n",
            1,
        );
        let (result, _) = e.complete(&typed, "(Stdout, Std");
        let names = labels(&result);

        assert!(
            names.contains(&"Stderr".to_string()) && names.contains(&"self".to_string()),
            "{:?}",
            names
        );
        assert!(!names.contains(&"Stdout".to_string()), "{:?}", names);
    }
}

/// Code being edited: names are given from the documents as they are, or
/// from a repaired copy whose blank lines are all in method bodies (as
/// completion after a `.`), never from an older check. Where a
/// declaration has an error, or the blank lines are outside a method's
/// body, they're not given: what's in scope may be different.
#[test]
fn names_in_code_being_edited() {
    let (mut e, text) = edited("broken", false);

    // Another method's body half typed: from a repaired copy.
    let typed = text
        .replacen("    @count += amount\n", "    @count += \n", 1)
        .replacen(
            "    counter.add(NAMES.size)\n",
            "    co\n    counter.add(NAMES.size)\n",
            1,
        );
    let (result, status) = e.complete(&typed, "    co\n");

    assert_eq!(status["repaired"], true, "{}", status);
    assert!(labels(&result).contains(&"counter".to_string()));

    // A declaration with an error elsewhere: no names.
    let typed = text
        .replacen(
            "fn pub describe(name: String)",
            "fn pub describe(name: Strin)",
            1,
        )
        .replacen(
            "    counter.add(NAMES.size)\n",
            "    co\n    counter.add(NAMES.size)\n",
            1,
        );
    let (result, status) = e.complete(&typed, "    co\n");

    assert_eq!(result, Value::Null);
    assert_eq!(status["reason"]["code"], "declarationErrors", "{}", status);

    // A signature half typed elsewhere, blank in the copy: no names.
    let typed = text
        .replacen(
            "fn pub describe(name: String) -> String {",
            "fn pub describe(name: String) -> {",
            1,
        )
        .replacen(
            "    counter.add(NAMES.size)\n",
            "    co\n    counter.add(NAMES.size)\n",
            1,
        );
    let (result, status) = e.complete(&typed, "    co\n");

    assert_eq!(result, Value::Null, "{}", status);
}
