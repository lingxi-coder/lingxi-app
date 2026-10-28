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
import { waitForRtcFixture } from './fixtures/remote-rtc-process.mjs';

const root = resolve(fileURLToPath(new URL('..', import.meta.url)));

// The real production files, bundled the way electron-vite would: the hidden
// page and its preload, plus a fixture main that drives them.
const node = (entry, out) => build({ entryPoints: [join(root, entry)], outfile: out, bundle: true, platform: 'node', format: 'cjs', external: ['electron'], logLevel: 'error' });
const browser = (entry, out) => build({ entryPoints: [join(root, entry)], outfile: out, bundle: true, platform: 'browser', format: 'esm', logLevel: 'error' });

test('real Electron Remote Control transport: hidden WebRTC window, MessagePort, 2 MiB both ways, lockdown, RtdbClient over net.fetch', { timeout: 180000 }, async () => {
  const temp = await mkdtemp(join(tmpdir(), 'lingxi-remote-rtc-'));
  let child;
  try {
    await Promise.all([
      node('test/fixtures/remote-rtc-electron.ts', join(temp, 'main.cjs')),
      node('src/preload/remoteRtc.ts', join(temp, 'remoteRtc.cjs')),
      browser('src/renderer/remoteRtc/main.ts', join(temp, 'page.js')),
      browser('test/fixtures/remote-rtc-viewer.ts', join(temp, 'viewer.js')),
      browser('../web/src/remote/fileWorker.ts', join(temp, 'file-worker.js')),
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
      throw new Error(`Electron is not installed: ${error.message}`, { cause: error });
    }
    child = spawn(electron, [join(temp, 'main.cjs')], { cwd: root, env, stdio: ['ignore', 'pipe', 'pipe'] });
    // The fixture reports only after its real assertions; an unsigned Electron
    // may linger after quit, so cleanup terminates it after the passing result.
    const result = await waitForRtcFixture(child);
    assert.equal(result.passed, true);
    assert.equal(result.bytes, Number(process.env.LINGXI_TEST_BYTES ?? 2 * 1024 * 1024));
    assert.ok(['host', 'prflx', 'srflx'].includes(result.candidate), `candidate type ${result.candidate}`);
  } finally {
    if (child && child.pid !== undefined && child.exitCode === null && child.signalCode === null) {
      child.kill('SIGTERM');
      await new Promise((done) => { const timer = setTimeout(() => { child.kill('SIGKILL'); done(); }, 1500); child.once('exit', () => { clearTimeout(timer); done(); }); });
    }
    await rm(temp, { recursive: true, force: true });
  }
});
