//! Go to definition, type definition and references, on the core API.
//! tests/lsp.rs covers the same over LSP.
mod common;

use common::{Project, position_of};
use cuttlebone::dump::upstream_std;
use cuttlebone::position::{Encoding, Position};
use cuttlebone::query::{self, Place, Request, Unavailable};
use std::collections::HashMap;
use std::path::Path;

/// Where a place is, for assertions: the file relative to the project (or
/// `std:` and the path in std), the 0-based line, and the text it covers.
fn describe(project: &Project, place: &Place) -> (String, u32, String) {
    let std = upstream_std();
    let file = if let Ok(rel) = place.file.strip_prefix(&project.root) {
        rel.display().to_string()
    } else if let Ok(rel) = place.file.strip_prefix(&std) {
        format!("std:{}", rel.display())
    } else {
        place.file.display().to_string()
    };

    (file, place.range.start.line, text_at(&place.file, place))
}

/// The text a place's range covers, which must be on one line.
pub fn text_at(file: &Path, place: &Place) -> String {
    let text = std::fs::read_to_string(file).unwrap();
    let line = text.lines().nth(place.range.start.line as usize).unwrap();
    let units: Vec<u16> = line.encode_utf16().collect();

    assert_eq!(place.range.start.line, place.range.end.line, "{:?}", place);
    String::from_utf16(
        &units[place.range.start.character as usize..place.range.end.character as usize],
    )
    .unwrap()
}

fn request(project: &Project, file: &str, needle: &str, offset: u32) -> Request {
    let text = project.read(file);

    Request {
        file: project.path(file),
        position: position_of(&text, needle, offset),
        encoding: Encoding::Utf16,
        contents: HashMap::new(),
    }
}

fn definition(needle: &str, offset: u32) -> Option<(String, u32, String)> {
    let project = Project::new("navigation");
    let answer = query::definition(
        &project.session,
        &request(&project, "src/main.inko", needle, offset),
    );

    answer.result.unwrap().map(|p| describe(&project, &p))
}

fn type_definition(needle: &str, offset: u32) -> Option<(String, u32, String)> {
    let project = Project::new("navigation");
    let answer = query::type_definition(
        &project.session,
        &request(&project, "src/main.inko", needle, offset),
    );

    answer.result.unwrap().map(|p| describe(&project, &p))
}

fn at(file: &str, line: u32, text: &str) -> Option<(String, u32, String)> {
    Some((file.to_string(), line, text.to_string()))
}

const MAIN: &str = "src/main.inko";

#[test]
fn a_method_call_goes_to_the_method() {
    assert_eq!(definition("point.moved(2)", 6), at(MAIN, 14, "moved"));
}

#[test]
fn a_static_call_goes_to_the_static_method_and_the_type() {
    assert_eq!(definition("Point.new(x: 1", 6), at(MAIN, 10, "new"));
    assert_eq!(definition("Point.new(x: 1", 0), at(MAIN, 6, "Point"));
}

#[test]
fn a_call_through_a_trait_bound_goes_to_the_traits_method() {
    assert_eq!(
        definition("shape.area", 6),
        at("src/geometry/shapes.inko", 3, "area")
    );
}

#[test]
fn a_call_on_a_type_goes_to_its_implementation() {
    assert_eq!(
        definition("c.area", 2),
        at("src/geometry/shapes.inko", 15, "area")
    );
}

#[test]
fn a_call_into_std_goes_to_the_std_source() {
    let (file, _, text) = definition("'中文😀'.size", 7).unwrap();

    assert_eq!(
        (file.as_str(), text.as_str()),
        ("std:std/string.inko", "size")
    );
}

#[test]
fn an_inherited_default_method_goes_to_the_traits_method() {
    // `map` is a default method of std.iter.Iter, which Array's iterator
    // inherits without redefining it.
    let (file, _, text) = definition("iter.map", 5).unwrap();

    assert_eq!((file.as_str(), text.as_str()), ("std:std/iter.inko", "map"));
}

#[test]
fn variables_go_to_their_bindings() {
    // let
    assert_eq!(definition("point.moved(2)", 0), at(MAIN, 43, "point"));
    // a parameter
    assert_eq!(definition("@x + by", 5), at(MAIN, 14, "by"));
    // a pattern
    assert_eq!(definition("-> side", 3), at(MAIN, 36, "side"));
    // a closure argument
    assert_eq!(definition("c.area", 0), at(MAIN, 48, "c"));
    // a `for` loop's variable
    assert_eq!(definition("shape.area", 0), at(MAIN, 27, "shape"));
    // a reassigned variable
    assert_eq!(definition("sum += ", 0), at(MAIN, 25, "sum"));
}

