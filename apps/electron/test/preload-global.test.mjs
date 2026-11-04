import assert from 'node:assert/strict';
import { test } from 'node:test';
import { runInNewContext } from 'node:vm';
import { build } from 'esbuild';

test('the bundled preload preserves the desktop global ABI', async () => {
  const result = await build({
    entryPoints: ['src/preload/index.ts'], bundle: true, write: false,
    platform: 'node', format: 'cjs', external: ['electron'], logLevel: 'silent',
  });
  const exposed = [];
  const electron = {
    contextBridge: { exposeInMainWorld: (name, api) => exposed.push([name, api]) },
    ipcRenderer: { on() {}, removeListener() {}, invoke() { return Promise.resolve(); }, send() {} },
  };
  runInNewContext(result.outputFiles[0].text, {
    module: { exports: {} }, exports: {},
    require(name) { assert.equal(name, 'electron'); return electron; },
    process: { platform: 'darwin', env: {} }, console, setTimeout, clearTimeout, Buffer, URL,
  });
  assert.equal(exposed.length, 1);
  assert.equal(exposed[0][0], 'lingxi');
  assert.equal(typeof exposed[0][1].account.request, 'function');
  assert.equal(typeof exposed[0][1].teams.localBots, 'function');
  assert.equal(typeof exposed[0][1].saveImage, 'function');
});
