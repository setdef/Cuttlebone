//! `textDocument/codeAction` (docs/lsp.md): quick fixes for diagnostics,
//! chosen by their IDs, over LSP in both encodings. The oracle is the
//! installed `inko check`, run on a copy of the project with each fix
//! applied: every fix offered must leave fewer of the diagnostic it fixes,
//! and no error the project didn't have. Fixes that wouldn't aren't
//! offered.
use crate::common::client::{Client, uri};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The installed `inko`, which must be the pinned release.
fn inko() -> &'static str {
    let out = Command::new("inko")
        .arg("--version")
        .output()
        .expect("the code action oracle needs `inko` on PATH");

    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("inko {}", cuttlebone::check::upstream_version()),
        "the code action oracle needs the installed inko to be the pinned release"
    );
    "inko"
}

/// What `inko check --format json` reports for the project in `dir`, with
/// a home directory of its own (it creates and locks one in the data
/// directory).
fn inko_check(dir: &Path) -> Vec<Value> {
    let home = crate::common::tmp().join("actions-home");

    std::fs::create_dir_all(&home).unwrap();

    let out = Command::new(inko())
        .args(["check", "--format", "json"])
        .current_dir(dir)
        .env("HOME", &home)
        .env("XDG_DATA_HOME", home.join("data"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);

    serde_json::from_str(stderr.trim())
        .unwrap_or_else(|e| panic!("inko check printed {:?}: {}", stderr, e))
}

struct Project {
    root: PathBuf,
    client: Client,
    utf8: bool,
}

impl Project {
    fn new(name: &str, utf8: bool, files: &[(&str, &str)], capabilities: Value) -> Project {
        let root = crate::common::tmp().join(format!(
            "actions-{}-{}",
            name,
            if utf8 { "utf8" } else { "utf16" }
        ));
        let _ = std::fs::remove_dir_all(&root);

        for (path, text) in files {
            let path = root.join(path);

            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        let root = root.canonicalize().unwrap();
        let mut all = json!({
            "general": { "positionEncodings": if utf8 { ["utf-8"] } else { ["utf-16"] } },
        });

        crate::common::client::merge_deep(&mut all, capabilities);

        let client = Client::initialized_with(
            Default::default(),
            &root,
            json!({ "statusNotifications": true, "diagnosticsDebounceMs": 0 }),
            all,
        );

        Project { root, client, utf8 }
    }

    fn path(&self, file: &str) -> PathBuf {
        self.root.join(file)
    }

    /// Opens a file, and waits for its diagnostics, as published.
    fn open(&mut self, file: &str) -> Vec<Value> {
        let path = self.path(file);
        let text = std::fs::read_to_string(&path).unwrap();
        let wanted = uri(&path);

        self.client.open(&path, &text);
        self.client.wait_for(|n| {
            n["method"] == "textDocument/publishDiagnostics" && n["params"]["uri"] == wanted
        })["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .clone()
    }

    /// The code actions for `diagnostics`, at their range, as a client
    /// asks: the whole of the first's range, with them as the context.
    fn actions(&mut self, file: &str, diagnostics: &[Value], extra: Value) -> (i64, Value) {
        let mut context = json!({ "diagnostics": diagnostics });

        for (k, v) in extra.as_object().unwrap() {
            context[k] = v.clone();
        }

        let id = self.client.send_request(
            "textDocument/codeAction",
            json!({
                "textDocument": { "uri": uri(&self.path(file)) },
                "range": diagnostics[0]["range"],
                "context": context,
            }),
        );

        (id, self.client.response(id)["result"].clone())
    }

    /// The project's files with an action's edits applied.
    fn applied(&self, action: &Value) -> HashMap<PathBuf, String> {
        let mut out = HashMap::new();

        for (file, edits) in action["edit"]["changes"].as_object().unwrap() {
            let path = url::Url::parse(file).unwrap().to_file_path().unwrap();
            let text = std::fs::read_to_string(&path).unwrap();

            out.insert(path, apply(&text, edits, self.utf8));
        }

        out
    }

    /// Checks a copy of the project with the action applied with the
    /// installed `inko check`: what it reports.
    fn checked(&self, action: &Value) -> Vec<Value> {
        let copy = self.root.with_extension("applied");
        let _ = std::fs::remove_dir_all(&copy);

        copy_dir(&self.root, &copy);

        for (path, text) in self.applied(action) {
            std::fs::write(copy.join(path.strip_prefix(&self.root).unwrap()), text).unwrap();
        }

        inko_check(&copy)
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();

    for entry in std::fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let target = to.join(path.file_name().unwrap());

        if path.is_dir() {
            copy_dir(&path, &target);
        } else {
            std::fs::copy(&path, &target).unwrap();
        }
    }
}

/// `text` with `edits` applied, reading their ranges in the encoding.
fn apply(text: &str, edits: &Value, utf8: bool) -> String {
    let offset = |p: &Value| {
        let line = p["line"].as_u64().unwrap() as usize;
        let character = p["character"].as_u64().unwrap() as usize;
        let start: usize = text.split_inclusive('\n').take(line).map(str::len).sum();
        let mut units = 0;

        for (i, c) in text[start..].char_indices() {
            if units >= character || c == '\n' {
                return start + i;
            }

            units += if utf8 { c.len_utf8() } else { c.len_utf16() };
        }

        text.len()
    };
    let mut out = text.to_string();
    let mut edits: Vec<&Value> = edits.as_array().unwrap().iter().collect();

    edits.sort_by_key(|e| std::cmp::Reverse(offset(&e["range"]["start"])));

    for e in edits {
        let (start, end) = (offset(&e["range"]["start"]), offset(&e["range"]["end"]));

        out.replace_range(start..end, e["newText"].as_str().unwrap());
    }

    out
}

/// The diagnostic with `id`, of those published.
fn with_id<'a>(diagnostics: &'a [Value], id: &str) -> &'a Value {
    diagnostics
        .iter()
        .find(|d| d["code"] == id)
        .unwrap_or_else(|| panic!("no {} in {:?}", id, diagnostics))
}

/// Checks that every action fixes its diagnostic without adding an error,
/// of the actions for it (the request's range may have others, and an
/// action may fix several), by the oracle: `inko check` on the project
/// with it applied reports
/// fewer of the diagnostic than on the project as it is, and each error
/// (by file, ID and message) at most as often as before, less the one
/// fixed. Returns their titles and the main file after each.
fn fixed(project: &Project, actions: &Value, fixes: &Value, file: &str) -> Vec<(String, String)> {
    let before = inko_check(&project.root);
    let key = |d: &Value| {
        (
            d["file"]
                .as_str()
                .unwrap_or_default()
                .rsplit('/')
                .next()
                .unwrap()
                .to_string(),
            d["id"].as_str().unwrap_or_default().to_string(),
            d["message"].as_str().unwrap_or_default().to_string(),
        )
    };
    let count = |list: &[Value], k: &(String, String, String), errors: bool| {
        list.iter()
            .filter(|d| (!errors || d["level"] == "error") && key(d) == *k)
            .count()
    };
    let is_fixed = |d: &Value| d["id"] == fixes["code"] && d["message"] == fixes["message"];

    actions
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| {
            a["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d == fixes)
        })
        .map(|a| {
            assert_eq!(a["kind"], "quickfix");

            let after = project.checked(a);

            assert!(
                after.iter().filter(|d| is_fixed(d)).count()
                    < before.iter().filter(|d| is_fixed(d)).count(),
                "{} left {:?}",
                a["title"],
                after
            );

            for e in after.iter().filter(|d| d["level"] == "error") {
                let k = key(e);
                let less = usize::from(is_fixed(e));

                assert!(
                    count(&after, &k, true) <= count(&before, &k, true).saturating_sub(less),
                    "{} added {}: before {:?}, after {:?}",
                    a["title"],
                    e,
                    before,
                    after
                );
            }

            let text = project
                .applied(a)
                .remove(&project.path(file))
                .unwrap_or_else(|| std::fs::read_to_string(project.path(file)).unwrap());

            (a["title"].as_str().unwrap().to_string(), text)
        })
        .collect()
}

const LIB: &str = "fn pub helper -> Int {
  1
}
";