#[test]
fn a_field_goes_to_its_declaration() {
    assert_eq!(definition("@x + by", 0), at(MAIN, 7, "@x"));
    assert_eq!(definition("@label)", 1), at(MAIN, 8, "@label"));
    // A field read through a call.
    assert_eq!(definition("moved(3).x", 9), at(MAIN, 7, "@x"));
}

#[test]
fn type_names_go_to_their_declarations() {
    // In a signature.
    assert_eq!(
        definition("-> Point {\n    Point(x", 3),
        at(MAIN, 6, "Point")
    );
    assert_eq!(definition("ref Shape", 4), at(MAIN, 19, "Shape"));
    // A trait in a bound.
    assert_eq!(
        definition("T: Area", 3),
        at("src/geometry/shapes.inko", 2, "Area")
    );
    // A type parameter.
    assert_eq!(definition("Array[T]", 6), at(MAIN, 24, "T"));
    // A type in std.
    let (file, _, text) = definition("ref Array[T]", 4).unwrap();

    assert_eq!(
        (file.as_str(), text.as_str()),
        ("std:std/array.inko", "Array")
    );
    // Creating an instance.
    assert_eq!(definition("    Point(x: @x", 4), at(MAIN, 6, "Point"));
}

#[test]
fn a_constant_goes_to_its_declaration() {
    assert_eq!(
        definition("ORIGIN.to_string", 0),
        at("src/geometry/shapes.inko", 0, "ORIGIN")
    );
}

#[test]
fn a_constructor_goes_to_the_constructor() {
    assert_eq!(definition("Shape.Square(3)", 6), at(MAIN, 20, "Square"));
    // In a pattern.
    assert_eq!(definition("case Square(side)", 5), at(MAIN, 20, "Square"));
    // In std.
    let (file, _, text) = definition("Option.Some", 7).unwrap();

    assert_eq!(
        (file.as_str(), text.as_str()),
        ("std:std/option.inko", "Some")
    );
}

#[test]
fn a_module_in_an_import_goes_to_its_file() {
    assert_eq!(definition("import beta", 7), at("src/beta.inko", 0, ""));
    assert_eq!(
        definition("geometry.shapes (", 9),
        at("src/geometry/shapes.inko", 0, "")
    );
    // There's no module `geometry`.
    assert_eq!(definition("geometry.shapes (", 0), None);
    // A module used as a receiver.
    assert_eq!(definition("beta.Error.new", 0), at("src/beta.inko", 0, ""));
}

#[test]
fn an_imported_symbol_goes_to_its_declaration() {
    assert_eq!(
        definition("alpha (Error)", 7),
        at("src/alpha.inko", 1, "Error")
    );
    assert_eq!(
        definition("(Area, Circle", 7),
        at("src/geometry/shapes.inko", 6, "Circle")
    );
    assert_eq!(
        definition("Circle, ORIGIN)", 8),
        at("src/geometry/shapes.inko", 0, "ORIGIN")
    );
}

#[test]
fn same_named_types_in_different_modules_stay_apart() {
    assert_eq!(
        definition("Error.new('a')", 0),
        at("src/alpha.inko", 1, "Error")
    );
    assert_eq!(
        definition("Error.new('a')", 6),
        at("src/alpha.inko", 4, "new")
    );
    assert_eq!(
        definition("beta.Error.new", 5),
        at("src/beta.inko", 1, "Error")
    );
    assert_eq!(
        definition("beta.Error.new", 11),
        at("src/beta.inko", 4, "new")
    );
    assert_eq!(
        definition("other.message", 6),
        at("src/beta.inko", 8, "message")
    );
    assert_eq!(
        definition("error.message", 6),
        at("src/alpha.inko", 8, "message")
    );
}

#[test]
fn a_declarations_own_name_goes_to_itself() {
    assert_eq!(definition("fn pub moved", 7), at(MAIN, 14, "moved"));
}

#[test]
fn nothing_there_is_none() {
    // A comment, a literal, and a keyword.
    assert_eq!(definition("# Names", 2), None);
    assert_eq!(definition("'héllo'", 1), None);
    assert_eq!(definition("match shape", 1), None);
}

#[test]
fn type_definition_strips_ownership_and_type_arguments() {
    // `point` is a Point.
    assert_eq!(type_definition("point.moved(2)", 0), at(MAIN, 6, "Point"));
    // `maybe` is an Option[Point], and `shape` in `describe` a ref Shape.
    let (file, _, text) = type_definition("maybe.get", 0).unwrap();

    assert_eq!(
        (file.as_str(), text.as_str()),
        ("std:std/option.inko", "Option")
    );
    assert_eq!(type_definition("match shape", 7), at(MAIN, 19, "Shape"));
    // A method call is its return type.
    assert_eq!(
        type_definition("other.message", 6).unwrap().0,
        "std:std/string.inko"
    );
    // A type parameter's value is the type parameter.
    assert_eq!(type_definition("shape.area", 0), at(MAIN, 24, "T"));
    // A type name is that type.
    assert_eq!(type_definition("ref Shape", 4), at(MAIN, 19, "Shape"));
}

