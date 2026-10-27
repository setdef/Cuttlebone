//! `textDocument/formatting` (docs/lsp.md): against the oracle, the
//! installed `inko fmt` (given the text on stdin, so it writes no file),
//! over every module of std and the fixtures, as they are and with their
//! indentation taken away; and over LSP in both encodings, with nothing
//! written.
use crate::common::client::{Client, uri};
use cuttlebone::dump::upstream_std;
use cuttlebone::syntax;
use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The installed `inko`, which must be the pinned release.
fn inko() -> &'static str {
    let out = Command::new("inko")
        .arg("--version")
        .output()
        .expect("the formatting oracle needs `inko` on PATH");

    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("inko {}", cuttlebone::check::upstream_version()),
        "the formatting oracle needs the installed inko to be the pinned release"
    );
    "inko"
}

/// What `inko fmt -` makes of `text`, or `None` if it refuses it.
fn inko_fmt(text: &str) -> Option<String> {
    let mut child = Command::new(inko())
        .args(["fmt", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    child
        .stdin
        .take()
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();

    let out = child.wait_with_output().unwrap();

    out.status
        .success()
        .then(|| String::from_utf8(out.stdout).unwrap())
}

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

/// Every line without its indentation: text `inko fmt` has to indent
/// again.
fn flattened(text: &str) -> String {
    text.split_inclusive('\n')
        .map(|l| l.trim_start_matches([' ', '\t']))
        .collect()
}

#[test]
fn formatting_is_inko_fmts_over_std_and_the_fixtures() {
    let std = upstream_std();
    let mut files = Vec::new();

    sources(&std, &mut files);
    sources(&std.parent().unwrap().join("test"), &mut files);
    sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures"),
        &mut files,
    );
    files.sort();

    let texts: Vec<(String, String)> = files
        .iter()
        .flat_map(|f| {
            let text = std::fs::read_to_string(f).unwrap();
            let name = f.display().to_string();

            [(name.clone(), flattened(&text)), (name, text)]
        })
        .collect();
    let chunks: Vec<&[(String, String)]> = texts.chunks(texts.len().div_ceil(8)).collect();
    let results: Vec<(usize, usize, usize, Vec<String>)> = std::thread::scope(|s| {
        let handles: Vec<_> = chunks
            .iter()
            .map(|chunk| {
                s.spawn(move || {
                    let (mut same, mut changed, mut refused) = (0, 0, 0);
                    let mut wrong = Vec::new();

                    for (name, text) in *chunk {
                        let want = inko_fmt(text);
                        let got = syntax::format(text);

                        match (&want, &got) {
                            (None, None) => refused += 1,
                            (Some(w), Some(g)) if w == g && w == text => same += 1,
                            (Some(w), Some(g)) if w == g => changed += 1,
                            _ => wrong.push(name.clone()),
                        }
                    }

                    (same, changed, refused, wrong)
                })
            })
            .collect();

        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let same: usize = results.iter().map(|r| r.0).sum();
    let changed: usize = results.iter().map(|r| r.1).sum();
    let refused: usize = results.iter().map(|r| r.2).sum();
    let wrong: Vec<&String> = results.iter().flat_map(|r| &r.3).collect();

    assert!(wrong.is_empty(), "formatted unlike inko fmt: {:?}", wrong);
    // std is formatted, and none of it is flat; the fixtures that don't
    // parse are refused by both. 259 files, as they are and flat.
    assert!(same >= 250, "{} already formatted", same);
    assert!(changed >= 260, "{} formatted", changed);
    assert!(refused >= 2, "{} refused", refused);
}

/// A module with something for `inko fmt` to change on most lines, and
/// text after non-ASCII characters on the lines it changes.
const MESSY: &str = "import std.stdio (Stdout)
import std.fmt (Format)

type async Main {
fn async main {
      let s = 'héllo 中文😀'   +    'x'
  Stdout.new.print(s + 'é'  )
  }
}
";

fn server(root: &Path, utf8: bool) -> Client {
    let encodings = if utf8 {
        json!(["utf-8"])
    } else {
        json!(["utf-16"])
    };

    Client::initialized_with(
        Default::default(),
        root,
        json!({ "statusNotifications": true, "diagnostics": false }),
        json!({ "general": { "positionEncodings": encodings } }),
    )
}

fn project(name: &str, text: &str) -> (PathBuf, PathBuf) {
    let root = crate::common::tmp().join(format!("formatting-{}", name));
    let _ = std::fs::remove_dir_all(&root);

    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.inko"), text).unwrap();

    let root = root.canonicalize().unwrap();
    let main = root.join("src/main.inko");

    (root, main)
}

/// `text` with `edits` applied, reading their ranges in the encoding.
fn apply(text: &str, edits: &Value, utf8: bool) -> String {
    let offset = |p: &Value| {
        let line = p["line"].as_u64().unwrap() as usize;
        let character = p["character"].as_u64().unwrap() as usize;
        let start: usize = text.split_inclusive('\n').take(line).map(str::len).sum();
        let mut units = 0;

        for (i, c) in text[start..].char_indices() {
            if units >= character {
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

fn format(client: &mut Client, path: &Path) -> (i64, Value) {
    let id = client.send_request(
        "textDocument/formatting",
        json!({
            "textDocument": { "uri": uri(path) },
            "options": { "tabSize": 8, "insertSpaces": false },
        }),
    );

    (id, client.response(id)["result"].clone())
}

#[test]
fn formatting_over_lsp_gives_inko_fmts_text_and_writes_nothing() {
    let want = inko_fmt(MESSY).unwrap();

    assert_ne!(want, MESSY);

    for utf8 in [false, true] {
        let (root, main) = project(if utf8 { "utf8" } else { "utf16" }, MESSY);
        let modified = std::fs::metadata(&main).unwrap().modified().unwrap();
        let mut client = server(&root, utf8);

        // Not open: the file's text.
        let (id, edits) = format(&mut client, &main);

        assert_eq!(apply(MESSY, &edits, utf8), want, "UTF-8: {}", utf8);
        assert_eq!(edits.as_array().unwrap().len(), 1);
        assert_eq!(client.status(id).unwrap()["status"], "fresh");

        // Open, with other text: the document's. The options don't
        // matter: `inko fmt` has none.
        let other = MESSY.replace("'x'", "'y'");

        client.open(&main, &other);

        let (_, edits) = format(&mut client, &main);

        assert_eq!(apply(&other, &edits, utf8), want.replace("'x'", "'y'"));

        // Formatted already: no edits.
        client.change(&main, 2, json!([{ "text": want }]));

        let (_, edits) = format(&mut client, &main);

        assert_eq!(edits, json!([]));

        // Nothing was written.
        assert_eq!(std::fs::read_to_string(&main).unwrap(), MESSY);
        assert_eq!(
            std::fs::metadata(&main).unwrap().modified().unwrap(),
            modified
        );
    }
}

#[test]
fn formatting_text_that_doesnt_parse_has_no_answer() {
    let (root, main) = project("broken", MESSY);
    let mut client = server(&root, false);
    let broken = MESSY.replace("Stdout.new.print(s + 'é'  )", "Stdout.new.print(s + 'é'");

    assert_eq!(inko_fmt(&broken), None);
    client.open(&main, &broken);

    let (id, result) = format(&mut client, &main);
    let status = client.status(id).unwrap();

    assert_eq!(result, Value::Null);
    assert_eq!(status["status"], "unavailable");
    assert_eq!(status["reason"]["code"], "noHir");
}
