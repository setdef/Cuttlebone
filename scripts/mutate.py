#!/usr/bin/env python3
"""Breaks behaviours on purpose, one at a time, and checks that a test fails.

Each mutation replaces one exact snippet of source (it must occur exactly
once), runs the tests that should catch it, and restores the file. A
mutation reported as NOT CAUGHT means a behaviour has no test; SKIP means
its snippet changed, so update the snippet. It exits with status 1 unless
every mutation it ran is caught.

The last field names what should catch the mutation: `--test FILE [FILTER]`
for the tests in tests/FILE.rs, `--lib FILTER` for unit tests in src/, or
`sh:COMMAND` for a command that must fail (for rules that clippy or a check
script enforces).

Usage: scripts/mutate.py [--since REV] [--jobs N | --serial] [--list] [WORD...]

  (no options)   every mutation: the exhaustive tier (AGENTS.md)
  --since REV    only the mutations a change since REV could affect: the
                 milestone tier. See `selected`.
  --list         print what would run, and why, without running anything
  --jobs N       how many mutations' tests run at once (default: half the
                 cores, at most 5)
  --trees N      how many mutations are built at once
  --serial       build and test each mutation in place, one at a time, as
                 the runner did before milestone 6
  WORD...        only the mutations whose names contain one of the words

Mutations are built in several copies of the tree at once (`--trees N`,
default a third of the cores, at most 3), in target/mutate, each with a
copy-on-write clone of the tree's build, so only Cuttlebone is built again.
A mutation's tests then run from copies of its build (the test binary and
`cuttlebone`), each with a directory of its own for the files tests make
(`CUTTLEBONE_TEST_TMPDIR`), while the copy builds the next. Tests that read
the source tree with git (tests/guardrails.rs, tests/citations.rs), and
`sh:` commands, run in the tree itself, and the
mutations of .upstream (which every copy links to) and tests/latency.rs
run alone, after the rest. Every mutation replays the whole edit-replay
corpus (`CUTTLEBONE_CORPUS_FULL`).
"""
import json, os, shutil, subprocess, sys, threading, time
from concurrent.futures import ThreadPoolExecutor
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
M = [
 ("patch: prepare() back in check_with", ".upstream/inko/compiler/src/compiler.rs",
  "    ) -> Result<(), CompileError> {\n        let start = Instant::now();\n\n        // When checking a project",
  "    ) -> Result<(), CompileError> {\n        self.prepare()?;\n        let start = Instant::now();\n\n        // When checking a project",
  "--test lsp queries_never_sync"),
 ("no required-version check", "src/check.rs", "    check_required_version(target)?;\n\n    let mut compiler = Compiler::new(target.config()?);\n    let start", "    let mut compiler = Compiler::new(target.config()?);\n    let start", "--test manifest"),
 ("diagnostics skip the required-version check", "src/check.rs", "    check_required_version(target)?;\n\n    let mut compiler = Compiler::new(target.config()?);\n    // The compiler", "    let mut compiler = Compiler::new(target.config()?);\n    // The compiler", "--test manifest diagnostics_refuse"),
 ("dependency manifests ignored", "src/check.rs", "paths.push(version.join(MANIFEST_FILE));", "let _ = version;", "--test manifest"),
 ("always UTF-16", "src/lsp.rs", '"utf-8" => Some(Encoding::Utf8),', '"utf-8" => Some(Encoding::Utf16),', "--test lsp utf8"),
 ("UTF-8 counts chars", "src/position.rs", "Encoding::Utf8 => text.len() as u32,", "Encoding::Utf8 => text.chars().count() as u32,", "--test lsp non_ascii"),
 ("incremental edits replace everything", "src/lsp.rs", "    let Some(r) = change.range else {", "    let Some(r) = None::<lsp::Range> else {", "--test lsp several_changes"),
 ("edit ranges in UTF-16 regardless", "src/lsp.rs", "            encoding,\n        )\n        .unwrap_or(text.len())", "            Encoding::Utf16,\n        )\n        .unwrap_or(text.len())", "--test lsp non_ascii_positions_and_ranges_in_utf8"),
 ("no timeout", "src/lsp.rs", "        let query = match &self.running {\n            Some((_, check)) => Some(check.started + self.options.timeout),", "        let query = match &self.running {\n            Some((_, _check)) => None,", "--test lsp a_hung_check"),
 ("timeout doesn't kill", "src/lsp.rs", "        check.kill();\n        log(&format!(\n            \"check for {} timed out", "        std::mem::forget(check);\n        log(&format!(\n            \"check for {} timed out", "--test lsp a_hung_check"),
 ("cancel ignored", "src/lsp.rs", "self.cancel(connection, &id);", "let _ = id;", "--test lsp cancelling"),
 # Requests are answered for the text they were about (LSP 3.18).
 ("edits don't freeze queued queries", "src/lsp.rs", "        ) {\n            self.freeze();\n        }\n", "        ) {\n        }\n", "--test lsp a_request_is_answered"),
 ("freezing forgets the fallback", "src/lsp.rs", "                f.fallback.as_deref().cloned(),", "                None,", "--test stale a_query_frozen"),
 ("edits kill the running check", "src/lsp.rs", "    fn freeze(&mut self) {\n", "    fn freeze(&mut self) {\n        if let Some((_, check)) = self.running.take() {\n            check.kill();\n        }\n", "--test lsp an_edit_does_not"),
 ("frozen checks are good checks", "src/lsp.rs", "            if let Ok(job) = &job\n                && pending.frozen.is_none()\n", "            if let Ok(job) = &job\n", "--test stale a_check_of"),
 ("cancelRequest cancels everything", "src/lsp.rs", "        if let Some(i) = self.queue.iter().position(|p| p.id == *id) {", "        let _ = id;\n        if let Some(i) = (!self.queue.is_empty()).then_some(0) {", "--test lsp cancelling"),
 ("shutdown answers with errors", "src/lsp.rs", "        for pending in std::mem::take(&mut self.queue) {\n            self.answer(connection, pending, Outcome::Failed(reason.clone()));\n        }", "        for pending in std::mem::take(&mut self.queue) {\n            reply_error(connection, pending.id, ErrorCode::RequestCanceled, \"cancelled\");\n        }", "--test lsp shutdown_kills"),
 ("no debounce", "src/lsp.rs", "(at + self.options.debounce).max(Instant::now())", "at.max(Instant::now())", "--test lsp checks_wait"),
 ("status always sent", "src/lsp.rs", "        if self.options.status {", "        if true {", "--test lsp without_opting_in"),
 ("always markdown", "src/lsp.rs", "        let contents = if self.support.markdown {", "        let contents = if true {", "--test lsp plain_text"),
 ("checks in the server's thread", "src/lsp.rs", "            match job.and_then(|job| self.send(self.checks, &job)) {\n                Ok(check) => self.running = Some((pending, check)),\n                Err(reason) => self.answer(connection, pending, Outcome::Failed(reason)),\n            }",
  "            match job {\n                Ok(job) => self.answer(connection, pending, job.run(&crate::query::Cache::new(0))),\n                Err(reason) => self.answer(connection, pending, Outcome::Failed(reason)),\n            }", "--test lsp aborting"),
 ("--version without the version", "src/bin/cuttlebone.rs", 'println!("cuttlebone {} (Inko {})", VERSION, upstream_version());', 'println!("cuttlebone 0.1.0 (Inko {})", upstream_version());', "--test lsp version_prints"),
 ("serverInfo without the version", "src/lsp.rs", "                version: Some(VERSION.to_string()),", "                version: Some(\"0.1.0\".to_string()),", "--test lsp initialize_advertises"),
 # Only what the client's capabilities say it supports (LSP 3.18).
 ("hover markdown for every client", "src/lsp.rs", '                .is_some_and(|f| f == "markdown"),', '                .is_none_or(|f| f == "markdown"),', "--test conformance hover_and_completion"),
 ("hover format ignores the client's order", "src/lsp.rs", '.find(|f| *f == "markdown" || *f == "plaintext")', '.find(|f| *f == "markdown")', "--test lsp a_client_without_markdown"),
 ("labelDetails for every client", "src/lsp.rs", "            label_details: support\n                .label_details\n                .then_some(", "            label_details: true\n                .then_some(", "--test conformance hover_and_completion"),
 ("Constant kind for every client", "src/lsp.rs", "MemberKind::Constant if support.constant_kind =>", "MemberKind::Constant if true =>", "--test conformance hover_and_completion"),
 ("Constant kind for no client", "src/lsp.rs", "MemberKind::Constant if support.constant_kind =>", "MemberKind::Constant if false =>", "--test conformance hover_and_completion"),
 ("label offsets for every client", "src/lsp.rs", "            if support.label_offsets {", "            if true {", "--test conformance signature_help_without"),
 ("parameter text off by one", "src/lsp.rs", "        if units >= start && units < end {", "        if units > start && units <= end {", "--test conformance signature_help_without"),
 ("signature activeParameter for every client", "src/lsp.rs", "        if support.signature_active_parameter {", "        if true {", "--test conformance signature_help_without"),
 ("null activeParameter for every client", "src/lsp.rs", "        None if support.no_active_parameter => Some(Value::Null),", "        None => Some(Value::Null),", "--test conformance signature_help_without"),
 ("no null activeParameter", "src/lsp.rs", "        None if support.no_active_parameter => Some(Value::Null),", "", "--test signature past_the_last"),
 ("diagnostic tags for every client", "src/lsp.rs", "tags: (unnecessary && self.support.unnecessary_tag)", "tags: (unnecessary || self.support.unnecessary_tag)", "--test conformance diagnostics_without"),
 ("the conformance check off", "tests/common/client.rs", "        if let Err(e) = spec().check(&message, method, &self.capabilities) {", "        if let Err(e) = Ok::<(), String>(()) {", "--test conformance the_client_fails"),
 ("version mismatch accepted", "src/lsp.rs", "    if installed != supported {", "    if installed == \"never\" {", "--test lsp a_different_inko"),
 ("no placeholder", "src/query.rs", "        copy.insert_str(offset, PLACEHOLDER);\n", "        let _ = offset;\n", "--test complete"),
 ("didSave text ignored", "src/lsp.rs", "                    doc.text = text;\n", "                    let _ = &text;\n", "--test lsp saving"),
 ("didClose ignored", "src/lsp.rs", "                    && let Some(doc) = self.documents.remove(&p.text_document.uri)", "                    && let Some(doc) = None::<Document>", "--test lsp a_session"),
 ("workspace folders ignored", "src/lsp.rs", "        (Some(folders), _, _) if !folders.is_empty() => {", "        (Some(folders), _, _) if folders.is_empty() => {", "--test lsp workspace_folders"),
 ("other open documents not sent", "src/lsp.rs", "                contents: self.contents(),\n            },\n            deadline: Some(self.options.timeout + DEADLINE_MARGIN),\n        })", "                contents: self.contents().into_iter().filter(|(p, _)| Some(p) == pending.uri.to_file_path().ok().as_ref()).collect(),\n            },\n            deadline: Some(self.options.timeout + DEADLINE_MARGIN),\n        })", "--test lsp unsaved_changes_to_an_imported"),
 ("hover has no range", "src/lsp.rs", "            range: Some(range(t.range)),", "            range: None,", "--test lsp a_session"),
 ("exit status ignores shutdown", "src/lsp.rs", 'Message::Notification(n) if n.method == "exit" => return Some(self.shut_down),', 'Message::Notification(n) if n.method == "exit" => return Some(true),', "--test lsp exit_without"),
 ("requests after shutdown answered", "src/lsp.rs", "        if self.shut_down {\n            return reply_error(", "        if false {\n            return reply_error(", "--test lsp shutdown_then_exit"),
 ("crash not reported", "src/worker.rs", "Ok(outcome) if status.as_ref().is_ok_and(ExitStatus::success) => (outcome, None),", "Ok(outcome) => (outcome, None),\n            _ if true => (Outcome::Failed(Unavailable::new(\"x\", \"\")), None),", "--test lsp aborting"),
 ("shutdown doesn't cancel", "src/lsp.rs", "                self.shut_down = true;\n                self.cancel_all(connection);", "                self.shut_down = true;", "--test lsp shutdown_kills"),
 ("completion kinds swapped", "src/lsp.rs", "MemberKind::Field => lsp::CompletionItemKind::FIELD,", "MemberKind::Field => lsp::CompletionItemKind::PROPERTY,", "--test lsp a_session"),
 ("no capability for completion trigger", "src/lsp.rs", 'trigger_characters: Some(vec![".".to_string()]),', "trigger_characters: None,", "--test lsp initialize_advertises"),
 ("full sync advertised", "src/lsp.rs", "change: Some(lsp::TextDocumentSyncKind::INCREMENTAL),", "change: Some(lsp::TextDocumentSyncKind::FULL),", "--test lsp initialize_advertises"),
 ("no worker deadline", "src/worker.rs", "Err(RecvTimeoutError::Timeout) => std::process::exit(DEADLINE_STATUS),", "Err(RecvTimeoutError::Timeout) => return,", "--test lsp a_hung_worker"),
 ("byte_offset ignores CR", "src/position.rs", "        .find(['\\n', '\\r'])", "        .find(['\\n'])", "--test complete byte_offsets"),
 ("a lone CR doesn't end a line", "src/position.rs", "        b'\\r' if text.as_bytes().get(end + 1) == Some(&b'\\n') => Some(end + 2),\n        _ => Some(end + 1),", "        b'\\r' if text.as_bytes().get(end + 1) == Some(&b'\\n') => Some(end + 2),\n        b'\\r' => Some(line_end(text, end + 1) + 1),\n        _ => Some(end + 1),", "--test lsp a_lone_carriage"),
 ("a CR LF is two line ends", "src/position.rs", "        b'\\r' if text.as_bytes().get(end + 1) == Some(&b'\\n') => Some(end + 2),\n", "", "--lib lone_carriage_returns"),
 ("a position between CR and LF", "src/position.rs", "    if offset > 0 && bytes[offset - 1] == b'\\r' && bytes.get(offset) == Some(&b'\\n') {\n        offset -= 1;\n    }\n", "", "--lib lone_carriage_returns"),
 ("compiler lines end at a lone CR", "src/position.rs", "                .chain(text.match_indices('\\n').map(|(i, _)| i + 1))", "                .chain(text.match_indices(['\\n', '\\r']).map(|(i, _)| i + 1))", "--test lsp a_lone_carriage"),
 # Definition, type definition and references (milestone 3).
 ("constructor calls aren't constructors", "src/walker.rs", "if id.kind(db) == MethodKind::Constructor", "if id.kind(db) == MethodKind::Constructor && false", "--test definition constructor"),
 ("inherited methods not followed to the trait", "src/walker.rs", "MethodSource::Inherited(_, original) => {\n                        SymbolId::Method(original).canonical(db)\n                    }", "MethodSource::Inherited(_, _original) => self,", "--test definition inherited"),
 ("bounded type parameters not followed to the original", "src/walker.rs", "Some(original) if original != id =>", "Some(original) if original != id && false =>", "--test corpus_definition"),
 ("Pointer[T] is T", "src/walker.rs", "    if let TypeRef::Pointer(_) = typ {\n        return None;\n    }\n", "", "--test corpus_definition"),
 ("a for loop's pattern is a reference", "src/walker.rs", "                if !synthetic {\n                    self.refer(n.name.location, n.constructor_id.map(SymbolId::Constructor));", "                if true {\n                    self.refer(n.name.location, n.constructor_id.map(SymbolId::Constructor));", "--test corpus_definition"),
 ("a negation's == is a reference", "src/walker.rs", "if !synthetic && !negation {", "if !synthetic {", "--test corpus_definition"),
 ("named arguments not recorded", "src/walker.rs", "                                self.refer(a.name.location, symbol);\n", "                                let _ = symbol;\n", "--test definition named_argument"),
 ("import paths not recorded", "src/walker.rs", "            self.refer(segment.location, module.map(SymbolId::Module));\n", "", "--test definition a_module_in_an_import"),
 ("imported symbols not recorded", "src/walker.rs", "            self.refer(symbol.name.location, target);\n", "", "--test definition an_imported_symbol"),
 ("field reads not recorded", "src/walker.rs", "self.refer(n.location, n.info.as_ref().map(|i| SymbolId::Field(i.id)));", "let _ = n.info.as_ref().map(|i| SymbolId::Field(i.id));", "--test definition a_field"),
 ("type names not recorded", "src/walker.rs", "        self.refer(node.name.location, symbol);\n", "        let _ = symbol;\n", "--test definition type_names"),
 ("static receivers not recorded", "src/walker.rs", "ConstantKind::Unknown => self.static_symbol(node),", "ConstantKind::Unknown => None,", "--test definition a_static_call"),
 ("pattern variables not declared", "src/walker.rs", "                    self.declare(n.name.location, SymbolId::Variable(id));\n", "", "--test definition variables"),
 ("closure arguments not declared", "src/walker.rs", "                        self.declare(arg.name.location, SymbolId::Variable(a.variable));\n", "", "--test definition variables"),
 ("method arguments not declared", "src/walker.rs", "            add(arg.name.location, Some(SymbolId::Variable(var.variable)));\n", "", "--test definition variables"),
 ("modules not declared", "src/query.rs", "            index.declared.insert(\n                SymbolId::Module(module.module_id),", "            index.declared.insert(\n                SymbolId::Module(ModuleId(u32::MAX)),", "--test definition a_module_in_an_import"),
 ("qualified names cover the qualifier", "src/walker.rs", "    if location.line_start != location.line_end || width == 0 || width > location.column_end {", "    if true {", "--test corpus_definition"),
 ("type definition keeps the instance", "src/walker.rs", "TypeEnum::TypeInstance(ins) => SymbolId::Type(ins.instance_of()),", "TypeEnum::TypeInstance(_) => return None,", "--test definition type_definition_strips"),
 ("type definition ignores type names", "src/query.rs", "symbol: SymbolId::Type(_) | SymbolId::Trait(_) | SymbolId::TypeParameter(_),", "symbol: SymbolId::Trait(_),", "--test definition type_definition_strips"),
 ("includeDeclaration ignored", "src/query.rs", ".filter(|_| include_declaration);", ".filter(|_| true);", "--test definition references_to_a_variable"),
 ("references only in what the file imports", "src/query.rs", "            target.file = None;\n", "", "--test definition references_search_modules"),
 ("references in std and dependencies", "src/query.rs", "} else if scope.in_project(&file)\n", "} else if !file.as_os_str().is_empty()\n", "--test definition references_to_a_std_symbol"),
 ("other files' ranges use the request's lines", "src/query.rs", "range: range(&Lines::with_encoding(text, self.encoding), &location),", "range: range(self.lines, &location),", "--test navigation references_honour"),
 ("no capability for definition", "src/lsp.rs", "definition_provider: Some(lsp::OneOf::Left(true)),", "definition_provider: None,", "--test lsp initialize_advertises"),
 ("includeDeclaration not read", "src/lsp.rs", '.pointer("/context/includeDeclaration")', '.pointer("/context/include_declaration")', "--test navigation references_honour"),
 ("no status for definitions", "src/lsp.rs", "            Outcome::Place(answer) => {\n                let status = status_of(&answer);", "            Outcome::Place(answer) => {\n                let status = (Ok(()), Duration::ZERO, Source::Fresh, false);", "--test navigation broken_code"),
 ("type definition answers definition", "src/worker.rs", "Query::TypeDefinition => Question::TypeDefinition,", "Query::TypeDefinition => Question::Definition,", "--test navigation type_definition"),
 # The guardrails (AGENTS.md).
 ("query uses lsp", "src/query.rs", "use crate::check::{Fingerprint, Target, diagnose, project_root, run};\n", "use crate::check::{Fingerprint, Target, diagnose, project_root, run};\nuse crate::lsp as _;\n", "--test guardrails layers"),
 ("position uses walker", "src/position.rs", "impl Encoding {\n", "use crate::walker as _;\nimpl Encoding {\n", "--test guardrails layers"),
 # The editor's name is split so that this file doesn't name one itself.
 ("an editor named in code", "src/lsp.rs", "//! The language server: standard LSP over stdio", "//! The language server, for " + "Ema" + "cs: standard LSP over stdio", "--test guardrails editor"),
 ("a binary named after Inko", "Cargo.toml", 'name = "dump-types"', 'name = "inko-dump-types"', "--test guardrails named_after"),
 ("an unjustified patch line", "patches/upstream.patch", "+    pub overlay: IndexMap<PathBuf, Vec<u8>>,\n", "+    pub overlay: IndexMap<PathBuf, Vec<u8>>,\n+    pub unjustified: bool,\n", "--test guardrails patches_readme"),
 ("a path from one machine", "AGENTS.md", "Never modify any checkout of Inko's own repository.", "Never modify ~/src/inko.", "--test guardrails paths"),
 ("a version without a changelog entry", "CHANGELOG.md", "versions here were given afterwards.\n", "versions here were given afterwards.\n\n## 0.99.0\n\n[Report](docs/history/MILESTONE-99.md)\n", "--test guardrails the_version_has"),
 ("a changelog entry without its report", "CHANGELOG.md", "[Report](docs/history/MILESTONE-6.md)", "[Report](docs/history/MILESTONE-5.md)", "--test guardrails the_version_has"),
 ("docs show another version", "docs/lsp.md", "- `cuttlebone --version` prints `cuttlebone ", "- `cuttlebone --version` prints `cuttlebone 0.0.1` or `cuttlebone ", "--test guardrails docs_show"),
 ("printing to stdout in the library", "src/query.rs", "pub fn type_at(session: &Session, request: &Request) -> Answer<TypeAt> {\n", "pub fn type_at(session: &Session, request: &Request) -> Answer<TypeAt> {\n    println!();\n", "sh:cargo clippy --quiet --all-targets"),
 ("unsafe code", "src/position.rs", "impl Encoding {\n", "impl Encoding {\n    pub fn unsafe_code() {\n        unsafe {}\n    }\n", "sh:cargo clippy --quiet --all-targets"),
 ("fault injection in release builds", "Cargo.toml", "fault-injection = []\n", "default = [\"fault-injection\"]\nfault-injection = []\n", "sh:scripts/check-release.sh"),
 ("a patch that doesn't apply to the release", "patches/upstream.patch", "-mod hir;\n", "-mod hir_typo;\n", "sh:scripts/check-patch.sh"),
 # Diagnostics (milestone 4).
 ("diagnostics skip MIR", "src/check.rs", "            inspect(db, hir);\n            true", "            inspect(db, hir);\n            false", "--test diagnostics every_fixture"),
 ("warnings dropped", "src/check.rs", "        .map(|d| Reported {", "        .filter(|d| d.is_error())\n        .map(|d| Reported {", "--test diagnostics every_fixture"),
 ("diagnostics ignore unsaved text", "src/query.rs", "            overlay.insert(canonical(p)?, text.clone());", "            canonical(p)?;\n            let _ = text;", "--test diagnostics unsaved"),
 ("diagnostic ranges in UTF-16 regardless", "src/query.rs", "range: range(&Lines::with_encoding(text, request.encoding), &l),", "range: range(&Lines::with_encoding(text, Encoding::Utf16), &l),", "--test publish ranges_are"),
 ("a panic discards what was found", "src/check.rs", "        Err(_) => Ok(Diagnosis {\n            diagnostics,", "        Err(_) => Ok(Diagnosis {\n            diagnostics: Vec::new(),", "--test publish a_panicking"),
 ("std files are targets", "src/query.rs", "    if file.starts_with(session.std.canonicalize()", "    if false && file.starts_with(session.std.canonicalize()", "--test publish targets_are"),
 ("dependencies are targets", "src/query.rs", "    if file.ancestors().any(|a| {", "    if false && file.ancestors().any(|a| {", "--test publish targets_are"),
 ("lone files check their directory", "src/query.rs", "    Some(if in_sources(&root, &file) { root } else { file })", "    Some(root)", "--test publish targets_are"),
 ("workspace folders not checked at start", "src/lsp.rs", "            if root.join(\"src\").is_dir() || root.join(\"test\").is_dir() {", "            if false {", "--test publish every_file_with_diagnostics"),
 ("files without diagnostics not cleared", "src/lsp.rs", "        for uri in before.difference(&published) {", "        for uri in before.difference(&before) {", "--test publish diagnostics_follow"),
 ("closing doesn't recheck", "src/lsp.rs", "                    self.edited(connection, &path, unsaved, true);\n", "                    let _ = (path, unsaved);\n", "--test publish diagnostics_follow"),
 ("no unnecessary tags", "src/lsp.rs", "let unnecessary = d.id == \"unused-symbol\" || d.id == \"unreachable\";", "let unnecessary = false;", "--test publish every_file_with_diagnostics"),
 ("warnings published as errors", "src/lsp.rs", "                    lsp::DiagnosticSeverity::WARNING", "                    lsp::DiagnosticSeverity::ERROR", "--test publish every_file_with_diagnostics"),
 ("diagnostics not superseded", "src/lsp.rs", "        if changed\n            && let Some((target, check))", "        if false\n            && let Some((target, check))", "--test publish superseded"),
 ("no diagnostics debounce", "src/lsp.rs", "            Instant::now() + self.options.diagnostics_debounce", "            Instant::now()", "--test publish diagnostics_wait_for_their"),
 ("diagnostics don't wait for queries", "src/lsp.rs", "if self.diagnoser.running.is_some() || self.running.is_some() || !self.queue.is_empty() {", "if self.diagnoser.running.is_some() {", "--test publish diagnostics_wait_for_running"),
 ("no diagnostics timeout", "src/lsp.rs", ".take_if(|(_, c)| c.started.elapsed() >= self.options.timeout)\n        {", ".take_if(|(_, c)| c.started.elapsed() >= self.options.timeout && false)\n        {", "--test publish superseded"),
 ("diagnostics can't be turned off", "src/lsp.rs", "        if !self.options.diagnostics {\n            return;\n        }\n", "", "--test publish diagnostics_can_be_turned_off"),
 ("diagnostics in the server's thread", "src/lsp.rs", "        match Check::start(self.checks, &job, self.finished.0.clone()) {\n            Ok(check) => self.diagnoser.running = Some((target, check)),", "        match Ok::<_, Unavailable>(job.run(&crate::query::Cache::new(0))) {\n            Ok(Outcome::Diagnostics(answer)) => self.publish(connection, target, answer),\n            Ok(_) => {}", "--test latency queries_stay_fast"),
 # Document highlights.
 ("assignments to variables aren't writes", "src/walker.rs", "\"AssignVariable\",\n                    Some(n.resolved_type),\n                    synthetic,\n                );\n\n                if !generated(&n.variable.name) {\n                    self.refer(n.variable.location, n.variable_id.map(SymbolId::Variable));\n                    self.writes.push(n.variable.location);", "\"AssignVariable\",\n                    Some(n.resolved_type),\n                    synthetic,\n                );\n\n                if !generated(&n.variable.name) {\n                    self.refer(n.variable.location, n.variable_id.map(SymbolId::Variable));", "--test highlight a_variable"),
 ("bindings aren't writes", "src/query.rs", "                SymbolId::Variable(_) => Access::Write,", "                SymbolId::Variable(_) => Access::Read,", "--test highlight a_variable"),
 ("assignments to fields aren't writes", "src/walker.rs", "                self.writes.push(n.field.location);\n                self.expression(&n.value);\n            }\n            E::ReplaceField", "                self.expression(&n.value);\n            }\n            E::ReplaceField", "--test highlight a_field"),
 ("setter assignments aren't writes", "src/walker.rs", "                self.refer(n.name.location, call_symbol(&n.kind));\n                self.writes.push(n.name.location);", "                self.refer(n.name.location, call_symbol(&n.kind));", "--test highlight a_field"),
 ("highlights match by name", "src/query.rs", "        for r in walker.references.iter().filter(|r| r.symbol == symbol) {", "        for r in walker.references.iter().filter(|r| r.symbol.name(scope.db) == symbol.name(scope.db)) {", "--test highlight same_named"),
 ("highlight kinds swapped", "src/lsp.rs", "Access::Read => lsp::DocumentHighlightKind::READ,", "Access::Read => lsp::DocumentHighlightKind::TEXT,", "--test highlight a_variable"),
 ("no documentHighlightProvider", "src/lsp.rs", "                document_highlight_provider: Some(lsp::OneOf::Left(true)),\n", "", "--test lsp initialize_advertises"),
 # Signature help.
 ("no closing parenthesis inserted", "src/query.rs", "        copy.insert(offset, ')');", "        let _ = offset;", "--test signature unfinished"),
 ("named arguments taken as positional", "src/query.rs", "            name: Some(name), ..\n        }) => params.iter().position(|p| &p.name == name),", "            name: Some(_), ..\n        }) => Some(slot),", "--test signature named"),
 ("commas not counted", "src/query.rs", "                    .starts_with(',')", "                    .starts_with(';')", "--test signature positional"),
 ("the outermost call wins", "src/query.rs", "        .max_by_key(|l| paren(l))?;", "        .min_by_key(|l| paren(l))?;", "--test signature nested"),
 ("the call's type arguments ignored", "src/query.rs", "    let format = || TypeFormatter::new(db, None, Some(arguments));", "    let format = || { let _ = arguments; TypeFormatter::new(db, None, None) };", "--test signature type_arguments"),
 ("parameter offsets from the label's start", "src/query.rs", "        .map_or(0, |i| i + 1);", "        .map_or(0, |_| 0);", "--test signature positional"),
 ("after the ) counts as inside", "src/query.rs", "at <= (l.location.line_end, l.location.column_end))", "at <= (l.location.line_end, l.location.column_end + 1))", "--test signature null_outside"),
 ("the ( counts as inside", "src/query.rs", ".filter(|l| paren(l) < at && at", ".filter(|l| paren(l) <= at && at", "--test signature null_outside"),
 ("no signatureHelpProvider", "src/lsp.rs", "trigger_characters: Some(vec![\"(\".to_string(), \",\".to_string()]),", "trigger_characters: None,", "--test lsp initialize_advertises"),
 # Inlay hints.
 ("literals aren't obvious", "src/walker.rs", "            E::Int(_)\n            | E::Float(_)", "            E::Float(_)", "--test inlay let_types"),
 ("casts aren't obvious", "src/walker.rs", "            | E::Nil(_)\n            | E::TypeCast(_) => true,", "            | E::Nil(_) => true,", "--test inlay let_types"),
 ("constructors aren't obvious", "src/walker.rs", "                CallKind::TypeInstance(_) => true,", "                CallKind::TypeInstance(_) => false,", "--test inlay let_types"),
 ("every static method is obvious", "src/walker.rs", "receiver.is_some() && receiver == type_symbol(self.db, info.returns)", "receiver.is_some()", "--test inlay a_static_method"),
 ("no static method is obvious", "src/walker.rs", "receiver.is_some() && receiver == type_symbol(self.db, info.returns)", "receiver.is_none()", "--test inlay the_fixtures"),
 ("annotated lets get hints", "src/walker.rs", "                if let hir::Pattern::Identifier(p) = &n.pattern\n                    && p.value_type.is_none()\n                    && !synthetic", "                if let hir::Pattern::Identifier(p) = &n.pattern\n                    && !synthetic", "--test inlay let_types"),
 ("annotated closure arguments get hints", "src/walker.rs", "                    if arg.value_type.is_none() {\n                        self.untyped.push(", "                    if true {\n                        self.untyped.push(", "--test inlay closure_arguments"),
 ("unknown types get hints", "src/query.rs", "typ.contains(\"<error>\") || typ.contains('?')", "typ.contains('?')", "--test inlay unknown_types"),
 ("named arguments get hints", "src/query.rs", "                .take_while(|a| a.name.is_none())", "                .take_while(|_| true)", "--test inlay parameter_names"),
 ("variables named like the parameter get hints", "src/query.rs", "                if arg.variable.as_ref() == Some(&param.name) {\n                    continue;\n                }\n", "", "--test inlay parameter_names"),
 ("operators are argument lists", "src/walker.rs", "                if n.parens && !synthetic && !operator {", "                if n.parens && !synthetic {", "--test inlay parameter_names"),
 ("operators get signature help", "src/walker.rs", "                if n.parens && !synthetic && !operator {", "                if n.parens && !synthetic {", "--test signature null_outside"),
 ("enum constructors' generated names", "src/query.rs", "                .filter(|(m, _)| m.kind(db) != MethodKind::Constructor)", "                .filter(|_| true)", "--test inlay parameter_names"),
 ("hints past the range", "src/query.rs", "hints.retain(|h| *position <= h.position && h.position <= *end);", "hints.retain(|h| *position <= h.position);", "--test inlay only_the_requested_range"),
 ("the range's end not read", "src/lsp.rs", "                        line: json_u32(&request.params, \"/range/end/line\"),", "                        line: u32::MAX,", "--test inlay only_the_requested_range"),
 ("parameter hints without padding", "src/lsp.rs", "                                padding_right: Some(h.kind == HintKind::Parameter),", "                                padding_right: None,", "--test inlay only_the_requested_range"),
 ("no inlayHintProvider", "src/lsp.rs", "                inlay_hint_provider: Some(lsp::OneOf::Left(true)),\n", "", "--test lsp initialize_advertises"),
 # Answers in code being edited (milestone 5).
 ("an edit's tail starts at the old end", "src/edits.rs", "same.push((old + skip, start + len, n - skip));", "same.push((old + skip, end + len, n - skip));", "--lib edits"),
 ("text next to an edit counts as unchanged", "src/edits.rs", "        let from = range.start.checked_sub(1);", "        let from = Some(range.start).filter(|_| range.start > 0);", "--lib edits"),
 ("text added at the start of a file ignored", "src/edits.rs", "        if (from.is_none() && (old, new) != (0, 0))", "        if (from.is_none() && false)", "--lib edits"),
 ("runs aren't bounded", "src/edits.rs", "        while self.same.len() > MAX_RUNS {", "        while self.same.len() > usize::MAX {", "--lib edits"),
 ("byte offsets as positions in UTF-16", "src/position.rs", "        character: encoding.len(&before[start..]),", "        character: Encoding::Utf16.len(&before[start..]),", "--lib position"),
 ("the batch's answers out of order", "src/query.rs", "        for (i, answer) in group.iter().zip(pipeline(", "        for (i, answer) in group.iter().rev().zip(pipeline(", "--test replay"),
 # Stale answers.
 ("completion answers from older results", "src/query.rs", "                | Question::InlayHints { .. }\n        ),", "                | Question::InlayHints { .. }\n                | Question::Members\n        ),", "--test stale read_only"),
 ("references answer from older results", "src/query.rs", "                | Question::InlayHints { .. }\n        ),", "                | Question::InlayHints { .. }\n                | Question::References { .. }\n        ),", "--test stale read_only"),
 ("no answers from older results", "src/query.rs", "                matches!(&results[*i], Err(r) if recoverable(r))", "                matches!(&results[*i], Err(r) if recoverable(r) && false)", "--test stale"),
 ("positions not mapped back", "src/query.rs", "Some(position_at(&old, changes.old_offset(byte)?, encoding))", "Some(position_at(&old, byte, encoding))", "--test stale read_only"),
 ("ranges not mapped forward", "src/query.rs", "        let mapped = self.changes.new_range(start..end)?;", "        let mapped = start..end;", "--test stale read_only"),
 ("other files' ranges not mapped forward", "src/query.rs", "        let mapped = v.changes.new_range(start..end)?;", "        let mapped = start..end;", "--test stale definitions_into"),
 ("the position's unit not checked", "src/query.rs", "                    && !guard.safe(byte_offset(&old, old_position, encoding)?)", "                    && !guard.safe(byte_offset(&old, old_position, encoding)?) && false", "--test stale a_changed_declaration"),
 ("hints' units not checked", "src/query.rs", "                    if !self.safe(byte_offset(self.old, h.position, e)?) {", "                    if !self.safe(byte_offset(self.old, h.position, e)?) && false {", "--test stale hints_in_the_method"),
 ("units a change touched count as safe", "src/query.rs", "        let limit = if touching.is_empty() {", "        let limit = if true {", "--test stale read_only"),
 ("units mentioning a changed declaration count as safe", "src/query.rs", "            .any(|n| self.all.iter().any(|e| e.affects(n)))\n        {", "            .any(|n| self.all.iter().any(|e| e.affects(n) && false))\n        {", "--test stale a_changed_declaration"),
 ("units with errors count as safe", "src/query.rs", "        if types.iter().any(|(t, _)| t.is_none()) {", "        if types.iter().any(|(t, _)| t.is_none()) && false {", "--test replay"),
 ("highlights missing uses in changed text", "src/query.rs", "            let added = self\n                .effects", "            let added = false && self\n                .effects", "--test replay"),
 ("highlights missing uses in unsafe units", "src/query.rs", "            added || unsafe_use", "            added", "--test replay"),
 ("inlay hints for a range with nothing safe", "src/query.rs", "        if !covered {\n            return None;\n        }", "        if !covered && false {\n            return None;\n        }", "--test replay"),
 ("added names aren't prefixes", "src/units.rs", "self.names.contains(name) || self.prefixes.iter().any(|p| name.starts_with(p.as_str()))", "self.names.contains(name) || self.prefixes.contains(name)", "--lib units"),
 ("changes in a body change other methods", "src/units.rs", "        && body.start < old_range.start\n        && old_range.end < body.end\n    {", "        && body.start < old_range.start\n        && old_range.end < body.end\n        && false\n    {", "--test stale read_only"),
 ("a copy's check counts as the documents'", "src/query.rs", "            answer.typed &= !changed;", "            let _ = changed;", "--test stale a_copy"),
 # Repaired copies.
 ("repaired copies answer references", "src/query.rs", "        Source::Repaired => !matches!(question, Question::References { .. }),", "        Source::Repaired => true,", "--test repaired an_edit"),
 ("no repaired copies", "src/query.rs", "    if repair\n        && !wanted.is_empty()", "    if repair && false\n        && !wanted.is_empty()", "--test repaired"),
 ("changed lines aren't blanked alone", "src/query.rs", "            candidates.push(changed.clone());\n        } else {", "        } else {", "--test repaired the_changed_line"),
 ("unclosed brackets not looked for", "src/query.rs", "            if let Some(line) = unclosed(&blank(text, &with)) {", "            if let Some(line) = None::<usize>.filter(|_| !with.is_empty() || text.is_empty()) {", "--test repaired a_method_being_edited"),
 ("a bracket of another kind closes", "src/query.rs", "                        Some((o, _)) if o == expected => {}\n                        Some((_, line)) => return Some(line),", "                        Some((_, _)) => {}", "--test replay"),
 ("the parser's lines not blanked", "src/query.rs", "                    match parse_error_line(&blank(text, &lines)) {", "                    match Some(0).filter(|_| false).or(parse_error_line(\"\")) {", "--test replay"),
 ("no answers before the edit", "src/query.rs", "        } else if self.repaired && touching.iter().all(|e| e.body) {", "        } else if false {", "--test repaired a_method_being_edited"),
 ("answers before an edit in an older check", "src/query.rs", "        } else if self.repaired && touching.iter().all(|e| e.body) {", "        } else if touching.iter().all(|e| e.body) {", "--test replay"),
 ("unfinished types before the edit count", "src/query.rs", "        if limit < usize::MAX && types.iter().any(|(_, complete)| !complete) {", "        if limit < usize::MAX && types.iter().any(|(_, complete)| !complete) && false {", "--test repaired a_type_the_blank"),
 ("placeholders in type arguments count as inferred", "src/walker.rs", "            TypeEnum::TypeInstance(ins) => arguments(ins.type_arguments(db)),", "            TypeEnum::TypeInstance(_) => true,", "--test repaired a_type_the_blank"),
 ("completion's placeholder is an error", "src/query.rs", "            .filter(|(start, end, ..)| !placeholders.iter().any(|p| start <= p && p < end))", "            .filter(|_| true)", "--test repaired an_edit"),
 ("declaration errors aren't repaired", "src/query.rs", "            candidates.push(declarations);", "            let _ = declarations;", "--test repaired a_changed_declaration"),
 ("body changes blanked for declaration errors", "src/query.rs", "            if !effect(outline, old, &new, &c).body {", "            if !effect(outline, old, &new, &c).body || true {", "--test repaired a_changed_declaration"),
 ("blank lines are separate changes", "src/query.rs", "                Some((_, last)) if *last + 1 == *line => *last = *line,", "                Some((_, last)) if *last + 1 == *line && false => *last = *line,", "--test repaired a_changed_declaration"),
 ("whole items change what they name", "src/units.rs", "                Some(declared) => effect.names.extend(declared),", "                Some(_) => effect.prefixes.extend(names(text).map(String::from)),", "--lib units"),
 ("no repaired status", "src/lsp.rs", "                params[\"repaired\"] = json!(true);", "                let _ = json!(true);", "--test repaired an_edit"),
 ("the norepair fault ignored", "src/worker.rs", "        let repair = fault(self).as_deref() != Some(\"norepair\");", "        let repair = fault(self).is_some() || true;", "--test stale read_only"),
 # Stale answers over LSP.
 ("no stale status", "src/lsp.rs", "                    (Ok(()), Source::Stale) => \"stale\",", "                    (Ok(()), Source::Stale) => \"fresh\",", "--test stale read_only"),
 ("no fallback sent", "src/lsp.rs", "            fallback: self.snapshots.fallback(\n                &file,\n                query::diagnostics_target(&self.session, &file).as_deref(),\n            ),", "            fallback: None,", "--test stale read_only"),
 ("no fallback from the target's check", "src/lsp.rs", "                query::diagnostics_target(&self.session, &file).as_deref(),\n            ),", "                None,\n            ),", "--test stale unsaved_text"),
 ("diagnostics checks not kept for their target", "src/lsp.rs", "                    .chain([target.clone()])\n", "", "--test stale unsaved_text"),
 ("edits not recorded", "src/lsp.rs", "                                self.snapshots.edited(path, &before, start, end, len);", "                                let _ = (path, &before, start, end, len);", "--test stale read_only"),
 ("whole-text changes not recorded", "src/lsp.rs", "        let edit = crate::edits::difference(text, &change.text);", "        let edit = (0, 0, 0);", "--test stale a_diagnostics_check"),
 ("documents opened later not recorded", "src/lsp.rs", "                        self.snapshots\n                            .opened(path, disk.as_deref(), &p.text_document.text);", "                        let _ = (path, disk.as_deref(), &p.text_document.text);", "--test stale unsaved_text"),
 ("diagnostics checks aren't good checks", "src/lsp.rs", "            self.snapshots.finished(done.id, &checked);", "            self.snapshots.finished(done.id, &[]);", "--test stale a_diagnostics_check"),
 ("queries aren't good checks", "src/lsp.rs", "        self.snapshots.finished(done.id, file.as_slice());", "        self.snapshots.finished(done.id, &[]);", "--test stale read_only"),
 ("versions aren't bounded", "src/edits.rs", "        while self.versions.len() > MAX_VERSIONS {", "        while self.versions.len() > usize::MAX {", "--lib snapshots"),
 ("unused versions kept", "src/edits.rs", "        self.versions.retain(|(id, _)| used.contains(id));", "", "--lib snapshots"),
 ("running checks aren't bounded", "src/edits.rs", "        while self.running.len() >= MAX_RUNNING {", "        while self.running.len() >= usize::MAX {", "--lib snapshots"),
 # Kept checks (milestone 6).
 ("kept checks used whatever changed on disk", "src/query.rs", "            if entry.fingerprint.current() {", "            if true {", "--test cache a_file_changed_on_disk"),
 ("kept checks found by target but not text", "src/query.rs", "kept.iter().position(|k| k.target == *target)", "kept.iter().position(|k| k.target.root == target.root && k.target.file == target.file)", "--test cache an_edit"),
 ("fingerprints leave out directories", "src/check.rs", "            directories(&dir, &mut paths);", "            let _ = &dir;", "--test cache a_file_created"),
 ("fingerprints compare nothing", "src/check.rs", "self.stamps.iter().all(|(p, s)| Stamp::of(p) == *s)", "self.stamps.iter().all(|(p, _)| Stamp::of(p).is_some())", "--test cache a_file_changed_on_disk"),
 ("fingerprints leave out the change time", "src/check.rs", "(meta.ctime(), meta.ctime_nsec(), meta.ino(), meta.dev())", "(0, 0, 0, 0)", "--test cache a_check_that_read"),
 ("checks kept while what they read changed", "src/query.rs", "        if keep && self.capacity > 0 && fingerprint.settled() {", "        if keep && self.capacity > 0 {", "--test cache a_check_of_a_file_that_may"),
 ("settled ignores the start", "src/check.rs", "                .is_none_or(|latest| before(latest, self.started))", "                .is_none_or(|latest| before(latest, self.started + Duration::from_secs(3600)))", "--test cache a_check_that_read"),
 ("coarse clocks get no margin", "src/check.rs", "let margin = if fine { Duration::ZERO } else { COARSE };", "let margin = if fine || true { Duration::ZERO } else { COARSE };", "--lib coarse"),
 ("the cache keeps more than its capacity", "src/query.rs", "            if kept.len() > self.capacity {", "            if kept.len() > self.capacity + 1 {", "--test cache the_cache_keeps"),
 ("kept checks not moved to the end when used", "src/query.rs", "                kept.push(entry);\n                return (result, true);", "                kept.insert(0, entry);\n                return (result, true);", "--test cache the_cache_keeps"),
 ("completion's copies kept", "src/query.rs", "                    keep: !changed,", "                    keep: true,", "--test cache completions_copies"),
 ("repaired answers marked by the fresh check", "src/query.rs", "                sources[*i] = Source::Repaired;\n                cached[*i] = kept;", "                sources[*i] = Source::Repaired;", "--test cache repaired_and_stale"),
 ("stale answers marked by the fresh check", "src/query.rs", "                    sources[*i] = Source::Stale;\n                    cached[*i] = kept;", "                    sources[*i] = Source::Stale;", "--test cache repaired_and_stale"),
 ("no cached status", "src/lsp.rs", "            if cached {\n                params[\"cached\"]", "            if false {\n                params[\"cached\"]", "--test cache kept_answers_are_marked"),
 ("the queries' worker not kept", "src/lsp.rs", "        self.idle = worker;\n", "        drop(worker);\n", "--test cache kept_answers_are_marked"),
 ("workers keep no checks", "src/worker.rs", "    let cache = Cache::new(kept_checks());", "    let cache = Cache::new(0);", "--test cache kept_answers_are_marked"),
 ("the tests can't turn kept checks off", "src/worker.rs", "    if std::env::var_os(NO_CACHE_VARIABLE).is_some() {\n        return 0;", "    if std::env::var_os(NO_CACHE_VARIABLE).is_some() {\n        return query::KEPT_CHECKS;", "--test cache the_tests_can_turn"),
 ("shutdown leaves the waiting worker", "src/lsp.rs", "        if let Some(worker) = self.idle.take() {\n            worker.kill();\n        }\n", "", "--test cache shutdown_stops"),
 ("a dead waiting worker sent a job", "src/worker.rs", "        if !sent {\n            self.kill();", "        if false {\n            self.kill();", "--test cache a_waiting_worker_that_died"),
 ("the tests' switch for kept checks in release builds", "src/worker.rs", "    #[cfg(feature = \"fault-injection\")]\n    if std::env::var_os(NO_CACHE_VARIABLE).is_some() {", "    if std::env::var_os(\"CUTTLEBONE_NO_CACHE\").is_some() {", "sh:scripts/check-release.sh"),
 # The tiers (milestone 6).
 ("the whole corpus replays the sample", "tests/replay.rs", "            if full {\n                return Some(s);\n            }\n", "", "--test replay"),
 ("the milestone tier ignores changed snippets' files", "scripts/mutate.py", "        if path in changed:\n            why = \"changed \" + path", "        if False:\n            why = \"changed \" + path", "sh:python3 scripts/check-selection.py"),
 ("no worker deadline between jobs", "src/worker.rs", "        let _ = deadlines.send(job.deadline.map(|d| Instant::now() + d));", "        let _ = deadlines.send(None::<Instant>);", "--test lsp a_hung_worker"),
]


