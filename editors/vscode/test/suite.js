// Runs inside VS Code: points the extension at $CUTTLEBONE, opens $FILE,
// and asks VS Code for the hover at the first $NEEDLE plus $OFFSET
// characters, which must show $EXPECTED in a Markdown code block.
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
  const text = document.getText();
  const index = text.indexOf(process.env.NEEDLE);

  assert.notStrictEqual(index, -1, `${process.env.NEEDLE} isn't in the file`);

  // OFFSET counts characters; VS Code's offsets are UTF-16.
  const offset = index + [...process.env.NEEDLE].slice(0, Number(process.env.OFFSET)).join('').length;
  const position = document.positionAt(offset);
  const expected = '```inko\n' + process.env.EXPECTED + '\n```';
  let shown = [];

  // The first hover can race the server's start; retry briefly.
  await waitFor(async () => {
    const hovers = await vscode.commands.executeCommand('vscode.executeHoverProvider', document.uri, position);

    shown = hovers.flatMap((h) => h.contents.map((c) => (typeof c === 'string' ? c : c.value)));
    return shown.includes(expected);
  }, `a hover showing ${process.env.EXPECTED}`, () => `hovers: ${JSON.stringify(shown)}`);

  console.log(`vscode: ok: hover at ${process.env.NEEDLE} shows ${process.env.EXPECTED} (positions in ${encoding})`);
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