/// Another module with a `helper`, of another type: importing it doesn't
/// type check.
const OTHER: &str = "fn pub helper -> String {
  'a'
}
";

#[test]
fn an_undefined_symbol_is_imported_from_where_it_type_checks() {
    let main = "import std.stdio (Stdout)

type async Main {
  fn async main {
    let n = 'héllo 中文😀'.size + helper

    Stdout.new.print(n.to_string)
  }
}
";

    for utf8 in [false, true] {
        let mut project = Project::new(
            "import",
            utf8,
            &[
                ("src/main.inko", main),
                ("src/lib.inko", LIB),
                ("src/other.inko", OTHER),
            ],
            json!({}),
        );
        let published = project.open("src/main.inko");
        let undefined = with_id(&published, "invalid-symbol");
        let (id, actions) =
            project.actions("src/main.inko", std::slice::from_ref(undefined), json!({}));
        let fixes = fixed(&project, &actions, undefined, "src/main.inko");

        assert_eq!(client_status(&project, id), "fresh");
        assert_eq!(
            fixes,
            [(
                "Import helper from lib".to_string(),
                main.replace(
                    "import std.stdio (Stdout)",
                    "import lib (helper)\nimport std.stdio (Stdout)"
                )
            )],
            "UTF-8: {}",
            utf8
        );
        // The only fix is the preferred one.
        assert_eq!(actions[0]["isPreferred"], true);
    }
}