# The test files built into tests/main.rs's binary, `integration`. Every
# other file in tests/ is a binary of its own.
INTEGRATION = "integration"
OWN_BINARIES = {"latency"}
# Tests that read the source tree, so they must run before it's restored.
IN_PLACE = {"guardrails", "citations"}
TIMEOUT = 900


def target(test):
    """What runs a `--test FILE FILTER` or `--lib FILTER` target: the cargo
    arguments that build it, the module its tests are in (`None` for a
    binary of their own), and the filter."""
    words = test.split(" ", 2)
    if words[0] == "--lib":
        return ["--lib"], None, words[1]
    name = words[1]
    flt = words[2] if len(words) > 2 else ""
    if name in OWN_BINARIES:
        return ["--test", name], None, flt
    return ["--test", INTEGRATION], name, flt


def tests_in(exe, module, flt, root=ROOT):
    """The tests of a test binary that a target means: those in `module`
    (if any) whose names contain `flt`, as libtest's own filter would in a
    binary of their own."""
    r = subprocess.run([exe, "--list", "--format", "terse"], cwd=root, capture_output=True, text=True, timeout=TIMEOUT)
    names = [l[:-len(": test")] for l in r.stdout.splitlines() if l.endswith(": test")]
    if module:
        names = [n for n in names if n.startswith(module + "::")]
    return [n for n in names if flt in (n.split("::", 1)[1] if module else n)]


