//! Go to definition, type definition and references over LSP: the real
//! `cuttlebone` binary over stdio, as in tests/lsp.rs.

use crate::common::client::{Client, Launch, at, fake_inko_with_std, uri};
use crate::common::fixture;
use cuttlebone::check::upstream_version;
use cuttlebone::dump::upstream_std;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use url::Url;

const MAIN: &str = "src/main.inko";

/// The file, line and text of a location, or `None` for a null result.
type Expected = Option<(String, u32, String)>;

/// A server on the navigation fixture, in UTF-8 or UTF-16.
fn start(utf8: bool, options: Value) -> (Client, PathBuf, String) {
    let root = fixture("navigation");
    let text = std::fs::read_to_string(root.join(MAIN)).unwrap();
    let offered = if utf8 { "utf-8" } else { "utf-16" };
    let client = Client::initialized_with(
        Launch::default(),
        &root,
        options,
        json!({ "general": { "positionEncodings": [offered] } }),
    );

    (client, root, text)
}

/// A request at `needle` plus `offset` characters in main.inko.
fn ask(
    client: &mut Client,
    method: &str,
    root: &Path,
    text: &str,
    needle: &str,
    offset: usize,
    utf8: bool,
) -> (i64, Value) {
    let (line, character) = at(text, needle, offset, utf8);
    let id = client.send_at(method, &root.join(MAIN), line, character);

    (id, client.response(id))
}

/// Where a `Location` is: the file relative to `root` (or its absolute
/// path), the 0-based line, and the text its range covers, counting the
/// range in UTF-8 or UTF-16.
fn place(root: &Path, location: &Value, utf8: bool) -> (String, u32, String) {
    let path = Url::parse(location["uri"].as_str().unwrap())
        .unwrap()
        .to_file_path()
        .unwrap();
    let range = &location["range"];
    let line = range["start"]["line"].as_u64().unwrap() as u32;

    assert_eq!(range["end"]["line"], line, "{}", location);

    let source = std::fs::read_to_string(&path).unwrap();
    let text = source.lines().nth(line as usize).unwrap_or("");
    let (start, end) = (
        range["start"]["character"].as_u64().unwrap() as usize,
        range["end"]["character"].as_u64().unwrap() as usize,
    );
    let covered = if utf8 {
        text[start..end].to_string()
    } else {
        let units: Vec<u16> = text.encode_utf16().collect();

        String::from_utf16(&units[start..end]).unwrap()
    };
    let file = match path.strip_prefix(root) {
        Ok(rel) => rel.display().to_string(),
        Err(_) => path.display().to_string(),
    };

    (file, line, covered)
}

fn expect(file: &str, line: u32, text: &str) -> Expected {
    Some((file.to_string(), line, text.to_string()))
}

