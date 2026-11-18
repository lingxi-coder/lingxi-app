import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import react from '@vitejs/plugin-react';
import electronBinary from 'electron';
import { createServer } from 'vite';

const electronRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const fixtureRoot = join(electronRoot, 'test', 'fixtures');

test('Desktop Inbox filters and routes conversations and reads visible Assistant results', async () => {
  const temporary = mkdtempSync(join(tmpdir(), 'lingxi-unified-inbox-ui-'));
  let vite;
  let child;
  let deadline;
  try {
    vite = await createServer({
      root: fixtureRoot,
      cacheDir: join(temporary, 'vite'),
      configFile: false,
      logLevel: 'error',
      server: { host: '127.0.0.1', port: 0, hmr: false },
      plugins: [react()],
      optimizeDeps: { entries: ['unified-inbox-ui.html'] },
      resolve: { alias: { '@renderer': join(electronRoot, 'src', 'renderer') }, dedupe: ['react', 'react-dom'] },
    });
    await vite.listen();
    const address = vite.httpServer?.address();
    assert.ok(address && typeof address === 'object' && address.port);
    const url = `http://127.0.0.1:${address.port}/unified-inbox-ui.html`;
    const userData = join(temporary, 'electron');
    mkdirSync(userData);
    child = spawn(electronBinary, [join(fixtureRoot, 'unified-inbox-ui-electron.mjs'), url], {
      cwd: electronRoot,
      env: { ...process.env, ELECTRON_ENABLE_LOGGING: '0', ELECTRON_IS_DEV: '0', LINGXI_TEST_USER_DATA: userData },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let output = '';
    let errors = '';
    child.stdout.on('data', chunk => { output += chunk; });
    child.stderr.on('data', chunk => { errors += chunk; });
    deadline = setTimeout(() => child.kill('SIGTERM'), 45_000);
    const [code, signal] = await once(child, 'close');
    assert.equal(code, 0, `Desktop Inbox Electron fixture exited ${code ?? signal}\n${errors}\n${output}`);
    console.log(output.trim());
  } finally {
    clearTimeout(deadline);
    if (child && child.exitCode === null && child.signalCode === null) child.kill('SIGTERM');
    await vite?.close();
    rmSync(temporary, { recursive: true, force: true });
  }
});
