import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { join, resolve } from 'node:path';
import react from '@vitejs/plugin-react';
import { createServer } from 'vite';

// Usage: node scripts/capture-apple-desktop.mjs /absolute/output/directory
const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
const fixtureRoot = join(root, 'test/fixtures');
const temporaryRoot = mkdtempSync(join(tmpdir(), 'lingxi-apple-visual-'));
const vite = await createServer({ root: fixtureRoot, cacheDir: join(temporaryRoot, 'vite'), configFile: false,
  logLevel: 'error', server: { host: '127.0.0.1', port: 0, hmr: false }, plugins: [react()],
  resolve: { alias: { '@renderer': join(root, 'src/renderer') }, dedupe: ['react', 'react-dom'] },
});
let child;
try {
  await vite.listen();
  const address = vite.httpServer.address();
  child = spawn(process.execPath, [join(root, 'node_modules/electron/cli.js'), join(fixtureRoot, 'apple-desktop-electron.mjs'), `http://127.0.0.1:${address.port}/apple-desktop-fixture.html`], {
    cwd: root, env: { ...process.env, ELECTRON_ENABLE_LOGGING: '0', ELECTRON_IS_DEV: '0', LINGXI_TEST_USER_DATA: join(temporaryRoot, 'user-data'), LINGXI_VISUAL_OUTPUT: resolve(process.argv[2] || '/tmp/lingxi-apple-visual') }, stdio: 'inherit',
  });
  process.exitCode = await new Promise((resolveExit, reject) => {
    const timeout = setTimeout(() => { child.kill('SIGTERM'); reject(new Error('Visual capture timed out')); }, 60000);
    child.once('error', error => { clearTimeout(timeout); reject(error); });
    child.once('exit', code => { clearTimeout(timeout); resolveExit(code ?? 1); });
  });
} finally {
  if (child && child.exitCode === null) child.kill('SIGTERM');
  await vite.close();
  rmSync(temporaryRoot, { recursive: true, force: true });
}