fn definitions(utf8: bool) {
    let (mut client, root, text) = start(utf8, json!({}));
    let std = upstream_std().join("std");
    let std = |file: &str| std.join(file).display().to_string();
    let shapes = "src/geometry/shapes.inko";
    let cases: Vec<(&str, usize, Expected)> = vec![
        // A method call, a static call, and the type it's called on.
        ("point.moved(2)", 6, expect(MAIN, 14, "moved")),
        ("Point.new(x: 1", 6, expect(MAIN, 10, "new")),
        ("Point.new(x: 1", 0, expect(MAIN, 6, "Point")),
        // Through a trait bound, and on a type implementing it.
        ("shape.area", 6, expect(shapes, 3, "area")),
        ("c.area", 2, expect(shapes, 15, "area")),
        // Into std.
        ("areas.size", 6, expect(&std("array.inko"), 707, "size")),
        // Variables: let, a parameter, a pattern, a closure argument.
        ("point.moved(2)", 0, expect(MAIN, 43, "point")),
        ("@x + by", 5, expect(MAIN, 14, "by")),
        ("-> side", 3, expect(MAIN, 36, "side")),
        ("c.area", 0, expect(MAIN, 48, "c")),
        // A field, and a named argument.
        ("@x + by", 0, expect(MAIN, 7, "@x")),
        ("new(x: 1", 4, expect(MAIN, 10, "x")),
        // Types in signatures and expressions, a trait, a type parameter.
        ("ref Shape", 4, expect(MAIN, 19, "Shape")),
        ("T: Area", 3, expect(shapes, 2, "Area")),
        ("Array[T]", 6, expect(MAIN, 24, "T")),
        ("    Point(x: @x", 4, expect(MAIN, 6, "Point")),
        // A constant, and constructors in a call and a pattern.
        ("ORIGIN.to_string", 0, expect(shapes, 0, "ORIGIN")),
        ("Shape.Square(3)", 6, expect(MAIN, 20, "Square")),
        ("case Square(side)", 5, expect(MAIN, 20, "Square")),
        ("Option.Some", 7, expect(&std("option.inko"), 19, "Some")),
        // Modules in imports, and imported symbols.
        ("import beta", 7, expect("src/beta.inko", 0, "")),
        ("geometry.shapes (", 9, expect(shapes, 0, "")),
        ("alpha (Error)", 7, expect("src/alpha.inko", 1, "Error")),
        ("Circle, ORIGIN)", 8, expect(shapes, 0, "ORIGIN")),
        // Same-named types in different modules.
        ("beta.Error.new", 5, expect("src/beta.inko", 1, "Error")),
        ("Error.new('a')", 0, expect("src/alpha.inko", 1, "Error")),
        // After non-ASCII text, to a range after non-ASCII text.
        ("point.moved(3).x", 15, expect(MAIN, 7, "@x")),
        ("s.size.to_string })", 0, expect(MAIN, 55, "s")),
        // Nothing: a comment, a literal, a keyword.
        ("# Names", 2, None),
        ("'héllo'", 2, None),
        ("match shape", 1, None),
    ];

    for (needle, offset, expected) in cases {
        let (_, response) = ask(
            &mut client,
            "textDocument/definition",
            &root,
            &text,
            needle,
            offset,
            utf8,
        );
        let result = &response["result"];
        let got = (!result.is_null()).then(|| place(&root, result, utf8));

        assert_eq!(
            got,
            expected,
            "definition at {:?}+{} in {}",
            needle,
            offset,
            if utf8 { "UTF-8" } else { "UTF-16" }
        );
    }
}

#[test]
fn definition_of_every_kind_of_name_in_utf16() {
    definitions(false);
}

#[test]
fn definition_of_every_kind_of_name_in_utf8() {
    definitions(true);
}

fn type_definitions(utf8: bool) {
    let (mut client, root, text) = start(utf8, json!({}));
    let std = upstream_std().join("std");
    let std = |file: &str| std.join(file).display().to_string();
    let cases: Vec<(&str, usize, Expected)> = vec![
        ("point.moved(2)", 0, expect(MAIN, 6, "Point")),
        // Option[Point]: the type arguments are stripped.
        ("maybe.get", 0, expect(&std("option.inko"), 17, "Option")),
        // ref Shape: the ownership is stripped.
        ("match shape", 7, expect(MAIN, 19, "Shape")),
        // A call is its return type.
        ("point.moved(2)", 6, expect(MAIN, 6, "Point")),
        // A value of a type parameter is the parameter.
        ("shape.area", 0, expect(MAIN, 24, "T")),
        // A type name is that type.
        ("ref Shape", 4, expect(MAIN, 19, "Shape")),
        // A tuple is a TupleN.
        ("let pair", 4, expect(&std("tuple.inko"), 88, "Tuple2")),
        // A closure has no declaration.
        ("fn (c)", 0, None),
        // After non-ASCII text.
        (
            "s.size.to_string })",
            0,
            expect(&std("string.inko"), 163, "String"),
        ),
        ("# Names", 2, None),
    ];

    for (needle, offset, expected) in cases {
        let (_, response) = ask(
            &mut client,
            "textDocument/typeDefinition",
            &root,
            &text,
            needle,
            offset,
            utf8,
        );
        let result = &response["result"];
        let got = (!result.is_null()).then(|| place(&root, result, utf8));

        assert_eq!(got, expected, "type definition at {:?}+{}", needle, offset);
    }
}

