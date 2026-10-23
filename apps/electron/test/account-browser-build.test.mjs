import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import test from 'node:test';
import { build } from 'electron-vite';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');

test('packaged browser sign-in keeps Node shims outside the served login script', { timeout: 30_000 }, async t => {
  const temp = mkdtempSync(join(tmpdir(), 'lingxi-account-browser-build-'));
  t.after(() => rmSync(temp, { recursive: true, force: true }));
  symlinkSync(join(root, 'node_modules'), join(temp, 'node_modules'), 'dir');
  const entry = join(temp, 'entry.mjs');
  // The real main bundle also contains CommonJS dependencies. Keep that build
  // condition here so electron-vite's ESM shim runs on the embedded HTML.
  writeFileSync(entry, `export { browserSignIn } from ${JSON.stringify(join(root, 'src/main/account-browser.ts'))};\nexport const resolveFirebase = () => require.resolve('firebase/package.json');\n`);
  const configFile = join(temp, 'electron.vite.config.mjs');
  writeFileSync(configFile, `export default {main:{root:${JSON.stringify(root)},build:{outDir:${JSON.stringify(join(temp, 'out'))},rollupOptions:{input:${JSON.stringify(entry)},preserveEntrySignatures:'strict',output:{format:'es',entryFileNames:'entry.mjs'}}}}};`);
  await build({ configFile, logLevel: 'silent', ignoreConfigWarning: true });
  const { browserSignIn } = await import(pathToFileURL(join(temp, 'out/entry.mjs')).href);
  let opened;
  const ready = new Promise(resolve => { opened = resolve; });
  const controller = new AbortController();
  const pending = browserSignIn({ apiKey: 'test-key', authDomain: 'demo-lingxi.firebaseapp.com', projectId: 'demo-lingxi', appId: 'test-app' }, 'google.com', async url => opened(url), controller.signal);
  // Attach the rejection handler before cleanup can cancel the callback.
  const cancelled = assert.rejects(pending, /取消/);
  try {
    const url = await ready;
    const response = await fetch(url);
    const html = await response.text();
    const script = html.match(/<script type="module"[^>]*>([\s\S]*?)<\/script>/)?.[1];
    assert.ok(script);
    assert.doesNotMatch(script, /node:|__cjs_mod__|createRequire|import\.meta/);
    assert.match(script, /button\.onclick\s*=/);
    assert.match(response.headers.get('content-security-policy'), /script-src 'self' 'nonce-/);
    for (const name of ['firebase-app.js', 'firebase-auth.js']) {
      const sdk = await fetch(new URL(`/${name}`, url));
      assert.equal(sdk.status, 200);
      assert.match(sdk.headers.get('content-type'), /javascript/);
      assert.doesNotMatch(await sdk.text(), /from["']https:\/\/www\.gstatic\.com/);
    }
  } finally {
    controller.abort();
    await cancelled;
  }
});