#[test]
fn type_definition_of_tuples_closures_and_never() {
    // A tuple is an instance of std.tuple's TupleN.
    let (file, _, text) = type_definition("let pair", 4).unwrap();

    assert_eq!(
        (file.as_str(), text.as_str()),
        ("std:std/tuple.inko", "Tuple2")
    );
    // A closure's type has no declaration.
    assert_eq!(type_definition("fn (c)", 0), None);
}

#[test]
fn definition_in_code_with_errors_is_unavailable_with_the_same_reasons() {
    let project = Project::new("navigation");
    let file = project.path(MAIN);
    let text = project.read(MAIN);
    let query = |text: String, needle: &str| -> Result<Option<Place>, Unavailable> {
        let request = Request {
            file: file.clone(),
            position: position_of(&text, needle, 0),
            encoding: Encoding::Utf16,
            contents: HashMap::from([(file.clone(), text.clone())]),
        };

        query::definition(&project.session, &request).result
    };

    // A syntax error: a repaired copy answers before it, but not after.
    let broken = text.replacen("let pair = (1, 'two')", "let pair = (1, ", 1);

    assert!(query(broken.clone(), "point.moved(2)").unwrap().is_some());
    assert_eq!(query(broken, "pair.0").unwrap_err().code, "noHir");

    // An undefined type in a signature.
    let broken = text.replacen("ref Shape", "ref Nope", 1);

    assert_eq!(
        query(broken, "point.moved(2)").unwrap_err().code,
        "declarationErrors"
    );

    // An error in one method body leaves the others with answers.
    let broken = text.replacen("@code.to_string", "@code.nope", 1);
    let ok = query(broken.clone(), "point.moved(2)");

    assert!(ok.unwrap().is_some());

    // A call that doesn't resolve has no definition, and isn't a guess.
    let broken = text.replacen("point.moved(2)", "point.nope(2)", 1);

    assert_eq!(query(broken, "nope(2)").map(|p| p.is_none()), Ok(true));
}

#[test]
fn positions_and_ranges_count_the_requests_encoding() {
    let project = Project::new("navigation");
    let text = project.read(MAIN);
    let needle = "'中文😀'.size + point.moved(3)";
    let utf16 = position_of(&text, needle, 0);
    let line = text.lines().nth(utf16.line as usize).unwrap();
    let units = |byte: usize, encoding| match encoding {
        Encoding::Utf16 => line[..byte].encode_utf16().count() as u32,
        Encoding::Utf8 => byte as u32,
    };

    for encoding in [Encoding::Utf16, Encoding::Utf8] {
        let character = units(line.find("moved(3)").unwrap(), encoding);
        let request = Request {
            file: project.path(MAIN),
            position: Position {
                line: utf16.line,
                character,
            },
            encoding,
            contents: HashMap::new(),
        };
        let place = query::definition(&project.session, &request)
            .result
            .unwrap()
            .unwrap();

        assert_eq!(place.range.start.line, 14, "{:?}", encoding);

        // `.x` after the non-ASCII text, and a range on a line with
        // non-ASCII text in std/string.inko, in the same encoding.
        let x = units(line.find(").x").unwrap() + 2, encoding);
        let place = query::definition(
            &project.session,
            &Request {
                position: Position {
                    line: utf16.line,
                    character: x,
                },
                ..request.clone()
            },
        )
        .result
        .unwrap()
        .unwrap();

        assert_eq!(
            place.range.start,
            Position {
                line: 7,
                character: 10
            },
            "{:?}",
            encoding
        );
    }
}

fn references(
    file: &str,
    needle: &str,
    offset: u32,
    declaration: bool,
) -> Vec<(String, u32, String)> {
    let project = Project::new("navigation");
    let answer = query::references(
        &project.session,
        &request(&project, file, needle, offset),
        declaration,
    );

    answer
        .result
        .unwrap()
        .unwrap()
        .iter()
        .map(|p| describe(&project, p))
        .collect()
}

fn places(list: &[(&str, u32, &str)]) -> Vec<(String, u32, String)> {
    list.iter()
        .map(|(f, l, t)| (f.to_string(), *l, t.to_string()))
        .collect()
}

