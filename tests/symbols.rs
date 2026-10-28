//! Document symbols over LSP, in both encodings: every kind of
//! declaration, from the syntax alone, in code that doesn't parse, for
//! documents that aren't files, and for clients without hierarchical
//! symbols or the newer symbol kinds.
use crate::common::client::{Client, Launch, at, uri};
use crate::common::fixture;
use serde_json::{Value, json};
use std::path::Path;

fn encodings(utf8: bool) -> Value {
    if utf8 {
        json!({ "general": { "positionEncodings": ["utf-8"] } })
    } else {
        json!({ "general": { "positionEncodings": ["utf-16"] } })
    }
}

fn session(root: &Path, utf8: bool) -> Client {
    Client::initialized_with(
        Launch::default(),
        root,
        json!({ "statusNotifications": true, "diagnostics": false }),
        encodings(utf8),
    )
}

/// The document symbols of `path` (by its URI, which needn't be a file).
fn symbols(client: &mut Client, uri: &str) -> (i64, Value) {
    let id = client.send_request(
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": uri } }),
    );

    (id, client.response(id)["result"].clone())
}

/// The text a range covers, reading its characters in the encoding.
fn slice(text: &str, range: &Value, utf8: bool) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let byte = |p: &Value| -> usize {
        let line = p["line"].as_u64().unwrap() as usize;
        let start: usize = lines[..line].iter().map(|l| l.len() + 1).sum();
        let mut units = 0;

        for (i, c) in lines[line].char_indices() {
            if units >= p["character"].as_u64().unwrap() {
                return start + i;
            }

            units += if utf8 { c.len_utf8() } else { c.len_utf16() } as u64;
        }

        start + lines[line].len()
    };

    text[byte(&range["start"])..byte(&range["end"])].to_string()
}

/// Each symbol as `depth kind name | detail | selection's text | range's
/// first line ... last line`.
fn shown(text: &str, list: &Value, utf8: bool, depth: usize, out: &mut Vec<String>) {
    for s in list.as_array().unwrap() {
        let range = slice(text, &s["range"], utf8);
        let first = range.lines().next().unwrap_or("").trim().to_string();
        let last = range.lines().last().unwrap_or("").trim().to_string();

        out.push(format!(
            "{}{} {} | {} | {} | {} ... {}",
            "  ".repeat(depth),
            s["kind"],
            s["name"].as_str().unwrap(),
            s["detail"].as_str().unwrap_or(""),
            slice(text, &s["selectionRange"], utf8),
            first,
            last
        ));

        if let Some(children) = s.get("children") {
            shown(text, children, utf8, depth + 1, out);
        }
    }
}

const EXPECTED: &[&str] = &[
    "14 MAX | let pub MAX = 10 | MAX | let pub MAX = 10 ... let pub MAX = 10",
    "14 NAMES | let NAMES = [\"日本\", \"👩‍👩‍👦\"] | NAMES | let NAMES = [\"日本\", \"👩‍👩‍👦\"] ... let NAMES = [\"日本\", \"👩‍👩‍👦\"]",
    "5 Counter | type pub Counter | Counter | type pub Counter { ... }",
    "  8 count | let mut @count: Int | @count | let mut @count: Int ... let mut @count: Int",
    "  8 label | let pub @label: String | @label | let pub @label: String ... let pub @label: String",
    "  6 new | fn pub static new(label: String, count: Int) -> Counter | new | fn pub static new( ... }",
    "  6 add | fn pub mut add(amount: Int) | add | fn pub mut add(amount: Int) { ... }",
    "19 impl Counter | impl Counter | Counter | impl Counter { ... }",
    "  6 same | fn pub same(other: ref Counter) -> Bool | same | fn pub same(other: ref Counter) -> Bool { ... }",
    "12 describe | fn pub describe(name: String) -> String | describe | fn pub describe(name: String) -> String { ... }",
    "12 total | fn total(shape: ref Shape) -> Float | total | fn total(shape: ref Shape) -> Float { ... }",
    "5 Main | type async Main | Main | type async Main { ... }",
    "  6 main | fn async main | main | fn async main { ... }",
];

