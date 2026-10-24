//! A session over LSP on tests/fixtures/features, for the tests of
//! highlights, signature help and inlay hints.
use super::client::{Client, Launch, at};
use super::fixture;
use serde_json::{Value, json};
use std::path::PathBuf;

pub struct Session {
    pub client: Client,
    pub file: PathBuf,
    pub text: String,
    pub utf8: bool,
}

impl Session {
    pub fn new(utf8: bool) -> Session {
        Session::launched(utf8, Launch::default())
    }

    pub fn launched(utf8: bool, launch: Launch) -> Session {
        let root = fixture("features");
        let file = root.join("src/main.inko");
        let text = std::fs::read_to_string(&file).unwrap();
        let encodings = if utf8 {
            json!(["utf-8"])
        } else {
            json!(["utf-16"])
        };
        let client = Client::initialized_with(
            launch,
            &root,
            json!({ "statusNotifications": true, "diagnostics": false }),
            json!({ "general": { "positionEncodings": encodings } }),
        );

        Session {
            client,
            file,
            text,
            utf8,
        }
    }

    /// The response to `method` at `needle`, plus `offset` characters.
    pub fn request(&mut self, method: &str, needle: &str, offset: usize) -> (i64, Value) {
        let (line, character) = at(&self.text, needle, offset, self.utf8);
        let id = self
            .client
            .send_at(method, &self.file.clone(), line, character);

        (id, self.client.response(id))
    }

    /// The text a range covers, and its line, reading the range's
    /// characters in the session's encoding.
    pub fn slice(&self, range: &Value) -> (u64, String) {
        let line = range["start"]["line"].as_u64().unwrap();
        let text = self.text.lines().nth(line as usize).unwrap();
        let byte = |character: u64| {
            let mut units = 0;

            for (i, c) in text.char_indices() {
                if units >= character {
                    return i;
                }

                units += if self.utf8 {
                    c.len_utf8() as u64
                } else {
                    c.len_utf16() as u64
                };
            }

            text.len()
        };

        assert_eq!(range["end"]["line"], line);

        let start = byte(range["start"]["character"].as_u64().unwrap());
        let end = byte(range["end"]["character"].as_u64().unwrap());

        (line, text[start..end].to_string())
    }
}
