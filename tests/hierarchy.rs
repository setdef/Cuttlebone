//! Implementations, declarations, and call and type hierarchies over LSP
//! (docs/lsp.md), in both encodings, across the project, a dependency in
//! `dep/` and std, by the compiler's ids.
use crate::common::client::{Client, Launch, at, uri};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use url::Url;

const GEO: &str = "# A shape.
trait pub Shape {
  fn pub area -> Int
}

type pub Square {
  let @side: Int
}

impl Shape for Square {
  fn pub area -> Int {
    @side * @side
  }
}
";

const SHAPES: &str = "import geo (Shape)
import std.string (ToString)

trait pub Named {
  fn pub name -> String
}

trait pub Labelled: Named {
  fn pub label -> String {
    '日本 😀 ${name}'
  }
}

type pub Circle {
  let @r: Int
}

impl Shape for Circle {
  fn pub area -> Int {
    @r * @r
  }
}

impl Named for Circle {
  fn pub name -> String {
    'circle'
  }
}

impl Labelled for Circle {}

type pub Box {}

impl Named for Box {
  fn pub name -> String {
    'box'
  }
}

impl Labelled for Box {
  fn pub label -> String {
    'b'
  }
}

impl ToString for Box {
  fn pub to_string -> String {
    'box'
  }
}
";

const MAIN: &str = "import shapes (Box, Circle, Named)

fn describe[T: Named](value: ref T) -> String {
  '日本 😀' + value.name
}

fn helper -> Int {
  1
}

fn total(c: ref Circle) -> Int {
  let s = '日本 😀'.size + c.area

  s + helper + helper
}

type async Main {
  fn async main {
    let c = Circle(r: 2)

    describe(c)
    total(c)
    Box().to_string
  }
}

fn extern getpid -> Int32

fn pid -> Int32 {
  getpid
}
";

fn project(name: &str) -> PathBuf {
    let root = crate::common::tmp().join(format!("hierarchy-{}", name));
    let _ = std::fs::remove_dir_all(&root);
    let url = "https://example.com/acme/geo";
    let dir = compiler::pkg::manifest::Url::parse(url)
        .unwrap()
        .directory_name();
    let package = root.join("dep").join(&dir).join("v1");

    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(package.join("src")).unwrap();
    std::fs::write(
        root.join("inko.pkg"),
        format!(
            "require {} 1.0.0 0123456789012345678901234567890123456789\n",
            url
        ),
    )
    .unwrap();
    std::fs::write(package.join("inko.pkg"), "").unwrap();
    std::fs::write(package.join("src/geo.inko"), GEO).unwrap();
    std::fs::write(root.join("src/shapes.inko"), SHAPES).unwrap();
    std::fs::write(root.join("src/main.inko"), MAIN).unwrap();
    root.canonicalize().unwrap()
}

fn server(root: &Path, utf8: bool) -> Client {
    Client::initialized_with(
        Launch::default(),
        root,
        json!({ "statusNotifications": true, "diagnostics": false }),
        json!({
            "general": { "positionEncodings": if utf8 { ["utf-8"] } else { ["utf-16"] } },
        }),
    )
}

/// A file's name, the 0-based line of a range, and its text, in either
/// encoding.
fn spot(root: &Path, uri: &Value, range: &Value, utf8: bool) -> (String, u32, String) {
    let path = Url::parse(uri.as_str().unwrap())
        .unwrap()
        .to_file_path()
        .unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let line = range["start"]["line"].as_u64().unwrap() as u32;
    let units = |p: &Value| p["character"].as_u64().unwrap() as usize;
    let line_text = text.lines().nth(line as usize).unwrap();
    let slice = |from: usize, to: usize| -> String {
        if utf8 {
            line_text[from..to].to_string()
        } else {
            let units: Vec<u16> = line_text.encode_utf16().collect();

            String::from_utf16(&units[from..to]).unwrap()
        }
    };
    let name = path
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .trim_end_matches(".inko")
        .to_string();
    let _ = root;

    assert_eq!(range["end"]["line"], line);
    (
        name,
        line,
        slice(units(&range["start"]), units(&range["end"])),
    )
}

fn locations(root: &Path, result: &Value, utf8: bool) -> Vec<(String, u32, String)> {
    let list = match result {
        Value::Array(list) => list.clone(),
        Value::Null => return Vec::new(),
        one => vec![one.clone()],
    };

    list.iter()
        .map(|l| spot(root, &l["uri"], &l["range"], utf8))
        .collect()
}