fn check_fixture(utf8: bool) {
    let root = fixture("symbols");
    let file = root.join("src/main.inko");
    let text = std::fs::read_to_string(&file).unwrap();
    let mut client = session(&root, utf8);

    // Read from disk: the document isn't open.
    let (id, result) = symbols(&mut client, &uri(&file));
    let mut out = Vec::new();

    shown(&text, &result, utf8, 0, &mut out);
    assert_eq!(out, EXPECTED);

    let status = client.status(id).unwrap();

    assert_eq!(status["status"], "fresh", "{}", status);
    assert!(status.get("repaired").is_none(), "{}", status);

    // The other module: an enum, its constructors, a trait, an `impl` of it.
    let shapes = root.join("src/shapes.inko");
    let text = std::fs::read_to_string(&shapes).unwrap();
    let (_, result) = symbols(&mut client, &uri(&shapes));
    let mut out = Vec::new();

    shown(&text, &result, utf8, 0, &mut out);
    assert_eq!(
        out,
        [
            "10 Shape | type pub enum Shape | Shape | type pub enum Shape { ... }",
            "  22 Circle | case Circle(Float) | Circle | case Circle(Float) ... case Circle(Float)",
            "  22 Square | case Square | Square | case Square ... case Square",
            "11 Area | trait pub Area | Area | trait pub Area { ... }",
            "  6 area | fn pub area -> Float | area | fn pub area -> Float ... fn pub area -> Float",
            "  6 double | fn pub double -> Float | double | fn pub double -> Float { ... }",
            "19 impl Area for Shape | impl Area for Shape | Shape | impl Area for Shape { ... }",
            "  6 area | fn pub area -> Float | area | fn pub area -> Float { ... }",
            "12 shape_count | fn pub shape_count -> Int | shape_count | fn pub shape_count -> Int { ... }",
            "12 shape_helper | fn shape_helper -> Int | shape_helper | fn shape_helper -> Int { ... }",
        ]
    );
}

#[test]
fn document_symbols_in_utf8() {
    check_fixture(true);
}

#[test]
fn document_symbols_in_utf16() {
    check_fixture(false);
}

/// Edits that keep the module from parsing: the symbols are those of a
/// copy with the lines in the way blank, at their places in the text as it
/// is, and the status says so.
#[test]
fn document_symbols_in_code_that_does_not_parse() {
    for utf8 in [false, true] {
        let root = fixture("symbols");
        let file = root.join("src/main.inko");
        let text = std::fs::read_to_string(&file).unwrap();
        let mut client = session(&root, utf8);

        client.open(&file, &text);

        // A statement half typed: every symbol is still there.
        let (line, character) = at(&text, "amount\n  }", 0, utf8);

        client.change(
            &file,
            2,
            json!([{
                "range": {
                    "start": { "line": line, "character": character },
                    "end": { "line": line, "character": character + 6 },
                },
                "text": "",
            }]),
        );

        let broken = text.replacen("@count += amount\n", "@count += \n", 1);
        let (id, result) = symbols(&mut client, &uri(&file));
        let mut out = Vec::new();

        shown(&broken, &result, utf8, 0, &mut out);
        assert_eq!(out, EXPECTED, "utf8: {}", utf8);
        assert_eq!(client.status(id).unwrap()["repaired"], true);

        // A signature half typed: that method is gone, the rest is where
        // it is. Nothing after it moved.
        let (line, character) = at(
            &broken,
            "fn pub describe(name: String) -> String {",
            0,
            utf8,
        );

        client.change(
            &file,
            3,
            json!([{
                "range": {
                    "start": { "line": line, "character": character + 16 },
                    "end": { "line": line, "character": character + 42 },
                },
                "text": "",
            }]),
        );

        let broken = broken.replacen(
            "fn pub describe(name: String) -> String {",
            "fn pub describe(",
            1,
        );
        let (_, result) = symbols(&mut client, &uri(&file));
        let mut out = Vec::new();

        shown(&broken, &result, utf8, 0, &mut out);

        let expected: Vec<&str> = EXPECTED
            .iter()
            .copied()
            .filter(|s| !s.contains("describe"))
            .collect();

        assert_eq!(out, expected, "utf8: {}", utf8);

        // A brace a method's body opens and nothing closes yet: the type
        // and everything after it stay (no body can hold a declaration,
        // so it closes in the method); the method, which may end at
        // another `}`, is gone.
        let open = text.replacen(
            "    @count += amount\n",
            "    @count += amount\n    if true {\n",
            1,
        );

        client.change(&file, 4, json!([{ "text": open }]));

        let (id, result) = symbols(&mut client, &uri(&file));
        let mut out = Vec::new();

        shown(&open, &result, utf8, 0, &mut out);

        let expected: Vec<&str> = EXPECTED
            .iter()
            .copied()
            .filter(|s| !s.contains("add(amount"))
            .collect();

        assert_eq!(out, expected, "utf8: {}", utf8);
        assert_eq!(client.status(id).unwrap()["repaired"], true);
    }
}