def test_file(test):
    """The file that holds a mutation's tests."""
    if test.startswith("sh:"):
        return None
    words = test.split(" ")
    return None if words[0] == "--lib" else "tests/%s.rs" % words[1]


def run_git(*args):
    return subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True, check=True).stdout


def changed_since(rev):
    """Every file that differs from REV: committed since, staged, or not,
    and new files git doesn't ignore."""
    files = set(run_git("diff", "--name-only", rev).split())
    files |= set(run_git("ls-files", "--others", "--exclude-standard").split())
    return files


def selected(mutations, changed, read=None):
    """The mutations a change to `changed` could affect, each with why:
    its snippet is in a changed file; its tests are; its command names one;
    or its snippet isn't found exactly once any more (it moved, and must be
    updated)."""
    read = read or (lambda path: open(os.path.join(ROOT, path)).read())
    out = []
    for m in mutations:
        name, path, old, new, test = m
        why = None
        if path in changed:
            why = "changed " + path
        elif test_file(test) in changed:
            why = "changed " + test_file(test)
        elif test.startswith("sh:") and any(w in changed for w in test[3:].split()):
            why = "changed what %s runs" % test[3:]
        else:
            try:
                if read(path).count(old) != 1:
                    why = "its snippet moved"
            except OSError:
                why = "its file is gone"
        if why:
            out.append((m, why))
    return out


