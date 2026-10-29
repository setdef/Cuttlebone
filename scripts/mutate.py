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

Usage: scripts/mutate.py [--since REV] [--shard K/N] [--jobs N | --serial] [--list] [--control] [WORD...]

  (no options)   every mutation: the exhaustive mutations of the release
                 tier, which CI runs in shards (AGENTS.md)
  --since REV    only the mutations a change since REV could affect: the
                 milestone tier. See `selected`.
  --shard K/N    only every Nth of the mutations chosen, starting at the
                 Kth (1 to N), leaving out those `--shard dist` runs: one
                 of N runners' share, as the release workflow runs the
                 exhaustive tier on Linux. Each shard runs its own
                 control first.
  --shard dist   only the mutations scripts/check-dist.sh catches, which
                 builds this machine's release archive the way only macOS
                 does outside Docker (scripts/dist-build.sh). The K/N
                 shards and this one run every mutation between them.
  --list         print what would run, and why, without running anything
  --jobs N       how many mutations' tests run at once (default: half the
                 cores, at most 5)
  --trees N      how many mutations are built at once
  --serial       build and test each mutation in place, one at a time, as
                 the runner did before milestone 6
  --control      only check that every test passes unmutated in a copy of
                 the tree, which every run of mutations in copies does first
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
 ("no required-version check", "src/check.rs", "    check_required_version(target)?;\n\n    let mut config = target.config()?;\n\n    config.check_after_errors", "    let mut config = target.config()?;\n\n    config.check_after_errors", "--test manifest"),
 ("diagnostics skip the required-version check", "src/check.rs", "    check_required_version(target)?;\n\n    let mut config = target.config()?;\n\n    config.exclude", "    let mut config = target.config()?;\n\n    config.exclude", "--test manifest diagnostics_refuse"),
 ("dependency manifests ignored", "src/check.rs", "paths.push(version.join(MANIFEST_FILE));", "let _ = version;", "--test manifest"),
 ("an installed std's file checked as main", "src/check.rs", "            config.source = self.std.clone();\n", "", "--test navigation answers_in_a_file_of_an_installed_std"),
 ("always UTF-16", "src/lsp.rs", '"utf-8" => Some(Encoding::Utf8),', '"utf-8" => Some(Encoding::Utf16),', "--test lsp utf8"),
 ("UTF-8 counts chars", "src/position.rs", "Encoding::Utf8 => text.len() as u32,", "Encoding::Utf8 => text.chars().count() as u32,", "--test lsp non_ascii"),
 ("incremental edits replace everything", "src/lsp.rs", "    let Some(r) = change.range else {", "    let Some(r) = None::<lsp::Range> else {", "--test lsp several_changes"),
 ("edit ranges in UTF-16 regardless", "src/lsp.rs", "            encoding,\n        )\n        .unwrap_or(text.len())", "            Encoding::Utf16,\n        )\n        .unwrap_or(text.len())", "--test lsp non_ascii_positions_and_ranges_in_utf8"),
 ("no timeout", "src/lsp.rs", "            (Some((_, check)), _) => Some(check.started + self.options.timeout),", "            (Some((_, _check)), _) => None,", "--test lsp a_hung_check"),
 ("timeout doesn't kill", "src/lsp.rs", "        check.kill();\n        log(&format!(\n            \"check for {} timed out", "        std::mem::forget(check);\n        log(&format!(\n            \"check for {} timed out", "--test lsp a_hung_check"),
 ("cancel ignored", "src/lsp.rs", "self.cancel(connection, &id);", "let _ = id;", "--test lsp cancelling"),
 # The client's process (LSP's processId and --clientProcessId).
 ("the client's process not watched", "src/lsp.rs", "            Some(_) => crossbeam_channel::tick(Duration::from_secs(1)),", "            Some(_) => crossbeam_channel::never::<Instant>(),", "--test lsp the_server_exits_when"),
 ("processId ignored", "src/lsp.rs", "                server.parent = watched(params.process_id.or(client));", "                server.parent = watched(client);", "--test lsp the_server_exits_when"),
 ("--clientProcessId ignored", "src/bin/cuttlebone.rs", "            Some(pid) => client = Some(pid),", "            Some(pid) => client = Some(pid).filter(|_: &u32| false),", "--test lsp the_server_exits_when"),
 # What an answer covers.
 ("hints cover everything asked", "src/query.rs", "            covered: Some(coverage),", "            covered: None,", "--test replay"),
 ("coverage stops at the first gap", "src/query.rs", "                (false, Some(_)) => runs.extend(run.take()),", "                (false, Some(_)) => {\n                    runs.extend(run.take());\n                    break;\n                }", "--test replay"),
 ("coverage not clipped to the request", "src/query.rs", "                            *covered = covered\n                                .iter()\n                                .filter(|r| r.start <= *end && *position <= r.end)\n                                .map(|r| Range {\n                                    start: r.start.max(*position),\n                                    end: r.end.min(*end),\n                                })\n                                .collect();", "                            let _ = covered;", "--test stale hints_in_the_method"),
 ("no coverage in the status", "src/lsp.rs", "                params[\"covered\"] = json!(ranges);", "                let _ = ranges;", "--test stale hints_in_the_method"),
 ("no answer covers the request", "src/lsp.rs", "                    covered.unwrap_or_default().into_iter().map(range).collect();", "                    covered.unwrap_or(vec![query::Range { start: pending.position, end: pending.position }]).into_iter().map(range).collect();", "--test stale hints_in_the_method"),
 ("a fresh answer covers nothing", "src/lsp.rs", "                            .unwrap_or_else(|| vec![asked]),", "                            .unwrap_or_default(),", "--test stale hints_in_the_method"),
 # Requests are answered for the text they were about (LSP 3.18).
 ("edits don't freeze queued queries", "src/lsp.rs", "        ) {\n            self.freeze();\n        }\n", "        ) {\n        }\n", "--test lsp a_request_is_answered"),
 ("freezing forgets the fallback", "src/lsp.rs", "                f.fallback.as_deref().cloned(),", "                None,", "--test stale a_query_frozen"),
 ("edits kill the running check", "src/lsp.rs", "    fn freeze(&mut self) {\n", "    fn freeze(&mut self) {\n        if let Some((_, check)) = self.running.take() {\n            check.kill();\n        }\n", "--test lsp an_edit_does_not"),
 ("frozen checks are good checks", "src/lsp.rs", "            if let Ok(job) = &job\n                && pending.frozen.is_none()\n", "            if let Ok(job) = &job\n", "--test stale a_check_of"),
 ("cancelRequest cancels everything", "src/lsp.rs", "        if let Some(i) = self.queue.iter().position(|p| p.id == *id) {", "        let _ = id;\n        if let Some(i) = (!self.queue.is_empty()).then_some(0) {", "--test lsp cancelling"),
 ("shutdown answers with errors", "src/lsp.rs", "        for pending in std::mem::take(&mut self.queue) {\n            self.answer(connection, pending, Outcome::Failed(reason.clone()));\n        }", "        for pending in std::mem::take(&mut self.queue) {\n            reply_error(connection, pending.id, ErrorCode::RequestCanceled, \"cancelled\");\n        }", "--test lsp shutdown_kills"),
 ("no debounce", "src/lsp.rs", "(at + self.options.debounce).max(Instant::now())", "at.max(Instant::now())", "--test lsp checks_wait"),
 ("status always sent", "src/lsp.rs", "        if self.options.status {\n            let (state, elapsed, source, cached) = status;", "        if true {\n            let (state, elapsed, source, cached) = status;", "--test lsp without_opting_in"),
 ("always markdown", "src/lsp.rs", "        let markdown = self.support.markdown;", "        let markdown = true;", "--test lsp plain_text"),
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
 ("noActiveParameterSupport read from the spec text's place only", "src/lsp.rs", "            no_active_parameter: yes(&format!(\"{}/noActiveParameterSupport\", signature))\n", "            no_active_parameter: false\n", "--test signature past_the_last"),
 ("no null activeParameter", "src/lsp.rs", "        None if support.no_active_parameter => Some(Value::Null),", "", "--test signature past_the_last"),
 ("diagnostic tags for every client", "src/lsp.rs", "tags: (unnecessary && self.support.unnecessary_tag)", "tags: (unnecessary || self.support.unnecessary_tag)", "--test conformance diagnostics_without"),
 ("the conformance check off", "tests/common/client.rs", "        if let Err(e) = spec().check(&message, method, &self.capabilities) {", "        if let Err(e) = Ok::<(), String>(()) {", "--test conformance the_client_fails"),
 ("version mismatch accepted", "src/lsp.rs", "    if installed != supported {", "    if installed == \"never\" {", "--test lsp a_different_inko"),
 ("no placeholder", "src/query.rs", "        copy.insert_str(offset, PLACEHOLDER);\n", "        let _ = offset;\n", "--test complete"),
 ("didSave text ignored", "src/lsp.rs", "                    doc.text = text;\n", "                    let _ = &text;\n", "--test lsp saving"),
 ("didClose ignored", "src/lsp.rs", "                    && let Some(doc) = self.documents.remove(&p.text_document.uri)", "                    && let Some(doc) = None::<Document>", "--test lsp a_session"),
 ("workspace folders ignored", "src/lsp.rs", "        (Some(folders), _, _) if !folders.is_empty() => {", "        (Some(folders), _, _) if folders.is_empty() => {", "--test lsp workspace_folders"),
 ("other open documents not sent", "src/lsp.rs", "                (file, self.contents(), fallback)", "                (file.clone(), self.contents().into_iter().filter(|(p, _)| *p == file).collect(), fallback)", "--test lsp unsaved_changes_to_an_imported"),
 ("hover has no range", "src/lsp.rs", "            range: Some(range(h.range)),", "            range: None,", "--test lsp a_session"),
 ("exit status ignores shutdown", "src/lsp.rs", 'Message::Notification(n) if n.method == "exit" => return Some(self.shut_down),', 'Message::Notification(n) if n.method == "exit" => return Some(true),', "--test lsp exit_without"),
 ("requests after shutdown answered", "src/lsp.rs", "        if self.shut_down {\n            return reply_error(", "        if false {\n            return reply_error(", "--test lsp shutdown_then_exit"),
 ("crash not reported", "src/worker.rs", "Ok(outcome) if status.as_ref().is_ok_and(ExitStatus::success) => (outcome, None),", "Ok(outcome) => (outcome, None),\n            _ if true => (Outcome::Failed(Unavailable::new(\"x\", \"\")), None),", "--test lsp aborting"),
 ("shutdown doesn't cancel", "src/lsp.rs", "                self.shut_down = true;\n                self.cancel_all(connection);", "                self.shut_down = true;", "--test lsp shutdown_kills"),
 ("completion kinds swapped", "src/lsp.rs", "MemberKind::Field => lsp::CompletionItemKind::FIELD,", "MemberKind::Field => lsp::CompletionItemKind::PROPERTY,", "--test lsp a_session"),
 ("no capability for completion trigger", "src/lsp.rs", 'trigger_characters: Some(vec![".".to_string(), "@".to_string()]),', "trigger_characters: None,", "--test lsp initialize_advertises"),
 ("full sync advertised", "src/lsp.rs", "change: Some(lsp::TextDocumentSyncKind::INCREMENTAL),", "change: Some(lsp::TextDocumentSyncKind::FULL),", "--test lsp initialize_advertises"),
 ("no worker deadline", "src/worker.rs", "Err(RecvTimeoutError::Timeout) => std::process::exit(DEADLINE_STATUS),", "Err(RecvTimeoutError::Timeout) => return,", "--test lsp a_hung_worker"),
 ("byte_offset ignores CR", "src/position.rs", "    start + line.find('\\r').unwrap_or(line.len())", "    start + line.len()", "--test complete byte_offsets"),
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
 ("a compound assignment's definition is the setter only", "src/query.rs", "        if r.location == selected.location && found.iter().all(|f| f.symbol != r.symbol) {", "        if std::ptr::eq(r, selected) {", "--test navigation definition_at_a_compound"),
 ("one definition as an array", "src/lsp.rs", "if matches!(pending.query, Query::Definition | Query::Declaration)\n                            && places.len() == 1 =>", "if false =>", "--test navigation definition_at_a_compound"),
 ("a saved document counts as a change on disk", "src/check.rs", "let seen = Seen::of(&p, target.overlay.contains_key(&p), false);", "let seen = Seen::of(&p, false, false);", "--test cache a_save_keeps"),
 ("a directory's stamp alone counts", "src/check.rs", "            Seen::Directory(stamp, seen) => Stamp::of(path) == *stamp || names(path) == *seen,", "            Seen::Directory(stamp, _) => Stamp::of(path) == *stamp,", "--test cache a_save_keeps"),
 ("a directory's names never change", "src/check.rs", "            Seen::Directory(stamp, seen) => Stamp::of(path) == *stamp || names(path) == *seen,", "            Seen::Directory(_, _) => true,", "--test cache a_file_created"),
 ("references at std's declarations search std", "src/query.rs", "    } else if file.starts_with(&std) {\n        projects.collect()", "    } else if file.starts_with(&std) {\n        Vec::new()", "--test definition references_at_a_declaration_outside"),
 ("references at a dependency's declarations search the dependency", "src/query.rs", "        projects\n            .filter(|p| file.starts_with(p.join(\"dep\")))\n            .take(1)\n            .collect()", "        Vec::new()", "--test definition references_at_a_declaration_outside"),
 ("references in the asked file outside the project", "src/query.rs", "            if !scope.in_project(&file) {\n                continue;\n            } else if", "            if false {\n            } else if", "--test definition references_at_a_declaration_outside"),
 ("references in a module the project doesn't use unavailable", "src/query.rs", "            if checked.target.file.is_none() && !in_sources(&checked.target.root, &file) {", "            if false {", "--test definition references_at_a_declaration_outside"),
 ("another project's references dropped", "src/query.rs", "            places.extend(more);\n", "            let _ = more;\n", "--test definition references_at_a_declaration_outside"),
 ("imports of modules not found aren't blank", "src/query.rs", "            if !imports.is_empty() {\n                blank_lines(&mut out, path, text, &imports);\n            }\n", "", "--test repaired an_import_of_a_module"),
 ("imports of modules not found blank only in open documents", "src/query.rs", "        if !docs.contains_key(&path) {\n            docs.insert(path.clone(), read(&path).ok()?);\n        }\n\n        imports", "        if !docs.contains_key(&path) {\n            continue;\n        }\n\n        imports", "--test repaired an_import_of_a_module"),
 ("no imports of modules not found reported", "src/check.rs", "            .filter(|d| unfound_module(d))", "            .filter(|_| false)", "--test repaired an_import_of_a_module"),
 ("every name in an import counts", "src/units.rs", "            Kind::Import => out.extend(import_names(&source[item.range.clone()]).map(String::from)),", "            Kind::Import => out.extend(names(&source[item.range.clone()]).map(String::from)),", "--test repaired an_import_of_a_module"),
 ("an import's module name doesn't count", "src/units.rs", "    names(path).last().into_iter().chain(names(list))", "    names(list)", "--lib an_import_binds"),
 ("declaration errors' ids ignored", "src/query.rs", "        let counts = |name: &str, ids: &Ids| match ids.get(name) {", "        let counts = |name: &str, ids: &Ids| match ids.get(name).filter(|_| false) {", "--test broken a_declaration_error_affects"),
 ("a mention by the error's own id doesn't count", "src/query.rs", "self.symbols.get(at).is_none_or(|s| about.contains(s))", "self.symbols.get(at).is_none_or(|_| false)", "--test broken a_declaration_error_affects"),
 ("names in types count by id", "src/query.rs", "                types.contains(name)\n                    || written", "                written", "--test broken a_declaration_error_affects"),
 ("an error's ids from anywhere in its items", "src/query.rs", "                if header.contains(&start) && named.iter().any(|n| n == name) {", "                if named.iter().any(|n| n == name) {", "--test broken a_declaration_error_affects"),
 ("includeDeclaration ignored", "src/query.rs", ".filter(|_| include_declaration);", ".filter(|_| true);", "--test definition references_to_a_variable"),
 ("references only in what the file imports", "src/query.rs", "            } else if project && in_sources(&target.root, &file) {\n                target.file = None;\n", "            } else if project && in_sources(&target.root, &file) {\n", "--test definition references_search_modules"),
 ("references in std and dependencies", "src/query.rs", "            if !scope.in_project(&file) {\n                continue;\n", "            if file.as_os_str().is_empty() {\n                continue;\n", "--test definition references_to_a_std_symbol"),
 ("other files' ranges use the request's lines", "src/query.rs", "            range: self.file(file).location(&location, self.encoding),", "            range: range(self.lines, &location),", "--test navigation references_honour"),
 ("no capability for definition", "src/lsp.rs", "definition_provider: Some(lsp::OneOf::Left(true)),", "definition_provider: None,", "--test lsp initialize_advertises"),
 ("includeDeclaration not read", "src/lsp.rs", '.pointer("/context/includeDeclaration")', '.pointer("/context/include_declaration")', "--test navigation references_honour"),
 ("no status for definitions", "src/lsp.rs", "            Outcome::Place(answer) => {\n                let status = status_of(&answer);", "            Outcome::Place(answer) => {\n                let status = (Ok(()), Duration::ZERO, Source::Fresh, false);", "--test navigation broken_code"),
 ("type definition answers definition", "src/worker.rs", "Query::TypeDefinition => Question::TypeDefinition,", "Query::TypeDefinition => Question::Definition,", "--test navigation type_definition"),
 # The guardrails (AGENTS.md).
 ("query uses lsp", "src/query.rs", "use crate::check::{Fingerprint, Reported, Target, diagnose, project_root, run, run_after_errors};\n", "use crate::check::{Fingerprint, Reported, Target, diagnose, project_root, run, run_after_errors};\nuse crate::lsp as _;\n", "--test guardrails layers"),
 ("position uses walker", "src/position.rs", "impl Encoding {\n", "use crate::walker as _;\nimpl Encoding {\n", "--test guardrails layers"),
 # The editor's name is split so that this file doesn't name one itself.
 ("an editor named in code", "src/lsp.rs", "//! The language server: standard LSP over stdio", "//! The language server, for " + "Ema" + "cs: standard LSP over stdio", "--test guardrails editor"),
 ("a binary named after Inko", "Cargo.toml", 'name = "dump-types"', 'name = "inko-dump-types"', "--test guardrails named_after"),
 ("an unjustified patch line", "patches/upstream.patch", "+    pub overlay: IndexMap<PathBuf, Vec<u8>>,\n", "+    pub overlay: IndexMap<PathBuf, Vec<u8>>,\n+    pub unjustified: bool,\n", "--test guardrails patches_readme"),
 ("a path from one machine", "AGENTS.md", "Never modify any checkout of Inko's own repository.", "Never modify ~/src/inko.", "--test guardrails paths"),
 ("std's path not canonical in the tests", "src/dump.rs", "    path.canonicalize().unwrap_or(path)", "    path", "sh:scripts/mutate.py --control"),
 ("a version without a changelog entry", "CHANGELOG.md", "versions here were given afterwards.\n", "versions here were given afterwards.\n\n## 0.99.0\n\n[Report](docs/history/MILESTONE-99.md)\n", "--test guardrails the_version_has"),
 ("a changelog entry without its report", "CHANGELOG.md", "[Report](docs/history/MILESTONE-6.md)", "[Report](docs/history/MILESTONE-5.md)", "--test guardrails the_version_has"),
 ("docs show another version", "docs/lsp.md", "- `cuttlebone --version` prints `cuttlebone ", "- `cuttlebone --version` prints `cuttlebone 0.0.1` or `cuttlebone ", "--test guardrails docs_show"),
 ("printing to stdout in the library", "src/query.rs", "pub fn type_at(session: &Session, request: &Request) -> Answer<TypeAt> {\n", "pub fn type_at(session: &Session, request: &Request) -> Answer<TypeAt> {\n    println!();\n", "sh:cargo clippy --quiet --all-targets"),
 ("unsafe code", "src/position.rs", "impl Encoding {\n", "impl Encoding {\n    pub fn unsafe_code() {\n        unsafe {}\n    }\n", "sh:cargo clippy --quiet --all-targets"),
 ("fault injection in release builds", "Cargo.toml", "fault-injection = []\n", "default = [\"fault-injection\"]\nfault-injection = []\n", "sh:scripts/check-release.sh"),
 ("a patch that doesn't apply to the release", "patches/upstream.patch", "-mod hir;\n", "-mod hir_typo;\n", "sh:scripts/check-patch.sh"),
 # The release tooling (docs/releases.md).
 ("release binaries may link anything", "scripts/dist-deps.sh", "    exit 1\nfi\n\necho \"$bin links only", "fi\n\necho \"$bin links only", "sh:scripts/check-dist.sh"),
 ("the macOS release links Homebrew's LLVM directory", "scripts/dist-build.sh", "        export LLVM_SYS_181_PREFIX=\"$prefix\"\n", "", "sh:scripts/check-dist.sh"),
 ("an archive of any version verifies", "scripts/dist-verify.py", "check(args.version in (None, version)", "check(True", "sh:scripts/check-dist.sh"),
 ("an archive for any Inko verifies", "scripts/dist-verify.py", "check(args.inko_version in (None, inko)", "check(True", "sh:scripts/check-dist.sh"),
 ("archives without Cuttlebone's license", "scripts/dist-build.sh", "cp LICENSE \"$out/LICENSE\"\n", "", "sh:scripts/check-dist.sh"),
 ("an archive with another LICENSE verifies", "scripts/dist-verify.py", "(work / stem / \"LICENSE\").read_bytes() == (root / \"LICENSE\").read_bytes()", "True", "sh:scripts/check-dist.sh"),
 ("Cargo.toml without the license", "Cargo.toml", "license = \"MPL-2.0\"\n", "", "--test guardrails cuttlebone_is_mpl_2_0"),
 ("the index in the order given", "scripts/dist-index.py", "releases.sort(key=lambda r: tuple(map(int, r[\"cuttlebone\"].split(\".\"))), reverse=True)\n", "", "sh:scripts/check-dist.sh"),
 ("an index in any order checks", "scripts/dist-check-index.py", "expect(versions == sorted(set(versions), key=key, reverse=True)", "expect(True", "sh:scripts/check-dist.sh"),
 ("an index's Inko versions not checked", "scripts/dist-check-index.py", "expect(set(release[\"inko\"]) == inko,", "expect(True,", "sh:scripts/check-dist.sh"),
 ("a version twice in the index", "scripts/dist-index.py", "    if any(r[\"cuttlebone\"] == version for r in releases):\n", "    if False:\n", "sh:scripts/check-dist.sh"),
 ("strict latency limits scaled by the load", "tests/latency.rs", "    if std::env::var_os(STRICT_VARIABLE).is_some() {\n        let scale", "    if std::env::var_os(STRICT_VARIABLE).is_some() && false {\n        let scale", "--test latency limits_are_as_written"),
 # The baseline: strict limits scale by the machine's measured speed,
 # and a slower Cuttlebone still fails them (scripts/latency.sh).
 ("strict limits below as written", "tests/latency.rs", "    base.mul_f64(scale.unwrap_or(1.0).max(1.0))", "    base.mul_f64(scale.unwrap_or(1.0))", "--test latency strict_limits_scale"),
 ("strict limits not scaled by the baseline", "tests/latency.rs", "    base.mul_f64(scale.unwrap_or(1.0).max(1.0))", "    base.mul_f64(scale.map_or(1.0, |_| 1.0))", "--test latency strict_limits_scale"),
 ("a deliberate slowdown under the scaled limits", "src/query.rs", ") -> Result<Option<Vec<Place>>, Unavailable> {\n    {\n        let Some(reference) = symbol_at(", ") -> Result<Option<Vec<Place>>, Unavailable> {\n    std::thread::sleep(Duration::from_millis(3000));\n    {\n        let Some(reference) = symbol_at(", "sh:scripts/latency.sh references_across_std"),
 ("published checksums not checked", "scripts/dist-check-release.py", "expect(sha256 == build[\"sha256\"]", "expect(True", "sh:scripts/check-dist.sh"),
 # Diagnostics (milestone 4).
 ("diagnostics skip MIR", "src/check.rs", "            inspect(db, hir);\n            true", "            inspect(db, hir);\n            false", "--test diagnostics every_fixture"),
 ("warnings dropped", "src/check.rs", "    let diagnostics = compiler.diagnostics().map(reported).collect();", "    let diagnostics = compiler.diagnostics().filter(|d| d.is_error()).map(reported).collect();", "--test diagnostics every_fixture"),
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
 ("warnings published as errors", "src/lsp.rs", "                lsp::DiagnosticSeverity::WARNING", "                lsp::DiagnosticSeverity::ERROR", "--test publish every_file_with_diagnostics"),
 ("diagnostics not superseded", "src/lsp.rs", "        if changed\n            && let Some((target, check))", "        if false\n            && let Some((target, check))", "--test publish superseded"),
 ("no diagnostics debounce", "src/lsp.rs", "            Instant::now() + self.options.diagnostics_debounce", "            Instant::now()", "--test publish diagnostics_wait_for_their"),
 ("diagnostics don't wait for queries", "src/lsp.rs", "        if self.diagnoser.running.is_some()\n            || self.running.is_some()\n            || self.abandoned.is_some()\n            || !self.queue.is_empty()\n        {", "        if self.diagnoser.running.is_some() {", "--test publish diagnostics_wait_for_running"),
 ("no diagnostics timeout", "src/lsp.rs", ".take_if(|(_, c)| c.started.elapsed() >= self.options.timeout)\n        {", ".take_if(|(_, c)| c.started.elapsed() >= self.options.timeout && false)\n        {", "--test publish superseded"),
 ("diagnostics can't be turned off", "src/lsp.rs", "        if !self.options.diagnostics && !self.support.pull {\n            return;\n        }\n", "", "--test publish diagnostics_can_be_turned_off"),
 ("diagnostics in the server's thread", "src/lsp.rs", "        match Check::start(self.checks, &job, self.finished.0.clone()) {\n            Ok(check) => {\n                self.diagnoser.running = Some((target, check));\n                self.diagnoser.sent = Some(job.request.contents);\n            }", "        match Ok::<_, Unavailable>(job.run(&crate::query::Cache::new(0))) {\n            Ok(Outcome::Diagnostics(answer)) => self.publish(connection, target, answer),\n            Ok(_) => {}", "--test latency queries_stay_fast"),
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
 ("named arguments taken as positional", "src/query.rs", "            name: Some(name), ..\n        }) => names.iter().position(|p| p == name),", "            name: Some(_), ..\n        }) => Some(slot),", "--test signature named"),
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
 ("hints past the range", "src/query.rs", "hints.hints.retain(|h| inside(h.position));", "hints.hints.retain(|h| *position <= h.position);", "--test inlay only_the_requested_range"),
 ("the range's end not read", "src/lsp.rs", "                Query::InlayHints {\n                    end: Position {\n                        line: json_u32(&request.params, \"/range/end/line\"),", "                Query::InlayHints {\n                    end: Position {\n                        line: u32::MAX,", "--test inlay only_the_requested_range"),
 ("parameter hints without padding", "src/lsp.rs", "                                padding_right: Some(h.kind == HintKind::Parameter),", "                                padding_right: None,", "--test inlay only_the_requested_range"),
 ("no inlayHintProvider", "src/lsp.rs", "                inlay_hint_provider: Some(lsp::OneOf::Left(true)),\n", "", "--test lsp initialize_advertises"),
 # Answers in code being edited (milestone 5).
 ("an edit's tail starts at the old end", "src/edits.rs", "same.push((old + skip, start + len, n - skip));", "same.push((old + skip, end + len, n - skip));", "--lib edits"),
 ("text next to an edit counts as unchanged", "src/edits.rs", "        let from = range.start.checked_sub(1);", "        let from = Some(range.start).filter(|_| range.start > 0);", "--lib edits"),
 ("text added at the start of a file ignored", "src/edits.rs", "        if (from.is_none() && (old, new) != (0, 0))", "        if (from.is_none() && false)", "--lib edits"),
 ("runs aren't bounded", "src/edits.rs", "        while self.same.len() > MAX_RUNS {", "        while self.same.len() > usize::MAX {", "--lib edits"),
 ("byte offsets as positions in UTF-16", "src/position.rs", "        character: encoding.len(&before[start..]),", "        character: Encoding::Utf16.len(&before[start..]),", "--lib position"),
 ("the batch's answers out of order", "src/query.rs", "        for (i, answer) in group.iter().zip(found) {", "        for (i, answer) in group.iter().rev().zip(found) {", "--test replay"),
 # Stale answers.
 ("completion answers from older results", "src/query.rs", "                | Question::TypeItem\n        ),", "                | Question::TypeItem\n                | Question::Complete\n        ),", "--test stale read_only"),
 ("references answer from older results", "src/query.rs", "                | Question::TypeItem\n        ),", "                | Question::TypeItem\n                | Question::References { .. }\n        ),", "--test stale read_only"),
 ("no answers from older results", "src/query.rs", "                matches!(&results[*i], Err(r) if recoverable(r))", "                matches!(&results[*i], Err(r) if recoverable(r) && false)", "--test stale"),
 ("positions not mapped back", "src/query.rs", "Some(position_at(&old, changes.old_offset(byte)?, encoding))", "Some(position_at(&old, byte, encoding))", "--test stale read_only"),
 ("ranges not mapped forward", "src/query.rs", "        let mapped = self.changes.new_range(start..end)?;", "        let mapped = start..end;", "--test stale read_only"),
 ("other files' ranges not mapped forward", "src/query.rs", "        let mapped = v.changes.new_range(start..end)?;", "        let mapped = start..end;", "--test stale definitions_into"),
 ("the position's unit not checked", "src/query.rs", "    if !matches!(question, Question::InlayHints { .. }) && !guard.safe(offset) {", "    if !matches!(question, Question::InlayHints { .. }) && !guard.safe(offset) && false {", "--test stale a_changed_declaration"),
 ("hints' units not checked", "src/query.rs", "        if !self.safe(old) {\n            return None;\n        }\n\n        self.changes.new_range(old..old)", "        self.changes.new_range(old..old)", "--test stale hints_in_the_method"),
 ("units a change touched count as safe", "src/query.rs", "        let limit = if touching.is_empty() {", "        let limit = if true {", "--test stale read_only"),
 ("units mentioning a changed declaration count as safe", "src/query.rs", ".any(|(e, ids)| e.affects(n) && counts(n, ids))", ".any(|(e, ids)| e.affects(n) && counts(n, ids) && false)", "--test stale a_changed_declaration"),
 ("units with errors count as safe", "src/query.rs", "        if types.iter().any(|(t, _)| t.is_none()) {", "        if types.iter().any(|(t, _)| t.is_none()) && false {", "--test replay"),
 ("highlights missing uses in changed text", "src/query.rs", "            let added = self\n                .effects", "            let added = false && self\n                .effects", "--test replay"),
 ("highlights missing uses in unsafe units", "src/query.rs", "            added || self.mentioned_unsafely(name)", "            added", "--test replay"),
 ("inlay hints for a range with nothing safe", "src/query.rs", "        if !covered || coverage.is_empty() {\n            return None;\n        }", "        if false {\n            return None;\n        }", "--test stale hints_in_the_method"),
 ("added names aren't prefixes", "src/units.rs", "self.names.contains(name) || self.prefixes.iter().any(|p| name.starts_with(p.as_str()))", "self.names.contains(name) || self.prefixes.contains(name)", "--lib units"),
 ("changes in a body change other methods", "src/units.rs", "        && body.start < old_range.start\n        && old_range.end < body.end\n    {", "        && body.start < old_range.start\n        && old_range.end < body.end\n        && false\n    {", "--test stale read_only"),
 ("a copy's check counts as the documents'", "src/query.rs", "            answer.typed &= !changed;", "            let _ = changed;", "--test stale a_copy"),
 # Repaired copies.
 ("repaired copies answer references", "src/query.rs", "            !question.whole_project() && !matches!(question, Question::OutgoingCalls)", "            true", "--test repaired an_edit"),
 ("no repaired copies", "src/query.rs", "    if repair\n        && !wanted.is_empty()", "    if repair && false\n        && !wanted.is_empty()", "--test repaired"),
 ("changed lines aren't blanked alone", "src/query.rs", "            candidates.push(changed.clone());\n        } else {", "        } else {", "--test repaired the_changed_line"),
 ("unclosed brackets not looked for", "src/query.rs", "            if let Some(line) = unclosed(&blank(text, &with)) {", "            if let Some(line) = None::<usize>.filter(|_| !with.is_empty() || text.is_empty()) {", "--test repaired a_method_being_edited"),
 ("a bracket of another kind closes", "src/syntax.rs", "                        Some((_, line)) => return (Some(line), None),", "                        Some((_, _)) => {}", "--test replay"),
 ("the parser's lines not blanked", "src/query.rs", "                    match parse_error_line(&blank(text, &lines)) {", "                    match Some(0).filter(|_| false).or(parse_error_line(\"\")) {", "--test replay"),
 ("no answers before the edit", "src/query.rs", "        } else if self.repaired && touching.iter().all(|e| e.body) {", "        } else if false {", "--test repaired a_method_being_edited"),
 ("answers before an edit in an older check", "src/query.rs", "        } else if self.repaired && touching.iter().all(|e| e.body) {", "        } else if touching.iter().all(|e| e.body) {", "--test stale an_older_check"),
 ("unfinished types before the edit count", "src/query.rs", "        if limit < usize::MAX && types.iter().any(|(_, complete)| !complete) {", "        if limit < usize::MAX && types.iter().any(|(_, complete)| !complete) && false {", "--test repaired a_type_the_blank"),
 ("placeholders in type arguments count as inferred", "src/walker.rs", "            TypeEnum::TypeInstance(ins) => arguments(ins.type_arguments(db)),", "            TypeEnum::TypeInstance(_) => true,", "--test repaired a_type_the_blank"),
 ("completion's placeholder is an error", "src/query.rs", "            .filter(|(start, end, ..)| !placeholders.iter().any(|p| start <= p && p < end))", "            .filter(|_| true)", "--test repaired an_edit"),
 ("declaration errors aren't repaired", "src/query.rs", "            candidates.push(declarations);", "            let _ = declarations;", "--test repaired a_changed_declaration"),
 ("body changes blanked for declaration errors", "src/query.rs", "            if !effect(outline, old, &new, &c).body {", "            if !effect(outline, old, &new, &c).body || true {", "--test repaired a_changed_declaration"),
 ("blank lines are separate changes", "src/query.rs", "                Some((_, last)) if *last + 1 == *line => *last = *line,", "                Some((_, last)) if *last + 1 == *line && false => *last = *line,", "--test repaired a_changed_declaration"),
 ("whole items change what they name", "src/units.rs", "                Some(declared) => effect.names.extend(declared),", "                Some(_) => effect.prefixes.extend(names(text).map(String::from)),", "--lib units"),
 ("no repaired status", "src/lsp.rs", "                params[\"repaired\"] = json!(true);", "                let _ = json!(true);", "--test repaired an_edit"),
 ("the norepair fault ignored", "src/worker.rs", "    return fault(job).as_deref() != Some(\"norepair\");", "    return fault(job).is_some() || true;", "--test stale read_only"),
 # Stale answers over LSP.
 ("no stale status", "src/lsp.rs", "                    (Ok(()), Source::Stale) => \"stale\",", "                    (Ok(()), Source::Stale) => \"fresh\",", "--test stale read_only"),
 ("no fallback sent", "src/lsp.rs", "        self.snapshots.fallback(\n            file,\n            query::diagnostics_target(&self.session, file).as_deref(),\n        )\n", "        None\n", "--test stale read_only"),
 ("no fallback from the target's check", "src/lsp.rs", "            query::diagnostics_target(&self.session, file).as_deref(),\n        )\n", "            None,\n        )\n", "--test stale unsaved_text"),
 ("diagnostics checks not kept for their target", "src/lsp.rs", "                    .chain([target.clone()])\n", "", "--test stale unsaved_text"),
 ("edits not recorded", "src/lsp.rs", "                                self.snapshots.edited(path, &before, start, end, len);", "                                let _ = (path, &before, start, end, len);", "--test stale read_only"),
 ("whole-text changes not recorded", "src/lsp.rs", "        let edit = crate::edits::difference(text, &change.text);", "        let edit = (0, 0, 0);", "--test stale a_whole_text_change"),
 ("documents opened later not recorded", "src/lsp.rs", "                        self.snapshots\n                            .opened(path, disk.as_deref(), &p.text_document.text);", "                        let _ = (path, disk.as_deref(), &p.text_document.text);", "--test stale unsaved_text"),
 ("diagnostics checks aren't good checks", "src/lsp.rs", "            self.snapshots.finished(done.id, &checked);", "            self.snapshots.finished(done.id, &[]);", "--test stale a_diagnostics_check"),
 ("queries aren't good checks", "src/lsp.rs", "        self.snapshots.finished(done.id, file.as_slice());", "        self.snapshots.finished(done.id, &[]);", "--test stale read_only"),
 ("versions aren't bounded", "src/edits.rs", "        while self.versions.len() > MAX_VERSIONS {", "        while self.versions.len() > usize::MAX {", "--lib snapshots"),
 ("unused versions kept", "src/edits.rs", "        self.versions.retain(|(id, _)| used.contains(id));", "", "--lib snapshots"),
 ("running checks aren't bounded", "src/edits.rs", "        while self.running.len() >= MAX_RUNNING {", "        while self.running.len() >= usize::MAX {", "--lib snapshots"),
 # Kept checks (milestone 6).
 ("kept checks used whatever changed on disk", "src/query.rs", "            if entry.fingerprint.current() {", "            if true {", "--test cache a_file_changed_on_disk"),
 ("kept checks found by target but not text", "src/query.rs", "list.iter().position(|k| k.target == *target)", "list.iter().position(|k| k.target.root == target.root && k.target.file == target.file)", "--test cache an_edit"),
 ("fingerprints leave out directories", "src/check.rs", "            directories(&dir, sources, &mut dirs, &mut paths);", "            let _ = (&dir, sources);", "--test cache a_file_created"),
 ("fingerprints compare nothing", "src/check.rs", "self.stamps.iter().all(|(p, s)| s.unchanged(p))", "self.stamps.iter().all(|(p, _)| p.exists())", "--test cache a_file_changed_on_disk"),
 ("fingerprints leave out the change time", "src/check.rs", "(meta.ctime(), meta.ctime_nsec(), meta.ino(), meta.dev())", "(0, 0, 0, 0)", "--test cache a_check_that_read"),
 ("checks kept while what they read changed", "src/query.rs", "        if keep && self.capacity > 0 && fingerprint.settled() {", "        if keep && self.capacity > 0 {", "--test cache a_check_of_a_file_that_may"),
 ("settled ignores the start", "src/check.rs", "                .is_none_or(|latest| before(latest, self.started))", "                .is_none_or(|latest| before(latest, self.started + Duration::from_secs(3600)))", "--test cache a_check_that_read"),
 ("coarse clocks get no margin", "src/check.rs", "let margin = if fine { Duration::ZERO } else { COARSE };", "let margin = if fine || true { Duration::ZERO } else { COARSE };", "--lib coarse"),
 ("the cache keeps more than its capacity", "src/query.rs", "            if list.len() > capacity {", "            if list.len() > capacity + 1 {", "--test cache the_cache_keeps"),
 ("kept checks not moved to the end when used", "src/query.rs", "                    list.push(entry);\n                    return (result, true);", "                    list.insert(0, entry);\n                    return (result, true);", "--test cache the_cache_keeps"),
 # Cancelled checks run on in their worker (milestone 9).
 ("cancelling kills the running check", "src/lsp.rs", "            self.abandoned = Some(Abandoned {\n                check,\n                waiting: None,\n            });", "            check.kill();", "--test lsp a_cancelled_check_is_kept"),
 ("a waiting query never kills a cancelled check", "src/lsp.rs", "                if now < waiting + ABANDONED_GRACE {", "                if true {", "--test lsp cancelling_a_running_check_answers"),
 ("a cancelled check killed when a query is ready", "src/lsp.rs", "                if now < waiting + ABANDONED_GRACE {", "                if false {", "--test lsp a_cancelled_check_is_kept"),
 ("no wake-up at the end of the grace", "src/lsp.rs", "                    Some(at) => timeout.min(at + ABANDONED_GRACE),", "                    Some(_) => timeout,", "--test lsp cancelling_a_running_check_answers"),
 ("cancelled checks never time out", "src/lsp.rs", "            .take_if(|a| a.check.started.elapsed() >= self.options.timeout)", "            .take_if(|_| false)", "--test lsp a_cancelled_check_that_hangs"),
 ("diagnostics start beside a cancelled check", "src/lsp.rs", "            || self.abandoned.is_some()\n            || !self.queue.is_empty()", "            || !self.queue.is_empty()", "--test lsp diagnostics_wait_for"),
 ("a cancelled check's worker not kept", "src/lsp.rs", "            let (_, worker) = abandoned.check.finish(&done);\n\n            self.idle = worker;", "            let (_, worker) = abandoned.check.finish(&done);\n\n            drop(worker);", "--test lsp a_cancelled_check_is_kept"),
 ("shutdown leaves a cancelled check", "src/lsp.rs", "        if let Some(abandoned) = self.abandoned.take() {\n            abandoned.check.kill();\n        }\n", "", "--test lsp shutdown_kills_a_cancelled"),
 # Checks with nothing to answer from (milestone 9).
 ("failures not kept", "src/query.rs", "        Ok(Err(Ok((reason, fingerprint)))) => Ok((Err(reason), fingerprint)),", "        Ok(Err(Ok((reason, _)))) => Err(reason),", "--test cache a_check_with_nothing"),
 ("failures' fingerprints leave out source files", "src/check.rs", "            || (sources && path.extension().is_some_and(|e| e == \"inko\"))", "            || (sources && path.extension().is_some_and(|e| e == \"none\"))", "--test cache a_module_fixed_on_disk"),
 ("failures kept with the checks", "src/query.rs", "                Err(_) => (&mut *failed, KEPT_FAILURES),", "                Err(_) => (&mut *kept, self.capacity),", "--test cache failures_dont_push_out"),
 ("failures kept without a bound", "src/query.rs", "                Err(_) => (&mut *failed, KEPT_FAILURES),", "                Err(_) => (&mut *failed, usize::MAX),", "--test cache failures_dont_push_out"),
 ("no answer cached though a repaired copy was checked", "src/query.rs", "                // Still no answer: a lookup only if this check was one too.\n                cached[*i] &= kept;", "                // Still no answer.", "--test cache an_answer_with_no_result"),
 ("no answer cached though the last good check was checked", "src/query.rs", "                } else {\n                    cached[*i] &= kept;\n                }", "                }", "--test cache an_answer_with_no_result"),
 ("completion's copies kept", "src/query.rs", "                    keep: !changed,", "                    keep: true,", "--test cache completions_copies"),
 ("repaired answers marked by the fresh check", "src/query.rs", "                sources[*i] = Source::Repaired;\n                cached[*i] = kept;", "                sources[*i] = Source::Repaired;", "--test cache repaired_and_stale"),
 ("stale answers marked by the fresh check", "src/query.rs", "                    sources[*i] = Source::Stale;\n                    cached[*i] = kept;", "                    sources[*i] = Source::Stale;", "--test cache repaired_and_stale"),
 ("no cached status", "src/lsp.rs", "            if cached {\n                params[\"cached\"]", "            if false {\n                params[\"cached\"]", "--test cache kept_answers_are_marked"),
 ("the queries' worker not kept", "src/lsp.rs", "        let (outcome, worker) = check.finish(&done);\n\n        self.idle = worker;\n", "        let (outcome, worker) = check.finish(&done);\n\n        drop(worker);\n", "--test cache kept_answers_are_marked"),
 ("workers keep no checks", "src/worker.rs", "    let cache = Cache::new(kept_checks());", "    let cache = Cache::new(0);", "--test cache kept_answers_are_marked"),
 ("the tests can't turn kept checks off", "src/worker.rs", "    if std::env::var_os(NO_CACHE_VARIABLE).is_some() {\n        return 0;", "    if std::env::var_os(NO_CACHE_VARIABLE).is_some() {\n        return query::KEPT_CHECKS;", "--test cache the_tests_can_turn"),
 ("shutdown leaves the waiting worker", "src/lsp.rs", "        if let Some(worker) = self.idle.take() {\n            worker.kill();\n        }\n", "", "--test cache shutdown_stops"),
 ("a dead waiting worker sent a job", "src/worker.rs", "        if !sent {\n            self.kill();", "        if false {\n            self.kill();", "--test cache a_waiting_worker_that_died"),
 ("the tests' switch for kept checks in release builds", "src/worker.rs", "    #[cfg(feature = \"fault-injection\")]\n    if std::env::var_os(NO_CACHE_VARIABLE).is_some() {", "    if std::env::var_os(\"CUTTLEBONE_NO_CACHE\").is_some() {", "sh:scripts/check-release.sh"),
 # The tiers (milestone 6).
 ("the whole corpus replays the sample", "tests/replay.rs", "            if full {\n                return Some(s);\n            }\n", "", "--test replay"),
 ("the milestone tier ignores changed snippets' files", "scripts/mutate.py", "        if path in changed:\n            why = \"changed \" + path", "        if False:\n            why = \"changed \" + path", "sh:python3 scripts/check-selection.py"),
 ("no worker deadline between jobs", "src/worker.rs", "        let _ = deadlines.send(job.deadline.map(|d| Instant::now() + d));", "        let _ = deadlines.send(None::<Instant>);", "--test lsp a_hung_worker"),
 # Errors in declarations (milestone 8).
 ("patch: the passes stop at the first error", ".upstream/inko/compiler/src/compiler.rs", "        let all = state.config.check_after_errors;", "        let all = false;", "--test broken a_declaration_error_in_an_import"),
 ("patch: a check after errors succeeds", ".upstream/inko/compiler/src/compiler.rs", "        if res && !state.diagnostics.has_errors() {", "        if res {", "--test broken every_pass_runs"),
 ("patch: the hook gets no diagnostics", ".upstream/inko/compiler/src/compiler.rs", "        if !inspect(&self.state.db, &hir, &self.state.diagnostics.iter().collect::<Vec<_>>()) {", "        if !inspect(&self.state.db, &hir, &[]) {", "--test broken a_declaration_error_in_an_import"),
 ("patch: an unresolved impl not skipped", ".upstream/inko/compiler/src/type_check/define_types.rs", "        let (Some(type_ins), Some(trait_ins)) =\n            (node.type_instance, node.trait_instance)\n        else {\n            return;\n        };\n", "        let type_ins = node.type_instance.unwrap();\n        let trait_ins = node.trait_instance.unwrap();\n", "--test broken what_an_error_left"),
 ("patch: an unresolved impl not skipped later", ".upstream/inko/compiler/src/type_check/methods.rs", "        let (Some(trait_ins), Some(type_ins)) =\n            (node.trait_instance, node.type_instance)\n        else {\n            return;\n        };\n", "        let trait_ins = node.trait_instance.unwrap();\n        let type_ins = node.type_instance.unwrap();\n", "--test broken what_an_error_left"),
 ("patch: an undefined method not skipped", ".upstream/inko/compiler/src/type_check/expressions.rs", "        let Some(method) = node.method_id else { return };", "        let method = node.method_id.unwrap();", "--test broken what_an_error_left"),
 ("patch: an unresolved required trait not skipped", ".upstream/inko/compiler/src/type_check/expressions.rs", "            let Some(req_ins) = req.resolved_type.as_trait_instance(self.db())\n            else {\n                continue;\n            };\n", "            let req_ins = req.resolved_type.as_trait_instance(self.db()).unwrap();\n", "--test broken what_an_error_left"),
 ("diagnostics run every pass", "src/check.rs", "    config.exclude = exclude.to_vec();", "    config.exclude = exclude.to_vec();\n    config.check_after_errors = true;", "--test diagnostics"),
 ("queries stop at the first error", "src/check.rs", "    run_with(target, false, false, true, inspect)", "    run_with(target, false, false, false, inspect)", "--test broken a_declaration_error_in_an_import"),
 ("queries get no errors", "src/check.rs", "            .filter(|d| d.is_error())\n", "            .filter(|d| d.is_error() && false)\n", "--test broken a_declaration_error_in_an_import"),
 ("body errors count as declarations'", "src/units.rs", "        return ErrorEffect::Body;\n", "        let _ = ErrorEffect::Body;\n", "--test broken an_error_in_a_body"),
 ("an error's item names not counted", "src/units.rs", "        let mut named = i.names.clone();\n", "        let mut named = Vec::new();\n", "--test replay"),
 ("an error's import names not counted", "src/units.rs", "        if i.kind == Kind::Import {\n            named.extend(import_names", "        if i.kind == Kind::Import && false {\n            named.extend(import_names", "--lib units"),
 ("declaration errors not guarded", "src/query.rs", "        if !checked.guarded() {", "        if true {", "--test broken a_declaration_error_in_an_import"),
 ("a guarded check is a good check", "src/query.rs", "        Ok((results, guarded)) => (results, !guarded, guarded, kept),", "        Ok((results, guarded)) => (results, true, guarded, kept),", "--test broken a_declaration_error_in_an_import"),
 ("guarded answers not marked", "src/query.rs", "            Ok(_) if guarded => Source::Guarded,", "            Ok(_) if false => Source::Guarded,", "--test broken a_declaration_error_in_an_import"),
 ("references hidden by a declaration error", "src/query.rs", "                        && hidden(&scope, &guards, *question)", "                        && false", "--test broken references_are_given"),
 ("hidden references looked for in the file only", "src/query.rs", "                        .filter(|f| *f == file || scope.in_project(f))", "                        .filter(|f| *f == file)", "--test broken references_are_given"),
 ("operators aren't names", "src/query.rs", "        if names(text).next().is_none() {\n            return !self.changes.is_none() || self.mentioned_unsafely(text);\n        }\n", "", "--test broken an_operator_used"),
 ("operators in changed text", "src/query.rs", "            return !self.changes.is_none() || self.mentioned_unsafely(text);", "            return self.mentioned_unsafely(text);", "--test stale an_operator_typed"),
 ("operators looked for as names", "src/query.rs", "        if names(name).next().is_none() {\n            return texts().iter().any(|text| text.contains(name));", "        if false {\n            return texts().iter().any(|text| text.contains(name));", "--test broken an_operator_used"),
 ("changes since the last good check not counted", "src/query.rs", "        errors.extend(changed.ok_or_else(declaration_errors)?);", "        let _ = changed;", "--test broken what_changed"),
 ("a panic after an error not checked again", "src/query.rs", "                    Err(_) => stopping(),", "                    Err(e) => Err(e),", "--test broken a_panic"),
 ("the stop fault ignored", "src/worker.rs", "        Some((_, suffix)) => cache.stopping_at_errors(suffix),", "        Some(_) => cache,", "--test repaired a_changed_declaration"),
 # Document symbols, from the syntax alone (milestone 12).
 ("outlines of text that doesn't parse not repaired", "src/syntax.rs", "    let (unclosed, misclosed) = unclosed_or_misclosed(text);", "    if !text.is_empty() {\n        return None;\n    }\n    let (unclosed, misclosed) = unclosed_or_misclosed(text);", "--test symbols document_symbols_in_code"),
 ("outlines never blank the line before the parser's", "src/syntax.rs", "        let Some((chosen, next)) = [before, Some(line)]", "        let Some((chosen, next)) = [None, Some(line)]", "--lib syntax"),
 ("outlines blank the line before the parser's on a tie", "src/syntax.rs", "        let Some((chosen, next)) = [before, Some(line)]", "        let Some((chosen, next)) = [Some(line), before]", "--test symbols document_symbols_in_code"),
 ("outlines keep signatures with blank lines", "src/syntax.rs", "                    .without_blank_signatures(text, &lines)\n", "", "--lib syntax"),
 ("outlines' nesting not compared with the text's", "src/syntax.rs", "                    .nested_as_in(text, &copy, &lines)", "", "--test replay"),
 ("outlines compare depths in the copy only", "src/syntax.rs", "                    (Some(a), Some(b)) => a - o == b - c,", "                    (Some(_), Some(_)) => true,", "--lib syntax"),
 ("outlines ignore a deeper start in the text", "src/syntax.rs", "                    || o <= c\n", "                    || true\n", "--lib syntax"),
 ("outlines ignore a blank container's header", "src/syntax.rs", "            header.is_none_or(|h| first < h)", "            header.is_none_or(|_| true)", "--lib syntax"),
 ("outlines of copies not moved to the text", "src/syntax.rs", "                ..o.moved(&copy, text)", "                ..o", "--test symbols document_symbols_in_code"),
 ("doc comments not read", "src/syntax.rs", "        ast::parser::Parser::with_comments(text.as_bytes().to_vec(), PathBuf::from(\"outline\"))", "        ast::parser::Parser::new(text.as_bytes().to_vec(), PathBuf::from(\"outline\"))", "--test syntax doc_comments"),
 ("doc comments across a blank line", "src/syntax.rs", "        if end.wrapping_sub(last) > 1 && !self.nodes.is_empty() {", "        if end.wrapping_sub(last) > 2 && !self.nodes.is_empty() {", "--lib syntax"),
 ("doc comments a line away", "src/syntax.rs", "            .is_some_and(|(_, end, _)| location.line_start.wrapping_sub(*end) == 1);", "            .is_some_and(|(_, end, _)| location.line_start.wrapping_sub(*end) <= 2);", "--lib syntax"),
 ("a module's doc comment after a blank line", "src/syntax.rs", "        && c.location.line_start.wrapping_sub(last) == 1", "        && c.location.line_start.wrapping_sub(last) >= 1", "--test syntax doc_comments"),
 ("an outline's constructor without members located at its case", "src/syntax.rs", "            r.start..r.end.max(name_range.end)", "            r", "--test symbols document_symbols_in_utf"),
 ("signatures with their bodies", "src/syntax.rs", "        let end = body.map_or(range.end, |b| self.bytes(b).start.max(range.start));", "        let end = range.end.max(body.map_or(0, |_| 0));", "--test symbols document_symbols_in_utf"),
 ("signatures on one line keep trailing commas", "src/syntax.rs", "        if line.starts_with([')', ']']) && out.ends_with(',') {\n            out.pop();\n        }\n", "", "--lib syntax"),
 ("signatures keep their indentation", "src/syntax.rs", "                l.strip_prefix(indent).unwrap_or(l)", "                l", "--lib syntax"),
 ("outlines miss the compiler's declarations", "src/syntax.rs", "                            Kind::Constructor,\n", "                            Kind::Field,\n", "--test syntax outlines_have"),
 ("document symbols not advertised", "src/lsp.rs", "                document_symbol_provider: Some(lsp::OneOf::Left(true)),", "                document_symbol_provider: None,", "--test symbols document_symbols_are_advertised"),
 ("hierarchical document symbols for every client", "src/lsp.rs", "        if !self.support.hierarchical_symbols {", "        if false {", "--test symbols document_symbols_for_a_client_without"),
 ("newer symbol kinds for every client", "src/lsp.rs", "        Kind::Implementation if support.symbol_kinds => lsp::SymbolKind::OBJECT,", "        Kind::Implementation if true => lsp::SymbolKind::OBJECT,", "--test symbols document_symbols_for_a_client_without"),
 ("newer symbol kinds for no client", "src/lsp.rs", "        Kind::Constructor if support.symbol_kinds => lsp::SymbolKind::ENUM_MEMBER,", "        Kind::Constructor if false => lsp::SymbolKind::ENUM_MEMBER,", "--test symbols document_symbols_in_utf"),
 ("flat document symbols without containers", "src/lsp.rs", "                    if let Some(parent) = parents.last() {\n                        info[\"containerName\"] = json!(parent.name);\n                    }\n", "", "--test symbols document_symbols_for_a_client_without"),
 ("document symbols read from disk", "src/lsp.rs", "            (Some(document), _) => Ok(document.text.clone()),", "            (Some(_), Ok(path)) => std::fs::read_to_string(&path)\n                .map_err(|e| Unavailable::new(\"fileNotFound\", e.to_string())),\n            (Some(document), _) => Ok(document.text.clone()),", "--test symbols document_symbols_in_code"),
 ("document symbols not repaired in the status", "src/lsp.rs", "            Ok(true) => params[\"repaired\"] = json!(true),", "            Ok(true) => {}", "--test symbols document_symbols_in_code"),
 ("document symbols when nothing parses", "src/lsp.rs", "                .ok_or_else(|| {\n                    Unavailable::new(\n                        \"noHir\",\n                        \"the document doesn't parse, nor any copy", "                .or_else(|| (!text.is_empty()).then(Default::default))\n                .ok_or_else(|| {\n                    Unavailable::new(\n                        \"noHir\",\n                        \"the document doesn't parse, nor any copy", "--test symbols document_symbols_of_a_document"),
 # Workspace symbols, from an index of every module (milestone 12).
 ("workspace symbols not advertised", "src/lsp.rs", "                workspace_symbol_provider: Some(lsp::OneOf::Left(true)),", "                workspace_symbol_provider: None,", "--test symbols workspace_symbols_for_a_client"),
 ("workspace symbols match the case", "src/index.rs", "    let lower = |s: &str| s.to_lowercase();", "    let lower = |s: &str| s.to_string();", "--lib ascii_names_match"),
 ("workspace symbols match runs only", "src/index.rs", "    let mut chars = n.char_indices();\n    let mut first = None;", "    if !q.is_empty() {\n        return None;\n    }\n    let mut chars = n.char_indices();\n    let mut first = None;", "--lib ascii_names_match"),
 ("workspace symbols ignore where they're from", "src/index.rs", "                .then_with(|| rank(x).cmp(&rank(y)))\n", "", "--test symbols workspace_symbols_from"),
 ("workspace symbols' files never read again", "src/index.rs", "                && *kept == stamp\n", "                && true\n", "--test symbols workspace_symbols_in_code"),
 ("workspace symbols keep deleted files", "src/index.rs", "        self.modules.retain(|path, _| seen.contains_key(path));\n", "", "--test symbols workspace_symbols_in_code"),
 ("workspace symbols ignore open documents", "src/lsp.rs", "        self.index.refresh(&roots, &open);", "        self.index.refresh(&roots, &HashMap::new());\n        let _ = open;", "--test symbols workspace_symbols_in_code"),
 ("workspace symbols without dependencies", "src/index.rs", "            for dep in manifest.into_dependencies() {", "            for dep in manifest.into_dependencies().into_iter().filter(|_| false) {", "--test symbols workspace_symbols_from"),
 ("workspace symbols create a missing manifest", "src/index.rs", "        if manifest.is_file()\n            && let Ok(manifest)", "        if true\n            && let Ok(manifest)", "--test symbols workspace_symbols_for_a_client"),
 ("workspace symbols without tests", "src/index.rs", "        out.push(Root {\n            dir: project.join(\"test\"),", "        out.push(Root {\n            dir: project.join(\"\\0test\"),", "--test symbols workspace_symbols_from"),
 ("document symbols from the index's older text", "src/lsp.rs", "                .filter(|m| m.text == text)\n", "                .filter(|_| true)\n", "--test symbols document_symbols_after_the_index"),
 ("builtin types as their declarations say", "src/syntax.rs", "                    ast::nodes::TypeKind::Builtin => t.name.name != \"CheckedIntResult\",", "                    ast::nodes::TypeKind::Builtin => t.public,", "--test names symbols_to_import"),
 ("every builtin type public", "src/syntax.rs", "                    ast::nodes::TypeKind::Builtin => t.name.name != \"CheckedIntResult\",", "                    ast::nodes::TypeKind::Builtin => true,", "--test names symbols_to_import"),
 ("modules aren't workspace symbols", "src/index.rs", "            if let Some(score) = matches(query, &module.name) {", "            if let Some(score) = None::<Score> {", "--test symbols workspace_symbols_of_modules"),
 ("only modules that parse are workspace symbols", "src/index.rs", "            if let Some(score) = matches(query, &module.name) {", "            if let Some(score) = matches(query, &module.name).filter(|_| module.outline.is_some()) {", "--test symbols workspace_symbols_of_modules"),
 ("a module's workspace symbol is a file", "src/lsp.rs", "                        \"kind\": lsp::SymbolKind::MODULE,", "                        \"kind\": lsp::SymbolKind::FILE,", "--test symbols workspace_symbols_of_modules"),
 ("a module's workspace symbol named by its file", "src/lsp.rs", "                        \"name\": f.module.name,", "                        \"name\": f.module.name.rsplit('.').next(),", "--test symbols workspace_symbols_of_modules"),
 ("newer workspace symbol kinds for every client", "src/lsp.rs", "            symbol_kinds: self.support.workspace_symbol_kinds,", "            symbol_kinds: true,", "--test symbols workspace_symbols_for_a_client"),
 ("workspace symbols at the whole declaration", "src/lsp.rs", "                let r = &f.declaration.name_range;", "                let r = &f.declaration.range;", "--test symbols workspace_symbols_from"),
 ("workspace symbols' containers without the module", "src/index.rs", "        let mut out = self.module.name.clone();", "        let mut out = String::new();", "--test symbols workspace_symbols_from"),
 # Hover with declarations and doc comments (milestone 12).
 ("hover shows no declaration", "src/query.rs", "    let declared = name.and_then(|r| declared(scope, r.symbol));", "    let declared = name.and_then(|_| None::<Declared>);", "--test hover hovers_show"),
 ("no hover on a declaration's own name", "src/query.rs", "        (None, Some(r)) if declared.is_some() => range(scope.lines, &r.location),", "        (None, Some(r)) if declared.is_some() && false => range(scope.lines, &r.location),", "--test hover hovers_show"),
 ("implementations don't inherit doc comments", "src/query.rs", "        && original != id\n", "        && original == id\n", "--test hover hovers_show"),
 ("declarations' paths without what they're in", "src/query.rs", "        path.push('.');\n        path.push_str(p.owner());", "        let _ = p;", "--test hover hovers_show"),
 ("no hover on a module", "src/query.rs", "    if let SymbolId::Module(_) = symbol {", "    if let SymbolId::Module(_) = symbol\n        && false\n    {", "--test hover hovers_show"),
 ("a stale hover's declaration not checked", "src/query.rs", "                            .map(|p| self.place(p))\n                            .collect::<Option<_>>()?,\n                        ..d", "                            .map(Some)\n                            .collect::<Option<_>>()?,\n                        ..d", "--test hover a_stale_hover"),
 ("hover sections in plain text as in Markdown", "src/lsp.rs", "            value: sections.join(if markdown { \"\\n\\n---\\n\\n\" } else { \"\\n\\n\" }),", "            value: sections.join(\"\\n\\n---\\n\\n\"),", "--test hover hovers_in_plain"),
 ("an impl block's owner is its trait", "src/syntax.rs", "        let typ = rest.split_once(\" for \").map_or(rest, |(_, t)| t);", "        let typ = rest;", "--test hover hovers_show"),
 ("hover reads no doc comment's text", "src/syntax.rs", "            header: from.unwrap_or(range.start)..end,", "            header: range.start..end,", "--test hover a_stale_hover"),
 # Completion of names (milestone 12).
 ("names never completed", "src/query.rs", "            None => names_in(scope)?,", "            None => None,", "--test names names_in_scope"),
 ("names completed in comments", "src/query.rs", "        .any(|l| (l.line_start, l.column_start) == first)", "        .any(|l| (l.line_start, l.column_start) == first || true)", "--test names names_in_scope"),
 ("variables in scope everywhere", "src/query.rs", "            .filter(|v| v.from < here && here <= v.to)", "            .filter(|_| true)", "--test names names_in_scope"),
 ("variables in scope before their let", "src/query.rs", "            .filter(|v| v.from < here && here <= v.to)", "            .filter(|v| here <= v.to)", "--test names names_in_scope"),
 ("self's methods not names", "src/query.rs", "            for m in members(db, id.receiver(db), receiver, module, true) {", "            for m in members(db, id.receiver(db), receiver, module, true).into_iter().filter(|_| false) {", "--test names names_in_scope"),
 ("fields are names", "src/query.rs", "                if m.kind != MemberKind::Field {", "                if true {", "--test names names_in_scope"),
 ("types are names anywhere", "src/query.rs", "    let types = !field && walker.type_names.iter().any(|l| contains(l, at));", "    let types = false && walker.type_names.iter().any(|l| contains(l, at));", "--test names types_in"),
 ("a placeholder's own error counts", "src/query.rs", "        errors.retain(|(path, e, _)| {", "        errors.retain(|(path, e, _)| true || {", "--test names types_in"),
 ("names from a copy with a declaration blank", "src/query.rs", "        (_, Some(Reply::Members(m))) if m.names.is_some() && !guard.names_hold(&m) => None,", "        (_, Some(Reply::Members(m))) if m.names.is_some() && false => None,", "--test names names_in_code"),
 ("type names never past an unsafe unit", "src/query.rs", "                if m.names.as_ref().is_some_and(|n| n.types) && guard.names_hold(&m) =>", "                if false =>", "--test names types_in"),
 ("a let's variables not in scope", "src/walker.rs", "                    self.see(before, (n.location.line_end, n.location.column_end), end);", "                    let _ = (before, end);", "--test names names_in_scope"),
 ("a closure's arguments not in scope", "src/walker.rs", "                        self.visible.push(Visible {\n                            variable: a.variable,\n                            from: (n.location.line_start, n.location.column_start),\n                            to: (n.location.line_end, n.location.column_end),\n                        });\n", "", "--test names names_in_scope"),
 ("a case's bindings not in scope", "src/walker.rs", "                        self.see(\n                            before,", "                        let _ = before;\n                        let _ = (\n                            before,", "--test names names_in_scope"),
 ("a method's arguments not in scope", "src/walker.rs", "            for arg in id.arguments(self.db) {\n                self.visible.push(Visible {", "            for arg in id.arguments(self.db).into_iter().filter(|_| false) {\n                self.visible.push(Visible {", "--test names names_in_scope"),
 ("symbols to import with nothing typed", "src/lsp.rs", "        if names.field || names.prefix.is_empty() || !result[\"items\"].is_array() {", "        if names.field || !result[\"items\"].is_array() {", "--test names symbols_to_import"),
 ("symbols to import that are bound", "src/lsp.rs", "                if !d.public || !kind_fits || names.bound.contains(&d.name) {", "                if !d.public || !kind_fits {", "--test names symbols_to_import"),
 ("private symbols to import", "src/lsp.rs", "                if !d.public || !kind_fits || names.bound.contains(&d.name) {", "                if !kind_fits || names.bound.contains(&d.name) {", "--test names symbols_to_import"),
 ("symbols to import from any module", "src/lsp.rs", "            if module.file == file || !module.importable_from(&file) {\n                continue;\n            }\n\n            let Some(o)", "            if module.file == file {\n                continue;\n            }\n\n            let Some(o)", "--test names symbols_to_import"),
 ("names are complete", "src/lsp.rs", "    let incomplete = members.names.as_ref().is_some_and(|n| !n.field);", "    let incomplete = false && members.names.as_ref().is_some_and(|n| !n.field);", "--test names names_in_scope"),
 ("imports not completed", "src/lsp.rs", "        let Some(context) = syntax::import_context(&text[..offset]) else {", "        let Some(context) = None::<syntax::ImportContext> else {", "--test names modules_and_symbols"),
 ("listed symbols offered again", "src/lsp.rs", "                        if d.public && d.kind.importable() && !listed.contains(&d.name) {", "                        if d.public && d.kind.importable() {", "--test names modules_and_symbols"),
 ("self never offered in an import", "src/lsp.rs", "                    if !listed.iter().any(|l| l == \"self\") {", "                    if false {", "--test names modules_and_symbols"),
 ("test modules importable from src", "src/index.rs", "            Origin::Test(project) => importer.starts_with(project.join(\"test\")),", "            Origin::Test(project) => importer.starts_with(project),", "--test names symbols_to_import"),
 ("imported symbols appended", "src/syntax.rs", "            .find(|(name, _, _)| name != \"self\" && name.as_str() > symbol);", "            .find(|_| false);", "--lib syntax"),
 ("new imports last", "src/syntax.rs", "        let at = match group.iter().find(|i| i.path.join(\".\").as_str() > module) {", "        let at = match group.iter().find(|_| false) {", "--lib syntax"),
 ("new imports after a doc comment", "src/syntax.rs", "        .map(|(doc, at)| doc.unwrap_or(*at))", "        .map(|(_, at)| *at)", "--lib syntax"),
 ("many positions converted wrong", "src/position.rs", "        let line = self.starts.partition_point(|s| *s <= offset) - 1;", "        let line = self.starts.partition_point(|s| *s < offset).max(1) - 1;", "--lib positions_are"),
 # The release workflow's shards (milestone 13).
 ("shards skip a mutation", "scripts/mutate.py", "    k, n = shard\n    return [(m, why) for m, why in chosen if m[4] != DIST][k - 1::n]", "    k, n = shard\n    return [(m, why) for m, why in chosen if m[4] != DIST][k::n]", "sh:python3 scripts/check-selection.py"),
 ("the dist shard runs nothing", "scripts/mutate.py", "    if shard == \"dist\":\n        return [(m, why) for m, why in chosen if m[4] == DIST]", "    if shard == \"dist\":\n        return []", "sh:python3 scripts/check-selection.py"),
 # Folding ranges and formatting (milestone 13).
 ("whitespace breaks comment lines", "src/syntax.rs", "            T::Whitespace => continue,\n            _ => {}\n        }\n\n        let first", "            _ => {}\n        }\n\n        let first", "--lib comment_lines_in_a_body"),
 ("a comment after code is a comment line", "src/syntax.rs", "        if token.kind == T::Comment && first {", "        if token.kind == T::Comment {", "--lib a_comment_after_code"),
 ("one comment line folds", "src/syntax.rs", "    (last > from).then_some(Fold {", "    (last >= from).then_some(Fold {", "--lib one_line_folds_nothing"),
 ("imports don't fold", "src/syntax.rs", "    text[start..end].contains('\\n').then_some(Fold {", "    false.then_some(Fold {", "--test folding blocks_comments"),
 ("any bracket closes a block", "src/syntax.rs", "open.iter().rposition(|(k, _, _)| *k == opening)", "open.len().checked_sub(1)", "--test folding code_that_doesnt_parse"),
 ("two-line blocks fold", "src/syntax.rs", "                    if l.line_start >= from + 2 {", "                    if l.line_start > from {", "--lib one_line_folds_nothing"),
 ("a block's closing line folds", "src/lsp.rs", "                        syntax::FoldKind::Block => (last.saturating_sub(1), None),", "                        syntax::FoldKind::Block => (last, None),", "--test folding blocks_comments"),
 ("folding kinds for every client", "src/lsp.rs", "(last, self.support.comment_folds.then_some(\"comment\"))", "(last, Some(\"comment\"))", "--test folding folds_have_kinds"),
 ("no folding range limit", "src/lsp.rs", "ranges.len() == self.support.folding_limit.unwrap_or(usize::MAX)", "ranges.len() == usize::MAX", "--test folding folds_have_kinds"),
 ("folds on the compiler's lines", "src/lsp.rs", "                    let last = positions.at(fold.end).line;", "                    let last = text[..fold.end].matches('\\n').count() as u32;", "--test folding folds_are_on_lsps_lines"),
 ("folding ranges not advertised", "src/lsp.rs", "                folding_range_provider: Some(lsp::FoldingRangeProviderCapability::Simple(true)),", "                folding_range_provider: None,", "--test lsp initialize_advertises"),
 # Selection ranges.
 ("no bracket's inside selected", "src/syntax.rs", "            found.push(first.range.start..last.range.end);", "            let _ = (first, last);", "--test selection selection_ranges_grow"),
 ("no chain up to the name", "src/syntax.rs", "        found.push(units[from].range.start..end);", "        let _ = end;", "--lib selections_take"),
 ("a call's arguments not with its name", "src/syntax.rs", "            end = units[*i + 1].range.end;", "            end = units[*i].range.end;", "--lib selections_take"),
 ("no element of a list", "src/syntax.rs", "            Some(T::ParenOpen | T::BracketOpen) => element(units, *i),", "            Some(T::ParenOpen | T::BracketOpen) => (*i, *i),", "--test selection selection_ranges_grow"),
 ("statements end at every line", "src/syntax.rs", "            && !goes_on(a)\n", "\n", "--lib a_statement_goes_on"),
 ("any bracket closes a selection's", "src/syntax.rs", "            open.iter()\n                .rposition(|(o, _)| o.as_ref().is_some_and(|(k, _)| kinds.contains(k)))", "            open.len().checked_sub(1).filter(|_| kinds.len() < 9)", "--lib a_bracket_closes_only"),
 ("no declarations selected", "src/syntax.rs", "            found.push(declaration.clone());", "            let _ = declaration;", "--test selection every_declaration"),
 ("selections not nested", "src/syntax.rs", "            Some(last) if !(range.start <= last.start && last.end <= range.end) => {}\n", "", "--lib ranges_that_overlap"),
 ("selections repeat", "src/syntax.rs", "            Some(last) if *last == range => {}\n", "", "--test selection selection_ranges_grow"),
 ("selection positions in UTF-16 regardless", "src/lsp.rs", "byte_offset(&text, position, self.encoding).ok_or_else(", "byte_offset(&text, position, Encoding::Utf16).ok_or_else(", "--test selection selection_ranges_grow"),
 ("selection ranges not advertised", "src/lsp.rs", "                selection_range_provider: Some(lsp::SelectionRangeProviderCapability::Simple(true)),", "                selection_range_provider: None,", "--test lsp initialize_advertises"),
 # Implementations, declarations, the hierarchies, file renames.
 ("implementations of any method", "src/query.rs", "        SymbolId::Method(id) if trait_method(db, id) => implementing(db, id)", "        SymbolId::Method(id) => implementing(db, id)", "--test hierarchy implementations_of"),
 ("a trait's method implements itself", "src/query.rs", "        .filter(|m| matches!(m.source(db), MethodSource::Implemented(_, o) if o == method))", "        .filter(|m| *m == method)", "--test hierarchy implementations_of"),
 ("no implementing types", "src/query.rs", "        SymbolId::Trait(id) => id\n            .implemented_by(db)\n            .iter()\n            .map(|t| SymbolId::Type(*t))", "        SymbolId::Trait(id) => std::iter::once(id)\n            .map(SymbolId::Trait)", "--test hierarchy implementations_of"),
 ("outgoing calls from a repaired copy", "src/query.rs", "            !question.whole_project() && !matches!(question, Question::OutgoingCalls)", "            !question.whole_project()", "--test replay"),
 ("whole-project answers from a repaired copy", "src/query.rs", "            !question.whole_project() && !matches!(question, Question::OutgoingCalls)", "            !matches!(question, Question::References { .. } | Question::OutgoingCalls)", "--test hierarchy whole_project_answers"),
 ("implementations checked per file", "src/query.rs", "            Question::References { .. }\n                | Question::Implementation", "            Question::References { .. }", "--test hierarchy"),
 ("declaration is the definition", "src/query.rs", "                MethodSource::Implemented(_, original) => SymbolId::Method(original),", "                MethodSource::Implemented(_, _) => r.symbol,", "--test hierarchy declarations_of"),
 ("declaration not advertised", "src/lsp.rs", "        capabilities[\"declarationProvider\"] = json!(true);", "", "--test lsp initialize_advertises"),
 ("incoming calls in one group", "src/query.rs", "            match calls.iter_mut().find(|(m, _, _)| *m == from) {", "            match calls.iter_mut().find(|(m, _, _)| *m != from) {", "--test hierarchy call_hierarchies"),
 ("outgoing calls of the whole file", "src/query.rs", "        if caller(scope.walker, &r.location) != Some(method) {", "        if caller(scope.walker, &r.location).is_none() {", "--test hierarchy call_hierarchies"),
 ("an extern function's receiver", "src/query.rs", "            match receiver(db, id) {\n                Some(TypeEnum::Module(_)) | None", "            match Some(id.receiver_id(db)) {\n                Some(TypeEnum::Module(_)) | None", "--test hierarchy call_hierarchies"),
 ("module methods are methods", "src/query.rs", "                Some(TypeEnum::Module(_)) | None => ItemKind::Function,", "                Some(TypeEnum::Module(_)) | None => ItemKind::Method,", "--test hierarchy call_hierarchies"),
 ("an item's range is its name", "src/query.rs", "            range: whole,\n        },", "            range: text.location(&location, scope.encoding),\n        },", "--test hierarchy"),
 ("incoming and outgoing the same way", "src/lsp.rs", "                    Query::IncomingCalls => \"from\",", "                    Query::IncomingCalls => \"to\",", "--test hierarchy call_hierarchies"),
 ("no requiring traits are subtypes", "src/query.rs", ".any(|r| r.instance_of() == id))", ".any(|r| r.instance_of() != r.instance_of()))", "--test hierarchy type_hierarchies"),
 ("no implementing types are subtypes", "src/query.rs", "            .chain(id.implemented_by(db).iter().map(|t| SymbolId::Type(*t)))", "            .chain(std::iter::empty())", "--test hierarchy type_hierarchies"),
 ("supertypes of a type are what it requires", "src/query.rs", "        SymbolId::Type(id) => id\n            .implemented_traits(db)\n            .map(|i| i.instance.instance_of())\n            .collect(),", "        SymbolId::Type(_) => Vec::new(),", "--test hierarchy type_hierarchies"),
 ("an item found at any name", "src/query.rs", "    (file == scope.file && contains(&location, scope.at)).then_some(r.symbol)", "    Some(r.symbol)", "--test hierarchy type_hierarchies"),
 ("a hierarchy's item not at its name", "src/lsp.rs", "            params[\"textDocument\"] = json!({ \"uri\": uri });\n            params[\"position\"] = start;", "            params[\"textDocument\"] = json!({ \"uri\": uri });", "--test hierarchy"),
 ("aliases renamed", "src/query.rs", "            if text[start..end] == last {", "            if !text[start..end].is_empty() {", "--test rename_files renaming_a_module"),
 ("uses by name not renamed", "src/lsp.rs", "            } else if last(&m.old) != last(&m.new) {", "            } else if false {", "--test rename_files renaming_a_module"),
 ("directories not moved", "src/lsp.rs", "            let modules: Vec<(PathBuf, PathBuf)> = if old.is_dir() {", "            let modules: Vec<(PathBuf, PathBuf)> = if false {", "--test rename_files moving_a_module"),
 ("any file name is a module's", "src/lsp.rs", "    (!parts.is_empty() && parts.iter().all(valid)).then(|| parts.join(\".\"))", "    (!parts.is_empty()).then(|| parts.join(\".\"))", "--test rename_files moving_a_module"),
 ("import paths not renamed", "src/query.rs", "            .filter(|(_, m)| *m == Some(module))", "            .filter(|(_, m)| m.is_none())", "--test rename_files"),
 ("no check past a module that doesn't parse", "src/query.rs", "        if !diagnosis.panicked\n            && let Some(left_out) = unparsed(&target, &diagnosis.diagnostics)", "        if false\n            && let Some(left_out) = unparsed(&target, &diagnosis.diagnostics)", "--test diagnostics every_fixture"),
 ("importers of a module that doesn't parse checked", "src/query.rs", "        if more.is_empty() {\n            break;\n        }\n\n        left_out.extend(more);", "        if more.is_empty() || true {\n            break;\n        }\n\n        left_out.extend(more);", "--test diagnostics every_fixture"),
 ("left-out files take the second check's diagnostics", "src/query.rs", "            diagnosis.diagnostics.retain(|d| left_out.contains(&d.file));", "            diagnosis.diagnostics.retain(|d| !left_out.contains(&d.file));", "--test diagnostics every_fixture"),
 ("a check past a module that doesn't parse leaves nothing out", "src/check.rs", "    config.exclude = exclude.to_vec();", "    let _ = exclude;", "--test diagnostics every_fixture"),
 ("patch: excluded files checked", ".upstream/inko/compiler/src/compiler.rs", "                .filter(|(_, p)| !self.state.config.exclude.contains(p))", "                .filter(|(_, p)| !p.as_os_str().is_empty())", "--test diagnostics every_fixture"),
 ("a module not found doesn't stop the check", "src/query.rs", "            .any(|d| d.error && (d.id == \"invalid-syntax\" || d.id == \"invalid-file\"))", "            .any(|d| d.error && d.id == \"invalid-syntax\")", "--test diagnostics every_fixture"),
 ("guards only for modules naming none of the names", "src/query.rs", "                        if !wanted.iter().any(|n| text.contains(n.as_str())) {", "                        if wanted.iter().any(|n| text.contains(n.as_str())) {", "--test hierarchy whole_project_answers_past"),
 ("unsafe units' names not collected", "src/query.rs", "                    .flat_map(|text| names(text).map(str::to_string))", "                    .flat_map(|text| names(text).map(str::to_string).filter(|_| false))", "--test hierarchy whole_project_answers_past"),
 ("no supertypes lenses for types", "src/query.rs", "            (syntax::Kind::Type | syntax::Kind::Enum, _) => one(", "            (syntax::Kind::Type | syntax::Kind::Enum, _) if false => one(", "--test hierarchy code_lenses_are"),
 ("no lenses for a trait's methods", "src/query.rs", "            (syntax::Kind::Method, Some(syntax::Kind::Trait)) => one(", "            (syntax::Kind::Method, Some(syntax::Kind::Trait)) if false => one(", "--test hierarchy code_lenses_are"),
 ("no lens for an impl's trait", "src/query.rs", "                        questions: vec![(LensKind::Trait, Question::Definition, range.start)],", "                        questions: vec![(LensKind::Trait, Question::TypeDefinition, range.start)],", "--test hierarchy code_lenses"),
 ("an impl's methods' lenses without their trait", "src/query.rs", "                    format!(\"{}.{}\", name, d.name),", "                    d.name.clone(),", "--test hierarchy code_lenses_are"),
 ("a lens of no implementations", "src/query.rs", "                (LensKind::Implementations, Some(Reply::Places(ps))) if !ps.is_empty() => (", "                (LensKind::Implementations, Some(Reply::Places(ps))) => (", "--test hierarchy code_lenses_are"),
 ("lenses covered without an answer", "src/query.rs", "                Err(r) => {\n                    complete = false;", "                Err(r) => {\n                    complete = true;", "--test hierarchy code_lenses_past"),
 ("lenses' coverage not in the status", "src/lsp.rs", "            if let Query::InlayHints { .. } | Query::Lenses = pending.query {", "            if let Query::InlayHints { .. } = pending.query {", "--test hierarchy code_lenses"),
 ("lenses for every client", "src/lsp.rs", "        if self.support.code_lens {", "        if self.support.code_lens || true {", "--test hierarchy code_lenses_past"),
 ("the lenses' command shows nothing", "src/lsp.rs", "        if let (true, Some(location)) = (self.support.show_document, first) {", "        if let (false, Some(location)) = (self.support.show_document, first) {", "--test hierarchy code_lenses_past"),
 ("a server request unchecked", "tests/common/spec.rs", "                        if !supports(capabilities, \"window.showDocument.support\") =>", "                        if false =>", "--test conformance the_checks_catch"),
 ("any server request allowed", "tests/common/spec.rs", "                    _ => Err(format!(\"the server sent a request, {}\", name)),", "                    _ => Ok(()),", "--test conformance the_checks_catch"),
 ("whole-project answers refused past any declaration error", "src/query.rs", "                    if question.whole_project()\n                        && found.is_some()\n                        && hidden(&scope, &guards, *question)", "                    if question.whole_project()", "--test hierarchy whole_project_answers_past"),
 ("whole-project answers past any declaration error", "src/query.rs", "                    if question.whole_project()\n                        && found.is_some()\n                        && hidden(&scope, &guards, *question)", "                    if question.whole_project()\n                        && found.is_some()\n                        && matches!(question, Question::References { .. })\n                        && hidden(&scope, &guards, *question)", "--test hierarchy whole_project_answers_past"),
 ("a module's uses hidden by nothing", "src/query.rs", "        return Some(vec![name.rsplit('.').next().unwrap_or(&name).to_string()]);", "        return Some(Vec::new());", "--test hierarchy whole_project_answers_past"),
 ("a module's uses asked at its start's unit", "src/query.rs", "                        Question::ModuleUses => inspect(*question, &scope)?,", "                        Question::ModuleUses if false => inspect(*question, &scope)?,", "--test hierarchy whole_project_answers_past"),
 ("default methods not marked", "src/lsp.rs", "                    let detail = match in_trait && d.has_body() {", "                    let detail = match false && d.has_body() {", "--test symbols"),
 ("every method marked as default", "src/lsp.rs", "                    let detail = match in_trait && d.has_body() {", "                    let detail = match d.has_body() {", "--test symbols"),
 ("required methods have bodies", "src/syntax.rs", "        self.header.end < self.range.end", "        self.header.end <= self.range.end", "--test corpus_symbols"),
 ("no doc comment step in selections", "src/syntax.rs", "                    std::iter::once(d.range.clone()).chain(documented)", "                    std::iter::once(d.range.clone()).chain(documented.filter(|_| false))", "--test selection"),
 ("a trailing comment is a selection step", "src/syntax.rs", "(d.documentation.is_some() && from.starts_with('#'))", "(d.documentation.is_some())", "--test selection every_declaration"),
 ("statements cross declarations' edges", "src/syntax.rs", "    found.retain(|r| {\n        !declarations.iter().any(|d| {", "    found.retain(|r| {\n        !declarations.iter().any(|d| false && {", "--test selection every_declaration"),
 ("no status for a rename that edits nothing", "src/lsp.rs", "            if self.options.status {\n                let params = json!({\n                    \"id\": request.id,\n                    \"method\": \"workspace/willRenameFiles\",", "            if false {\n                let params = json!({\n                    \"id\": request.id,\n                    \"method\": \"workspace/willRenameFiles\",", "--test rename_files"),
 ("a rename's status names the first module", "src/lsp.rs", "            if let Some(files) = &pending.files {", "            if let Some(files) = pending.files.as_ref().filter(|_| false) {", "--test rename_files every_file_rename"),
 ("file renames for every client", "src/lsp.rs", "        if self.support.will_rename {", "        if !self.support.will_rename || self.support.will_rename {", "--test rename_files only_clients"),
 # Pull diagnostics.
 ("pushed to clients that pull", "src/lsp.rs", "            if let Ok(uri) = Url::from_file_path(&file)\n                && !self.support.pull", "            if let Ok(uri) = Url::from_file_path(&file)", "--test pull a_workspace_pull_has_what"),
 ("pulls offered to every client", "src/lsp.rs", "        if self.support.pull {\n            capabilities[\"diagnosticProvider\"]", "        if !self.support.pull || self.support.pull {\n            capabilities[\"diagnosticProvider\"]", "--test pull only_clients"),
 ("a pull answered from another text's check", "src/lsp.rs", "                    .is_some_and(|r| r.contents == contents)", "                    .is_some()", "--test pull a_pull_is_answered_from"),
 ("a pull never unchanged", "src/lsp.rs", "                    let report = if previous.as_ref() == Some(&result) {", "                    let report = if false {", "--test pull a_document_pull_follows"),
 ("a workspace pull never held", "src/lsp.rs", "                    if !previous.is_empty() && items.iter().all(|i| i[\"kind\"] == \"unchanged\") {", "                    if false {", "--test pull a_workspace_pull_with_nothing_new"),
 ("files a workspace pull no longer reports not cleared", "src/lsp.rs", "                    for uri in previous.keys() {\n                        files.entry(uri.clone()).or_default();\n                    }", "", "--test pull a_workspace_pull_has_every_file"),
 ("pulls not cancelled", "src/lsp.rs", "        if let Some(i) = self.diagnoser.waiting.iter().position(|p| p.id() == id) {", "        if let Some(i) = None::<usize> {", "--test pull a_workspace_pull_with_nothing_new"),
 ("pulls need diagnostics on", "src/lsp.rs", "        if !self.options.diagnostics && !self.support.pull {", "        if !self.options.diagnostics {", "--test pull only_clients"),
 ("a failed check leaves its pulls waiting", "src/lsp.rs", "                self.fail_pulls(connection, target, reason);", "", "--test pull a_pull_whose_check_fails"),
 ("formatting drops comments", "src/syntax.rs", "        ast::parser::Parser::with_comments(text.as_bytes().to_vec(), PathBuf::from(\"format.inko\"))", "        ast::parser::Parser::new(text.as_bytes().to_vec(), PathBuf::from(\"format.inko\"))", "--test formatting formatting_is_inko"),
 ("formatted text gets an edit", "src/lsp.rs", "            Ok((text, formatted)) if text == formatted => (json!([]), Ok(false)),\n", "", "--test formatting formatting_over_lsp"),
 ("formatting edits in UTF-16 regardless", "src/lsp.rs", "                let positions = Positions::new(&text, self.encoding);\n\n                (\n                    json!([{", "                let positions = Positions::new(&text, Encoding::Utf16);\n\n                (\n                    json!([{", "--test formatting formatting_over_lsp"),
 ("formatting not advertised", "src/lsp.rs", "                document_formatting_provider: Some(lsp::OneOf::Left(true)),", "                document_formatting_provider: None,", "--test lsp initialize_advertises"),
 # Quick fixes (milestone 13).
 ("fixes not checked", "src/query.rs", "                if !fixed_without_errors(&before, &after, &d) {", "                if false {", "--test actions no_fix_is_offered"),
 ("fixes need no error at all", "src/query.rs", "    after.iter().filter(|a| a.error).all(|a| {", "    after.iter().filter(|a| a.error).all(|_| false) && after.iter().filter(|a| a.error).all(|a| {", "--test actions fixes_are_offered_in_a_file"),
 ("fixes may add an error the file had", "src/query.rs", "        count(after, &k, true) <= count(before, &k, true).saturating_sub(less)", "        count(before, &k, true) > 0 || count(after, &k, true) <= less", "--lib a_fix_removes_its_diagnostic"),
 ("fixes may add one more of an error", "src/query.rs", "        let less = usize::from(k == fixed_key);", "        let less = usize::from(k == fixed_key) + 1;", "--lib a_fix_removes_its_diagnostic"),
 ("fixes may leave their diagnostic", "src/query.rs", "    if count(after, &fixed_key, false) >= count(before, &fixed_key, false) {", "    if count(after, &fixed_key, false) > count(before, &fixed_key, false) {", "--lib a_fix_removes_its_diagnostic"),
 ("import names only from the context", "src/lsp.rs", "                        && !names.iter().any(|n| n == w)", "                        && false", "--test actions imports_are_offered_without"),
 ("import names from the range only, not its words", "src/lsp.rs", "                let start = text[..start].rfind(|c| !word(c)).map_or(0, |i| i + 1);", "", "--test actions imports_are_offered_without"),
 ("one fix only", "src/query.rs", "                if tried == MAX_FIXES {", "                if tried == 1 {", "--test actions several_imports"),
 ("fixes outside the range", "src/query.rs", "                    && d.range.start <= asked.end\n                    && asked.start <= d.range.end", "", "--test actions unused_imports"),
 ("a listed symbol removes its import", "src/query.rs", "                    Some(i) if listed.len() > 1 => {", "                    Some(i) if false => {", "--test actions unused_imports"),
 ("unused variables not renamed", "src/query.rs", "            if text[start..].starts_with(&name) {", "            if false {", "--test actions unused_imports"),
 ("no let _ for unused results", "src/query.rs", "            vec![edit(start..start, \"let _ = \")],", "            vec![edit(start..start, \"let _ =\")],", "--test actions unused_imports"),
 ("public fields not made mutable", "src/query.rs", "(t[..at].ends_with(\"let \") || t[..at].ends_with(\"let pub \"))", "t[..at].ends_with(\"let \")", "--test actions an_immutable"),
 ("refused assignments go nowhere", "src/query.rs", "        && let Some((file, location)) = assigned(scope)", "        && let Some((file, location)) = None::<(PathBuf, Location)>", "--test definition a_refused"),
 ("refused field assignments go to variables", "src/query.rs", "    if text[..start].ends_with('@') {\n        let method", "    if false {\n        let method", "--test definition a_refused"),
 ("a field's @ isn't skipped", "src/query.rs", "    let skip = usize::from(text[at..].starts_with('@'));", "    let skip = 0;", "--test definition a_refused"),
 ("the outermost variable assigned", "src/query.rs", "        .max_by_key(|v| v.from)\n        .map(|v| (scope.file.clone(), v.variable.location(db)))", "        .min_by_key(|v| v.from)\n        .map(|v| (scope.file.clone(), v.variable.location(db)))", "--test definition a_refused"),
 ("no missing-field fix", "src/query.rs", "        \"missing-field\" => {", "        \"missing-field-\" => {", "--test actions a_missing_field"),
 ("missing fields of another value", "src/query.rs", "        \"Float\" => \"0.0\",", "        \"Float\" => \"0\",", "--test actions a_missing_field"),
 ("missing fields never from a variable", "src/query.rs", "    if elsewhere {\n        values.push(field.to_string());", "    if false {\n        values.push(field.to_string());", "--test actions a_missing_field"),
 ("missing fields always after a comma", "src/query.rs", "    let separator = if written.ends_with('(') {", "    let separator = if false {", "--test actions a_missing_field"),
 ("missing fields after a trailing comma get another", "src/query.rs", "    } else if written.ends_with(',') {\n        \" \"", "    } else if false {\n        \" \"", "--test actions a_missing_field"),
 ("no stubs for missing methods", "src/query.rs", "        \"invalid-implementation\" => {", "        \"invalid-implementation-\" => {", "--test actions an_implementation"),
 ("stubs only for the diagnostic's method", "src/query.rs", "                    b.id == d.id\n                        && b.range == d.range", "                    b.id == d.id\n                        && b.message == d.message", "--test actions an_implementation"),
 ("stubs keep a trait's type parameters", "src/query.rs", "        replace = parameters.into_iter().zip(arguments).collect();", "        let _ = (parameters, arguments);", "--test actions an_implementation"),
 ("stubs never fill an empty body", "src/query.rs", "    if text[open + 1..close].trim().is_empty() {", "    if false {", "--test actions an_implementation"),
 ("traits not found in the file", "src/query.rs", "            if let Some(d) = named(&outline, name) {\n                return Some(d);", "            if let Some(d) = named(&outline, name).filter(|_| false) {\n                return Some(d);", "--test actions an_implementation"),
 ("traits not found where they're imported from", "src/query.rs", "    directories.into_iter().find_map(|d| {", "    directories.into_iter().take(0).find_map(|d| {", "--test actions an_implementation"),
 ("no fix for a missing trait", "src/query.rs", "        \"missing-trait\" => {", "        \"missing-trait-\" => {", "--test actions a_missing_trait"),
 ("a missing trait implemented without methods", "src/query.rs", "    if lacking.is_empty() {\n        return Some((at, empty));", "    if true {\n        return Some((at, empty));", "--test actions a_missing_trait"),
 ("no fix for unreachable code", "src/query.rs", "        \"unreachable\" if d.message == \"this code is unreachable\" => unreachable(", "        \"unreachable-\" if d.message == \"this code is unreachable\" => unreachable(", "--test actions unreachable_code"),
 ("unreachable arms not removed", "src/query.rs", "    if text[start..].starts_with(\"case \") {", "    if false {", "--test actions unreachable_code"),
 ("blank lines before unreachable code kept", "src/query.rs", "        from = previous;\n", "        break;\n", "--test actions unreachable_code"),
 ("unreachable code only to the first bracket", "src/syntax.rs", "                    if inner == Some(i) {\n                        return Some(at);", "                    if inner.is_some() {\n                        return Some(at);", "--test actions unreachable_code"),
 ("a fix for several diagnostics, an action each", "src/lsp.rs", "                if let Some(i) = seen.iter().position(|f| *f == fix) {", "                if let Some(i) = None::<usize> {", "--test actions an_implementation"),
 ("names refused past any declaration", "src/query.rs", "        self.effects.iter().all(|e| e.body || !e.top)\n            && self.all", "        self.all.iter().all(|(e, _)| e.body)\n            && self.all", "--test names names_past"),
 ("names past changes at the top of the module", "src/query.rs", "        self.effects.iter().all(|e| e.body || !e.top)\n", "        self.effects.iter().all(|e| e.body || e.top || !e.top)\n", "--test names"),
 ("names past declarations they list", "src/query.rs", "                    || (!e.declares.iter().any(|n| depends.contains(n.as_str()))", "                    || (true", "--test names names_past"),
 ("names past changes in self's type", "src/query.rs", "                        && !e.within.iter().any(|n| containers.contains(n.as_str())))", "                        && true)", "--test names names_past"),
 ("names past changes in self's traits", "src/query.rs", "            out.push(t.name(db).clone());\n", "", "--test names names_past"),
 ("no keywords", "src/lsp.rs", "                    keywords(&names, &mut result);\n", "", "--test names keywords"),
 ("expression keywords in types", "src/query.rs", "    let words = if !types {", "    let words = if true {", "--test names keywords"),
 ("every type keyword after ref", "src/query.rs", "    {\n        &[\"fn\"]\n    } else {", "    {\n        TYPE_KEYWORDS\n    } else {", "--test names keywords"),
 ("names' changes inside a container declare it", "src/units.rs", "        .filter(|n| !effect.within.contains(*n))\n", "", "--test names names_past"),
 ("a container's header is inside it", "src/units.rs", "    let header = item.kind == Kind::Container && in_header(old, item, &old_range);", "    let header = false;", "--lib what_a_change_declares"),
 ("braces opened in bodies move what follows", "src/syntax.rs", "        let in_bodies = blank.iter().all(|&b| {", "        let in_bodies = false && blank.iter().all(|&b| {", "--lib a_blank_line_that_opened"),
 ("the method with an open brace kept", "src/syntax.rs", "                list.retain(|d| !open.contains(&d.range));", "                list.retain(|_| true);", "--lib a_blank_line_that_opened"),
 ("braces opened outside bodies move nothing", "src/syntax.rs", "                    true\n                }\n                _ => false,\n            }\n        });", "                    true\n                }\n                _ => true,\n            }\n        });", "--lib a_blank_line_that_opened"),
 ("the misclosed brace never tried", "src/syntax.rs", "        (Some(a), Some(Some(b))) if b.keeps(&a) => Some(b),", "        (Some(a), Some(Some(b))) if false && b.keeps(&a) => Some(b),", "--test symbols document_symbols_in_code_that_does_not_parse"),
 ("any closed brace is misclosed", "src/syntax.rs", "                                && indent(line) != indent(lines[at])\n", "", "--test symbols document_symbols_in_code_that_does_not_parse"),
 ("the misclosed copy even with fewer symbols", "src/syntax.rs", "        ours.len() > theirs.len() && theirs.iter().all(|t| ours.contains(t))", "        true", "--lib the_brace_left_open"),
 ("no signature for a constructor", "src/walker.rs", "                            CallKind::TypeInstance(info) => Some(info.resolved_type),", "                            CallKind::TypeInstance(_) => None,", "--test signature a_types_constructor"),
 ("a constructor's fields without its type arguments", "src/query.rs", "                let arguments = ins.type_arguments(db).cloned();", "                let arguments = None::<TypeArguments>;", "--test signature a_types_constructor"),
 ("collapsed text for every client", "src/lsp.rs", "                    if self.support.collapsed_text {", "                    if true {", "--test folding folds_have_kinds"),
 ("no collapsed text", "src/lsp.rs", "                        range[\"collapsedText\"] = json!(collapsed_text(&text, &fold));", "", "--test folding"),
 ("a comment collapses to its first line with its line end", "src/lsp.rs", "            rest[..rest.find(['\\n', '\\r']).unwrap_or(rest.len())]", "            rest[..rest.find('\\n').unwrap_or(rest.len())]", "--test folding folds_are_on_lsps_lines"),
 ("every block collapses to braces", "src/lsp.rs", "            Some('(') => \"(...)\",", "            Some('(') => \"{...}\",", "--test folding blocks_comments"),
 ("impl blocks aren't workspace symbols", "src/index.rs", "                if let Some(score) = matches(query, &declaration.name) {", "                if declaration.kind != crate::syntax::Kind::Implementation\n                    && let Some(score) = matches(query, &declaration.name)\n                {", "--test symbols workspace_symbols_from"),
 ("an empty query is capped", "src/lsp.rs", "            (false, None) => WORKSPACE_SYMBOLS,", "            (_, None) => WORKSPACE_SYMBOLS,", "--test symbols every_workspace_symbol"),
 ("partial results capped", "src/lsp.rs", "            (false, None) => WORKSPACE_SYMBOLS,\n            _ => usize::MAX,", "            (false, _) => WORKSPACE_SYMBOLS,\n            _ => usize::MAX,", "--test symbols every_workspace_symbol"),
 ("no partial results", "src/lsp.rs", "        let result = match token {\n            Some(token) => {", "        let result = match None::<lsp::ProgressToken> {\n            Some(token) => {", "--test symbols every_workspace_symbol"),
 ("partial results in one batch", "src/lsp.rs", "                for batch in found.chunks(WORKSPACE_SYMBOLS) {", "                for batch in found.chunks(usize::MAX) {", "--test symbols every_workspace_symbol"),
 ("code actions for clients without literals", "src/lsp.rs", "        if self.support.code_actions && quickfix {", "        if quickfix {", "--test actions code_actions_are_only"),
 ("quick fixes for any kind", "src/lsp.rs", "        let quickfix = only.is_none_or(|kinds| {", "        let quickfix = true || only.is_none_or(|kinds| {", "--test actions code_actions_are_only"),
 ("isPreferred for every client", "src/lsp.rs", "                if only && self.support.preferred_actions {", "                if only {", "--test actions code_actions_are_only"),
 ("every fix preferred", "src/lsp.rs", "            let only = fixes.len() == 1;", "            let only = true;", "--test actions several_imports"),
 ("no imports offered", "src/lsp.rs", "                if d.public && d.kind.importable() && names.contains(&d.name) {", "                if false {", "--test actions an_undefined_symbol"),
 ("code actions not advertised", "src/lsp.rs", "                code_action_provider: self.support.code_actions.then(|| {", "                code_action_provider: false.then(|| {", "--test lsp initialize_advertises"),
 ("ascii matching keeps case", "src/index.rs", "    if n.len() >= q.len() && n[..q.len()].eq_ignore_ascii_case(q) {", "    if n.starts_with(q) {", "--lib ascii_names_match"),
 ("ascii subsequences keep case", "src/index.rs", "        let (at, _) = chars.find(|(_, d)| d.eq_ignore_ascii_case(c))?;", "        let (at, _) = chars.find(|(_, d)| *d == c)?;", "--lib ascii_names_match"),
 # Changes of a file closed and changed on disk (milestone 13).
 ("changes paired with other text", "src/edits.rs", "    if out.old_len != old.len() || out.new_len != new.len() {\n        return Changes::between(old, new);\n    }\n", "", "--lib exact_changes"),
 ("changes don't go on from where they end", "src/edits.rs", "        && end != new\n    {\n        out.replace_text(end, new);", "        && end != new\n        && false\n    {\n        out.replace_text(end, new);", "--lib exact_changes"),
 ("closing records no end", "src/edits.rs", "            if v.changes.contains_key(file) {\n                v.ends", "            if false {\n                v.ends", "--lib a_closed_file"),
 ("opening ignores where changes ended", "src/edits.rs", "            if let Some(end) = v.ends.remove(file) {", "            if let Some(end) = None::<String> {", "--lib a_closed_file"),
 # After an undo (milestone 13).
 ("an undo answered from a check of other text", "src/query.rs", "list.iter().position(|k| k.target == *target)", "list.iter().position(|k| k.target.root == target.root && k.target.file == target.file && k.target.tests == target.tests)", "--test undo undone_incremental"),
 ("an undo answered from a check of a module since restored", "src/query.rs", "            if entry.fingerprint.current() {", "            if true {", "--test undo a_module_changed"),
 ("only a notification's first change applied", "src/lsp.rs", "                        for change in p.content_changes {", "                        for change in p.content_changes.into_iter().take(1) {", "--test undo in_one_notification"),
]


