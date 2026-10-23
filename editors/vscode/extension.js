// A minimal extension that starts `cuttlebone` over stdio for .inko files.
// Everything else (hover, completion after `.`) is the standard LSP client.
const vscode = require('vscode');
const { LanguageClient } = require('vscode-languageclient/node');

let client;

async function activate() {
  const config = vscode.workspace.getConfiguration('cuttlebone');
  const inko = config.get('inko');

  client = new LanguageClient(
    'cuttlebone',
    'Cuttlebone',
    { command: config.get('path') },
    {
      documentSelector: [{ scheme: 'file', language: 'inko' }],
      initializationOptions: inko ? { inko } : {},
    },
  );
  await client.start();
  return { client };
}

function deactivate() {
  return client && client.stop();
}

module.exports = { activate, deactivate };