#[test]
fn references_to_same_named_types_in_different_modules_stay_apart() {
    let alpha = places(&[
        ("src/alpha.inko", 4, "Error"),
        ("src/alpha.inko", 5, "Error"),
        ("src/main.inko", 1, "Error"),
        ("src/main.inko", 45, "Error"),
    ]);

    assert_eq!(references(MAIN, "Error.new('a')", 0, false), alpha);
    // The same from the declaration, which the results then include.
    let mut with_declaration = alpha.clone();

    with_declaration.insert(0, ("src/alpha.inko".to_string(), 1, "Error".to_string()));
    assert_eq!(
        references("src/alpha.inko", "type pub Error", 9, true),
        with_declaration
    );

    assert_eq!(
        references(MAIN, "beta.Error.new", 5, false),
        places(&[
            ("src/beta.inko", 4, "Error"),
            ("src/beta.inko", 5, "Error"),
            ("src/main.inko", 46, "Error"),
        ])
    );
}

#[test]
fn references_to_same_named_methods_stay_apart() {
    assert_eq!(
        references(MAIN, "other.message", 6, true),
        places(&[
            ("src/beta.inko", 8, "message"),
            ("src/main.inko", 52, "message")
        ])
    );
    assert_eq!(
        references(MAIN, "error.message", 6, false),
        places(&[("src/main.inko", 52, "message")])
    );
}

#[test]
fn references_to_a_variable() {
    assert_eq!(
        references(MAIN, "point.moved(2)", 0, true),
        places(&[
            (MAIN, 43, "point"),
            (MAIN, 44, "point"),
            (MAIN, 50, "point"),
        ])
    );
    assert_eq!(
        references(MAIN, "point.moved(2)", 0, false),
        places(&[(MAIN, 44, "point"), (MAIN, 50, "point")])
    );
}

#[test]
fn references_to_a_constructor_include_patterns_and_calls() {
    assert_eq!(
        references(MAIN, "case Square(side)", 5, true),
        places(&[
            (MAIN, 20, "Square"),
            (MAIN, 36, "Square"),
            (MAIN, 53, "Square")
        ])
    );
}

#[test]
fn references_to_a_field_include_reads_and_initializers() {
    assert_eq!(
        references(MAIN, "let pub @x", 8, true),
        places(&[
            (MAIN, 7, "@x"),
            (MAIN, 11, "x"),
            (MAIN, 15, "x"),
            (MAIN, 15, "@x"),
            (MAIN, 50, "x"),
            (MAIN, 54, "x"),
        ])
    );
}

#[test]
fn references_to_a_std_symbol_are_only_in_the_project() {
    // String.size: the declaration is in std, so it's left out.
    assert_eq!(
        references(MAIN, "'中文😀'.size", 7, true),
        places(&[(MAIN, 50, "size"), (MAIN, 55, "size")])
    );
}

#[test]
fn references_search_modules_that_dont_import_the_file() {
    // alpha.inko imports nothing, but main.inko uses its `message`.
    assert_eq!(
        references("src/alpha.inko", "fn pub message", 7, false),
        places(&[("src/main.inko", 52, "message")])
    );
}

#[test]
fn references_on_nothing_are_none() {
    let project = Project::new("navigation");
    let answer = query::references(
        &project.session,
        &request(&project, MAIN, "# Names", 2),
        true,
    );

    assert_eq!(answer.result, Ok(None));
}

#[test]
fn a_named_argument_goes_to_the_argument_or_field() {
    assert_eq!(definition("new(x: 1", 4), at(MAIN, 10, "x"));
    assert_eq!(definition("Point(x: x", 6), at(MAIN, 7, "@x"));
    assert_eq!(
        references(MAIN, "fn static new(x", 14, true),
        places(&[(MAIN, 10, "x"), (MAIN, 11, "x"), (MAIN, 43, "x")])
    );
}

#[test]
fn a_qualified_constant_is_two_references() {
    // `shapes.ORIGIN`: the module, then the constant, each at its own name.
    assert_eq!(
        references(MAIN, "ORIGIN.to_string", 0, false),
        places(&[
            (MAIN, 3, "ORIGIN"),
            (MAIN, 52, "ORIGIN"),
            ("src/qualified.inko", 3, "ORIGIN")
        ])
    );

    let project = Project::new("navigation");
    let file = "src/qualified.inko";

    for (offset, expected) in [
        (0, ("src/geometry/shapes.inko", 0, "")),
        (7, ("src/geometry/shapes.inko", 0, "ORIGIN")),
    ] {
        let place = query::definition(
            &project.session,
            &request(&project, file, "shapes.ORIGIN", offset),
        )
        .result
        .unwrap()
        .unwrap();

        assert_eq!(
            describe(&project, &place),
            (expected.0.to_string(), expected.1, expected.2.to_string())
        );
    }
}