fn client_status(project: &Project, id: i64) -> Value {
    project.client.status(id).unwrap()["status"].clone()
}

#[test]
fn several_imports_that_type_check_are_all_offered() {
    let main = "type async Main {
  fn async main {
    let _ = helper
  }
}
";
    let mut project = Project::new(
        "imports",
        false,
        &[
            ("src/main.inko", main),
            ("src/lib.inko", LIB),
            ("src/other.inko", OTHER),
        ],
        json!({}),
    );
    let published = project.open("src/main.inko");
    let undefined = with_id(&published, "invalid-symbol");
    let (_, actions) = project.actions("src/main.inko", std::slice::from_ref(undefined), json!({}));
    let titles: Vec<String> = fixed(&project, &actions, undefined, "src/main.inko")
        .into_iter()
        .map(|(t, _)| t)
        .collect();

    assert_eq!(
        titles,
        ["Import helper from lib", "Import helper from other"]
    );
    assert!(
        actions
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a.get("isPreferred").is_none())
    );
}

#[test]
fn unused_imports_variables_and_results_are_fixed() {
    let main = "import std.stdio (Stdout, Stderr)
import std.fs.path (Path)

type async Main {
  fn async main {
    let unused = 'é'
    let out = Stdout.new

    out.print('a')
  }
}
";

    for utf8 in [false, true] {
        let mut project = Project::new("unused", utf8, &[("src/main.inko", main)], json!({}));
        let published = project.open("src/main.inko");
        let fix = |project: &mut Project, published: &[Value], message: &str| {
            let d = published
                .iter()
                .find(|d| d["message"] == message)
                .unwrap_or_else(|| panic!("no {:?} in {:?}", message, published))
                .clone();
            let (_, actions) =
                project.actions("src/main.inko", std::slice::from_ref(&d), json!({}));
            let fixes = fixed(project, &actions, &d, "src/main.inko");

            // None for the diagnostics outside the range.
            assert_eq!(
                fixes.len(),
                actions.as_array().unwrap().len(),
                "{}",
                actions
            );
            fixes
        };

        assert_eq!(
            fix(&mut project, &published, "the symbol 'Stderr' is unused"),
            [(
                "Remove Stderr from the import".to_string(),
                main.replace("(Stdout, Stderr)", "(Stdout)")
            )]
        );
        assert_eq!(
            fix(&mut project, &published, "the symbol 'Path' is unused"),
            [(
                "Remove the unused import".to_string(),
                main.replace("import std.fs.path (Path)\n", "")
            )]
        );
        assert_eq!(
            fix(&mut project, &published, "the symbol 'unused' is unused"),
            [(
                "Rename unused to _unused".to_string(),
                main.replace("let unused", "let _unused")
            )]
        );
    }

    // The last expression is the method's value, so the one before.
    let main = "type async Main {
  fn async main {
    1 + 2
    let _ = 0
  }
}
";
    let mut project = Project::new("result", false, &[("src/main.inko", main)], json!({}));
    let published = project.open("src/main.inko");
    let unused = with_id(&published, "unused-result");
    let (_, actions) = project.actions("src/main.inko", std::slice::from_ref(unused), json!({}));

    assert_eq!(
        fixed(&project, &actions, unused, "src/main.inko"),
        [(
            "Assign the result to _".to_string(),
            main.replace("    1 + 2", "    let _ = 1 + 2")
        )]
    );
}