def verdict_of(r, out):
    if "could not compile" in out or "error[" in out:
        return "BUILD ERROR"
    if r.returncode != 0 and "FAILED" in out:
        failed = [l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.endswith("FAILED")]
        return "caught by " + ", ".join(failed)
    if r.returncode == 0:
        return "NOT CAUGHT"
    return "other failure: " + out[-500:]


def run_sh(command):
    r = subprocess.run(command, shell=True, cwd=ROOT, capture_output=True, text=True, timeout=TIMEOUT)
    out = r.stdout + r.stderr
    reason = next((l.strip() for l in out.splitlines() if l.startswith("error") or l.startswith("the ") or l.startswith("patches/")), "")
    return "caught by %s: %s" % (command, reason) if r.returncode != 0 else "NOT CAUGHT"


def test_env(extra=None):
    env = dict(os.environ)
    env["CUTTLEBONE_CORPUS_FULL"] = "1"
    env.update(extra or {})
    return env


def build(cargo, root):
    """Builds a test target in a tree, and returns its executable, or None
    and the output if it doesn't build."""
    r = subprocess.run(["cargo", "test", "--no-run", "--message-format=json", *cargo],
                       cwd=root, capture_output=True, text=True, timeout=TIMEOUT)
    exe = None
    for line in r.stdout.splitlines():
        try:
            m = json.loads(line)
        except ValueError:
            continue
        if m.get("reason") == "compiler-artifact" and m.get("executable") and m["profile"]["test"]:
            exe = m["executable"]
    if r.returncode != 0 or not exe:
        text = "\n".join(json.loads(l).get("message", {}).get("rendered", "") or "" for l in r.stdout.splitlines() if l.startswith("{")) + r.stderr
        return None, text or "could not compile"
    return exe, ""