#[test]
fn type_definition_in_utf16() {
    type_definitions(false);
}

#[test]
fn type_definition_in_utf8() {
    type_definitions(true);
}

fn references(
    client: &mut Client,
    root: &Path,
    text: &str,
    needle: &str,
    offset: usize,
    declaration: bool,
    utf8: bool,
) -> Vec<(String, u32, String)> {
    let (line, character) = at(text, needle, offset, utf8);
    let response = client.request(
        "textDocument/references",
        json!({
            "textDocument": { "uri": uri(&root.join(MAIN)) },
            "position": { "line": line, "character": character },
            "context": { "includeDeclaration": declaration },
        }),
    );

    response["result"]
        .as_array()
        .unwrap_or_else(|| panic!("no references: {}", response))
        .iter()
        .map(|l| place(root, l, utf8))
        .collect()
}

#[test]
fn references_honour_include_declaration_and_keep_same_names_apart() {
    for utf8 in [false, true] {
        let (mut client, root, text) = start(utf8, json!({}));
        let alpha = vec![
            ("src/alpha.inko".to_string(), 4, "Error".to_string()),
            ("src/alpha.inko".to_string(), 5, "Error".to_string()),
            (MAIN.to_string(), 1, "Error".to_string()),
            (MAIN.to_string(), 45, "Error".to_string()),
        ];
        let mut with_declaration = alpha.clone();

        with_declaration.insert(0, ("src/alpha.inko".to_string(), 1, "Error".to_string()));

        assert_eq!(
            references(&mut client, &root, &text, "Error.new('a')", 0, false, utf8),
            alpha
        );
        assert_eq!(
            references(&mut client, &root, &text, "Error.new('a')", 0, true, utf8),
            with_declaration
        );
        assert_eq!(
            references(&mut client, &root, &text, "beta.Error.new", 5, false, utf8),
            vec![
                ("src/beta.inko".to_string(), 4, "Error".to_string()),
                ("src/beta.inko".to_string(), 5, "Error".to_string()),
                (MAIN.to_string(), 46, "Error".to_string()),
            ]
        );
        // In another file, after non-ASCII text: Int.to_string.
        assert!(
            references(
                &mut client,
                &root,
                &text,
                "ORIGIN.to_string",
                7,
                false,
                utf8
            )
            .contains(&("src/beta.inko".to_string(), 9, "to_string".to_string())),
        );
        // After non-ASCII text.
        assert_eq!(
            references(&mut client, &root, &text, "case s ->", 5, true, utf8),
            vec![(MAIN.to_string(), 55, "s".to_string()); 2]
        );
    }
}

#[test]
fn references_without_a_context_leave_the_declaration_out() {
    let (mut client, root, text) = start(false, json!({}));
    let (line, character) = at(&text, "Error.new('a')", 0, false);
    let response = client.request(
        "textDocument/references",
        json!({
            "textDocument": { "uri": uri(&root.join(MAIN)) },
            "position": { "line": line, "character": character },
        }),
    );

    assert_eq!(
        response["result"].as_array().unwrap().len(),
        4,
        "{}",
        response
    );
}

