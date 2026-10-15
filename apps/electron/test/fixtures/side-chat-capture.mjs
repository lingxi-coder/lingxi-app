import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { join, resolve } from 'node:path';
import react from '@vitejs/plugin-react';
import { createServer } from 'vite';
const root = resolve(fileURLToPath(new URL('../..', import.meta.url)));
const temporaryRoot = mkdtempSync(join(tmpdir(), 'lingxi-side-fixture-'));
const vite = await createServer({ root: join(root, 'test/fixtures'), cacheDir: join(temporaryRoot, 'vite'), configFile: false, logLevel: 'error', optimizeDeps: { entries: ['side-chat-fixture.html'] }, server: { host: '127.0.0.1', port: 0, hmr: false }, plugins: [react()], resolve: { alias: { '@renderer': join(root, 'src/renderer') }, dedupe: ['react', 'react-dom'] } });
let child;
try {
  await vite.listen();
  child = spawn(process.execPath, [join(root, 'node_modules/electron/cli.js'), join(root, 'test/fixtures/side-chat-electron.mjs'), `http://127.0.0.1:${vite.httpServer.address().port}/side-chat-fixture.html`], { cwd: root, env: { ...process.env, ELECTRON_ENABLE_LOGGING: '0', LINGXI_TEST_USER_DATA: join(temporaryRoot, 'user-data'), LINGXI_VISUAL_OUTPUT: resolve(process.argv[2] || '/tmp/lingxi-side-visual') }, stdio: 'inherit' });
  process.exitCode = await new Promise((resolveExit, reject) => {
    const timer = setTimeout(() => { child.kill('SIGTERM'); reject(new Error('Side fixture timed out')); }, 60000);
    child.once('error', (error) => { clearTimeout(timer); reject(error); }); child.once('exit', (code) => { clearTimeout(timer); resolveExit(code ?? 1); });
  });
} finally { if (child && child.exitCode === null) child.kill('SIGTERM'); await vite.close(); rmSync(temporaryRoot, { recursive: true, force: true }); }