def run_tests(exe, names, work=None, cwd=ROOT):
    """Runs the named tests of a test binary: a mutation's copy, with the
    files tests make in `work`, or the tree's own build."""
    if not names:
        return "NO TESTS: the filter matches none"
    extra = {}
    if work:
        extra = {
            "CUTTLEBONE_TEST_BIN": os.path.join(work, "cuttlebone"),
            "CUTTLEBONE_TEST_TMPDIR": os.path.join(work, "tmp"),
        }
    try:
        r = subprocess.run([exe, "--exact", *names], cwd=cwd, capture_output=True, text=True,
                           timeout=TIMEOUT, env=test_env(extra))
        return verdict_of(r, r.stdout + r.stderr)
    except subprocess.TimeoutExpired:
        return "TIMED OUT after %ds" % TIMEOUT
    finally:
        if work:
            shutil.rmtree(work, ignore_errors=True)


def make_tree(path):
    """A copy of the tree at `path` to build mutations in: every file git
    tracks or doesn't ignore, `.upstream` as a link to the tree's own, and
    a clone of its build (copy-on-write where the file system can), so
    only Cuttlebone itself is built again."""
    shutil.rmtree(path, ignore_errors=True)
    files = run_git("ls-files", "--cached", "--others", "--exclude-standard").split("\n")
    for f in files:
        if not f or not os.path.isfile(os.path.join(ROOT, f)):
            continue
        os.makedirs(os.path.join(path, os.path.dirname(f)), exist_ok=True)
        shutil.copy2(os.path.join(ROOT, f), os.path.join(path, f))
    os.symlink(os.path.join(ROOT, ".upstream"), os.path.join(path, ".upstream"))
    os.makedirs(os.path.join(path, "target"))
    subprocess.run(["cp", "-c", "-R", os.path.join(ROOT, "target", "debug"), os.path.join(path, "target", "debug")],
                   check=False, capture_output=True)
    if not os.path.isdir(os.path.join(path, "target", "debug")):
        shutil.copytree(os.path.join(ROOT, "target", "debug"), os.path.join(path, "target", "debug"), symlinks=True)