# The test files built into tests/main.rs's binary, `integration`. Every
# other file in tests/ is a binary of its own.
INTEGRATION = "integration"
OWN_BINARIES = {"latency"}
# Tests that read the source tree, so they must run before it's restored.
IN_PLACE = {"guardrails", "citations"}
TIMEOUT = 900
# What catches the mutations of the release tooling, which only macOS
# can run outside Docker (`--shard dist`).
DIST = "sh:scripts/check-dist.sh"


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


def in_shard(chosen, shard):
    """The mutations of `chosen` (each with why) in a shard: "dist", or
    (K, N)."""
    if shard == "dist":
        return [(m, why) for m, why in chosen if m[4] == DIST]
    k, n = shard
    return [(m, why) for m, why in chosen if m[4] != DIST][k - 1::n]


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
    # A command may run every test (`--control`), which takes longer than
    # one mutation's tests.
    try:
        r = subprocess.run(command, shell=True, cwd=ROOT, capture_output=True, text=True, timeout=3 * TIMEOUT)
    except subprocess.TimeoutExpired:
        return "TIMED OUT after %ds" % (3 * TIMEOUT)
    out = r.stdout + r.stderr
    reason = next((l.strip() for l in out.splitlines() if l.startswith("error") or l.startswith("the ") or l.startswith("patches/")), "")
    return "caught by %s: %s" % (command, reason) if r.returncode != 0 else "NOT CAUGHT"


