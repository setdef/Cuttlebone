#!/usr/bin/env python3
"""Breaks behaviours on purpose, one at a time, and checks that a test fails.

Each mutation replaces one exact snippet of source (it must occur exactly
once), runs the tests that should catch it, and restores the file. Run it
from anywhere; pass words to run only the mutations whose names contain
them, e.g. `scripts/mutate.py timeout`. A mutation reported as NOT CAUGHT
means a behaviour has no test; SKIP means its snippet changed, so update
the snippet.
"""
import subprocess, sys, os, json
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
M = [
 ("patch: prepare() back in check_with", ".upstream/inko/compiler/src/compiler.rs",
  "    ) -> Result<(), CompileError> {\n        let start = Instant::now();\n\n        // When checking a project",
  "    ) -> Result<(), CompileError> {\n        self.prepare()?;\n        let start = Instant::now();\n\n        // When checking a project",
  "--test lsp queries_never_sync"),
 ("no required-version check", "src/lib.rs", "    check_required_version(target)?;\n", "", "--test manifest"),
 ("dependency manifests ignored", "src/lib.rs", "paths.push(version.join(MANIFEST_FILE));", "let _ = version;", "--test manifest"),
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
 ("byte_offset ignores CR", "src/query.rs", "    let line = line.strip_suffix('\\r').unwrap_or(line);\n", "", "--test complete byte_offsets"),
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
        r = subprocess.run("cargo test " + test.split(" ")[0] + " " + test.split(" ")[1] + (" -- " + test.split(" ",2)[2] if test.count(" ") >= 2 else ""),
                           shell=True, cwd=ROOT, capture_output=True, text=True, timeout=900)
        out = r.stdout + r.stderr
        if "could not compile" in out or "error[" in out:
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
print("%d of %d caught" % (sum(v.startswith("caught") for _, v in results), len(results)))
