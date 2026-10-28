//! `textDocument/selectionRange` (docs/lsp.md): the ranges around each
//! position, from the tokens and the declarations, over LSP in both
//! encodings, in code that doesn't parse; and over every module of std and
//! the fixtures, each chain nested, with every declaration around the
//! position in it.
use crate::common::client::{Client, at, uri};
use cuttlebone::dump::upstream_std;
use cuttlebone::position::{Encoding, Position, byte_offset};
use cuttlebone::syntax;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const TEXT: &str = "import std.stdio (Stdout)

# A doc comment.
type pub Counter {
  let @count: Int

  fn pub mut add(amount: Int, times: Int) {
    @count += amount * times
  }
}

type async Main {
  fn async main {
    let counter = Counter(count: 0)

    counter.add(1, times: 2)
    Stdout.new.print('héllo 中文😀 ${counter.count}'.to_string)
  }
}
";

fn project(name: &str) -> (PathBuf, PathBuf) {
    let root = crate::common::tmp().join(format!("selection-{}", name));
    let _ = std::fs::remove_dir_all(&root);

    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.inko"), TEXT).unwrap();

    let root = root.canonicalize().unwrap();
    let main = root.join("src/main.inko");

    (root, main)
}

fn server(root: &Path, utf8: bool) -> Client {
    Client::initialized_with(
        Default::default(),
        root,
        json!({ "statusNotifications": true, "diagnostics": false }),
        json!({
            "general": { "positionEncodings": if utf8 { ["utf-8"] } else { ["utf-16"] } },
        }),
    )
}

/// The texts of a selection range and its parents, innermost first.
fn texts(text: &str, mut range: &Value, utf8: bool) -> Vec<String> {
    let encoding = if utf8 {
        Encoding::Utf8
    } else {
        Encoding::Utf16
    };
    let offset = |p: &Value| {
        let position = Position {
            line: p["line"].as_u64().unwrap() as u32,
            character: p["character"].as_u64().unwrap() as u32,
        };

        byte_offset(text, position, encoding).unwrap()
    };
    let mut out = Vec::new();

    while !range.is_null() {
        out.push(
            text[offset(&range["range"]["start"])..offset(&range["range"]["end"])].to_string(),
        );
        range = &range["parent"];
    }

    out
}

fn select(client: &mut Client, path: &Path, positions: &[(u32, u32)]) -> (i64, Value) {
    let positions: Vec<Value> = positions
        .iter()
        .map(|(line, character)| json!({ "line": line, "character": character }))
        .collect();
    let id = client.send_request(
        "textDocument/selectionRange",
        json!({ "textDocument": { "uri": uri(path) }, "positions": positions }),
    );

    (id, client.response(id)["result"].clone())
}

const COUNTER: &str = "type pub Counter {
  let @count: Int

  fn pub mut add(amount: Int, times: Int) {
    @count += amount * times
  }
}";

const ADD: &str = "fn pub mut add(amount: Int, times: Int) {
    @count += amount * times
  }";

const MAIN: &str = "type async Main {
  fn async main {
    let counter = Counter(count: 0)

    counter.add(1, times: 2)
    Stdout.new.print('héllo 中文😀 ${counter.count}'.to_string)
  }
}";

const MAIN_METHOD: &str = "fn async main {
    let counter = Counter(count: 0)

    counter.add(1, times: 2)
    Stdout.new.print('héllo 中文😀 ${counter.count}'.to_string)
  }";

