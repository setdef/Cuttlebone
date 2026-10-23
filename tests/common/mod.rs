//! Helpers shared by the integration tests.
#![allow(dead_code)]
use cuttlebone::dump::upstream_std;
use cuttlebone::position::{Encoding, Position};
use cuttlebone::query::{self, MembersAt, Request, Session, TypeAt, Unavailable};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A completion placeholder that isn't `query::PLACEHOLDER`: `members_at`
/// works with any name, and `query::complete` inserts its own.
pub const PLACEHOLDER: &str = "someMemberName";

pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
        .canonicalize()
        .unwrap()
}

/// A project in tests/fixtures, checked against the pinned std.
pub struct Project {
    pub root: PathBuf,
    pub session: Session,
}

impl Project {
    pub fn new(name: &str) -> Project {
        let root = fixture(name);

        Project {
            session: Session {
                std: upstream_std(),
                projects: vec![root.clone()],
            },
            root,
        }
    }

    pub fn path(&self, file: &str) -> PathBuf {
        self.root.join(file)
    }

    pub fn read(&self, file: &str) -> String {
        std::fs::read_to_string(self.path(file)).unwrap()
    }

    /// A request at `needle` (plus `offset` UTF-16 units into it) in
    /// `file`, whose contents are `text` instead of what's on disk if given.
    pub fn request(&self, file: &str, text: Option<&str>, needle: &str, offset: u32) -> Request {
        let path = self.path(file);
        let source = text.map(str::to_string).unwrap_or_else(|| self.read(file));
        let mut contents = HashMap::new();

        if let Some(text) = text {
            contents.insert(path.clone(), text.to_string());
        }

        Request {
            file: path,
            position: position_of(&source, needle, offset),
            encoding: Encoding::Utf16,
            contents,
        }
    }

    pub fn type_at(
        &self,
        file: &str,
        text: Option<&str>,
        needle: &str,
        offset: u32,
    ) -> Result<Option<TypeAt>, Unavailable> {
        query::type_at(&self.session, &self.request(file, text, needle, offset)).result
    }

    /// The type at `needle`, which must be fresh and non-null.
    pub fn type_of(&self, file: &str, needle: &str, offset: u32) -> String {
        match self.type_at(file, None, needle, offset) {
            Ok(Some(t)) => t.typ,
            other => panic!("no type at {:?}+{}: {:?}", needle, offset, other),
        }
    }

    /// Replaces `member` in `receiver.member` (the first occurrence of
    /// `before`) with a placeholder, and asks for the
    /// members at the placeholder.
    pub fn members_at(
        &self,
        file: &str,
        before: &str,
        after: &str,
    ) -> Result<Option<MembersAt>, Unavailable> {
        let text = self.read(file);

        assert!(text.contains(before), "{:?} isn't in {}", before, file);

        let text = text.replacen(before, after, 1);
        let request = self.request(
            file,
            Some(&text),
            after,
            after.find(PLACEHOLDER).unwrap() as u32,
        );

        query::members_at(&self.session, &request).result
    }
}

/// The UTF-16 position of the first occurrence of `needle`, plus `offset`
/// UTF-16 units.
pub fn position_of(text: &str, needle: &str, offset: u32) -> Position {
    let byte = text
        .find(needle)
        .unwrap_or_else(|| panic!("{:?} isn't in the text", needle));
    let before = &text[..byte];
    let line = before.matches('\n').count() as u32;
    let start = before.rfind('\n').map_or(0, |i| i + 1);

    Position {
        line,
        character: before[start..].encode_utf16().count() as u32 + offset,
    }
}
pub mod client;
pub mod features;