#[test]
fn an_immutable_variable_or_field_is_made_mutable_where_its_defined() {
    let main = "type async Main {
  fn async main {
    let x = 'é'

    x = 'ü'
    let _ = x
  }
}
";
    let counter = "type pub Counter {
  let pub @count: Int

  fn pub mut add {
    @count = @count + 1
  }
}
";
    let uses = "import counter (Counter)

type async Main {
  fn async main {
    let c = Counter(count: 0)

    c.add
  }
}
";

    for utf8 in [false, true] {
        let mut project = Project::new("variable", utf8, &[("src/main.inko", main)], json!({}));
        let published = project.open("src/main.inko");
        let assign = with_id(&published, "invalid-assign");
        let (_, actions) =
            project.actions("src/main.inko", std::slice::from_ref(assign), json!({}));

        assert_eq!(
            fixed(&project, &actions, assign, "src/main.inko"),
            [(
                "Make x mutable".to_string(),
                main.replace("let x", "let mut x")
            )],
            "UTF-8: {}",
            utf8
        );

        // A field, defined in another module than the one asked about.
        let mut project = Project::new(
            "field",
            utf8,
            &[("src/main.inko", uses), ("src/counter.inko", counter)],
            json!({}),
        );
        let published = project.open("src/counter.inko");
        let field = with_id(&published, "invalid-assign");
        let (_, actions) =
            project.actions("src/counter.inko", std::slice::from_ref(field), json!({}));

        assert_eq!(
            fixed(&project, &actions, field, "src/counter.inko"),
            [(
                "Make count mutable".to_string(),
                counter.replace("let pub @count", "let pub mut @count")
            )]
        );
    }
}

/// A file with other errors still gets the fixes that remove their
/// diagnostic and add no error: both undefined names are imported, each
/// on its own, the other left undefined.
#[test]
fn fixes_are_offered_in_a_file_with_other_errors() {
    let main = "import std.stdio (Stderr)

type async Main {
  fn async main {
    let _ = helper
    let _ = Stdout.new
    let _ = nope
  }
}
";

    for utf8 in [false, true] {
        let mut project = Project::new(
            "others",
            utf8,
            &[("src/main.inko", main), ("src/lib.inko", LIB)],
            json!({}),
        );
        let published = project.open("src/main.inko");
        let find = |message: &str| {
            published
                .iter()
                .find(|d| d["message"] == message)
                .unwrap_or_else(|| panic!("no {:?} in {:?}", message, published))
                .clone()
        };

        for (message, title, before, after) in [
            (
                "the symbol 'helper' is undefined",
                "Import helper from lib",
                "import std.stdio (Stderr)",
                "import lib (helper)\nimport std.stdio (Stderr)",
            ),
            (
                "the symbol 'Stdout' is undefined",
                "Import Stdout from std.stdio",
                "import std.stdio (Stderr)",
                "import std.stdio (Stderr, Stdout)",
            ),
        ] {
            let d = find(message);
            let (id, actions) =
                project.actions("src/main.inko", std::slice::from_ref(&d), json!({}));

            assert_eq!(
                fixed(&project, &actions, &d, "src/main.inko"),
                [(title.to_string(), main.replace(before, after))],
                "UTF-8: {}",
                utf8
            );
            assert_eq!(client_status(&project, id), "fresh");
        }
    }
}

/// A fix that would add an error isn't offered, though the file has
/// another: `other`'s `helper` is a `String`, where an `Int` is needed.
#[test]
fn no_fix_is_offered_that_adds_an_error() {
    let main = "type async Main {
  fn async main {
    let a: Int = helper
    let _ = a
    let _ = nope
  }
}
";
    let mut project = Project::new(
        "adds",
        false,
        &[("src/main.inko", main), ("src/other.inko", OTHER)],
        json!({}),
    );
    let published = project.open("src/main.inko");
    let undefined = published
        .iter()
        .find(|d| d["message"] == "the symbol 'helper' is undefined")
        .unwrap()
        .clone();
    let (id, actions) =
        project.actions("src/main.inko", std::slice::from_ref(&undefined), json!({}));

    assert_eq!(actions, json!([]));
    assert_eq!(client_status(&project, id), "fresh");

    // With `lib`'s too, only that one.
    let mut project = Project::new(
        "adds-lib",
        false,
        &[
            ("src/main.inko", main),
            ("src/other.inko", OTHER),
            ("src/lib.inko", LIB),
        ],
        json!({}),
    );
    let published = project.open("src/main.inko");
    let undefined = published
        .iter()
        .find(|d| d["message"] == "the symbol 'helper' is undefined")
        .unwrap()
        .clone();
    let (_, actions) =
        project.actions("src/main.inko", std::slice::from_ref(&undefined), json!({}));
    let titles: Vec<String> = fixed(&project, &actions, &undefined, "src/main.inko")
        .into_iter()
        .map(|(t, _)| t)
        .collect();

    assert_eq!(titles, ["Import helper from lib"]);
}