/// An open document that isn't a file, such as an unsaved new one, has
/// symbols too: they need no check. Lines may end at `\r\n`.
#[test]
fn document_symbols_of_a_document_that_is_not_a_file() {
    let root = fixture("symbols");
    let mut client = session(&root, false);
    let text = "type A {\r\n  let @a: Int\r\n}\r\n\r\nfn b -> Int {\r\n  1\r\n}\r\n";
    let untitled = "untitled:Untitled-1";

    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": {
            "uri": untitled, "languageId": "inko", "version": 1, "text": text,
        }}),
    );

    let (_, result) = symbols(&mut client, untitled);

    assert_eq!(
        result,
        json!([
            {
                "name": "A", "detail": "type A", "kind": 5,
                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 2, "character": 1 } },
                "selectionRange": { "start": { "line": 0, "character": 5 }, "end": { "line": 0, "character": 6 } },
                "children": [{
                    "name": "a", "detail": "let @a: Int", "kind": 8,
                    "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 13 } },
                    "selectionRange": { "start": { "line": 1, "character": 6 }, "end": { "line": 1, "character": 8 } },
                }],
            },
            {
                "name": "b", "detail": "fn b -> Int", "kind": 12,
                "range": { "start": { "line": 4, "character": 0 }, "end": { "line": 6, "character": 1 } },
                "selectionRange": { "start": { "line": 4, "character": 3 }, "end": { "line": 4, "character": 4 } },
            },
        ])
    );

    // Text that no copy with lines blank makes parse.
    let garbage = "untitled:Untitled-3";

    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": {
            "uri": garbage, "languageId": "inko", "version": 1, "text": ")\n".repeat(40),
        }}),
    );

    let (id, result) = symbols(&mut client, garbage);

    assert_eq!(result, Value::Null);
    assert_eq!(client.status(id).unwrap()["reason"]["code"], "noHir");

    // Not open, and not a file: nothing to read.
    let (id, result) = symbols(&mut client, "untitled:Untitled-2");

    assert_eq!(result, Value::Null);
    assert_eq!(client.status(id).unwrap()["reason"]["code"], "notAFile");
}

/// A client without hierarchical document symbols gets
/// `SymbolInformation[]`, each with its container, and without the newer
/// symbol kinds, the nearest it has.
#[test]
fn document_symbols_for_a_client_without_hierarchy() {
    let root = fixture("symbols");
    let file = root.join("src/shapes.inko");
    let mut client = Client::launch(Launch::default());
    let response = client.initialize_exactly(&root, json!({ "diagnostics": false }), json!({}));

    assert!(response.get("result").is_some(), "{}", response);
    client.notify("initialized", json!({}));

    let (_, result) = symbols(&mut client, &uri(&file));
    let shown: Vec<String> = result
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            assert_eq!(s["location"]["uri"], uri(&file));

            format!(
                "{} {} {}",
                s["kind"],
                s["name"].as_str().unwrap(),
                s["containerName"].as_str().unwrap_or("-")
            )
        })
        .collect();

    // Constructor (9) for EnumMember (22), Class (5) for Object (19).
    assert_eq!(
        shown,
        [
            "10 Shape -",
            "9 Circle Shape",
            "9 Square Shape",
            "11 Area -",
            "6 area Area",
            "6 double Area",
            "5 impl Area for Shape -",
            "6 area impl Area for Shape",
            "12 shape_count -",
            "12 shape_helper -",
        ]
    );
}

#[test]
fn document_symbols_are_advertised() {
    let mut client = Client::start();
    let response = client.initialize(&fixture("symbols"), json!({}), json!({}));

    assert_eq!(
        response["result"]["capabilities"]["documentSymbolProvider"],
        true
    );
}

