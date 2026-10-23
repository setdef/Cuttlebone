// Runs inside VS Code: points the extension at $CUTTLEBONE, opens $FILE,
// and asks VS Code for the hover at the first $NEEDLE plus $OFFSET
// characters, which must show $EXPECTED in a Markdown code block. Then asks
// for the definition at the first $DEFINITION plus $DEFINITION_OFFSET
// characters, which must cover $DEFINED on line $DEFINED_LINE (1-based) of
// $FILE. Then inserts $BROKEN after line $BROKEN_AFTER, waits for a
// diagnostic with code $ERROR_CODE covering $ERROR_TEXT, removes the line
// and waits for the diagnostic to go. Nothing is saved.
const assert = require('assert');
const vscode = require('vscode');

async function run() {
  const config = vscode.workspace.getConfiguration('cuttlebone');

  await config.update('path', process.env.CUTTLEBONE, vscode.ConfigurationTarget.Global);

  if (process.env.INKO) {
    await config.update('inko', process.env.INKO, vscode.ConfigurationTarget.Global);
  }

  const document = await vscode.workspace.openTextDocument(process.env.FILE);

  await vscode.window.showTextDocument(document);
  assert.strictEqual(document.languageId, 'inko');

  const extension = vscode.extensions.getExtension('local.cuttlebone');
  const { client } = await extension.activate();

  await waitFor(() => client.isRunning(), 'the server to start');

  const encoding = client.initializeResult.capabilities.positionEncoding;
  const position = positionOf(document, process.env.NEEDLE, process.env.OFFSET);
  const expected = '```inko\n' + process.env.EXPECTED + '\n```';
  let shown = [];

  // The first hover can race the server's start; retry briefly.
  await waitFor(async () => {
    const hovers = await vscode.commands.executeCommand('vscode.executeHoverProvider', document.uri, position);

    shown = hovers.flatMap((h) => h.contents.map((c) => (typeof c === 'string' ? c : c.value)));
    return shown.includes(expected);
  }, `a hover showing ${process.env.EXPECTED}`, () => `hovers: ${JSON.stringify(shown)}`);

  // With the wrong encoding, the position is past the end of the line, and
  // there's no definition.
  const at = positionOf(document, process.env.DEFINITION, process.env.DEFINITION_OFFSET);
  let found = [];

  await waitFor(async () => {
    const locations = await vscode.commands.executeCommand('vscode.executeDefinitionProvider', document.uri, at);

    // Location or LocationLink.
    found = locations.map((l) => ({
      file: (l.uri || l.targetUri).fsPath,
      range: l.targetSelectionRange || l.range || l.targetRange,
    }));
    return found.some(
      (l) =>
        l.file === document.uri.fsPath &&
        l.range.start.line === Number(process.env.DEFINED_LINE) - 1 &&
        document.getText(l.range) === process.env.DEFINED,
    );
  }, `a definition covering ${process.env.DEFINED}`, () => `definitions: ${JSON.stringify(found)}`);

  // Diagnostics: insert a line with an error, wait for its diagnostic, then
  // remove it and wait for the diagnostic to go.
  const after = Number(process.env.BROKEN_AFTER);
  const code = process.env.ERROR_CODE;
  const errors = () =>
    vscode.languages
      .getDiagnostics(document.uri)
      .filter((d) => (typeof d.code === 'object' ? d.code.value : d.code) === code)
      .map((d) => ({ line: d.range.start.line + 1, text: document.getText(d.range) }));
  const insert = new vscode.WorkspaceEdit();

  insert.insert(document.uri, new vscode.Position(after, 0), process.env.BROKEN + '\n');
  assert.ok(await vscode.workspace.applyEdit(insert));
  await waitFor(
    () => {
      const found = errors();

      return found.length === 1 && found[0].line === after + 1 && found[0].text === process.env.ERROR_TEXT;
    },
    `a ${code} diagnostic covering ${process.env.ERROR_TEXT}`,
    () => `diagnostics: ${JSON.stringify(vscode.languages.getDiagnostics(document.uri))}`,
  );

  const remove = new vscode.WorkspaceEdit();

  remove.delete(document.uri, new vscode.Range(after, 0, after + 1, 0));
  assert.ok(await vscode.workspace.applyEdit(remove));
  await waitFor(
    () => errors().length === 0,
    `the ${code} diagnostic to go`,
    () => `diagnostics: ${JSON.stringify(vscode.languages.getDiagnostics(document.uri))}`,
  );
  await vscode.commands.executeCommand('workbench.action.files.revert');

  console.log(`vscode: ok: hover at ${process.env.NEEDLE} shows ${process.env.EXPECTED} (positions in ${encoding})`);
  console.log(
    `vscode: ok: definition at ${process.env.DEFINITION} is ${process.env.DEFINED} on line ${process.env.DEFINED_LINE}`,
  );
  console.log(
    `vscode: ok: a ${code} diagnostic covered ${process.env.ERROR_TEXT} on line ${after + 1}, and cleared when the line was removed`,
  );
}

// The position `offset` characters into the first `needle`.
function positionOf(document, needle, offset) {
  const index = document.getText().indexOf(needle);

  assert.notStrictEqual(index, -1, `${needle} isn't in the file`);

  // The offset counts characters; VS Code's offsets are UTF-16.
  return document.positionAt(index + [...needle].slice(0, Number(offset)).join('').length);
}

async function waitFor(condition, what, detail = () => '') {
  const deadline = Date.now() + 20000;

  while (Date.now() < deadline) {
    if (await condition()) {
      return;
    }

    await new Promise((resolve) => setTimeout(resolve, 100));
  }

  throw new Error(`timed out waiting for ${what}. ${detail()}`);
}

module.exports = { run };
