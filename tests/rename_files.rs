//! `workspace/willRenameFiles` (docs/lsp.md): the edits to every import of
//! a module that moves, and to its uses by its own name, over LSP in both
//! encodings, each applied to a copy of the project and checked with the
//! files moved.
use crate::common::client::{Client, Launch, uri};
use cuttlebone::position::{Encoding, Position, byte_offset};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use url::Url;

const UTIL: &str = "fn pub helper -> Int {\n  1\n}\n";

const MAIN: &str = "import app.util
import app.util (helper)

fn total -> Int {
  let s = '日本 😀'.size + util.helper

  s + helper
}

type async Main {
  fn async main {
    total
  }
}
";

const OTHER: &str = "import app.util (self as u)\n\nfn pub x -> Int {\n  u.helper\n}\n";

fn project(name: &str) -> PathBuf {
    let root = crate::common::tmp().join(format!("rename-files-{}", name));
    let _ = std::fs::remove_dir_all(&root);

    std::fs::create_dir_all(root.join("src/app")).unwrap();
    std::fs::write(root.join("src/app/util.inko"), UTIL).unwrap();
    std::fs::write(root.join("src/main.inko"), MAIN).unwrap();
    std::fs::write(root.join("src/other.inko"), OTHER).unwrap();
    root.canonicalize().unwrap()
}

fn server(root: &Path, utf8: bool, rename: bool) -> Client {
    Client::initialized_with(
        Launch::default(),
        root,
        json!({ "statusNotifications": true, "diagnostics": false }),
        json!({
            "general": { "positionEncodings": if utf8 { ["utf-8"] } else { ["utf-16"] } },
            "workspace": { "fileOperations": { "willRename": rename } },
        }),
    )
}

fn will_rename(client: &mut Client, from: &Path, to: &Path) -> (i64, Value) {
    let id = client.send_request(
        "workspace/willRenameFiles",
        json!({ "files": [{ "oldUri": uri(from), "newUri": uri(to) }] }),
    );
    let response = client.response(id);

    assert!(response.get("error").is_none(), "{}", response);
    (id, response["result"].clone())
}

/// Every file's text with a workspace edit applied, by path relative to
/// `root`.
fn applied(root: &Path, edit: &Value, utf8: bool) -> BTreeMap<String, String> {
    let encoding = if utf8 {
        Encoding::Utf8
    } else {
        Encoding::Utf16
    };
    let mut out = BTreeMap::new();

    for (uri, edits) in edit["changes"].as_object().unwrap() {
        let path = Url::parse(uri).unwrap().to_file_path().unwrap();
        let mut text = std::fs::read_to_string(&path).unwrap();
        let offset = |text: &str, p: &Value| {
            byte_offset(
                text,
                Position {
                    line: p["line"].as_u64().unwrap() as u32,
                    character: p["character"].as_u64().unwrap() as u32,
                },
                encoding,
            )
            .unwrap()
        };
        let mut edits: Vec<(usize, usize, String)> = edits
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    offset(&text, &e["range"]["start"]),
                    offset(&text, &e["range"]["end"]),
                    e["newText"].as_str().unwrap().to_string(),
                )
            })
            .collect();

        edits.sort();

        for (start, end, new) in edits.into_iter().rev() {
            text.replace_range(start..end, &new);
        }

        out.insert(path.strip_prefix(root).unwrap().display().to_string(), text);
    }

    out
}

#[test]
fn renaming_a_module_renames_its_imports_and_uses() {
    for utf8 in [false, true] {
        let root = project(if utf8 { "utf8" } else { "utf16" });
        let mut client = server(&root, utf8, true);
        let util = root.join("src/app/util.inko");
        let (id, edit) = will_rename(&mut client, &util, &root.join("src/app/tools.inko"));

        assert_eq!(client.status(id).unwrap()["status"], "fresh");

        let mut want = BTreeMap::new();

        want.insert(
            "src/main.inko".to_string(),
            MAIN.replace("app.util", "app.tools")
                .replace("util.helper", "tools.helper"),
        );
        // An alias stays.
        want.insert(
            "src/other.inko".to_string(),
            OTHER.replace("app.util", "app.tools"),
        );
        assert_eq!(applied(&root, &edit, utf8), want, "UTF-8: {}", utf8);
    }
}

#[test]
fn moving_a_module_or_its_directory_renames_its_imports() {
    let root = project("move");
    let mut client = server(&root, false, true);
    let util = root.join("src/app/util.inko");

    // Its last name is the same: only the paths change.
    let (_, edit) = will_rename(&mut client, &util, &root.join("src/lib/util.inko"));
    let got = applied(&root, &edit, false);

    assert_eq!(got["src/main.inko"], MAIN.replace("app.util", "lib.util"));
    assert_eq!(got["src/other.inko"], OTHER.replace("app.util", "lib.util"));

    // A directory: every module in it.
    let (_, edit) = will_rename(&mut client, &root.join("src/app"), &root.join("src/core"));
    let got = applied(&root, &edit, false);

    assert_eq!(got["src/main.inko"], MAIN.replace("app.util", "core.util"));

    // Out of src/, or to a name no module can have: nothing.
    assert_eq!(
        will_rename(&mut client, &util, &root.join("util.inko")).1,
        Value::Null
    );
    assert_eq!(
        will_rename(&mut client, &util, &root.join("src/app/My-Util.inko")).1,
        Value::Null
    );
    // A module nothing imports.
    assert_eq!(
        will_rename(
            &mut client,
            &root.join("src/other.inko"),
            &root.join("src/another.inko")
        )
        .1,
        Value::Null
    );
}

/// The edits are applied, the files moved, and the project still has no
/// error: what `inko check` would say, from Cuttlebone's own diagnostics.
#[test]
fn the_project_checks_after_the_rename() {
    let root = project("check");
    let mut client = server(&root, false, true);
    let util = root.join("src/app/util.inko");
    let to = root.join("src/core/tools.inko");
    let (_, edit) = will_rename(&mut client, &util, &to);

    for (file, text) in applied(&root, &edit, false) {
        std::fs::write(root.join(file), text).unwrap();
    }

    std::fs::create_dir_all(to.parent().unwrap()).unwrap();
    std::fs::rename(&util, &to).unwrap();

    let report = cuttlebone::query::diagnostics(
        &cuttlebone::query::Session {
            std: cuttlebone::dump::upstream_std(),
            projects: vec![root.clone()],
        },
        &cuttlebone::query::Request {
            file: root.clone(),
            position: Position {
                line: 0,
                character: 0,
            },
            encoding: Encoding::Utf16,
            contents: Default::default(),
        },
    );
    let found = report.result.unwrap().unwrap().diagnostics;

    assert!(found.is_empty(), "{:?}", found);
}

#[test]
fn only_clients_that_rename_files_are_asked_to() {
    let root = project("capability");
    let mut client = Client::start();
    let result = client.initialize(
        &root,
        json!({ "diagnostics": false }),
        json!({ "workspace": { "fileOperations": { "willRename": false } } }),
    );

    assert_eq!(result["result"]["capabilities"].get("workspace"), None);

    let mut client = Client::start();
    let result = client.initialize(
        &root,
        json!({ "diagnostics": false }),
        json!({ "workspace": { "fileOperations": { "willRename": true } } }),
    );

    assert_eq!(
        result["result"]["capabilities"]["workspace"]["fileOperations"]["willRename"]["filters"][0]
            ["pattern"]["glob"],
        "**/*.inko"
    );
}
