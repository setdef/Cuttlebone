// Launches VS Code with this extension and runs test/suite.js in it, with a
// fresh user data directory and no other extensions. Uses $VSCODE if set,
// otherwise the usual install location, otherwise downloads VS Code.
//
// Environment: CUTTLEBONE (the binary), WORKSPACE (a project directory),
// FILE (an .inko file in it), NEEDLE, OFFSET and EXPECTED (see suite.js),
// INKO (optional).
const fs = require('fs');
const os = require('os');
const path = require('path');
const { runTests } = require('@vscode/test-electron');

const candidates = [
  process.env.VSCODE,
  '/Applications/Visual Studio Code.app/Contents/MacOS/Code',
  '/Applications/Visual Studio Code.app/Contents/MacOS/Electron',
  '/usr/share/code/code',
].filter(Boolean);

async function main() {
  const data = fs.mkdtempSync(path.join(os.tmpdir(), 'cuttlebone-vscode-'));

  try {
    await runTests({
      vscodeExecutablePath: candidates.find((p) => fs.existsSync(p)),
      extensionDevelopmentPath: path.resolve(__dirname, '..'),
      extensionTestsPath: path.resolve(__dirname, 'suite.js'),
      launchArgs: [
        process.env.WORKSPACE,
        '--disable-extensions',
        '--disable-workspace-trust',
        '--skip-welcome',
        '--skip-release-notes',
        '--user-data-dir',
        path.join(data, 'user'),
        '--extensions-dir',
        path.join(data, 'extensions'),
      ],
    });
  } catch (e) {
    console.error('vscode: FAIL:', e.message || e);
    process.exitCode = 1;
  } finally {
    fs.rmSync(data, { recursive: true, force: true });
  }
}

main();
