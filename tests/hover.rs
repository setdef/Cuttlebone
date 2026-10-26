//! Hover over LSP, in both encodings: the type at the position, and the
//! declaration of what the name there is, with its signature as written,
//! where it is, and its doc comment, for the project, a dependency and
//! std; for declarations without a doc comment, an implementation that
//! has its trait's, a module, and a declaration's own name. And that an
//! answer from an older check doesn't show a doc comment edited since.
use crate::common::client::{Client, Launch, at};
use crate::symbols::project_with_a_dependency;
use serde_json::{Value, json};
use std::path::Path;

fn session(root: &Path, utf8: bool, launch: Launch) -> Client {
    let encodings = if utf8 {
        json!(["utf-8"])
    } else {
        json!(["utf-16"])
    };

    Client::initialized_with(
        launch,
        root,
        json!({ "statusNotifications": true, "diagnostics": false, "debounceMs": 0 }),
        json!({ "general": { "positionEncodings": encodings } }),
    )
}

/// The hover at `needle` plus `offset` characters in `file`'s `text`, and
/// the text its range covers.
fn hover(
    client: &mut Client,
    file: &Path,
    text: &str,
    needle: &str,
    offset: usize,
    utf8: bool,
) -> (Value, String) {
    let (line, character) = at(text, needle, offset, utf8);
    let (id, response) = client.hover(file, line, character);
    let result = response["result"].clone();

    assert_eq!(client.status(id).unwrap()["status"], "fresh", "{}", needle);

    if result.is_null() {
        return (result, String::new());
    }

    let range = &result["range"];
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

    (
        result["contents"].clone(),
        text[byte(&range["start"])..byte(&range["end"])].to_string(),
    )
}

fn markdown(value: &str) -> Value {
    json!({ "kind": "markdown", "value": value })
}

#[test]
fn hovers_show_declarations_and_doc_comments() {
    for utf8 in [false, true] {
        let root = project_with_a_dependency(&format!("hover-{}", utf8));
        let main = root.join("src/main.inko");
        let text = std::fs::read_to_string(&main).unwrap();
        let mut client = session(&root, utf8, Launch::default());
        let mut at =
            |needle: &str, offset: usize| hover(&mut client, &main, &text, needle, offset, utf8);

        // The project's method, with its signature as written, over lines,
        // and its doc comment.
        assert_eq!(
            at("Counter.new(label", 8),
            (
                markdown(
                    "```inko\nCounter\n```\n\n---\n\n```inko\nfn pub static new(\n  label: String,\n  count: Int,\n) -> Counter\n```\n\nDefined in `main.Counter`\n\n---\n\nMakes one."
                ),
                "Counter.new(label: \"a\", count: MAX)".to_string()
            )
        );
        // None: no doc comment.
        assert_eq!(
            at("counter.add(NAMES", 8),
            (
                markdown(
                    "```inko\nNil\n```\n\n---\n\n```inko\nfn pub mut add(amount: Int)\n```\n\nDefined in `main.Counter`"
                ),
                "counter.add(NAMES.size)".to_string()
            )
        );
        // A constant.
        assert_eq!(
            at("count: MAX)", 7),
            (
                markdown(
                    "```inko\nInt\n```\n\n---\n\n```inko\nlet pub MAX = 10\n```\n\nDefined in `main`\n\n---\n\nHow many there are at most."
                ),
                "MAX".to_string()
            )
        );
        // After non-ASCII text on its line.
        assert_eq!(
            at("describe(counter.label)}", 0),
            (
                markdown(
                    "```inko\nString\n```\n\n---\n\n```inko\nfn pub describe(name: String) -> String\n```\n\nDefined in `main`"
                ),
                "describe(counter.label)".to_string()
            )
        );
        // An implementation of a trait's method without a doc comment of
        // its own has the trait's, and is where the implementation is.
        assert_eq!(
            at("shape.area + shape.double", 6),
            (
                markdown(
                    "```inko\nFloat\n```\n\n---\n\n```inko\nfn pub area -> Float\n```\n\nDefined in `shapes.Shape`\n\n---\n\nThe area."
                ),
                "shape.area".to_string()
            )
        );
        // A trait's default method.
        assert_eq!(
            at("shape.double", 6).0,
            markdown(
                "```inko\nFloat\n```\n\n---\n\n```inko\nfn pub double -> Float\n```\n\nDefined in `shapes.Area`"
            )
        );
        // A field, a type and a constructor from the type's `Shape.Square`.
        assert_eq!(
            at("counter.label)}", 8).0,
            markdown(
                "```inko\nString\n```\n\n---\n\n```inko\nlet pub @label: String\n```\n\nDefined in `main.Counter`"
            )
        );
        assert_eq!(
            at("Shape.Square)", 6).0,
            markdown(
                "```inko\nShape\n```\n\n---\n\n```inko\ncase Square\n```\n\nDefined in `shapes.Shape`"
            )
        );
        // std's, with its doc comment.
        let (stdout, range) = at("Stdout.new", 0);
        let value = stdout["value"].as_str().unwrap();

        assert_eq!(range, "Stdout");
        assert!(
            value.starts_with("```inko\nStdout\n```\n\n---\n\n```inko\ntype pub inline Stdout\n```\n\nDefined in `std.stdio`\n\n---\n\nThe standard output stream"),
            "{}",
            value
        );
        // A declaration's own name: no type, the declaration, the name's
        // range.
        assert_eq!(
            at("fn pub describe", 7),
            (
                markdown(
                    "```inko\nfn pub describe(name: String) -> String\n```\n\nDefined in `main`"
                ),
                "describe".to_string()
            )
        );
        // A variable: its type alone, as before.
        assert_eq!(
            at("counter.add(NAMES", 0),
            (markdown("```inko\nCounter\n```"), "counter".to_string())
        );

        // A dependency's, and a module.
        let uses = root.join("src/uses_geo.inko");
        let text = std::fs::read_to_string(&uses).unwrap();

        client.open(&uses, &text);

        assert_eq!(
            hover(&mut client, &uses, &text, "geo.area", 4, utf8).0,
            markdown(
                "```inko\nInt\n```\n\n---\n\n```inko\nfn pub area(side: Int) -> Int\n```\n\nDefined in `geo`\n\n---\n\nThe area of a square."
            )
        );
        assert_eq!(
            hover(&mut client, &uses, &text, "import geo", 7, utf8),
            (
                markdown("Module `geo`\n\n---\n\nGeometry."),
                "geo".to_string()
            )
        );
    }
}