/// What's selected at `to_string`, after non-ASCII text, at `times` in a
/// named argument, and at `amount` in an expression, innermost first.
fn expected() -> Vec<Vec<&'static str>> {
    let print = "Stdout.new.print('héllo 中文😀 ${counter.count}'.to_string)";
    let body = MAIN_METHOD
        .strip_prefix("fn async main {\n    ")
        .unwrap()
        .strip_suffix("\n  }")
        .unwrap();
    let add_body = "@count += amount * times";
    let counter_body = COUNTER
        .strip_prefix("type pub Counter {\n  ")
        .unwrap()
        .strip_suffix("\n}")
        .unwrap();

    vec![
        vec![
            "to_string",
            "'héllo 中文😀 ${counter.count}'.to_string",
            "('héllo 中文😀 ${counter.count}'.to_string)",
            print,
            body,
            "{\n    let counter = Counter(count: 0)\n\n    counter.add(1, times: 2)\n    \
             Stdout.new.print('héllo 中文😀 ${counter.count}'.to_string)\n  }",
            // Also what's inside Main's braces.
            MAIN_METHOD,
            "{\n  fn async main {\n    let counter = Counter(count: 0)\n\n    \
             counter.add(1, times: 2)\n    \
             Stdout.new.print('héllo 中文😀 ${counter.count}'.to_string)\n  }\n}",
            MAIN,
            TEXT,
        ],
        vec![
            "times",
            "times: 2",
            "1, times: 2",
            "(1, times: 2)",
            "counter.add(1, times: 2)",
            body,
            "{\n    let counter = Counter(count: 0)\n\n    counter.add(1, times: 2)\n    \
             Stdout.new.print('héllo 中文😀 ${counter.count}'.to_string)\n  }",
            // Also what's inside Main's braces.
            MAIN_METHOD,
            "{\n  fn async main {\n    let counter = Counter(count: 0)\n\n    \
             counter.add(1, times: 2)\n    \
             Stdout.new.print('héllo 中文😀 ${counter.count}'.to_string)\n  }\n}",
            MAIN,
            TEXT,
        ],
        vec![
            "amount",
            add_body,
            "{\n    @count += amount * times\n  }",
            ADD,
            counter_body,
            "{\n  let @count: Int\n\n  fn pub mut add(amount: Int, times: Int) {\n    \
             @count += amount * times\n  }\n}",
            COUNTER,
            // With its doc comment.
            "# A doc comment.\ntype pub Counter {\n  let @count: Int\n\n  \
             fn pub mut add(amount: Int, times: Int) {\n    @count += amount * times\n  }\n}",
            TEXT,
        ],
    ]
}

fn positions(text: &str, utf8: bool) -> Vec<(u32, u32)> {
    vec![
        at(text, "to_string)", 3, utf8),
        at(text, "times: 2", 0, utf8),
        at(text, "amount *", 0, utf8),
    ]
}

#[test]
fn selection_ranges_grow_from_the_token() {
    for utf8 in [false, true] {
        let (root, main) = project(if utf8 { "utf8" } else { "utf16" });
        let mut client = server(&root, utf8);

        // Not open: the file's.
        let (id, result) = select(&mut client, &main, &positions(TEXT, utf8));
        let got: Vec<Vec<String>> = result
            .as_array()
            .unwrap()
            .iter()
            .map(|r| texts(TEXT, r, utf8))
            .collect();

        assert_eq!(got, expected(), "UTF-8: {}", utf8);
        assert_eq!(client.status(id).unwrap()["status"], "fresh");
    }
}

/// From the tokens: code that doesn't parse elsewhere selects as it did,
/// and a bracket nothing closes is a token like any other.
#[test]
fn code_that_doesnt_parse_selects_the_same() {
    let (root, main) = project("broken");
    let mut client = server(&root, false);
    let broken = TEXT.replace(
        "let counter = Counter(count: 0)",
        "let counter = Counter(count: ",
    );

    assert!(syntax::parse(&broken).is_none());
    client.open(&main, &broken);

    let (id, result) = select(&mut client, &main, &[at(&broken, "amount *", 0, false)]);

    let got = texts(&broken, &result[0], false);
    let want = &expected()[2];

    assert_eq!(got[..want.len() - 1], want[..want.len() - 1]);
    assert_eq!(got.last().unwrap(), &broken);
    assert_eq!(client.status(id).unwrap()["status"], "fresh");

    // In the method with the unclosed `(`: the statements around it are
    // still the same ones.
    let (_, result) = select(&mut client, &main, &[at(&broken, "times: 2", 0, false)]);
    let got = texts(&broken, &result[0], false);

    assert_eq!(got[..5], expected()[1][..5]);
    assert_eq!(got.last().unwrap(), &broken);
}

