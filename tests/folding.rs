//! `textDocument/foldingRange` (docs/lsp.md): blocks, comment lines,
//! imports, from the tokens alone, over LSP in both encodings, in code
//! that doesn't parse, with the text shown collapsed, for clients
//! without kinds, collapsed text or with a limit; and
//! against the declarations of every module of std and the fixtures.
use crate::common::client::{Client, uri};
use cuttlebone::dump::upstream_std;
use cuttlebone::position::{Encoding, position_at};
use cuttlebone::syntax::{self, FoldKind};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const TEXT: &str = "# The module's doc comment,
# over two lines.
import std.fmt (Format)
import std.stdio (
  Stdout,
)

# A doc comment
# for Counter.
type pub Counter {
  let @count: Int

  fn pub mut add(
    amount: Int,
    times: Int,
  ) {
    let s = '{ not a block'
    # { nor this
    @count += amount * times
  }
}

type async Main {
  fn async main {
    let a = [
      1,
      2,
    ]
    Stdout.new.print('héllo 中文😀'.to_string)
  }
}
";

/// The folds of `TEXT`: lines, kind, and the text shown collapsed.
const FOLDS: &[(u32, u32, Option<&str>, &str)] = &[
    (0, 1, Some("comment"), "# The module's doc comment,"),
    (2, 5, Some("imports"), "import ..."),
    (3, 4, None, "(...)"),
    (7, 8, Some("comment"), "# A doc comment"),
    (9, 19, None, "{...}"),
    (12, 14, None, "(...)"),
    (15, 18, None, "{...}"),
    (22, 29, None, "{...}"),
    (23, 28, None, "{...}"),
    (24, 26, None, "[...]"),
];

/// `FOLDS`, with kinds and collapsed text for a client that takes them
/// (as the tests' clients do, unless they say otherwise).
fn expected(kinds: bool) -> Value {
    expected_with(kinds, kinds)
}

fn expected_with(kinds: bool, collapsed: bool) -> Value {
    FOLDS
        .iter()
        .map(|(start, end, kind, text)| {
            let mut v = json!({ "startLine": start, "endLine": end });

            if let (true, Some(kind)) = (kinds, kind) {
                v["kind"] = json!(kind);
            }

            if collapsed {
                v["collapsedText"] = json!(text);
            }

            v
        })
        .collect()
}

fn project(name: &str) -> (PathBuf, PathBuf) {
    let root = crate::common::tmp().join(format!("folding-{}", name));
    let _ = std::fs::remove_dir_all(&root);

    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.inko"), TEXT).unwrap();

    let root = root.canonicalize().unwrap();
    let main = root.join("src/main.inko");

    (root, main)
}

fn server(root: &Path, utf8: bool, capabilities: Value) -> Client {
    let mut all = json!({
        "general": { "positionEncodings": if utf8 { ["utf-8"] } else { ["utf-16"] } },
    });

    crate::common::client::merge_deep(&mut all, capabilities);
    Client::initialized_with(
        Default::default(),
        root,
        json!({ "statusNotifications": true, "diagnostics": false }),
        all,
    )
}

fn folds(client: &mut Client, path: &Path) -> (i64, Value) {
    let id = client.send_request(
        "textDocument/foldingRange",
        json!({ "textDocument": { "uri": uri(path) } }),
    );

    (id, client.response(id)["result"].clone())
}

#[test]
fn blocks_comments_and_imports_fold() {
    for utf8 in [false, true] {
        let (root, main) = project(if utf8 { "utf8" } else { "utf16" });
        let mut client = server(&root, utf8, json!({}));

        // Not open: the file's.
        let (id, result) = folds(&mut client, &main);

        assert_eq!(result, expected(true), "UTF-8: {}", utf8);
        assert_eq!(client.status(id).unwrap()["status"], "fresh");

        client.open(&main, TEXT);
        assert_eq!(folds(&mut client, &main).1, expected(true));
    }
}

/// Folds come from the tokens, so code that doesn't parse folds as it
/// did, except at a bracket nothing closes.
#[test]
fn code_that_doesnt_parse_folds() {
    let (root, main) = project("broken");
    let mut client = server(&root, false, json!({}));
    let unclosed = TEXT.replace("@count += amount * times", "@count += (amount * times");

    assert!(syntax::parse(&unclosed).is_none());
    client.open(&main, &unclosed);
    assert_eq!(folds(&mut client, &main).1, expected(true));

    // Counter's `{` is never closed, once the method's `}` is gone.
    let open = TEXT.replacen(
        "    @count += amount * times\n  }\n",
        "    @count += amount * times\n",
        1,
    );
    let want: Vec<Value> = expected(true)
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["startLine"] != 9)
        .map(|f| {
            if f["startLine"] == 15 {
                json!({ "startLine": 15, "endLine": 18, "collapsedText": "{...}" })
            } else if f["startLine"].as_u64() > Some(15) {
                let mut f = f.clone();

                f["startLine"] = json!(f["startLine"].as_u64().unwrap() - 1);
                f["endLine"] = json!(f["endLine"].as_u64().unwrap() - 1);
                f
            } else {
                f.clone()
            }
        })
        .collect();

    assert!(syntax::parse(&open).is_none());
    client.change(&main, 2, json!([{ "text": open }]));
    assert_eq!(folds(&mut client, &main).1, json!(want));
}