#[test]
fn hovers_in_plain_text() {
    let root = project_with_a_dependency("hover-plain");
    let main = root.join("src/main.inko");
    let text = std::fs::read_to_string(&main).unwrap();
    let mut client = Client::initialized_with(
        Launch::default(),
        &root,
        json!({ "diagnostics": false }),
        json!({ "textDocument": { "hover": { "contentFormat": ["plaintext"] } } }),
    );
    let (line, character) = at(&text, "Counter.new(label", 8, false);
    let (_, response) = client.hover(&main, line, character);

    assert_eq!(
        response["result"]["contents"],
        json!({
            "kind": "plaintext",
            "value": "Counter\n\nfn pub static new(\n  label: String,\n  count: Int,\n) -> Counter\n\nDefined in main.Counter\n\nMakes one.",
        })
    );
}

/// An answer from the last good check (repaired copies off) shows the doc
/// comment it read only if that text is as it was: here it is, then it
/// was edited, and there's no answer.
#[test]
fn a_stale_hover_shows_a_doc_comment_only_if_unchanged() {
    let root = project_with_a_dependency("hover-stale");
    let main = root.join("src/main.inko");
    let text = std::fs::read_to_string(&main).unwrap();
    let mut client = session(
        &root,
        false,
        Launch {
            env: vec![("CUTTLEBONE_FAULT", "norepair:main.inko".to_string())],
            ..Launch::default()
        },
    );
    let ask = |client: &mut Client, text: &str| {
        let (line, character) = at(text, "Counter.new(label", 8, false);
        let (id, response) = client.hover(&main, line, character);

        (
            response["result"].clone(),
            client.status(id).unwrap().clone(),
        )
    };

    client.open(&main, &text);

    let (fresh, _) = ask(&mut client, &text);

    // A line that doesn't parse, in another method.
    let broken = text.replace("    @count += amount", "    @count += ");
    let (line, character) = at(&text, "amount\n  }", 0, false);

    client.change(
        &main,
        2,
        json!([{
            "range": {
                "start": { "line": line, "character": character },
                "end": { "line": line, "character": character + 6 },
            },
            "text": "",
        }]),
    );

    let (stale, status) = ask(&mut client, &broken);

    assert_eq!(status["status"], "stale", "{}", status);
    assert_eq!(stale["contents"], fresh["contents"]);

    // The doc comment edited: what the old check read isn't there.
    let edited = broken.replace("# Makes one.", "# Makes two.");
    let (line, character) = at(&broken, "Makes one", 6, false);

    client.change(
        &main,
        3,
        json!([{
            "range": {
                "start": { "line": line, "character": character },
                "end": { "line": line, "character": character + 3 },
            },
            "text": "two",
        }]),
    );

    let (none, status) = ask(&mut client, &edited);

    assert_eq!(none, Value::Null);
    assert_eq!(status["status"], "unavailable", "{}", status);
}
