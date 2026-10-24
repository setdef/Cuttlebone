//! Completion after a `.`: `query::complete` inserts its own placeholder at
//! the cursor, so a client sends its buffer as it is.

use crate::common::{Project, position_of};
use cuttlebone::position::byte_offset;
use cuttlebone::position::{Encoding, Position};
use cuttlebone::query::{self, PLACEHOLDER};
use std::collections::HashMap;

const MAIN: &str = "src/main.inko";

/// The members at the end of the first `needle` (plus `extra` UTF-16 units)
/// in `text`, sent as the file's unsaved contents.
fn complete_in(
    p: &Project,
    text: &str,
    needle: &str,
    extra: i32,
) -> Result<Option<query::MembersAt>, query::Unavailable> {
    let mut position = position_of(text, needle, needle.encode_utf16().count() as u32);

    position.character = (position.character as i32 + extra) as u32;

    let request = query::Request {
        file: p.path(MAIN),
        position,
        encoding: Encoding::Utf16,
        contents: HashMap::from([(p.path(MAIN), text.to_string())]),
    };

    query::complete(&p.session, &request).result
}

fn names(m: &query::MembersAt) -> Vec<&str> {
    m.members.iter().map(|m| m.name.as_str()).collect()
}

#[test]
fn after_a_dot_with_nothing_after_it() {
    let p = Project::new("members");
    let text = p.read(MAIN).replacen("name.size", "name.", 1);
    let m = complete_in(&p, &text, "name.", 0).unwrap().unwrap();

    assert_eq!(m.receiver, "String");
    assert!(names(&m).contains(&"size"));
    assert!(!names(&m).contains(&PLACEHOLDER));
}

#[test]
fn after_a_typed_prefix() {
    let p = Project::new("members");
    let text = p.read(MAIN).replacen("name.size", "name.si", 1);
    let m = complete_in(&p, &text, "name.si", 0).unwrap().unwrap();

    assert_eq!(m.receiver, "String");
    assert!(names(&m).contains(&"size"));
}

#[test]
fn in_the_middle_of_a_name() {
    let p = Project::new("members");
    let text = p.read(MAIN);
    let m = complete_in(&p, &text, "name.si", 0).unwrap().unwrap();

    assert_eq!(m.receiver, "String");
}

#[test]
fn on_disk_when_there_are_no_unsaved_contents() {
    let p = Project::new("members");
    let text = p.read(MAIN);
    let request = query::Request {
        file: p.path(MAIN),
        position: position_of(&text, "counter.count", 8),
        encoding: Encoding::Utf16,
        contents: HashMap::new(),
    };
    let m = query::complete(&p.session, &request)
        .result
        .unwrap()
        .unwrap();

    assert_eq!(m.receiver, "Counter");
    assert!(names(&m).contains(&"label"));
}

#[test]
fn unsaved_contents_under_another_name_for_the_file_replace_it() {
    // The file's own contents are replaced by the copy with the placeholder,
    // even when they're given under a path that isn't canonical.
    let p = Project::new("members");
    let text = p.read(MAIN).replacen("name.size", "name.", 1);
    let request = query::Request {
        file: p.path(MAIN),
        position: position_of(&text, "name.", 5),
        encoding: Encoding::Utf16,
        contents: HashMap::from([(p.root.join("src/../src/main.inko"), text.clone())]),
    };
    let m = query::complete(&p.session, &request)
        .result
        .unwrap()
        .unwrap();

    assert_eq!(m.receiver, "String");
}

#[test]
fn utf8_positions() {
    let p = Project::new("unicode");
    let text = p.read(MAIN).replacen("[3].size", "[3].", 1);
    let byte = text.find("[3].").unwrap() + 4;
    let line_start = text[..byte].rfind('\n').unwrap() + 1;
    let request = query::Request {
        file: p.path(MAIN),
        position: Position {
            line: text[..byte].matches('\n').count() as u32,
            character: (byte - line_start) as u32,
        },
        encoding: Encoding::Utf8,
        contents: HashMap::from([(p.path(MAIN), text.clone())]),
    };
    let m = query::complete(&p.session, &request)
        .result
        .unwrap()
        .unwrap();

    assert_eq!(m.receiver, "Array[Int]");
}

#[test]
fn where_nothing_has_members_there_is_no_result() {
    let p = Project::new("members");
    let text = p.read(MAIN);

    assert_eq!(complete_in(&p, &text, "# Receivers", 0), Ok(None));
}

#[test]
fn a_line_past_the_end_is_an_invalid_position() {
    let p = Project::new("members");
    let request = query::Request {
        file: p.path(MAIN),
        position: Position {
            line: 9999,
            character: 0,
        },
        encoding: Encoding::Utf16,
        contents: HashMap::new(),
    };

    assert_eq!(
        query::complete(&p.session, &request)
            .result
            .unwrap_err()
            .code,
        "invalidPosition"
    );
}

#[test]
fn byte_offsets() {
    let pos = |line, character| Position { line, character };
    let text = "aé😀b\r\nx\n";

    for (encoding, character, byte) in [
        (Encoding::Utf16, 0, 0),
        (Encoding::Utf16, 1, 1),
        (Encoding::Utf16, 2, 3),
        // Inside the surrogate pair: the start of the 😀.
        (Encoding::Utf16, 3, 3),
        (Encoding::Utf16, 4, 7),
        (Encoding::Utf16, 5, 8),
        // Past the end of the line: before the \r\n.
        (Encoding::Utf16, 99, 8),
        (Encoding::Utf8, 1, 1),
        (Encoding::Utf8, 2, 1),
        (Encoding::Utf8, 3, 3),
        (Encoding::Utf8, 7, 7),
    ] {
        assert_eq!(
            byte_offset(text, pos(0, character), encoding),
            Some(byte),
            "{:?} {}",
            encoding,
            character
        );
    }

    assert_eq!(byte_offset(text, pos(1, 0), Encoding::Utf16), Some(10));
    assert_eq!(byte_offset(text, pos(1, 5), Encoding::Utf16), Some(11));
    // The empty line after the last \n.
    assert_eq!(byte_offset(text, pos(2, 0), Encoding::Utf16), Some(12));
    assert_eq!(byte_offset(text, pos(3, 0), Encoding::Utf16), None);
}