#[test]
fn folds_have_kinds_and_a_limit_only_as_the_client_says() {
    let (root, main) = project("kinds");

    // No kinds at all.
    let mut client = server(
        &root,
        false,
        json!({ "textDocument": { "foldingRange": null } }),
    );

    assert_eq!(folds(&mut client, &main).1, expected(false));

    // Kinds, but no collapsed text.
    let mut client = server(
        &root,
        false,
        json!({ "textDocument": { "foldingRange": { "foldingRange": null } } }),
    );

    assert_eq!(folds(&mut client, &main).1, expected_with(true, false));

    // Comments only, and at most 4.
    let mut client = server(
        &root,
        false,
        json!({ "textDocument": { "foldingRange": {
            "foldingRangeKind": { "valueSet": ["comment"] },
            "rangeLimit": 4,
        }}}),
    );
    let want: Vec<Value> = expected(true).as_array().unwrap()[..4]
        .iter()
        .map(|f| {
            let mut f = f.clone();

            if f["kind"] == "imports" {
                f.as_object_mut().unwrap().remove("kind");
            }

            f
        })
        .collect();

    assert_eq!(folds(&mut client, &main).1, json!(want));
}

/// Lines are LSP's, which end at `\r\n` and a lone `\r` too.
#[test]
fn folds_are_on_lsps_lines() {
    let (root, main) = project("crlf");
    let mut client = server(&root, false, json!({}));

    client.open(&main, &TEXT.replace('\n', "\r\n"));
    assert_eq!(folds(&mut client, &main).1, expected(true));

    // A lone `\r` ends a line for LSP, not the compiler: the fold of the
    // method after one is a line further down.
    let lone = TEXT.replacen("type async Main {\n", "type async Main {\r\r\n", 1);

    client.change(&main, 2, json!([{ "text": lone }]));

    let got = folds(&mut client, &main).1;

    assert_eq!(
        got[7],
        json!({ "startLine": 22, "endLine": 30, "collapsedText": "{...}" })
    );
    assert_eq!(
        got[8],
        json!({ "startLine": 24, "endLine": 29, "collapsedText": "{...}" })
    );
    // A comment's first line, without the `\r` that ends it.
    assert_eq!(got[0]["collapsedText"], "# The module's doc comment,");

    // A lone `\r` in a comment ends LSP's line, not the comment: the text
    // shown collapsed is that line's.
    let lone = TEXT.replacen("# A doc comment\n", "# A doc\r comment\n", 1);

    client.change(&main, 3, json!([{ "text": lone }]));

    let got = folds(&mut client, &main).1;
    let doc = got
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["startLine"] == 7)
        .unwrap_or_else(|| panic!("no fold at line 7: {}", got));

    assert_eq!(doc["collapsedText"], "# A doc", "{}", doc);
}

/// Every declaration with a body over three lines or more, in std and the
/// fixtures, folds from its body's `{` to the line before its `}`, and
/// every doc comment of two lines or more folds too.
#[test]
fn every_declarations_body_and_doc_comment_folds() {
    let std = upstream_std();
    let mut files = Vec::new();
    let mut checked = (0, 0);

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

    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        let Some(outline) = syntax::parse(&text) else {
            continue;
        };
        let folds = syntax::folds(&text);
        let line = |offset: usize| position_at(&text, offset, Encoding::Utf16).line;

        for (d, _) in outline.all() {
            let end = d.range.end - 1;

            if text.as_bytes()[end] == b'}' && line(end) >= line(d.header.end) + 2 {
                checked.0 += 1;
                assert!(
                    folds
                        .iter()
                        .any(|f| f.kind == FoldKind::Block && f.end == end),
                    "{}: {} doesn't fold",
                    file.display(),
                    d.name
                );
            }

            // The doc comment is the comment lines right before it.
            if let Some(doc) = &d.documentation
                && doc.lines().count() >= 2
            {
                let above = line(d.range.start) - 1;

                checked.1 += 1;
                assert!(
                    folds.iter().any(|f| f.kind == FoldKind::Comment
                        && line(f.end) == above
                        && line(f.start) < above),
                    "{}: {}'s doc comment doesn't fold: {:?} {:?}",
                    file.display(),
                    d.name,
                    line(d.range.start),
                    folds
                        .iter()
                        .map(|f| (f.kind, line(f.start), line(f.end)))
                        .collect::<Vec<_>>()
                );
            }
        }
    }

    assert!(checked.0 > 3000 && checked.1 > 1000, "{:?}", checked);
}