def test_env(extra=None):
    env = dict(os.environ)
    env["CUTTLEBONE_CORPUS_FULL"] = "1"
    # The latency limits as written, as in the milestone tier.
    env["CUTTLEBONE_LATENCY_STRICT"] = "1"
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


def run_tests(exe, names, work=None, cwd=ROOT, why=None):
    """Runs the named tests of a test binary: a mutation's copy, with the
    files tests make in `work`, or the tree's own build. Each failing
    test's panic is appended to `why`, if given."""
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
        if why is not None:
            lines = (r.stdout + r.stderr).splitlines()
            why.extend("\n".join(lines[i:i + 4]) for i, l in enumerate(lines) if l.startswith("thread '"))
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
    # A clone where the file system can: clonefile on macOS, a reflink
    # elsewhere (a copy, on a file system without them).
    clone = ["-c"] if sys.platform == "darwin" else ["--reflink=auto"]
    subprocess.run(["cp", *clone, "-R", os.path.join(ROOT, "target", "debug"), os.path.join(path, "target", "debug")],
                   check=False, capture_output=True)
    if not os.path.isdir(os.path.join(path, "target", "debug")):
        shutil.copytree(os.path.join(ROOT, "target", "debug"), os.path.join(path, "target", "debug"), symlinks=True)