/// A client that sends no diagnostics in the context (it has its own)
/// still gets imports, for the names in the requested range.
#[test]
fn imports_are_offered_without_diagnostics_in_the_context() {
    let main = "type async Main {
  fn async main {
    let _ = helper
  }
}
";
    let mut project = Project::new(
        "nocontext",
        false,
        &[("src/main.inko", main), ("src/lib.inko", LIB)],
        json!({}),
    );
    let published = project.open("src/main.inko");
    let undefined = with_id(&published, "invalid-symbol").clone();
    let id = project.client.send_request(
        "textDocument/codeAction",
        json!({
            "textDocument": { "uri": uri(&project.path("src/main.inko")) },
            // Inside the name, not all of it.
            "range": {
                "start": { "line": 2, "character": 13 },
                "end": { "line": 2, "character": 14 },
            },
            "context": { "diagnostics": [] },
        }),
    );
    let actions = project.client.response(id)["result"].clone();

    assert_eq!(
        fixed(&project, &actions, &undefined, "src/main.inko"),
        [(
            "Import helper from lib".to_string(),
            format!("import lib (helper)\n\n{}", main)
        )]
    );
}

#[test]
fn code_actions_are_only_for_clients_and_kinds_that_take_them() {
    let main = "type async Main {
  fn async main {
    let _ = helper
  }
}
";
    let files = [("src/main.inko", main), ("src/lib.inko", LIB)];

    // Other kinds only: none.
    let mut project = Project::new("kinds", false, &files, json!({}));
    let published = project.open("src/main.inko");

    assert_eq!(
        project
            .actions("src/main.inko", &published, json!({ "only": ["refactor"] }))
            .1,
        json!([])
    );
    assert_eq!(
        project
            .actions("src/main.inko", &published, json!({ "only": ["quickfix"] }))
            .1
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // A client without literals (only commands) isn't offered any, nor
    // told there are any; without isPreferredSupport, none is preferred.
    let mut project = Project::new(
        "commands",
        false,
        &files,
        json!({ "textDocument": { "codeAction": null } }),
    );
    let published = project.open("src/main.inko");

    assert_eq!(
        project.actions("src/main.inko", &published, json!({})).1,
        json!([])
    );

    let mut project = Project::new(
        "preferred",
        false,
        &files,
        json!({ "textDocument": { "codeAction": { "isPreferredSupport": false } } }),
    );
    let published = project.open("src/main.inko");
    let actions = project.actions("src/main.inko", &published, json!({})).1;

    assert_eq!(actions.as_array().unwrap().len(), 1);
    assert!(actions[0].get("isPreferred").is_none());
}

/// Finds the published diagnostic with `message`.
fn with_message(published: &[Value], message: &str) -> Value {
    published
        .iter()
        .find(|d| d["message"] == message)
        .unwrap_or_else(|| panic!("no {:?} in {:?}", message, published))
        .clone()
}

#[test]
fn a_missing_field_is_added_to_the_call() {
    let main = "type Point {
  let @x: Int
  let @y: Float
  let @name: String
  let @tags: Array[String]
  let @other: Main
}

