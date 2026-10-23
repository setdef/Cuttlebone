//! The JSON protocol Parakeet speaks to Cuttlebone over stdio, documented in
//! docs/protocol.md: one JSON object per line in each direction.
use crate::position::Position;
use crate::query::{self, Answer, Range, Request, Session, Unavailable};
use crate::{VERSION, upstream_version};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::Command;

/// The protocol's version. It changes only when a change would break an
/// existing client.
pub const PROTOCOL: u64 = 1;

pub struct Server {
    session: Option<Session>,
}

/// What to do after replying.
#[derive(PartialEq, Eq)]
pub enum Next {
    Continue,
    Exit,
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

impl Server {
    pub fn new() -> Self {
        Server { session: None }
    }

    /// Reads requests from `input` and writes replies to `output` until the
    /// input ends or a `shutdown` request arrives.
    pub fn serve(&mut self, input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
        for line in input.lines() {
            let line = line?;

            if line.trim().is_empty() {
                continue;
            }

            let (reply, next) = self.handle(&line);

            serde_json::to_writer(&mut output, &reply)?;
            output.write_all(b"\n")?;
            output.flush()?;

            if next == Next::Exit {
                break;
            }
        }

        Ok(())
    }

    /// Handles one request line and returns its reply.
    pub fn handle(&mut self, line: &str) -> (Value, Next) {
        let request: Value = match serde_json::from_str(line) {
            Ok(v @ Value::Object(_)) => v,
            Ok(_) => {
                return (
                    invalid(Value::Null, "a request must be a JSON object"),
                    Next::Continue,
                );
            }
            Err(e) => {
                return (
                    invalid(Value::Null, &format!("invalid JSON: {}", e)),
                    Next::Continue,
                );
            }
        };
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let params = request.get("params").cloned().unwrap_or(Value::Null);

        let Some(method) = request.get("method").and_then(Value::as_str) else {
            return (invalid(id, "the request has no \"method\""), Next::Continue);
        };

        match method {
            "initialize" => (self.initialize(id, &params), Next::Continue),
            "shutdown" => (fresh(id, Value::Null), Next::Exit),
            "typeAt" | "membersAt" => {
                let Some(session) = &self.session else {
                    return (
                        unavailable(
                            id,
                            "notInitialized",
                            "send \"initialize\" first",
                            Map::new(),
                        ),
                        Next::Continue,
                    );
                };

                let request = match position_request(&params) {
                    Ok(r) => r,
                    Err(e) => return (invalid(id, &e), Next::Continue),
                };

                let reply = if method == "typeAt" {
                    query_reply(
                        id,
                        query::type_at(session, &request),
                        |t| json!({ "type": t.typ, "range": range(t.range), "kind": t.kind }),
                    )
                } else {
                    query_reply(id, query::members_at(session, &request), |m| {
                        let members: Vec<_> = m
                            .members
                            .iter()
                            .map(|m| {
                                json!({
                                    "name": m.name,
                                    "kind": m.kind.name(),
                                    "signature": m.signature,
                                    "type": m.typ,
                                    "module": m.module,
                                })
                            })
                            .collect();

                        json!({
                            "receiver": { "type": m.receiver, "range": range(m.receiver_range) },
                            "members": members,
                        })
                    })
                };

                (reply, Next::Continue)
            }
            _ => (
                unavailable(
                    id,
                    "unknownMethod",
                    &format!("there's no method {:?}", method),
                    Map::new(),
                ),
                Next::Continue,
            ),
        }
    }