def control():
    """Runs the tests, unmutated, as the runner runs a mutation's: built in
    a copy of the tree, from copies of the build. Returns the tests that
    fail, apart from those the runner runs in the tree itself. A test that
    fails there without a mutation would count every mutation it's named
    for as caught, proving nothing: before milestone 7, the tests that
    compared paths in std did, since `.upstream` is a link in a copy."""
    root = os.path.join(ROOT, "target", "mutate-control")
    make_tree(root)
    failed = []
    try:
        for cargo in (["--test", INTEGRATION], ["--lib"]):
            exe, out = build(cargo, root)
            if exe is None:
                return ["%s doesn't build: %s" % (" ".join(cargo), out[-500:])]
            names = [n for n in tests_in(exe, None, "", root) if n.split("::")[0] not in IN_PLACE]
            work = os.path.join(root, "run")
            os.makedirs(os.path.join(work, "tmp"), exist_ok=True)
            copy = os.path.join(work, os.path.basename(exe))
            shutil.copy2(exe, copy)
            shutil.copy2(os.path.join(root, "target", "debug", "cuttlebone"), os.path.join(work, "cuttlebone"))
            why = []
            verdict = run_tests(copy, names, work, cwd=root, why=why)
            if verdict != "NOT CAUGHT":
                failed.append("%s: %s%s" % (" ".join(cargo), verdict, "".join("\n  " + w.replace("\n", "\n  ") for w in why)))
    finally:
        shutil.rmtree(root, ignore_errors=True)
    return failed