/// A copy of tests/fixtures/symbols, with a dependency in `dep/`, as
/// `inko pkg sync` leaves one, and a test module.
pub fn project_with_a_dependency(name: &str) -> std::path::PathBuf {
    let root = crate::common::tmp().join(name);
    let url = "https://example.com/acme/geo";
    let dir = compiler::pkg::manifest::Url::parse(url)
        .unwrap()
        .directory_name();
    let package = root.join("dep").join(&dir).join("v1");

    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("test")).unwrap();
    std::fs::create_dir_all(package.join("src")).unwrap();

    for file in ["main.inko", "shapes.inko"] {
        std::fs::copy(
            fixture("symbols").join("src").join(file),
            root.join("src").join(file),
        )
        .unwrap();
    }

    std::fs::write(
        root.join("inko.pkg"),
        format!(
            "require {} 1.0.0 0123456789012345678901234567890123456789\n",
            url
        ),
    )
    .unwrap();
    std::fs::write(package.join("inko.pkg"), "").unwrap();
    std::fs::write(
        package.join("src/geo.inko"),
        "# Geometry.\n\n# The area of a square.\nfn pub area(side: Int) -> Int {\n  side * side\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/uses_geo.inko"),
        "import geo\n\nfn pub doubled(side: Int) -> Int {\n  geo.area(side) * 2\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("test/test_main.inko"),
        "fn pub test_counter_counts {}\n",
    )
    .unwrap();
    root.canonicalize().unwrap()
}

/// The workspace symbols for `query`, each as `kind name container
/// file:line:start-end` (the file relative to `root`, or `std/...`).
fn workspace(client: &mut Client, root: &Path, query: &str, utf8: bool) -> (i64, Vec<String>) {
    workspace_with(client, root, query, utf8, None)
}

/// `workspace`, with an open document's text for one file.
fn workspace_with(
    client: &mut Client,
    root: &Path,
    query: &str,
    utf8: bool,
    open: Option<(&Path, &str)>,
) -> (i64, Vec<String>) {
    let id = client.send_request("workspace/symbol", json!({ "query": query }));
    let result = client.response(id)["result"].clone();
    let std = cuttlebone::dump::upstream_std().canonicalize().unwrap();
    let shown = result
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            let path = url::Url::parse(s["location"]["uri"].as_str().unwrap())
                .unwrap()
                .to_file_path()
                .unwrap();
            let text = match open {
                Some((file, text)) if file == path => text.to_string(),
                _ => std::fs::read_to_string(&path).unwrap(),
            };
            let file = match path.strip_prefix(root) {
                Ok(p) => p.display().to_string(),
                Err(_) => format!("std/{}", path.strip_prefix(&std).unwrap().display()),
            };
            let range = &s["location"]["range"];

            format!(
                "{} {} {} {}:{}:{}",
                s["kind"],
                s["name"].as_str().unwrap(),
                s["containerName"].as_str().unwrap(),
                file,
                range["start"]["line"],
                slice(&text, range, utf8)
            )
        })
        .collect();

    (id, shown)
}

/// The project's declarations, its dependency's and std's, by a query that
/// matches as clients filter: the query's characters in order, in any
/// case. Ranges are the names, in the negotiated encoding.
#[test]
fn workspace_symbols_from_the_project_its_dependencies_and_std() {
    for utf8 in [false, true] {
        let root = project_with_a_dependency(&format!("workspace-symbols-{}", utf8));
        let mut client = session(&root, utf8);

        let (id, found) = workspace(&mut client, &root, "Counter", utf8);

        assert_eq!(
            found[0], "5 Counter main src/main.inko:11:Counter",
            "{:?}",
            found
        );
        assert_eq!(client.status(id).unwrap()["status"], "fresh");

        // In any case, and the characters in order.
        for query in ["counter", "COUNTER", "cntr"] {
            let (_, found) = workspace(&mut client, &root, query, utf8);

            assert!(
                found.contains(&"5 Counter main src/main.inko:11:Counter".to_string()),
                "{}: {:?}",
                query,
                found
            );
        }

        // The best first: the whole name, then its start, then a run of it.
        let (_, found) = workspace(&mut client, &root, "area", utf8);

        assert_eq!(
            &found[..4],
            [
                "6 area shapes.Area src/shapes.inko:12:area",
                "6 area shapes.Shape src/shapes.inko:20:area",
                "12 area geo dep/example.com-acme-geo/v1/src/geo.inko:3:area",
                "6 area std.html.Html std/std/html.inko:671:area",
            ]
            .map(String::from)
            .as_slice()
            .iter()
            .map(|s| s.replace("example.com-acme-geo", &dep_dir()))
            .collect::<Vec<_>>(),
            "{:?}",
            found
        );

        // `impl` blocks declare nothing.
        assert!(found.iter().all(|f| !f.contains(" impl ")), "{:?}", found);

        // std's, private ones too, with what they're in.
        let (_, found) = workspace(&mut client, &root, "Stdout", utf8);

        assert!(
            found[0].starts_with("5 Stdout std.stdio std/std/stdio.inko:")
                && found[0].ends_with(":Stdout"),
            "{:?}",
            found
        );

        let (_, found) = workspace(&mut client, &root, "test_counter", utf8);

        assert_eq!(
            found[0],
            "12 test_counter_counts test_main test/test_main.inko:0:test_counter_counts"
        );

        // A field, by its name, located with its `@`; a constructor.
        let (_, found) = workspace(&mut client, &root, "label", utf8);

        assert!(
            found.contains(&"8 label main.Counter src/main.inko:13:@label".to_string()),
            "{:?}",
            found
        );

        let (_, found) = workspace(&mut client, &root, "Square", utf8);

        assert_eq!(found[0], "22 Square shapes.Shape src/shapes.inko:6:Square");

        let (_, found) = workspace(&mut client, &root, "NAMES", utf8);

        assert_eq!(found[0], "14 NAMES main src/main.inko:6:NAMES");
    }
}