/// A request at `needle` (plus `offset` characters) in a file.
fn ask(
    client: &mut Client,
    method: &str,
    file: &Path,
    needle: &str,
    offset: usize,
    utf8: bool,
) -> Value {
    let text = std::fs::read_to_string(file).unwrap();
    let (line, character) = at(&text, needle, offset, utf8);
    let id = client.send_at(method, file, line, character);
    let response = client.response(id);

    assert!(response.get("error").is_none(), "{}", response);
    response["result"].clone()
}

/// A request's result, which must not be an error.
fn result(client: &mut Client, method: &str, params: Value) -> Value {
    let response = client.request(method, params);

    assert!(response.get("error").is_none(), "{}", response);
    response["result"].clone()
}

/// A file's name, a line, and the text there (`spot`).
type Spot = (String, u32, String);

fn s(file: &str, line: u32, text: &str) -> (String, u32, String) {
    (file.to_string(), line, text.to_string())
}

#[test]
fn implementations_of_traits_and_their_methods() {
    let root = project("implementations");

    for utf8 in [false, true] {
        let mut client = server(&root, utf8);
        let shapes = root.join("src/shapes.inko");
        let main = root.join("src/main.inko");
        let geo = std::fs::read_dir(root.join("dep"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path()
            .join("v1/src/geo.inko");
        let mut implementations = |file: &Path, needle: &str, offset: usize| {
            let result = ask(
                &mut client,
                "textDocument/implementation",
                file,
                needle,
                offset,
                utf8,
            );

            locations(&root, &result, utf8)
        };

        // A trait: the types implementing it.
        assert_eq!(
            implementations(&shapes, "trait pub Named", 10),
            vec![s("shapes", 13, "Circle"), s("shapes", 31, "Box")]
        );
        // From a name that refers to it.
        assert_eq!(
            implementations(&main, "T: Named", 3),
            vec![s("shapes", 13, "Circle"), s("shapes", 31, "Box")]
        );
        // A required method: every implementation.
        assert_eq!(
            implementations(&shapes, "fn pub name -> String\n", 7),
            vec![s("shapes", 24, "name"), s("shapes", 34, "name")]
        );
        // A call through a type parameter's bound is the trait's method.
        assert_eq!(
            implementations(&main, "value.name", 6),
            vec![s("shapes", 24, "name"), s("shapes", 34, "name")]
        );
        // A default method: only where it's redefined, not inherited.
        assert_eq!(
            implementations(&shapes, "fn pub label -> String {\n    '日本", 7),
            vec![s("shapes", 40, "label")]
        );
        // A dependency's trait, asked in its own file: the dependency's
        // and the project's.
        assert_eq!(
            implementations(&geo, "trait pub Shape", 10),
            vec![s("geo", 5, "Square"), s("shapes", 13, "Circle")]
        );
        assert_eq!(
            implementations(&geo, "fn pub area -> Int\n", 7),
            vec![s("geo", 10, "area"), s("shapes", 18, "area")]
        );

        // A std trait: std's types too, and Box.
        let std_trait = implementations(&shapes, "impl ToString", 5);

        assert!(
            std_trait.contains(&s("shapes", 31, "Box")),
            "{:?}",
            std_trait
        );
        assert!(
            std_trait
                .iter()
                .any(|(f, _, t)| f == "string" && t == "String")
        );

        // A type, and a type's own method, have none.
        assert_eq!(
            ask(
                &mut client,
                "textDocument/implementation",
                &shapes,
                "type pub Circle",
                9,
                utf8
            ),
            Value::Null
        );
        assert_eq!(
            ask(
                &mut client,
                "textDocument/implementation",
                &main,
                "c.area",
                2,
                utf8
            ),
            Value::Null
        );
    }
}

#[test]
fn declarations_of_implementations_are_the_traits_methods() {
    let root = project("declarations");

    for utf8 in [false, true] {
        let mut client = server(&root, utf8);
        let shapes = root.join("src/shapes.inko");
        let main = root.join("src/main.inko");
        let mut declaration = |file: &Path, needle: &str, offset: usize| {
            let result = ask(
                &mut client,
                "textDocument/declaration",
                file,
                needle,
                offset,
                utf8,
            );

            locations(&root, &result, utf8)
        };

        // The call is to Circle's area, which implements Shape's.
        assert_eq!(declaration(&main, "c.area", 2), vec![s("geo", 2, "area")]);
        // At the implementation's own name.
        assert_eq!(
            declaration(&shapes, "fn pub name -> String {\n    'circle", 7),
            vec![s("shapes", 4, "name")]
        );
        // A redefined default method.
        assert_eq!(
            declaration(&shapes, "fn pub label -> String {\n    'b'", 7),
            vec![s("shapes", 8, "label")]
        );
        // Anything else: where it's defined.
        assert_eq!(
            declaration(&main, "describe(c)", 0),
            vec![s("main", 2, "describe")]
        );
        assert_eq!(
            declaration(&main, "Circle(r", 0),
            vec![s("shapes", 13, "Circle")]
        );
        // Box's implements std's ToString.
        let std_method = declaration(&main, "Box().to", 6);

        assert_eq!(std_method.len(), 1);
        assert_eq!(
            (&*std_method[0].0, &*std_method[0].2),
            ("string", "to_string")
        );
    }
}

/// An item's name, kind, detail, and where its name is.
fn item(root: &Path, item: &Value, utf8: bool) -> (String, u64, String, (String, u32, String)) {
    // The whole declaration contains its name.
    let range = &item["range"];
    let selection = &item["selectionRange"];
    let start = |r: &Value| {
        (
            r["start"]["line"].as_u64(),
            r["start"]["character"].as_u64(),
        )
    };
    let end = |r: &Value| (r["end"]["line"].as_u64(), r["end"]["character"].as_u64());

    assert!(
        start(range) <= start(selection) && end(selection) <= end(range),
        "{}",
        item
    );

    (
        item["name"].as_str().unwrap().to_string(),
        item["kind"].as_u64().unwrap(),
        item["detail"].as_str().unwrap().to_string(),
        spot(root, &item["uri"], selection, utf8),
    )
}

#[test]
fn call_hierarchies_by_the_compilers_ids() {
    let root = project("calls");

    for utf8 in [false, true] {
        let mut client = server(&root, utf8);
        let main = root.join("src/main.inko");

        // At a call.
        let prepared = ask(
            &mut client,
            "textDocument/prepareCallHierarchy",
            &main,
            "total(c)",
            0,
            utf8,
        );

        assert_eq!(prepared.as_array().unwrap().len(), 1);
        // The whole declaration, to its closing brace.
        assert_eq!(
            prepared[0]["range"],
            json!({ "start": { "line": 10, "character": 0 }, "end": { "line": 14, "character": 1 } })
        );
        assert_eq!(
            item(&root, &prepared[0], utf8),
            (
                "total".to_string(),
                12,
                "main".to_string(),
                s("main", 10, "total")
            )
        );

        let incoming = result(
            &mut client,
            "callHierarchy/incomingCalls",
            json!({ "item": prepared[0] }),
        );
        let incoming = incoming.as_array().unwrap();

        assert_eq!(incoming.len(), 1);
        assert_eq!(
            item(&root, &incoming[0]["from"], utf8),
            (
                "main".to_string(),
                6,
                "main.Main".to_string(),
                s("main", 17, "main")
            )
        );
        assert_eq!(
            incoming[0]["fromRanges"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| spot(&root, &json!(uri(&main)), r, utf8))
                .collect::<Vec<_>>(),
            vec![s("main", 21, "total")]
        );

        let outgoing = result(
            &mut client,
            "callHierarchy/outgoingCalls",
            json!({ "item": prepared[0] }),
        );
        let got: Vec<(String, Vec<Spot>)> = outgoing
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                (
                    item(&root, &c["to"], utf8).3.0.clone()
                        + "."
                        + c["to"]["name"].as_str().unwrap(),
                    c["fromRanges"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|r| spot(&root, &json!(uri(&main)), r, utf8))
                        .collect(),
                )
            })
            .collect();

        let mut names: Vec<&str> = got.iter().map(|(n, _)| n.as_str()).collect();

        names.sort();
        assert_eq!(
            names,
            vec!["int.+", "main.helper", "shapes.area", "string.size"]
        );
        assert!(
            got.contains(&(
                "main.helper".to_string(),
                vec![s("main", 13, "helper"), s("main", 13, "helper")]
            )),
            "{:?}",
            got
        );
        assert!(
            got.contains(&("shapes.area".to_string(), vec![s("main", 11, "area")])),
            "{:?}",
            got
        );

        // A trait's method: its callers call it through a bound.
        let prepared = ask(
            &mut client,
            "textDocument/prepareCallHierarchy",
            &root.join("src/shapes.inko"),
            "fn pub name -> String\n",
            7,
            utf8,
        );

        assert_eq!(
            item(&root, &prepared[0], utf8),
            (
                "name".to_string(),
                6,
                "shapes.Named".to_string(),
                s("shapes", 4, "name")
            )
        );

        let incoming = result(
            &mut client,
            "callHierarchy/incomingCalls",
            json!({ "item": prepared[0] }),
        );
        let callers: Vec<String> = incoming
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["from"]["name"].as_str().unwrap().to_string())
            .collect();

        assert_eq!(callers, vec!["describe", "label"]);

        // An external function, which has no receiver.
        let external = ask(
            &mut client,
            "textDocument/prepareCallHierarchy",
            &main,
            "getpid\n}",
            0,
            utf8,
        );

        assert_eq!(
            (&external[0]["name"], &external[0]["kind"]),
            (&json!("getpid"), &json!(12))
        );

        // Not a method: none.
        assert_eq!(
            ask(
                &mut client,
                "textDocument/prepareCallHierarchy",
                &main,
                "Circle(r",
                0,
                utf8
            ),
            Value::Null
        );
    }
}