type async Main {
  fn async main {
    let name = 'é'
    let _ = Point(x: 1)
  }
}
";

    for utf8 in [false, true] {
        let mut project =
            Project::new("missing-field", utf8, &[("src/main.inko", main)], json!({}));
        let published = project.open("src/main.inko");

        for (field, fixes) in [
            ("y", vec![("Add y: 0.0", "Point(x: 1, y: 0.0)")]),
            (
                "name",
                vec![
                    ("Add name: ''", "Point(x: 1, name: '')"),
                    ("Add name: name", "Point(x: 1, name: name)"),
                ],
            ),
            ("tags", vec![("Add tags: []", "Point(x: 1, tags: [])")]),
            // No value of its type to give, and no variable of its name.
            ("other", vec![]),
        ] {
            let d = with_message(
                &published,
                &format!("the field '{}' must be assigned a value", field),
            );
            let (_, actions) =
                project.actions("src/main.inko", std::slice::from_ref(&d), json!({}));
            let wanted: Vec<(String, String)> = fixes
                .into_iter()
                .map(|(t, call)| (t.to_string(), main.replace("Point(x: 1)", call)))
                .collect();

            assert_eq!(
                fixed(&project, &actions, &d, "src/main.inko"),
                wanted,
                "{} (UTF-8: {})",
                field,
                utf8
            );
        }
    }

    // An empty call, and one across lines with a trailing comma.
    let main = "type Point {
  let @x: Int
  let @y: Int
}

type async Main {
  fn async main {
    let _ = Point(
      x: 1,
    )
    let _ = Point()
  }
}
";
    let mut project = Project::new(
        "missing-fields",
        false,
        &[("src/main.inko", main)],
        json!({}),
    );
    let published = project.open("src/main.inko");
    let mut titles = Vec::new();

    for d in published.iter().filter(|d| d["code"] == "missing-field") {
        let (_, actions) = project.actions("src/main.inko", std::slice::from_ref(d), json!({}));

        titles.extend(fixed(&project, &actions, d, "src/main.inko"));
    }

    assert!(titles.contains(&(
        "Add y: 0".to_string(),
        main.replace("      x: 1,\n", "      x: 1, y: 0\n")
    )));
    assert!(titles.contains(&(
        "Add x: 0".to_string(),
        main.replace("Point()", "Point(x: 0)")
    )));
}

const SHAPES: &str = "import std.fmt (Format, Formatter)

trait Shape: Format {
  fn area -> Int

  fn pub mut scale(by: Int, label: String) -> Array[Int]

  fn twice -> Int {
    area * 2
  }
}

trait Gen[T] {
  fn next -> Option[T]
}

type Point {
  let @x: Int
}

impl Format for Point {
  fn pub fmt(formatter: mut Formatter) {}
}

impl Shape for Point {
  fn area -> Int {
    1
  }
}

impl Gen[Int] for Point {}

type async Main {
  fn async main {}
}
";

#[test]
fn an_implementation_gets_the_methods_it_lacks() {
    for utf8 in [false, true] {
        let mut project = Project::new("stubs", utf8, &[("src/main.inko", SHAPES)], json!({}));
        let published = project.open("src/main.inko");
        let scale = with_message(
            &published,
            "the method 'fn pub mut scale(by: Int, label: String) -> Array[Int]' must be implemented for 'Point'",
        );
        let (_, actions) =
            project.actions("src/main.inko", std::slice::from_ref(&scale), json!({}));

        assert_eq!(
            fixed(&project, &actions, &scale, "src/main.inko"),
            [(
                "Implement scale".to_string(),
                SHAPES.replace(
                    "    1\n  }\n}",
                    "    1\n  }\n\n  fn pub mut scale(by: Int, label: String) -> Array[Int] {\n    panic('not implemented')\n  }\n}"
                )
            )],
            "UTF-8: {}",
            utf8
        );

        // A trait's type parameter is the block's type argument, in a body
        // that was empty.
        let next = with_message(
            &published,
            "the method 'fn next -> Option[T]' must be implemented for 'Point'",
        );
        let (_, actions) = project.actions("src/main.inko", std::slice::from_ref(&next), json!({}));

        assert_eq!(
            fixed(&project, &actions, &next, "src/main.inko"),
            [(
                "Implement next".to_string(),
                SHAPES.replace(
                    "impl Gen[Int] for Point {}",
                    "impl Gen[Int] for Point {\n  fn next -> Option[Int] {\n    panic('not implemented')\n  }\n}"
                )
            )]
        );
    }

    // A trait imported from std, whose type parameter its own module
    // names.
    let main = "import std.iter (Iter)

type Numbers {
  let @n: Int
}

impl Iter[Int] for Numbers {}

type async Main {
  fn async main {}
}
";
    let mut project = Project::new("stubs-std", false, &[("src/main.inko", main)], json!({}));
    let published = project.open("src/main.inko");
    let next = with_id(&published, "invalid-implementation").clone();
    let (_, actions) = project.actions("src/main.inko", std::slice::from_ref(&next), json!({}));

    assert_eq!(
        fixed(&project, &actions, &next, "src/main.inko"),
        [(
            "Implement next".to_string(),
            main.replace(
                "impl Iter[Int] for Numbers {}",
                "impl Iter[Int] for Numbers {\n  fn pub mut next -> Option[Int] {\n    panic('not implemented')\n  }\n}"
            )
        )]
    );

    // Several, in one fix for each of their diagnostics.
    let text = SHAPES.replace("  fn area -> Int {\n    1\n  }\n", "");
    let mut project = Project::new("stubs-all", false, &[("src/main.inko", &text)], json!({}));
    let published = project.open("src/main.inko");
    let area = with_message(
        &published,
        "the method 'fn area -> Int' must be implemented for 'Point'",
    );
    let (_, actions) = project.actions("src/main.inko", std::slice::from_ref(&area), json!({}));

    // One action, for both diagnostics.
    assert_eq!(actions.as_array().unwrap().len(), 1, "{}", actions);
    assert_eq!(
        actions[0]["diagnostics"].as_array().unwrap().len(),
        2,
        "{}",
        actions
    );
    assert_eq!(
        fixed(&project, &actions, &area, "src/main.inko"),
        [(
            "Implement the missing methods".to_string(),
            text.replace(
                "impl Shape for Point {\n}",
                "impl Shape for Point {\n  fn area -> Int {\n    panic('not implemented')\n  }\n\n  fn pub mut scale(by: Int, label: String) -> Array[Int] {\n    panic('not implemented')\n  }\n}"
            )
        )]
    );
}

