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
 ("no required-version check", "src/check.rs", "    check_required_version(target)?;\n", "", "--test manifest"),
 ("dependency manifests ignored", "src/check.rs", "paths.push(version.join(MANIFEST_FILE));", "let _ = version;", "--test manifest"),
 ("always UTF-16", "src/lsp.rs", '"utf-8" => Some(Encoding::Utf8),', '"utf-8" => Some(Encoding::Utf16),', "--test lsp utf8"),
 ("UTF-8 counts chars", "src/position.rs", "Encoding::Utf8 => text.len() as u32,", "Encoding::Utf8 => text.chars().count() as u32,", "--test lsp non_ascii"),
 ("incremental edits replace everything", "src/lsp.rs", "    let Some(r) = change.range else {", "    let Some(r) = None::<lsp::Range> else {", "--test lsp several_changes"),
 ("edit ranges in UTF-16 regardless", "src/lsp.rs", "            encoding,\n        )\n        .unwrap_or(text.len())", "            Encoding::Utf16,\n        )\n        .unwrap_or(text.len())", "--test lsp non_ascii_positions_and_ranges_in_utf8"),
 ("no timeout", "src/lsp.rs", "Some((_, check)) => Some(check.started + self.options.timeout),", "Some((_, _check)) => None,", "--test lsp a_hung_check"),
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
 ("didSave text ignored", "src/lsp.rs", "                    doc.text = text;\n", "                    let _ = (doc, text);\n", "--test lsp saving"),
 ("didClose ignored", "src/lsp.rs", "                    self.documents.remove(&p.text_document.uri);\n", "                    let _ = p;\n", "--test lsp a_session"),
 ("workspace folders ignored", "src/lsp.rs", "        (Some(folders), _, _) if !folders.is_empty() => {", "        (Some(folders), _, _) if folders.is_empty() => {", "--test lsp workspace_folders"),
 ("other open documents not sent", "src/lsp.rs", "            .filter(|(path, _)| path.is_file())", "            .filter(|(path, _)| path.is_file() && path == &file)", "--test lsp unsaved_changes_to_an_imported"),
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
 ("type definition answers definition", "src/worker.rs", "Outcome::Place(query::type_definition(&self.session, &self.request))", "Outcome::Place(query::definition(&self.session, &self.request))", "--test navigation type_definition"),
 # The guardrails (AGENTS.md).
 ("query uses lsp", "src/query.rs", "use crate::check::{Target, project_root, run};\n", "use crate::check::{Target, project_root, run};\nuse crate::lsp as _;\n", "--test guardrails layers"),
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