#[test]
fn broken_code_has_no_answers_and_the_same_status_reasons() {
    let (mut client, root, text) = start(false, json!({ "statusNotifications": true }));
    let file = root.join(MAIN);
    let methods = [
        "textDocument/definition",
        "textDocument/typeDefinition",
        "textDocument/references",
    ];

    client.open(&file, &text);

    for method in methods {
        let (id, response) = ask(
            &mut client,
            method,
            &root,
            &text,
            "point.moved(2)",
            0,
            false,
        );

        assert!(!response["result"].is_null(), "{}: {}", method, response);
        assert_eq!(client.status(id).unwrap()["status"], "fresh");
        assert_eq!(client.status(id).unwrap()["method"], method);
    }

    // After an unfinished statement in the same method, a repaired copy
    // can't answer (tests/repaired.rs), and before it nothing older may.
    for (version, broken, needle, code) in [
        (
            2,
            text.replacen("let pair = (1, 'two')", "let pair = (1, ", 1),
            "pair.0.to_string",
            "noHir",
        ),
        (
            3,
            text.replacen("ref Shape", "ref Nope", 1),
            "point.moved(2)",
            "declarationErrors",
        ),
    ] {
        client.change(&file, version, json!([{ "text": broken }]));

        for method in methods {
            let (id, response) = ask(&mut client, method, &root, &text, needle, 0, false);
            let status = client.status(id).unwrap();

            assert_eq!(response["result"], Value::Null, "{}", method);
            assert_eq!(status["status"], "unavailable", "{}", method);
            assert_eq!(status["method"], method);
            assert_eq!(status["reason"]["code"], code, "{}", method);
        }
    }
}

/// A copy of a directory, for an installed std that isn't .upstream/.
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

#[test]
fn locations_in_std_are_in_the_installed_std() {
    // An installed Inko whose std is a copy, as a package manager would put
    // it, so a location under .upstream/ can't pass.
    let installed = crate::common::tmp().join("installed-inko");
    let _ = std::fs::remove_dir_all(&installed);

    copy_dir(&upstream_std(), &installed.join("lib/inko/std/src"));

    let std = installed.join("lib/inko/std/src").canonicalize().unwrap();
    let inko = fake_inko_with_std(upstream_version(), &std, "installed-inko-bin");
    let (mut client, root, text) = start(false, json!({ "inko": inko }));

    for (method, needle, offset, name) in [
        ("textDocument/definition", "areas.size", 6, "size"),
        ("textDocument/definition", "Option.Some", 7, "Some"),
        ("textDocument/typeDefinition", "maybe.get", 0, "Option"),
    ] {
        let (_, response) = ask(&mut client, method, &root, &text, needle, offset, false);
        let path = Url::parse(response["result"]["uri"].as_str().unwrap())
            .unwrap()
            .to_file_path()
            .unwrap();

        assert!(
            path.starts_with(&std),
            "{} at {}: {}",
            method,
            needle,
            path.display()
        );
        assert!(path.is_file());
        assert_eq!(place(&root, &response["result"], false).2, name);
    }
}

/// Answers in a file of an installed std laid out as Inko installs it,
/// `lib/inko/std/std/float.inko`, with no `src/` in its path: checked as
/// the module its importers name (`std.float`), so its intrinsics are
/// std's, and in a module the project doesn't import too.
#[test]
fn answers_in_a_file_of_an_installed_std() {
    let installed = crate::common::tmp().join("installed-inko-std");
    let _ = std::fs::remove_dir_all(&installed);

    copy_dir(&upstream_std(), &installed.join("lib/inko/std"));

    let std = installed.join("lib/inko/std").canonicalize().unwrap();
    let inko = fake_inko_with_std(upstream_version(), &std, "installed-inko-std-bin");

    for utf8 in [false, true] {
        let (mut client, _, _) = start(utf8, json!({ "inko": inko }));

        for (module, needle, offset, expected) in [
            ("float", "_INKO.float_from_bits(bits)", 22, "Int"),
            (
                "float",
                "fn pub inline static from_bits",
                21,
                "Defined in `std.float.Float`",
            ),
            (
                "net/http/server",
                "fn pub static new -> Self {",
                14,
                "Defined in `std.net.http.server",
            ),
        ] {
            let file = std.join(format!("std/{}.inko", module));
            let text = std::fs::read_to_string(&file).unwrap();
            let (line, character) = at(&text, needle, offset, utf8);
            let (_, response) = client.hover(&file, line, character);
            let value = response["result"]["contents"]["value"]
                .as_str()
                .unwrap_or_else(|| panic!("{} at {}: {}", module, needle, response));

            assert!(
                value.contains(expected),
                "{} at {}: {}",
                module,
                needle,
                value
            );
        }
    }
}

