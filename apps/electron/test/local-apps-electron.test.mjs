import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import { existsSync } from 'node:fs';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { build } from 'esbuild';
import react from '@vitejs/plugin-react';
import { createServer } from 'vite';

const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
const workspace = resolve(root, '../..');
function fixtureBridgeServer() {
  const explicit = process.env.LINGXI_BRIDGE_SERVER_BIN;
  if (explicit) {
    if (!existsSync(explicit)) throw new Error(`LINGXI_BRIDGE_SERVER_BIN does not exist: ${explicit}`);
    return resolve(explicit);
  }
  // The fixture main process is bundled into a temporary directory, so normal
  // module-relative workspace discovery cannot find the build output there.
  const target = resolve(workspace, process.env.CARGO_TARGET_DIR || 'target');
  const name = process.platform === 'win32' ? 'bridge-server.exe' : 'bridge-server';
  for (const profile of ['debug', 'release']) {
    const candidate = join(target, profile, name);
    if (existsSync(candidate)) return candidate;
  }
  throw new Error(`Build bridge-server in ${target}, or set LINGXI_BRIDGE_SERVER_BIN`);
}
test('real Electron Local Apps: tool transport, library, independent windows, backends, permissions and durable updates', { timeout: 120000 }, async () => {
  const serverBin = fixtureBridgeServer();
  const temp = await mkdtemp(join(tmpdir(), 'lingxi-local-app-electron-'));
  const bundle = (entry, name) => build({ entryPoints: [join(root, entry)], outfile: join(temp, name), bundle: true, platform: 'node', format: 'cjs', external: ['electron'] });
  await Promise.all([bundle('test/fixtures/local-apps-electron.ts', 'main.cjs'), bundle('src/preload/localApp.ts', 'localApp.cjs'), bundle('src/preload/index.ts', 'preload.cjs')]);
  const vite = await createServer({ root: join(root, 'test/fixtures'), cacheDir: join(temp, 'vite'), configFile: false, logLevel: 'error', optimizeDeps: { entries: ['local-apps-fixture.html'] }, plugins: [react()], resolve: { dedupe: ['react', 'react-dom'] }, server: { host: '127.0.0.1', port: 0, hmr: false } });
  let child;
  try {
    await vite.listen();
    const port = vite.httpServer.address().port;
    const env = { ...process.env, LINGXI_BRIDGE_SERVER_BIN: serverBin, LINGXI_TEST_USER_DATA: temp, LINGXI_TEST_LIBRARY_URL: `http://127.0.0.1:${port}/local-apps-fixture.html` };
    delete env.ELECTRON_RUN_AS_NODE;
    child = spawn(createRequire(import.meta.url)('electron'), [join(temp, 'main.cjs')], {
      cwd: root, env, stdio: ['ignore', 'pipe', 'pipe'],
    });
    let out = '', err = '';
    child.stdout.on('data', chunk => { out += chunk; }); child.stderr.on('data', chunk => { err += chunk; });
    const result = await new Promise((resolve, reject) => {
      const timer = setTimeout(() => { child.kill(); reject(new Error(`Timed out: ${err}\nOutput: ${out}`)); }, 100000);
      // This fixture reports only after host.dispose() and every assertion.
      // Bare unsigned Electron on this macOS also hangs after `quit` with no
      // Node handles. Signed-app exit is independently asserted by package smoke.
      child.stdout.on('data', () => {
        try { const result = JSON.parse(out.trim().split('\n').at(-1)); if (result.passed === true) { clearTimeout(timer); resolve(result); } } catch { /* incomplete line */ }
      });
      child.once('error', error => { clearTimeout(timer); reject(error); });
      child.once('exit', code => { clearTimeout(timer); code === 0 ? resolve(JSON.parse(out.trim().split('\n').at(-1))) : reject(new Error(`Electron exited ${code}: ${err}\n${out}`)); });
    });
    assert.equal(result.passed, true);
    assert.equal(result.library, true);
    assert.equal(result.backend, true);
  } finally {
    if (child && child.exitCode === null) {
      const exited = new Promise(resolve => child.once('exit', resolve));
      child.kill('SIGTERM');
      let timer;
      const forced = new Promise(resolve => { timer = setTimeout(() => { child.kill('SIGKILL'); resolve(); }, 1500); });
      await Promise.race([exited, forced]);
      clearTimeout(timer);
    }
    // Electron/Chromium descendants can keep the captured pipe handles open
    // after the main process exits. Close the readers so the node:test worker
    // itself can finish after the fixture has reported its assertions.
    child?.stdout?.destroy();
    child?.stderr?.destroy();
    await vite.close(); await rm(temp, { recursive: true, force: true });
  }
});
