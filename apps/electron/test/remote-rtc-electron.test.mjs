import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { copyFile, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { build } from 'esbuild';

import { assertSandboxedPreloadBundle } from '../scripts/verify-package.mjs';

const root = resolve(fileURLToPath(new URL('..', import.meta.url)));

// The real production files, bundled the way electron-vite would: the hidden
// page and its preload, plus a fixture main that drives them.
const node = (entry, out) => build({ entryPoints: [join(root, entry)], outfile: out, bundle: true, platform: 'node', format: 'cjs', external: ['electron'], logLevel: 'error' });
const browser = (entry, out) => build({ entryPoints: [join(root, entry)], outfile: out, bundle: true, platform: 'browser', format: 'esm', logLevel: 'error' });

test('real Electron Remote Control transport: hidden WebRTC window, MessagePort, 2 MiB both ways, lockdown, RtdbClient over net.fetch', { timeout: 180000 }, async (t) => {
  const temp = await mkdtemp(join(tmpdir(), 'lingxi-remote-rtc-'));
  let child;
  try {
    await Promise.all([
      node('test/fixtures/remote-rtc-electron.ts', join(temp, 'main.cjs')),
      node('src/preload/remoteRtc.ts', join(temp, 'remoteRtc.cjs')),
      browser('src/renderer/remoteRtc/main.ts', join(temp, 'page.js')),
      browser('test/fixtures/remote-rtc-viewer.ts', join(temp, 'viewer.js')),
    ]);

    // The sandboxed preload may require nothing but electron.
    assertSandboxedPreloadBundle(await readFile(join(temp, 'remoteRtc.cjs'), 'utf8'));
    assert.deepEqual([...(await readFile(join(temp, 'remoteRtc.cjs'), 'utf8')).matchAll(/\brequire\(["']([^"']+)["']\)/g)].map((match) => match[1]), ['electron']);

    // The production page, its Content-Security-Policy included, with only the script reference pointed at the bundle.
    const html = await readFile(join(root, 'src/renderer/remote-rtc.html'), 'utf8');
    assert.match(html, /<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'self'" \/>/);
    assert.ok(html.includes('src="./remoteRtc/main.ts"'));
    await writeFile(join(temp, 'page.html'), html.replace('./remoteRtc/main.ts', './page.js'));
    await writeFile(join(temp, 'viewer.html'), '<!doctype html><meta charset="utf-8"><script type="module" src="./viewer.js"></script>');
    await copyFile(join(root, 'src/renderer/remote-rtc.html'), join(temp, 'remote-rtc.production.html'));

    const env = { ...process.env, LINGXI_TEST_ROOT: temp };
    delete env.ELECTRON_RUN_AS_NODE;
    let electron;
    try {
      electron = createRequire(import.meta.url)('electron');
    } catch (error) {
      t.skip(`Electron is not installed: ${error.message}`);
      return;
    }
    let out = '', err = '', ready = false, spawnError;
    child = spawn(electron, [join(temp, 'main.cjs')], { cwd: root, env, stdio: ['ignore', 'pipe', 'pipe'] });
    child.stdout.on('data', (chunk) => { out += chunk; if (out.includes('READY')) ready = true; });
    child.stderr.on('data', (chunk) => { err += chunk; });
    child.once('error', (error) => { spawnError = error; });

    const result = await new Promise((resolveResult, reject) => {
      // Electron that cannot start at all (no display, a broken binary) is a skip; once it said READY, anything wrong is a failure.
      const startTimer = setTimeout(() => { if (!ready) { child.kill(); resolveResult({ skipped: `Electron did not start within 40 s: ${err.trim().slice(0, 500)}` }); } }, 40000);
      const timer = setTimeout(() => { child.kill(); reject(new Error(`Timed out: ${err}\nOutput: ${out}`)); }, 160000);
      const finish = (value) => { clearTimeout(startTimer); clearTimeout(timer); resolveResult(value); };
      // The fixture reports after every assertion. Bare unsigned Electron on this macOS can hang after `quit`, so the first
      // `passed` line is the end of the test and the process is killed below.
      child.stdout.on('data', () => {
        const last = out.trim().split('\n').at(-1);
        if (!last.startsWith('{')) return;
        try {
          const parsed = JSON.parse(last);
          if (parsed.passed === true) finish(parsed);
          else if (parsed.passed === false) { clearTimeout(startTimer); clearTimeout(timer); reject(new Error(`Fixture failed: ${parsed.error}\n${err}`)); }
        } catch { /* incomplete line */ }
      });
      child.once('error', (error) => finish({ skipped: `Electron could not be launched: ${error.message}` }));
      child.once('exit', (code) => {
        if (!ready) finish({ skipped: `Electron exited ${code} before it was ready: ${err.trim().slice(0, 500)}` });
        else { clearTimeout(startTimer); clearTimeout(timer); reject(new Error(`Electron exited ${code}: ${err}\n${out}`)); }
      });
    });
    if (spawnError && !result.passed) { t.skip(String(spawnError)); return; }
    if (result.skipped) { t.skip(result.skipped); return; }
    assert.equal(result.passed, true);
    assert.equal(result.bytes, Number(process.env.LINGXI_TEST_BYTES ?? 2 * 1024 * 1024));
    assert.ok(['host', 'prflx', 'srflx'].includes(result.candidate), `candidate type ${result.candidate}`);
  } finally {
    if (child && child.exitCode === null) {
      child.kill('SIGTERM');
      await new Promise((done) => { const timer = setTimeout(() => { child.kill('SIGKILL'); done(); }, 1500); child.once('exit', () => { clearTimeout(timer); done(); }); });
    }
    await rm(temp, { recursive: true, force: true });
  }
});