    fn initialize(&mut self, id: Value, params: &Value) -> Value {
        if let Some(v) = params.get("protocol").and_then(Value::as_u64)
            && v != PROTOCOL
        {
            let mut extra = Map::new();

            extra.insert("protocol".into(), json!(PROTOCOL));

            return unavailable(
                id,
                "protocolMismatch",
                &format!("this build speaks protocol {}, not {}", PROTOCOL, v),
                extra,
            );
        }

        let inko = params
            .get("inko")
            .and_then(Value::as_str)
            .unwrap_or("inko")
            .to_string();
        let project = params
            .get("project")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .map(|p| p.canonicalize().unwrap_or(p));
        let supported = upstream_version();
        let mut extra = Map::new();

        extra.insert("cuttlebone".into(), json!(VERSION));
        extra.insert("supported".into(), json!(supported));

        let installed = match inko_output(&inko, &["--version"]) {
            Ok(out) => out.trim().trim_start_matches("inko").trim().to_string(),
            Err(e) => return unavailable(id, "inkoNotFound", &e, extra),
        };

        extra.insert("installed".into(), json!(installed));

        if installed != supported {
            return unavailable(
                id,
                "inkoVersionMismatch",
                &format!(
                    "this build of Cuttlebone supports Inko {}, but {} is Inko {}",
                    supported, inko, installed
                ),
                extra,
            );
        }

        let std = match inko_output(&inko, &["print", "std"]) {
            Ok(out) => PathBuf::from(out.trim()),
            Err(e) => return unavailable(id, "inkoNotFound", &e, extra),
        };

        if !std.is_dir() {
            return unavailable(
                id,
                "inkoNotFound",
                &format!(
                    "{} prints {} as its standard library, which isn't a directory",
                    inko,
                    std.display()
                ),
                extra,
            );
        }

        let reply = json!({
            "cuttlebone": VERSION,
            "inko": supported,
            "protocol": PROTOCOL,
            "std": std,
        });

        self.session = Some(Session { std, project });
        fresh(id, reply)
    }
}

fn inko_output(inko: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(inko)
        .args(args)
        .output()
        .map_err(|e| format!("couldn't run {}: {}", inko, e))?;

    if !out.status.success() {
        return Err(format!(
            "`{} {}` failed: {}",
            inko,
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn position_request(params: &Value) -> Result<Request, String> {
    let file = params
        .get("file")
        .and_then(Value::as_str)
        .ok_or("\"file\" must be a string")?;
    let position = params.get("position").ok_or("\"position\" is missing")?;
    let line = position.get("line").and_then(Value::as_u64);
    let character = position.get("character").and_then(Value::as_u64);
    let (Some(line), Some(character)) = (line, character) else {
        return Err("\"position\" needs \"line\" and \"character\" numbers".to_string());
    };
    let mut contents = HashMap::new();

    match params.get("contents") {
        None | Some(Value::Null) => {}
        Some(Value::Object(map)) => {
            for (path, text) in map {
                let text = text.as_str().ok_or("\"contents\" values must be strings")?;

                contents.insert(PathBuf::from(path), text.to_string());
            }
        }
        Some(_) => return Err("\"contents\" must be an object".to_string()),
    }

    Ok(Request {
        file: PathBuf::from(file),
        position: Position {
            line: line as u32,
            character: character as u32,
        },
        contents,
    })
}

fn query_reply<T>(id: Value, answer: Answer<T>, result: impl FnOnce(&T) -> Value) -> Value {
    let ms = answer.elapsed.as_secs_f64() * 1000.0;
    let mut reply = match answer.result {
        Ok(v) => fresh(id, v.as_ref().map_or(Value::Null, result)),
        Err(Unavailable { code, message }) => unavailable(id, code, &message, Map::new()),
    };

    reply["elapsedMs"] = json!((ms * 10.0).round() / 10.0);
    reply
}

fn range(r: Range) -> Value {
    json!({
        "start": { "line": r.start.line, "character": r.start.character },
        "end": { "line": r.end.line, "character": r.end.character },
    })
}

fn fresh(id: Value, result: Value) -> Value {
    json!({ "id": id, "status": "fresh", "result": result })
}

fn unavailable(id: Value, code: &str, message: &str, mut extra: Map<String, Value>) -> Value {
    extra.insert("code".into(), json!(code));
    extra.insert("message".into(), json!(message));
    json!({ "id": id, "status": "unavailable", "reason": extra })
}

fn invalid(id: Value, message: &str) -> Value {
    unavailable(id, "invalidRequest", message, Map::new())
}
