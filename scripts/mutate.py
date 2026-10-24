#!/usr/bin/env python3
"""Breaks behaviours on purpose, one at a time, and checks that a test fails.

Each mutation replaces one exact snippet of source (it must occur exactly
once), runs the tests that should catch it, and restores the file. Run it
from anywhere; pass words to run only the mutations whose names contain
them, e.g. `scripts/mutate.py timeout`. A mutation reported as NOT CAUGHT
means a behaviour has no test; SKIP means its snippet changed, so update
the snippet. It exits with status 1 unless every mutation is caught.

The last field names what should catch the mutation: `--test FILE [FILTER]`
for `cargo test`, or `sh:COMMAND` for a command that must fail (for rules
that clippy or a check script enforces).
"""
import subprocess, sys, os, json
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
 ("cancel ignored", "src/lsp.rs", "self.cancel(connection, |p| p.id == id, ErrorCode::RequestCanceled);", "let _ = id;", "--test lsp cancelling"),
 ("edits don't supersede", "src/lsp.rs", "                    self.supersede(connection, &uri);\n", "", "--test lsp an_edit_cancels"),
 ("supersede cancels every document", "src/lsp.rs", "self.cancel(connection, |p| &p.uri == uri, ErrorCode::ContentModified);", "let _ = uri; self.cancel(connection, |_| true, ErrorCode::ContentModified);", "--test lsp an_edit_cancels"),
 ("no debounce", "src/lsp.rs", "(at + self.options.debounce).max(Instant::now())", "at.max(Instant::now())", "--test lsp checks_wait"),
 ("status always sent", "src/lsp.rs", "        if self.options.status {", "        if true {", "--test lsp without_opting_in"),
 ("always markdown", "src/lsp.rs", "        let contents = if self.markdown {", "        let contents = if true {", "--test lsp plain_text"),
 ("checks in the server's thread", "src/lsp.rs", "            match job.and_then(|job| Check::start(self.checks, &job, self.finished.0.clone())) {\n                Ok(check) => self.running = Some((pending, check)),\n                Err(reason) => self.answer(connection, pending, Outcome::Failed(reason)),\n            }",
  "            match job {\n                Ok(job) => self.answer(connection, pending, job.run()),\n                Err(reason) => self.answer(connection, pending, Outcome::Failed(reason)),\n            }", "--test lsp aborting"),
 ("version mismatch accepted", "src/lsp.rs", "    if installed != supported {", "    if installed == \"never\" {", "--test lsp a_different_inko"),
 ("no placeholder", "src/query.rs", "        copy.insert_str(offset, PLACEHOLDER);\n", "        let _ = offset;\n", "--test complete"),
 ("didSave text ignored", "src/lsp.rs", "                    doc.text = text;\n", "                    let _ = &text;\n", "--test lsp saving"),
 ("didClose ignored", "src/lsp.rs", "                    && let Some(doc) = self.documents.remove(&p.text_document.uri)", "                    && let Some(doc) = None::<Document>", "--test lsp a_session"),
 ("workspace folders ignored", "src/lsp.rs", "        (Some(folders), _, _) if !folders.is_empty() => {", "        (Some(folders), _, _) if folders.is_empty() => {", "--test lsp workspace_folders"),
 ("other open documents not sent", "src/lsp.rs", "                contents: self.contents(),\n            },\n            deadline: Some(self.options.timeout + DEADLINE_MARGIN),\n        })", "                contents: self.contents().into_iter().filter(|(p, _)| Some(p) == pending.uri.to_file_path().ok().as_ref()).collect(),\n            },\n            deadline: Some(self.options.timeout + DEADLINE_MARGIN),\n        })", "--test lsp unsaved_changes_to_an_imported"),
 ("hover has no range", "src/lsp.rs", "            range: Some(range(t.range)),", "            range: None,", "--test lsp a_session"),
 ("exit status ignores shutdown", "src/lsp.rs", 'Message::Notification(n) if n.method == "exit" => return Some(self.shut_down),', 'Message::Notification(n) if n.method == "exit" => return Some(true),', "--test lsp exit_without"),
 ("requests after shutdown answered", "src/lsp.rs", "        if self.shut_down {\n            return reply_error(", "        if false {\n            return reply_error(", "--test lsp shutdown_then_exit"),
 ("crash not reported", "src/worker.rs", "Ok(outcome) if status.as_ref().is_ok_and(ExitStatus::success) => outcome,", "Ok(outcome) => outcome,\n            _ if true => Outcome::Failed(Unavailable::new(\"x\", \"\")),", "--test lsp aborting"),
 ("shutdown doesn't cancel", "src/lsp.rs", "                self.shut_down = true;\n                self.cancel_all(connection);", "                self.shut_down = true;", "--test lsp shutdown_kills"),
 ("completion kinds swapped", "src/lsp.rs", "MemberKind::Field => lsp::CompletionItemKind::FIELD,", "MemberKind::Field => lsp::CompletionItemKind::PROPERTY,", "--test lsp a_session"),
 ("no capability for completion trigger", "src/lsp.rs", 'trigger_characters: Some(vec![".".to_string()]),', "trigger_characters: None,", "--test lsp initialize_advertises"),
 ("full sync advertised", "src/lsp.rs", "change: Some(lsp::TextDocumentSyncKind::INCREMENTAL),", "change: Some(lsp::TextDocumentSyncKind::FULL),", "--test lsp initialize_advertises"),
 ("no worker deadline", "src/worker.rs", "            std::process::exit(DEADLINE_STATUS);", "            let _ = DEADLINE_STATUS;", "--test lsp a_hung_worker"),
 ("byte_offset ignores CR", "src/query.rs", "    let line = line.strip_suffix('\\r').unwrap_or(line);\n", "", "--test complete byte_offsets"),
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
 ("references in std and dependencies", "src/query.rs", "} else if scope.in_project(&file) {", "} else if !file.as_os_str().is_empty() {", "--test definition references_to_a_std_symbol"),
 ("other files' ranges use the request's lines", "src/query.rs", "range: range(&Lines::with_encoding(text, self.encoding), &location),", "range: range(self.lines, &location),", "--test navigation references_honour"),
 ("no capability for definition", "src/lsp.rs", "definition_provider: Some(lsp::OneOf::Left(true)),", "definition_provider: None,", "--test lsp initialize_advertises"),
 ("includeDeclaration not read", "src/lsp.rs", '.pointer("/context/includeDeclaration")', '.pointer("/context/include_declaration")', "--test navigation references_honour"),
 ("no status for definitions", "src/lsp.rs", "            Outcome::Place(answer) => {\n                let status = status_of(&answer);", "            Outcome::Place(answer) => {\n                let status = (Ok(()), Duration::ZERO);", "--test navigation broken_code"),
 ("type definition answers definition", "src/worker.rs", "Query::TypeDefinition => Question::TypeDefinition,", "Query::TypeDefinition => Question::Definition,", "--test navigation type_definition"),
 # The guardrails (AGENTS.md).
 ("query uses lsp", "src/query.rs", "use crate::check::{Target, diagnose, project_root, run};\n", "use crate::check::{Target, diagnose, project_root, run};\nuse crate::lsp as _;\n", "--test guardrails layers"),
 ("position uses walker", "src/position.rs", "impl Encoding {\n", "use crate::walker as _;\nimpl Encoding {\n", "--test guardrails layers"),
 # The editor's name is split so that this file doesn't name one itself.
 ("an editor named in code", "src/lsp.rs", "//! The language server: standard LSP over stdio", "//! The language server, for " + "Ema" + "cs: standard LSP over stdio", "--test guardrails editor"),
 ("a binary named after Inko", "Cargo.toml", 'name = "dump-types"', 'name = "inko-dump-types"', "--test guardrails named_after"),
 ("an unjustified patch line", "patches/upstream.patch", "+    pub overlay: IndexMap<PathBuf, Vec<u8>>,\n", "+    pub overlay: IndexMap<PathBuf, Vec<u8>>,\n+    pub unjustified: bool,\n", "--test guardrails patches_readme"),
 ("a path from one machine", "AGENTS.md", "Never modify any checkout of Inko's own repository.", "Never modify ~/src/inko.", "--test guardrails paths"),
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
 ("diagnostics in the server's thread", "src/lsp.rs", "        match Check::start(self.checks, &job, self.finished.0.clone()) {\n            Ok(check) => self.diagnoser.running = Some((target, check)),", "        match Ok::<_, Unavailable>(job.run()) {\n            Ok(Outcome::Diagnostics(answer)) => self.publish(connection, target, answer),\n            Ok(_) => {}", "--test latency queries_stay_fast"),
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
 ("the batch's answers out of order", "src/query.rs", "        for (i, answer) in group\n            .iter()\n            .zip(", "        for (i, answer) in group\n            .iter()\n            .rev()\n            .zip(", "--test replay"),
 # Stale answers.
 ("completion answers from older results", "src/query.rs", "                | Question::InlayHints { .. }\n        ),", "                | Question::InlayHints { .. }\n                | Question::Members\n        ),", "--test stale read_only"),
 ("references answer from older results", "src/query.rs", "                | Question::InlayHints { .. }\n        ),", "                | Question::InlayHints { .. }\n                | Question::References { .. }\n        ),", "--test stale read_only"),
 ("no answers from older results", "src/query.rs", "                matches!(&results[*i], Err(r) if recoverable(r))", "                matches!(&results[*i], Err(r) if recoverable(r) && false)", "--test stale"),
 ("positions not mapped back", "src/query.rs", "Some(position_at(&old, changes.old_offset(byte)?, encoding))", "Some(position_at(&old, byte, encoding))", "--test stale read_only"),
 ("ranges not mapped forward", "src/query.rs", "        let mapped = self.changes.new_range(start..end)?;", "        let mapped = start..end;", "--test stale read_only"),
 ("other files' ranges not mapped forward", "src/query.rs", "        let mapped = v.changes.new_range(start..end)?;", "        let mapped = start..end;", "--test stale definitions_into"),
 ("the position's unit not checked", "src/query.rs", "                        && !guard.safe(byte_offset(&old, old_position, encoding)?)", "                        && !guard.safe(byte_offset(&old, old_position, encoding)?) && false", "--test stale a_changed_declaration"),
 ("hints' units not checked", "src/query.rs", "                    if !self.safe(byte_offset(self.old, h.position, e)?) {", "                    if !self.safe(byte_offset(self.old, h.position, e)?) && false {", "--test stale hints_in_the_method"),
 ("units a change touched count as safe", "src/query.rs", "            if touched {\n                return false;\n            }", "            if touched && false {\n                return false;\n            }", "--test stale read_only"),
 ("units mentioning a changed declaration count as safe", "src/query.rs", "                .any(|n| self.all.iter().any(|e| e.affects(n)))", "                .any(|n| self.all.iter().any(|e| e.affects(n) && false))", "--test stale a_changed_declaration"),
 ("units with errors count as safe", "src/query.rs", "            if types.contains(&None) {", "            if types.contains(&None) && false {", "--test replay"),
 ("highlights missing uses in changed text", "src/query.rs", "            let added = self\n                .effects", "            let added = false && self\n                .effects", "--test replay"),
 ("highlights missing uses in unsafe units", "src/query.rs", "            added || unsafe_use", "            added", "--test replay"),
 ("inlay hints for a range with nothing safe", "src/query.rs", "        if !covered {\n            return None;\n        }", "        if !covered && false {\n            return None;\n        }", "--test replay"),
 ("added names aren't prefixes", "src/units.rs", "self.names.contains(name) || self.prefixes.iter().any(|p| name.starts_with(p.as_str()))", "self.names.contains(name) || self.prefixes.contains(name)", "--lib units"),
 ("changes in a body change other methods", "src/units.rs", "        && body.start < old_range.start\n        && old_range.end < body.end\n    {", "        && body.start < old_range.start\n        && old_range.end < body.end\n        && false\n    {", "--test stale read_only"),
 ("a copy's check counts as the documents'", "src/query.rs", "            answer.typed &= !changed;", "            let _ = changed;", "--test stale a_copy"),
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
]
only = sys.argv[1:]
results = []
for name, path, old, new, test in M:
    if only and not any(o in name for o in only): continue
    p = os.path.join(ROOT, path)
    src = open(p).read()
    if src.count(old) != 1:
        results.append((name, "SKIP: pattern count %d" % src.count(old))); print(results[-1], flush=True); continue
    open(p, "w").write(src.replace(old, new))
    try:
        if test.startswith("sh:"):
            r = subprocess.run(test[3:], shell=True, cwd=ROOT, capture_output=True, text=True, timeout=900)
            out = r.stdout + r.stderr
            reason = next((l.strip() for l in out.splitlines() if l.startswith("error") or l.startswith("the ") or l.startswith("patches/")), "")
            verdict = "caught by %s: %s" % (test[3:], reason) if r.returncode != 0 else "NOT CAUGHT"
        else:
            r = subprocess.run("cargo test " + test.split(" ")[0] + " " + test.split(" ")[1] + (" -- " + test.split(" ",2)[2] if test.count(" ") >= 2 else ""),
                               shell=True, cwd=ROOT, capture_output=True, text=True, timeout=900)
            out = r.stdout + r.stderr
        if test.startswith("sh:"):
            pass
        elif "could not compile" in out or "error[" in out:
            verdict = "BUILD ERROR"
        elif r.returncode != 0 and "FAILED" in out:
            failed = [l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.endswith("FAILED")]
            verdict = "caught by " + ", ".join(failed)
        elif r.returncode == 0:
            verdict = "NOT CAUGHT"
        else:
            verdict = "other failure: " + out[-500:]
    finally:
        open(p, "w").write(src)
    results.append((name, verdict)); print(results[-1], flush=True)
caught = sum(v.startswith("caught") for _, v in results)
print("%d of %d caught" % (caught, len(results)))
sys.exit(0 if results and caught == len(results) else 1)