def main(argv):
    cores = os.cpu_count() or 2
    since, jobs, trees, serial, listing, words, shard = None, max(1, min(5, cores // 2)), max(1, min(3, cores // 3)), False, False, [], None
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
        elif a == "--shard":
            shard = args.pop(0)
            if shard != "dist":
                k, n = (int(x) for x in shard.split("/"))
                if not 1 <= k <= n:
                    print("--shard K/N needs 1 <= K <= N", file=sys.stderr)
                    return 2
                shard = (k, n)
        elif a == "--list":
            listing = True
        elif a == "--control":
            failed = control()
            for f in failed:
                print("fails without a mutation, in a copy of the tree: %s" % f, flush=True)
            return 1 if failed else 0
        else:
            words.append(a)
    chosen = [(m, "all") for m in M]
    if since:
        changed = changed_since(since)
        chosen = selected(M, changed)
        shared = sorted(f for f in changed if f in ("scripts/mutate.py", "scripts/check-selection.py", "tests/main.rs", "Cargo.toml") or f.startswith("tests/common/"))
        if shared:
            print("note: %s changed since %s, which every mutation's tests depend on: the release tier's exhaustive mutations, in CI, cover it (AGENTS.md)" % (", ".join(shared), since), flush=True)
    if words:
        chosen = [(m, why) for m, why in chosen if any(w in m[0] for w in words)]
    if shard:
        chosen = in_shard(chosen, shard)
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
        if any(not m[4].startswith("sh:") and target(m[4])[1] not in IN_PLACE for m, _ in chosen):
            failed = control()
            if failed:
                for f in failed:
                    print("fails without a mutation, in a copy of the tree: %s" % f, flush=True)
                print("so its mutations would prove nothing; fix it first")
                return 1
            print("control: every test passes unmutated in a copy of the tree (%.0fs)" % (time.time() - start), flush=True)
        # Run in the tree itself: commands that use its build or its
        # .upstream, which every copy links to.
        here = [m for m, _ in chosen if m[4].startswith("sh:") or target(m[4])[1] in IN_PLACE]
        last = [m for m, _ in chosen if m[1].startswith(".upstream/")]
        built = [m for m, _ in chosen if m not in here and m not in last]
        queue = list(enumerate(built))
        alone = []
        pool = ThreadPoolExecutor(max_workers=jobs)
        futures = []

        # The copies are made before any mutation in the tree itself, so
        # that none copies one.
        copied = threading.Barrier(trees + 1)

        def builder(k):
            root = os.path.join(work_root, "trees", str(k))
            try:
                make_tree(root)
            finally:
                copied.wait()
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
        copied.wait()
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
