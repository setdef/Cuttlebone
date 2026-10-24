//! Checks the messages the server sends against LSP 3.18: their shape,
//! against the specification's meta model (tests/fixtures/lsp), and that
//! they use only what the client's capabilities say it supports. The test
//! client runs every message it receives through `Spec::check`, so every
//! test over LSP checks conformance too (AGENTS.md, "Spec").
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::OnceLock;

pub struct Spec {
    requests: HashMap<String, Value>,
    notifications: HashMap<String, Value>,
    structures: HashMap<String, Value>,
    enumerations: HashMap<String, Value>,
    aliases: HashMap<String, Value>,
}

/// The 3.18 meta model, read once.
pub fn spec() -> &'static Spec {
    static SPEC: OnceLock<Spec> = OnceLock::new();

    SPEC.get_or_init(|| {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/lsp/metaModel-3.18.json"
        );
        let model: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let by = |key: &str, name: &str| -> HashMap<String, Value> {
            model[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| (v[name].as_str().unwrap().to_string(), v.clone()))
                .collect()
        };

        assert_eq!(model["metaData"]["version"], "3.18.0");

        Spec {
            requests: by("requests", "method"),
            notifications: by("notifications", "method"),
            structures: by("structures", "name"),
            enumerations: by("enumerations", "name"),
            aliases: by("typeAliases", "name"),
        }
    })
}

/// The methods Cuttlebone defines itself, which the meta model doesn't
/// have (docs/lsp.md, "Extensions").
const EXTENSIONS: &[&str] = &["cuttlebone/status"];

/// Properties Cuttlebone adds to the spec's structures, each an extension
/// docs/lsp.md justifies ("Extensions"): the reason `initialize` refuses
/// a session, beside `retry`.
const EXTRA_PROPERTIES: &[(&str, &[&str])] = &[(
    "InitializeError",
    &["code", "cuttlebone", "supported", "installed"],
)];

impl Spec {
    /// Checks a message from the server. `method` is the method of the
    /// request a response answers, and `capabilities` the client's.
    pub fn check(
        &self,
        message: &Value,
        method: Option<&str>,
        capabilities: &Value,
    ) -> Result<(), String> {
        if message["jsonrpc"] != "2.0" {
            return Err("jsonrpc isn't \"2.0\"".to_string());
        }

        let object = message.as_object().ok_or("not an object")?;

        if let Some(name) = object.get("method").and_then(Value::as_str) {
            if object.contains_key("id") {
                return Err(format!("the server sent a request, {}", name));
            }

            if EXTENSIONS.contains(&name) {
                return Ok(());
            }

            let notification = self
                .notifications
                .get(name)
                .ok_or_else(|| format!("{} isn't a notification in the spec", name))?;

            if notification["messageDirection"] == "clientToServer" {
                return Err(format!("{} is sent by clients", name));
            }

            if let Some(params) = notification.get("params") {
                self.value(params, &message["params"], "params")?;
            }

            return gates(name, &message["params"], capabilities);
        }

        let method = method.ok_or("a response to no request")?;

        match (object.get("result"), object.get("error")) {
            (Some(result), None) => {
                let request = self
                    .requests
                    .get(method)
                    .ok_or_else(|| format!("{} isn't a request in the spec", method))?;

                self.value(&request["result"], result, "result")?;
                gates(method, result, capabilities)
            }
            (None, Some(error)) => {
                // The base protocol's ResponseError, which the meta model
                // doesn't have.
                let code = &error["code"];

                if !code
                    .as_i64()
                    .is_some_and(|n| (-(1 << 31)..(1 << 31)).contains(&n))
                {
                    return Err(format!("error.code: {} isn't an integer", code));
                }

                if !error["message"].is_string() {
                    return Err("error.message isn't a string".to_string());
                }

                if let Some(key) = error
                    .as_object()
                    .unwrap()
                    .keys()
                    .find(|k| !["code", "message", "data"].contains(&k.as_str()))
                {
                    return Err(format!("error.{} isn't a property of ResponseError", key));
                }

                // A request's own error data, such as initialize's.
                match (
                    self.requests.get(method).and_then(|r| r.get("errorData")),
                    error.get("data"),
                ) {
                    (Some(typ), Some(data)) => self.value(typ, data, "error.data"),
                    _ => Ok(()),
                }
            }
            _ => Err("a response needs exactly one of result and error".to_string()),
        }
    }