#[test]
fn a_position_past_the_end_has_no_answer() {
    let (root, main) = project("past");
    let mut client = server(&root, false);
    let (id, result) = select(&mut client, &main, &[(0, 0), (400, 0)]);

    assert_eq!(result, Value::Null);
    assert_eq!(
        client.status(id).unwrap()["reason"]["code"],
        "invalidPosition"
    );

    // Whitespace has no token: the statement around it, and so on.
    let (_, result) = select(&mut client, &main, &[(1, 0)]);

    assert_eq!(texts(TEXT, &result[0], false), vec![TEXT.to_string()]);
}

/// Over every module of std and the fixtures, at every token's start:
/// each range contains the one before it and is larger, the first is
/// at the position, the last is the whole text, and every declaration the
/// syntax has around the position is one of them.
#[test]
fn every_declaration_around_a_position_is_selected() {
    let std = upstream_std();
    let mut files = Vec::new();
    let mut checked = 0;
    let mut documented = 0;

    fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();

            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "inko") {
                out.push(path);
            }
        }
    }

    sources(&std, &mut files);
    sources(&std.parent().unwrap().join("test"), &mut files);
    sources(Path::new("tests/fixtures"), &mut files);
    files.sort();

    for (n, file) in files.iter().enumerate() {
        let text = std::fs::read_to_string(file).unwrap();
        let Some(outline) = syntax::parse(&text) else {
            continue;
        };
        let all = outline.all();

        // Every name of every declaration, and in a few modules, every
        // token's start too.
        let mut offsets: Vec<usize> = all.iter().map(|(d, _)| d.name_range.start).collect();

        if n % 16 == 0 {
            offsets.extend(
                text.char_indices()
                    .filter(|(i, c)| {
                        !c.is_whitespace()
                            && text[..*i]
                                .chars()
                                .next_back()
                                .is_none_or(|b| b.is_whitespace() || !b.is_alphanumeric())
                    })
                    .map(|(i, _)| i),
            );
        }

        for (offset, chain) in offsets.iter().zip(syntax::selections(&text, &offsets)) {
            let offset = *offset;

            checked += 1;
            assert!(!chain.is_empty());
            assert!(
                chain[0].start <= offset && offset <= chain[0].end,
                "{}: at {}: {:?}",
                file.display(),
                offset,
                chain
            );
            assert_eq!(chain.last().unwrap(), &(0..text.len()));

            for pair in chain.windows(2) {
                assert!(
                    pair[1].start <= pair[0].start
                        && pair[0].end <= pair[1].end
                        && pair[0] != pair[1],
                    "{}: at {}: {:?} isn't inside {:?}",
                    file.display(),
                    offset,
                    pair[0],
                    pair[1]
                );
            }

            for (d, _) in &all {
                if d.range.start <= offset && offset <= d.range.end {
                    assert!(
                        chain.contains(&d.range),
                        "{}: at {}: {} ({:?}) isn't selected: {:?}",
                        file.display(),
                        offset,
                        d.name,
                        d.range,
                        chain
                    );

                    // And again with its doc comment, from its first `#`.
                    let from = text[d.header.start..].trim_start();

                    if d.documentation.is_some() && from.starts_with('#') {
                        let comment = text.len() - from.len();

                        documented += 1;
                        assert!(
                            chain.contains(&(comment..d.range.end)),
                            "{}: at {}: {} with its doc comment isn't selected: {:?}",
                            file.display(),
                            offset,
                            d.name,
                            chain
                        );
                    }
                }
            }
        }
    }

    assert!(checked > 20_000, "{}", checked);
    assert!(documented > 5_000, "{}", documented);
}