#[test]
fn locations_in_dependencies_are_in_the_projects_dep_directory() {
    let root = crate::common::tmp().join("navigation-dep");
    let _ = std::fs::remove_dir_all(&root);
    let url = "https://example.com/acme/geo";
    let dir = compiler::pkg::manifest::Url::parse(url)
        .unwrap()
        .directory_name();
    let package = root.join("dep").join(&dir).join("v1");
    let text = "import geo\n\nfn example -> Int {\n  geo.area(2)\n}\n";

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
    std::fs::write(
        package.join("src/geo.inko"),
        "fn pub area(side: Int) -> Int {\n  side * side\n}\n",
    )
    .unwrap();
    std::fs::write(root.join(MAIN), text).unwrap();

    let root = root.canonicalize().unwrap();
    let mut client = Client::initialized(&root, json!({}));
    let mut dep = |needle: &str, offset: usize| {
        let (line, character) = at(text, needle, offset, false);
        let id = client.send_at("textDocument/definition", &root.join(MAIN), line, character);

        client.response(id)
    };
    let module = dep("import geo", 7);
    let call = dep("geo.area", 4);

    let file = format!("dep/{}/v1/src/geo.inko", dir);

    assert_eq!(
        place(&root, &module["result"], false),
        (file.clone(), 0, String::new())
    );
    assert_eq!(
        place(&root, &call["result"], false),
        (file, 0, "area".to_string())
    );
}

/// `c.value += 1` calls the getter `value` and then the setter `value=`,
/// both at the same name. Definition there is both, as a `Location[]`
/// (the setter first); a plain read or assignment is one `Location`.
#[test]
fn definition_at_a_compound_assignment_to_a_setter_is_the_getter_and_the_setter() {
    let root = crate::common::tmp().join("compound-setter");
    let file = root.join(MAIN);
    let text = "type Counter {\n  let @count: Int\n\n  fn pub value -> Int {\n    @count\n  }\n\n  \
                fn pub mut value=(value: Int) {\n    @count = value\n  }\n}\n\n\
                type async Main {\n  fn async main {\n    let c = Counter(count: 0)\n\n    \
                c.value = 2\n    c.value += 1\n    c.value\n  }\n}\n";

    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, text).unwrap();

    let root = root.canonicalize().unwrap();
    let mut client = Client::initialized(&root, json!({ "diagnostics": false }));
    let getter = (MAIN.to_string(), 3, "value".to_string());
    let setter = (MAIN.to_string(), 7, "value=".to_string());

    for (needle, expected) in [
        ("c.value += 1", vec![setter.clone(), getter.clone()]),
        ("c.value = 2", vec![setter.clone()]),
        ("c.value\n  }", vec![getter.clone()]),
    ] {
        let (line, character) = at(text, needle, 2, false);
        let id = client.send_at("textDocument/definition", &root.join(MAIN), line, character);
        let result = client.response(id)["result"].clone();
        let found: Vec<_> = match &result {
            Value::Array(locations) => locations.iter().map(|l| place(&root, l, false)).collect(),
            location => {
                assert_eq!(
                    expected.len(),
                    1,
                    "{}: one Location, not an array: {}",
                    needle,
                    result
                );
                vec![place(&root, location, false)]
            }
        };

        assert_eq!(found, expected, "{}: {}", needle, result);
        assert_eq!(
            result.is_array(),
            expected.len() > 1,
            "{}: {}",
            needle,
            result
        );
    }
}