fn names(items: &Value) -> Vec<String> {
    items
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn type_hierarchies_from_the_type_database() {
    let root = project("types");

    for utf8 in [false, true] {
        let mut client = server(&root, utf8);
        let main = root.join("src/main.inko");
        let shapes = root.join("src/shapes.inko");
        let prepared = ask(
            &mut client,
            "textDocument/prepareTypeHierarchy",
            &main,
            "Circle(r",
            0,
            utf8,
        );

        assert_eq!(
            item(&root, &prepared[0], utf8),
            (
                "Circle".to_string(),
                5,
                "shapes".to_string(),
                s("shapes", 13, "Circle")
            )
        );

        // A type's: the traits it implements, by where they're declared.
        let supertypes = result(
            &mut client,
            "typeHierarchy/supertypes",
            json!({ "item": prepared[0] }),
        );

        assert_eq!(names(&supertypes), vec!["Shape", "Named", "Labelled"]);
        assert_eq!(item(&root, &supertypes[0], utf8).3, s("geo", 1, "Shape"));
        assert_eq!(
            result(
                &mut client,
                "typeHierarchy/subtypes",
                json!({ "item": prepared[0] })
            ),
            json!([])
        );

        // A trait's: the traits it requires; and the traits that require
        // it, and the types that implement it.
        let named = ask(
            &mut client,
            "textDocument/prepareTypeHierarchy",
            &shapes,
            "trait pub Named",
            10,
            utf8,
        );

        assert_eq!(named[0]["kind"], 11);
        assert_eq!(
            names(&result(
                &mut client,
                "typeHierarchy/subtypes",
                json!({ "item": named[0] })
            )),
            vec!["Labelled", "Circle", "Box"]
        );

        let labelled = ask(
            &mut client,
            "textDocument/prepareTypeHierarchy",
            &shapes,
            "trait pub Labelled",
            10,
            utf8,
        );

        assert_eq!(
            names(&result(
                &mut client,
                "typeHierarchy/supertypes",
                json!({ "item": labelled[0] })
            )),
            vec!["Named"]
        );

        // An item whose name isn't at its range any more: none.
        let mut moved = prepared[0].clone();
        let (line, character) = at(MAIN, "Circle(r", 0, utf8);

        moved["uri"] = json!(uri(&main));
        moved["selectionRange"]["start"] = json!({ "line": line, "character": character });
        assert_eq!(
            result(
                &mut client,
                "typeHierarchy/supertypes",
                json!({ "item": moved })
            ),
            Value::Null
        );

        // Not a type: none.
        assert_eq!(
            ask(
                &mut client,
                "textDocument/prepareTypeHierarchy",
                &main,
                "describe(c)",
                0,
                utf8
            ),
            Value::Null
        );
    }
}

/// Implementations and the hierarchies' lists are complete or none: never
/// from a repaired copy, nor past an error in a declaration, which may
/// hide an `impl`. Preparing an item answers as definition does.
#[test]
fn whole_project_answers_are_complete_or_none() {
    let root = project("complete");
    let mut client = server(&root, false);
    let shapes = root.join("src/shapes.inko");
    let main = root.join("src/main.inko");
    let other = root.join("src/other.inko");

    // An `impl` of the trait with an error may be missing.
    std::fs::write(
        &other,
        "import shapes (Named)\n\ntype pub Thing {}\n\nimpl Named for Thingg {}\n",
    )
    .unwrap();

    let id = client.send_at("textDocument/implementation", &shapes, 3, 10);

    assert_eq!(client.response(id)["result"], Value::Null);
    assert_eq!(
        client.status(id).unwrap()["reason"]["code"],
        "declarationErrors"
    );

    std::fs::remove_file(&other).unwrap();

    // main doesn't parse: no implementation, even of shapes' trait.
    client.open(&main, &MAIN.replace("total(c)\n", "total(c\n"));

    for method in [
        "textDocument/implementation",
        "textDocument/prepareTypeHierarchy",
    ] {
        let id = client.send_at(method, &shapes, 3, 10);
        let response = client.response(id);

        if method == "textDocument/implementation" {
            assert_eq!(response["result"], Value::Null);
            assert_eq!(client.status(id).unwrap()["reason"]["code"], "noHir");
        } else {
            // A check of shapes alone, which doesn't import main.
            assert_eq!(response["result"][0]["name"], "Named");
        }
    }

    let named = result(
        &mut client,
        "textDocument/prepareTypeHierarchy",
        json!({ "textDocument": { "uri": uri(&shapes) }, "position": { "line": 3, "character": 10 } }),
    );
    let id = client.send_request("typeHierarchy/subtypes", json!({ "item": named[0] }));

    assert_eq!(client.response(id)["result"], Value::Null);
    assert_eq!(client.status(id).unwrap()["reason"]["code"], "noHir");
}

/// The whole-project answers a client asks for, by the request and its
/// status: implementations of a trait and of its method, the callers of
/// `helper` and of `total`, a type's supertypes, a trait's subtypes, and
/// a file rename.
/// `main` is main's text in the client, if it's open with other text.
fn whole_project(
    client: &mut Client,
    root: &Path,
    main: Option<&str>,
) -> Vec<(String, Value, Value)> {
    let shapes = root.join("src/shapes.inko");
    let main_text = main.map(str::to_string);
    let main = root.join("src/main.inko");
    let mut out = Vec::new();
    let mut record = |client: &mut Client, what: &str, method: &str, params: Value| {
        let id = client.send_request(method, params);
        let response = client.response(id);

        assert!(response.get("error").is_none(), "{}", response);

        let status = client.status(id).unwrap().clone();

        out.push((what.to_string(), response["result"].clone(), status));
    };
    let position = |file: &Path, needle: &str, offset: usize| {
        let text = match &main_text {
            Some(text) if file.ends_with("main.inko") => text.clone(),
            _ => std::fs::read_to_string(file).unwrap(),
        };
        let (line, character) = at(&text, needle, offset, false);

        json!({ "textDocument": { "uri": uri(file) }, "position": { "line": line, "character": character } })
    };

    record(
        client,
        "implementations of Named",
        "textDocument/implementation",
        position(&shapes, "Named {", 0),
    );
    record(
        client,
        "implementations of Named.name",
        "textDocument/implementation",
        position(&shapes, "name -> String\n}", 0),
    );

    for (what, needle) in [
        ("callers of helper", "helper -> Int"),
        ("callers of total", "total(c: "),
    ] {
        let items = result(
            client,
            "textDocument/prepareCallHierarchy",
            position(&main, needle, 0),
        );

        record(
            client,
            what,
            "callHierarchy/incomingCalls",
            json!({ "item": items[0] }),
        );
    }

    for (what, method, needle) in [
        ("supertypes of Box", "typeHierarchy/supertypes", "Box {}"),
        ("subtypes of Named", "typeHierarchy/subtypes", "Named {"),
    ] {
        let items = result(
            client,
            "textDocument/prepareTypeHierarchy",
            position(&shapes, needle, 0),
        );

        record(client, what, method, json!({ "item": items[0] }));
    }

    record(
        client,
        "renaming shapes",
        "workspace/willRenameFiles",
        json!({ "files": [{ "oldUri": uri(&shapes), "newUri": uri(&root.join("src/forms.inko")) }] }),
    );
    out
}

/// With an error in a declaration elsewhere, implementations, the
/// hierarchies' lists and file renames are answered, fresh and the same as
/// without it, wherever no unit the error makes unsafe names what they're
/// about, and refused only there; callers too in a document being edited
/// (as references are, docs/lsp.md, "Errors in declarations").
#[test]
fn whole_project_answers_past_a_declaration_error_elsewhere() {
    let root = project("past-errors");
    let mut client = server(&root, false);
    let other = root.join("src/other.inko");
    let main = root.join("src/main.inko");
    let clean = whole_project(&mut client, &root, None);

    for (what, result, status) in &clean {
        assert_eq!(status["status"], "fresh", "{}: {}", what, status);
        assert!(
            result.is_array() || result.is_object(),
            "{}: {}",
            what,
            result
        );
    }

    let answered = |got: &[(String, Value, Value)], refused: &[&str]| {
        for ((what, result, status), (_, want, _)) in got.iter().zip(&clean) {
            if refused.contains(&what.as_str()) {
                assert_eq!(*result, Value::Null, "{}", what);
                assert_eq!(
                    status["reason"]["code"], "declarationErrors",
                    "{}: {}",
                    what, status
                );
            } else {
                assert_eq!(status["status"], "fresh", "{}: {}", what, status);
                assert_eq!(result, want, "{}", what);
            }
        }
    };

    // An error that's about none of them.
    std::fs::write(&other, "fn pub broken -> Strin {\n  'x'\n}\n").unwrap();
    answered(&whole_project(&mut client, &root, None), &[]);

    // An `impl` of Named with an error: Named's implementations and
    // subtypes may miss it; the callers don't. Box's supertypes are
    // refused too: its own `impl Named for Box` mentions Named, so it isn't
    // safe, and it names Box. So is renaming shapes: other imports Named
    // from it.
    std::fs::write(
        &other,
        "import shapes (Named)\n\ntype pub Thing {}\n\nimpl Named for Thing {\n  fn pub name -> Strin {\n    'x'\n  }\n}\n",
    )
    .unwrap();
    answered(
        &whole_project(&mut client, &root, None),
        &[
            "implementations of Named",
            "implementations of Named.name",
            "supertypes of Box",
            "subtypes of Named",
            "renaming shapes",
        ],
    );
    // An `impl` of Named for a type that doesn't exist, with a method of
    // the trait's: it's missing from Named's implementations, and so is
    // its `name` from Named.name's, until the type is fixed.
    std::fs::write(
        &other,
        "import shapes (Named)\n\nimpl Named for Thingg {\n  fn pub name -> String {\n    'x'\n  }\n}\n",
    )
    .unwrap();

    let got = whole_project(&mut client, &root, None);

    for what in ["implementations of Named", "implementations of Named.name"] {
        let (_, result, status) = got.iter().find(|(w, _, _)| w == what).unwrap();

        assert_eq!(*result, Value::Null, "{}", what);
        assert_eq!(
            status["reason"]["code"], "declarationErrors",
            "{}: {}",
            what, status
        );
    }

    std::fs::remove_file(&other).unwrap();

    // Being typed in main, unsaved: a method whose signature doesn't
    // check yet, calling `helper`. Its callers may miss it, `total`'s
    // don't.
    let edited = MAIN.replace(
        "fn helper -> Int {",
        "fn added -> Strin {\n  helper\n}\n\nfn helper -> Int {",
    );

    client.open(&main, &edited);

    let got = whole_project(&mut client, &root, Some(&edited));
    let callers = |what: &str| got.iter().find(|(w, _, _)| w == what).unwrap();

    assert_eq!(callers("callers of helper").1, Value::Null);
    assert_eq!(
        callers("callers of helper").2["reason"]["code"],
        "declarationErrors"
    );
    assert_eq!(callers("callers of total").2["status"], "fresh");
    // Moved down by the four lines added above it.
    fn shifted(value: &Value, by: u64) -> Value {
        match value {
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| match (k.as_str(), v.as_u64()) {
                        ("line", Some(line)) => (k.clone(), json!(line + by)),
                        _ => (k.clone(), shifted(v, by)),
                    })
                    .collect(),
            ),
            Value::Array(list) => Value::Array(list.iter().map(|v| shifted(v, by)).collect()),
            other => other.clone(),
        }
    }

    let before = &clean
        .iter()
        .find(|(w, _, _)| w == "callers of total")
        .unwrap()
        .1;

    assert!(!before.as_array().unwrap().is_empty());
    assert_eq!(callers("callers of total").1, shifted(before, 4));
}