/// The name of the `dep/` directory of `https://example.com/acme/geo`.
fn dep_dir() -> String {
    compiler::pkg::manifest::Url::parse("https://example.com/acme/geo")
        .unwrap()
        .directory_name()
}

/// The project needn't type check, or even parse: an open document's
/// text is read as it is, and from a copy with lines blank where it
/// doesn't parse. Files changed on disk, created and deleted are read
/// again.
#[test]
fn workspace_symbols_in_code_that_does_not_type_check() {
    let root = project_with_a_dependency("workspace-symbols-broken");
    let file = root.join("src/main.inko");
    let text = std::fs::read_to_string(&file).unwrap();
    let mut client = session(&root, false);

    // An undefined type, a method typed but not saved, and a statement
    // half typed.
    let edited = text.replace("-> Counter {", "-> Countr {").replace(
        "fn total(",
        "fn pub halfway(by: Int) {\n  by +\n}\n\nfn total(",
    );

    client.open(&file, &edited);

    let open = Some((file.as_path(), edited.as_str()));
    let (_, found) = workspace_with(&mut client, &root, "halfway", false, open);

    assert_eq!(found[0], "12 halfway main src/main.inko:38:halfway");

    let (_, found) = workspace_with(&mut client, &root, "total", false, open);

    assert_eq!(found[0], "12 total main src/main.inko:42:total");

    // A file created, changed and deleted on disk.
    let other = root.join("src/other.inko");

    std::fs::write(&other, "fn pub fresh_method {}\n").unwrap();

    let (_, found) = workspace(&mut client, &root, "fresh_method", false);

    assert_eq!(
        found[0],
        "12 fresh_method other src/other.inko:0:fresh_method"
    );

    std::fs::write(&other, "\nfn pub fresher_method {}\n").unwrap();

    let (_, found) = workspace(&mut client, &root, "fresh", false);

    assert_eq!(
        found[0],
        "12 fresher_method other src/other.inko:1:fresher_method"
    );
    assert!(
        !found.iter().any(|f| f.contains("fresh_method")),
        "{:?}",
        found
    );

    std::fs::remove_file(&other).unwrap();

    let (_, found) = workspace(&mut client, &root, "fresher", false);

    assert!(
        !found.iter().any(|f| f.contains("other.inko")),
        "{:?}",
        found
    );
}

/// A client without the newer symbol kinds for workspace symbols gets the
/// nearest it has.
#[test]
fn workspace_symbols_for_a_client_without_newer_kinds() {
    let root = fixture("symbols");
    let mut client = Client::launch(Launch::default());
    let response = client.initialize_exactly(&root, json!({ "diagnostics": false }), json!({}));

    assert!(response["result"]["capabilities"]["workspaceSymbolProvider"] == true);
    client.notify("initialized", json!({}));

    let (_, found) = workspace(&mut client, &root, "Circle", false);

    assert_eq!(found[0], "9 Circle shapes.Shape src/shapes.inko:5:Circle");
    // Nothing is written: a project without a manifest still has none.
    assert!(!root.join("inko.pkg").exists());
}