    /// Checks `value` against a meta model type.
    fn value(&self, typ: &Value, value: &Value, path: &str) -> Result<(), String> {
        let mismatch = || {
            format!(
                "{}: {} isn't a {}",
                path,
                value,
                serde_json::to_string(typ).unwrap()
            )
        };

        match typ["kind"].as_str().unwrap() {
            "base" => {
                let ok = match typ["name"].as_str().unwrap() {
                    "string" | "URI" | "DocumentUri" | "RegExp" => value.is_string(),
                    "integer" => value
                        .as_i64()
                        .is_some_and(|n| (-(1 << 31)..(1 << 31)).contains(&n)),
                    "uinteger" => value.as_u64().is_some_and(|n| n < (1 << 31)),
                    "decimal" => value.is_number(),
                    "boolean" => value.is_boolean(),
                    "null" => value.is_null(),
                    other => panic!("an unknown base type {}", other),
                };

                if ok { Ok(()) } else { Err(mismatch()) }
            }
            "reference" => self.reference(typ["name"].as_str().unwrap(), value, path),
            "array" => {
                let items = value.as_array().ok_or_else(mismatch)?;

                for (i, item) in items.iter().enumerate() {
                    self.value(&typ["element"], item, &format!("{}[{}]", path, i))?;
                }

                Ok(())
            }
            "map" => {
                let entries = value.as_object().ok_or_else(mismatch)?;

                for (key, item) in entries {
                    self.value(&typ["value"], item, &format!("{}.{}", path, key))?;
                }

                Ok(())
            }
            "tuple" => {
                let items = value.as_array().ok_or_else(mismatch)?;
                let types = typ["items"].as_array().unwrap();

                if items.len() != types.len() {
                    return Err(mismatch());
                }

                for (i, (t, item)) in types.iter().zip(items).enumerate() {
                    self.value(t, item, &format!("{}[{}]", path, i))?;
                }

                Ok(())
            }
            "or" => {
                let mut errors = Vec::new();

                for t in typ["items"].as_array().unwrap() {
                    match self.value(t, value, path) {
                        Ok(()) => return Ok(()),
                        Err(e) => errors.push(e),
                    }
                }

                Err(format!("{}: matches none of: {}", path, errors.join("; ")))
            }
            "literal" => {
                self.properties(typ["value"]["properties"].as_array().unwrap(), value, path)
            }
            "stringLiteral" => {
                if *value == typ["value"] {
                    Ok(())
                } else {
                    Err(mismatch())
                }
            }
            other => panic!("an unknown type kind {}", other),
        }
    }

    fn reference(&self, name: &str, value: &Value, path: &str) -> Result<(), String> {
        // Anything at all.
        if matches!(name, "LSPAny" | "LSPObject" | "LSPArray") {
            return match name {
                "LSPObject" if !value.is_object() => Err(format!("{}: not an object", path)),
                "LSPArray" if !value.is_array() => Err(format!("{}: not an array", path)),
                _ => Ok(()),
            };
        }

        if let Some(alias) = self.aliases.get(name) {
            return self.value(&alias["type"], value, path);
        }

        if let Some(e) = self.enumerations.get(name) {
            self.value(&e["type"], value, path)?;

            let known = e["values"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["value"] == *value);

            return if known || e["supportsCustomValues"] == true {
                Ok(())
            } else {
                Err(format!("{}: {} isn't a {}", path, value, name))
            };
        }

        let s = self
            .structures
            .get(name)
            .unwrap_or_else(|| panic!("no type {} in the meta model", name));

        let mut properties = self.all_properties(s);

        for (structure, extra) in EXTRA_PROPERTIES {
            if *structure == name {
                properties.extend(extra.iter().map(|p| {
                    serde_json::json!({ "name": p, "type": { "kind": "reference", "name": "LSPAny" }, "optional": true })
                }));
            }
        }

        self.properties(&properties, value, &format!("{} ({})", path, name))
    }

    /// A structure's properties, with those of the structures it extends
    /// and mixes in.
    fn all_properties(&self, s: &Value) -> Vec<Value> {
        let mut out: Vec<Value> = s["properties"].as_array().unwrap().clone();

        for key in ["extends", "mixins"] {
            for parent in s[key].as_array().into_iter().flatten() {
                let name = parent["name"].as_str().unwrap();

                out.extend(self.all_properties(&self.structures[name]));
            }
        }

        out
    }

    /// Checks an object's properties: every required one present, none
    /// the type doesn't have, and each of the right type.
    fn properties(&self, properties: &[Value], value: &Value, path: &str) -> Result<(), String> {
        let object: &Map<String, Value> = value
            .as_object()
            .ok_or_else(|| format!("{}: {} isn't an object", path, value))?;

        for p in properties {
            let name = p["name"].as_str().unwrap();

            match object.get(name) {
                Some(v) => self.value(&p["type"], v, &format!("{}.{}", path, name))?,
                None if p["optional"] == true => {}
                None => return Err(format!("{}: {} is required", path, name)),
            }
        }

        for key in object.keys() {
            if !properties.iter().any(|p| p["name"] == *key) {
                return Err(format!("{}: {} isn't a property of it", path, key));
            }
        }

        Ok(())
    }
}

/// Whether the client's capabilities have `true` at a path such as
/// `textDocument.completion.completionItem.labelDetailsSupport`.
fn supports(capabilities: &Value, path: &str) -> bool {
    capability(capabilities, path) == &Value::Bool(true)
}