#[test]
fn a_missing_trait_is_implemented_with_its_methods() {
    let text = SHAPES
        .replace(
            "impl Format for Point {\n  fn pub fmt(formatter: mut Formatter) {}\n}\n\n",
            "",
        )
        .replace(
            "  fn area -> Int {\n    1\n  }\n",
            "  fn area -> Int {\n    1\n  }\n\n  fn pub mut scale(by: Int, label: String) -> Array[Int] {\n    []\n  }\n",
        )
        .replace("impl Gen[Int] for Point {}\n\n", "");

    for utf8 in [false, true] {
        let mut project = Project::new("trait", utf8, &[("src/main.inko", &text)], json!({}));
        let published = project.open("src/main.inko");
        let missing = with_id(&published, "missing-trait").clone();
        let (_, actions) =
            project.actions("src/main.inko", std::slice::from_ref(&missing), json!({}));
        let block = "  fn pub mut scale(by: Int, label: String) -> Array[Int] {\n    []\n  }\n}";

        assert_eq!(
            fixed(&project, &actions, &missing, "src/main.inko"),
            [(
                "Implement Format for Point".to_string(),
                text.replace(
                    block,
                    &format!(
                        "{}\n\nimpl Format for Point {{\n  fn pub fmt(formatter: mut Formatter) {{\n    panic('not implemented')\n  }}\n}}",
                        block
                    )
                )
            )],
            "UTF-8: {}",
            utf8
        );
    }
}

#[test]
fn unreachable_code_is_removed() {
    let main = "type async Main {
  fn async main {
    let _ = 1

    return

    let _ = 2
    let _ = [
      3,
    ]
  }

  fn f(v: Option[Int]) -> Int {
    match v {
      case Some(_) -> 1
      case Some(2) -> {
        3
      }
      case None -> 2
    }
  }
}
";

    for utf8 in [false, true] {
        let mut project = Project::new("unreachable", utf8, &[("src/main.inko", main)], json!({}));
        let published = project.open("src/main.inko");
        let mut fixes = Vec::new();

        for d in published.iter().filter(|d| d["code"] == "unreachable") {
            let (_, actions) = project.actions("src/main.inko", std::slice::from_ref(d), json!({}));

            fixes.extend(fixed(&project, &actions, d, "src/main.inko"));
        }

        assert_eq!(
            fixes,
            [
                (
                    "Remove the unreachable code".to_string(),
                    main.replace(
                        "    return\n\n    let _ = 2\n    let _ = [\n      3,\n    ]\n",
                        "    return\n"
                    )
                ),
                (
                    "Remove the unreachable code".to_string(),
                    main.replace("      case Some(2) -> {\n        3\n      }\n", "")
                ),
            ],
            "UTF-8: {}",
            utf8
        );
    }
}
