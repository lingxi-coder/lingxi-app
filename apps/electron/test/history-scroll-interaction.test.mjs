import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import react from '@vitejs/plugin-react';
import { createServer } from 'vite';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');

test('history pagination and delayed message growth keep every painted reading position during a wheel gesture', async t => {
  const cacheDir = mkdtempSync(join(tmpdir(), 'lingxi-history-scroll-vite-'));
  const userData = mkdtempSync(join(tmpdir(), 'lingxi-history-scroll-electron-'));
  const server = await createServer({
    root: join(root, 'test/fixtures'), cacheDir, configFile: false, logLevel: 'error',
    plugins: [react()], resolve: { alias: { '@renderer': join(root, 'src/renderer') }, dedupe: ['react', 'react-dom'] },
    server: { host: '127.0.0.1', port: 0, hmr: false },
  });
  let child;
  try {
    await server.listen();
    const port = server.httpServer.address().port;
    const result = await new Promise((accept, reject) => {
      let output = '', errors = '';
      child = spawn(process.execPath, [join(root, 'node_modules/electron/cli.js'), join(root, 'test/fixtures/history-scroll-electron.mjs'), `http://127.0.0.1:${port}/stage-scroll-fixture.html`], {
        cwd: root, env: { ...process.env, LINGXI_TEST_USER_DATA: userData, ELECTRON_ENABLE_LOGGING: '0', ELECTRON_IS_DEV: '0' },
        stdio: ['ignore', 'pipe', 'pipe'],
      });
      const timeout = setTimeout(() => { child.kill('SIGTERM'); reject(new Error(`History scroll fixture timed out\n${errors}`)); }, 40_000);
      child.stdout.on('data', chunk => { output += chunk; });
      child.stderr.on('data', chunk => { errors += chunk; });
      child.once('error', error => { clearTimeout(timeout); reject(error); });
      child.once('exit', code => {
        clearTimeout(timeout);
        if (code !== 0) { reject(new Error(`History scroll fixture exited ${code}\n${errors}\n${output}`)); return; }
        try { accept(JSON.parse(output.trim().split('\n').at(-1))); } catch (error) { reject(error); }
      });
    });
    if (process.env.LINGXI_SCROLL_EVIDENCE_FILE) writeFileSync(process.env.LINGXI_SCROLL_EVIDENCE_FILE, JSON.stringify(result, null, 2) + '\n');
    t.diagnostic(JSON.stringify(result.map(({ count, phases, wheelEvents }) => ({ count, wheelEvents, phases: phases.map(({ phase, maxDrift }) => ({ phase, maxDrift })) }))));
    for (const sample of result) {
      assert.ok(sample.wheelEvents >= 3, 'trusted wheel input reaches the real transcript');
      assert.equal(sample.historyRequests, 1, 'the gesture joins one in-flight page');
      assert.equal(sample.uniqueMessages, sample.messageCount, 'prepend retains unique message identity');
      assert.equal(sample.tail.reserved, 0, 'an explicit jump releases short-history tail space');
      assert.ok(sample.tail.gap <= 1, 'an explicit jump still follows the true bottom');
      for (const phase of sample.phases) {
        if (phase.phase === 'late-child-resize') assert.equal(phase.mutationResult, true, 'the preceding row receives a real delayed child resize');
        assert.ok(phase.frames.length >= 4, 'measure successive frames, not only the final location');
        assert.ok(phase.frames.every(frame => Number.isFinite(frame.offset)), 'the reading message stays mounted');
        assert.ok(phase.maxDrift <= 1, `${sample.count} rows, ${phase.phase}: reading position shifted ${phase.maxDrift}px during the gesture`);
      }
    }
  } finally {
    if (child?.exitCode === null) child.kill('SIGTERM');
    await server.close();
    rmSync(cacheDir, { recursive: true, force: true });
    rmSync(userData, { recursive: true, force: true });
  }
});