fn capability<'a>(capabilities: &'a Value, path: &str) -> &'a Value {
    path.split('.').fold(capabilities, |v, key| &v[key])
}

/// Checks that a result or notification uses only what the client says
/// it supports, where the spec makes something depend on a capability.
fn gates(method: &str, value: &Value, capabilities: &Value) -> Result<(), String> {
    match method {
        "initialize" => {
            let chosen = &value["capabilities"]["positionEncoding"];
            let offered = capability(capabilities, "general.positionEncodings");

            if !chosen.is_null()
                && chosen != "utf-16"
                && !offered.as_array().is_some_and(|o| o.contains(chosen))
            {
                return Err(format!("positionEncoding {} wasn't offered", chosen));
            }
        }
        "textDocument/hover" => {
            let kind = &value["contents"]["kind"];
            let formats = capability(capabilities, "textDocument.hover.contentFormat");
            let supported = formats.as_array().is_some_and(|f| f.contains(kind));

            // A client that names no formats gets plain text.
            if !kind.is_null() && kind != "plaintext" && !supported {
                return Err(format!(
                    "a hover in {}, which the client doesn't list",
                    kind
                ));
            }
        }
        "textDocument/completion" => {
            let item = "textDocument.completion.completionItem";
            let kinds = capability(
                capabilities,
                "textDocument.completion.completionItemKind.valueSet",
            );

            for i in value["items"].as_array().into_iter().flatten() {
                if i.get("labelDetails").is_some()
                    && !supports(capabilities, &format!("{}.labelDetailsSupport", item))
                {
                    return Err("labelDetails without labelDetailsSupport".to_string());
                }

                // Without a value set, only Text (1) to Reference (18).
                if let Some(kind) = i.get("kind")
                    && !kinds.as_array().is_some_and(|k| k.contains(kind))
                    && !kind.as_u64().is_some_and(|k| (1..=18).contains(&k))
                {
                    return Err(format!("completion item kind {} isn't supported", kind));
                }
            }
        }
        "textDocument/signatureHelp" => {
            let info = "textDocument.signatureHelp.signatureInformation";
            let no_active = supports(
                capabilities,
                "textDocument.signatureHelp.noActiveParameterSupport",
            );

            if value.get("activeParameter") == Some(&Value::Null) && !no_active {
                return Err("activeParameter null without noActiveParameterSupport".to_string());
            }

            for s in value["signatures"].as_array().into_iter().flatten() {
                if let Some(active) = s.get("activeParameter") {
                    if !supports(capabilities, &format!("{}.activeParameterSupport", info)) {
                        return Err(
                            "SignatureInformation.activeParameter without activeParameterSupport"
                                .to_string(),
                        );
                    }

                    if active.is_null() && !no_active {
                        return Err(
                            "activeParameter null without noActiveParameterSupport".to_string()
                        );
                    }
                }

                for p in s["parameters"].as_array().into_iter().flatten() {
                    if p["label"].is_array()
                        && !supports(
                            capabilities,
                            &format!("{}.parameterInformation.labelOffsetSupport", info),
                        )
                    {
                        return Err("label offsets without labelOffsetSupport".to_string());
                    }

                    if let Some(label) = p["label"].as_str()
                        && !s["label"].as_str().unwrap_or("").contains(label)
                    {
                        return Err(format!("parameter {:?} isn't in the signature", label));
                    }
                }
            }
        }
        "textDocument/publishDiagnostics" => {
            let tags = capability(
                capabilities,
                "textDocument.publishDiagnostics.tagSupport.valueSet",
            );

            for d in value["diagnostics"].as_array().into_iter().flatten() {
                for tag in d["tags"].as_array().into_iter().flatten() {
                    if !tags.as_array().is_some_and(|t| t.contains(tag)) {
                        return Err(format!("diagnostic tag {} isn't supported", tag));
                    }
                }

                if !d["message"].is_string() {
                    return Err("a diagnostic message that isn't a string".to_string());
                }
            }

            if value.get("version").is_some()
                && !supports(
                    capabilities,
                    "textDocument.publishDiagnostics.versionSupport",
                )
            {
                return Err("version without versionSupport".to_string());
            }
        }
        _ => {}
    }

    Ok(())
}

/// The capabilities of a client that supports everything Cuttlebone uses
/// when a client says it can: what most tests send, merged under their
/// own. Tests of what a client without them gets send their own instead.
pub fn capable_client() -> Value {
    serde_json::json!({
        "textDocument": {
            "hover": { "contentFormat": ["markdown", "plaintext"] },
            "completion": {
                "completionItem": { "labelDetailsSupport": true },
                "completionItemKind": { "valueSet": (1..=25).collect::<Vec<u32>>() },
            },
            "signatureHelp": {
                "signatureInformation": {
                    "parameterInformation": { "labelOffsetSupport": true },
                    "activeParameterSupport": true,
                },
                "noActiveParameterSupport": true,
            },
            "publishDiagnostics": { "tagSupport": { "valueSet": [1, 2] } },
        },
    })
}