def main(argv):
    cores = os.cpu_count() or 2
    since, jobs, trees, serial, listing, words = None, max(1, min(5, cores // 2)), max(1, min(3, cores // 3)), False, False, []
    args = list(argv)
    while args:
        a = args.pop(0)
        if a == "--since":
            since = args.pop(0)
        elif a == "--jobs":
            jobs = int(args.pop(0))
        elif a == "--trees":
            trees = int(args.pop(0))
        elif a == "--serial":
            serial = True
        elif a == "--list":
            listing = True
        else:
            words.append(a)
    chosen = [(m, "all") for m in M]
    if since:
        changed = changed_since(since)
        chosen = selected(M, changed)
        shared = sorted(f for f in changed if f in ("scripts/mutate.py", "scripts/check-selection.py", "tests/main.rs", "Cargo.toml") or f.startswith("tests/common/"))
        if shared:
            print("note: %s changed since %s, so the exhaustive tier must pass too (AGENTS.md)" % (", ".join(shared), since), flush=True)
    if words:
        chosen = [(m, why) for m, why in chosen if any(w in m[0] for w in words)]
    if listing:
        for m, why in chosen:
            print("%s  (%s)" % (m[0], why))
        print("%d of %d mutations" % (len(chosen), len(M)))
        return 0
    start = time.time()
    results = {}
    lock = threading.Lock()

    def record(name, verdict):
        with lock:
            results[name] = verdict
            print((name, verdict), flush=True)

    def apply(root, m):
        """Applies a mutation in a tree, and returns its file's text, to
        restore, or None if its snippet isn't there exactly once."""
        name, path, old, new, test = m
        p = os.path.join(root, path)
        src = open(p).read()
        if src.count(old) != 1:
            record(name, "SKIP: pattern count %d" % src.count(old))
            return None
        open(p, "w").write(src.replace(old, new))
        return src

    def in_place(m):
        """Builds and tests a mutation in the tree itself."""
        name, path, old, new, test = m
        src = apply(ROOT, m)
        if src is None:
            return
        try:
            if test.startswith("sh:"):
                record(name, run_sh(test[3:]))
                return
            cargo, module, flt = target(test)
            exe, _ = build(cargo, ROOT)
            if exe is None:
                record(name, "BUILD ERROR")
                return
            record(name, run_tests(exe, tests_in(exe, module, flt), cwd=ROOT))
        finally:
            open(os.path.join(ROOT, path), "w").write(src)

    if serial:
        for m, _ in chosen:
            in_place(m)
    else:
        work_root = os.path.join(ROOT, "target", "mutate")
        shutil.rmtree(work_root, ignore_errors=True)
        # Run in the tree itself: commands that use its build or its
        # .upstream, which every copy links to.
        here = [m for m, _ in chosen if m[4].startswith("sh:") or target(m[4])[1] in IN_PLACE]
        last = [m for m, _ in chosen if m[1].startswith(".upstream/")]
        built = [m for m, _ in chosen if m not in here and m not in last]
        queue = list(enumerate(built))
        alone = []
        pool = ThreadPoolExecutor(max_workers=jobs)
        futures = []

        def builder(k):
            root = os.path.join(work_root, "trees", str(k))
            make_tree(root)
            while True:
                with lock:
                    if not queue:
                        return
                    i, m = queue.pop(0)
                name, path, old, new, test = m
                src = apply(root, m)
                if src is None:
                    continue
                try:
                    cargo, module, flt = target(test)
                    began = time.time()
                    exe, _ = build(cargo, root)
                    if exe is None:
                        record(name, "BUILD ERROR")
                        continue
                    names = tests_in(exe, module, flt, root)
                    work = os.path.join(work_root, "runs", str(i))
                    os.makedirs(os.path.join(work, "tmp"))
                    copy = os.path.join(work, os.path.basename(exe))
                    shutil.copy2(exe, copy)
                    shutil.copy2(os.path.join(root, "target", "debug", "cuttlebone"), os.path.join(work, "cuttlebone"))
                    if os.environ.get("MUTATE_TIMES"):
                        print("%s: built and listed in %.1fs" % (name, time.time() - began), flush=True)
                finally:
                    open(os.path.join(root, path), "w").write(src)
                job = (name, copy, names, work, root)
                if cargo[1:] and cargo[1] in OWN_BINARIES:
                    with lock:
                        alone.append(job)
                else:
                    with lock:
                        futures.append(pool.submit(lambda j: record(j[0], run_tests(j[1], j[2], j[3], cwd=j[4])), job))

        threads = [threading.Thread(target=builder, args=(k,)) for k in range(trees)]
        for t in threads:
            t.start()
        for m in here:
            in_place(m)
        for t in threads:
            t.join()
        for f in list(futures):
            f.result()
        pool.shutdown()
        for job in alone:
            record(job[0], run_tests(job[1], job[2], job[3], cwd=job[4]))
        for m in last:
            in_place(m)
        shutil.rmtree(work_root, ignore_errors=True)
    ordered = [(m[0], results.get(m[0], "NOT RUN")) for m, _ in chosen]
    caught = sum(v.startswith("caught") for _, v in ordered)
    for name, v in ordered:
        if not v.startswith("caught"):
            print("%s: %s" % (name, v))
    print("%d of %d caught in %ds" % (caught, len(ordered), time.time() - start))
    return 0 if ordered and caught == len(ordered) else 1


def interrupted(signum, frame):
    raise KeyboardInterrupt


if __name__ == "__main__":
    import signal
    # So that `finally` restores a mutated file when the run is killed.
    signal.signal(signal.SIGTERM, interrupted)
    sys.exit(main(sys.argv[1:]))
