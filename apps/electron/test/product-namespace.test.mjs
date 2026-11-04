import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { test } from 'node:test';
import { createPackage } from '@electron/asar';
import { build } from 'esbuild';

import { packagedProductNamespace, productNamespace } from '../scripts/product-namespace.mjs';
import { verifyDesktopProductNamespace } from '../scripts/verify-desktop-package.mjs';
import { productNamespaceBinding, productNamespaceBindingFromBundle } from '../src/main/productNamespaceCodec.mjs';

async function writeArchive(root, platform, namespaceBytes, binding, { missing = false, noBinding = false } = {}) {
  const contents = join(root, 'archive');
  const payload = join(root, 'Sample.app');
  const resources = platform === 'darwin' ? join(payload, 'Contents', 'Resources') : join(payload, 'resources');
  await mkdir(join(contents, 'out', 'main'), { recursive: true });
  await mkdir(resources, { recursive: true });
  await writeFile(join(contents, 'out', 'main', 'index.js'), noBinding ? 'console.log("no binding");' : `const binding = ${JSON.stringify(binding)};`);
  if (!missing) await writeFile(join(contents, 'out', 'main', 'product-namespace.json'), namespaceBytes);
  await createPackage(contents, join(resources, 'app.asar'));
  return payload;
}

test('packaged acceptance reads the target app namespace bound to its main bundle', async t => {
  const root = await mkdtemp(join(tmpdir(), 'product-namespace-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const expected = { ...productNamespace(), sourceRevision: 'b'.repeat(40) };
  const bytes = Buffer.from(JSON.stringify(expected));
  const payload = await writeArchive(root, 'darwin', bytes, productNamespaceBinding(bytes));
  assert.deepEqual(packagedProductNamespace(payload), expected);
  assert.notEqual(expected.sourceRevision, productNamespace().sourceRevision);
});

for (const platform of ['darwin', 'win32', 'linux']) {
  test(`${platform} desktop package validates its exact namespace resource and bundle binding`, async t => {
    const root = await mkdtemp(join(tmpdir(), `product-namespace-${platform}-`));
    t.after(() => rm(root, { recursive: true, force: true }));
    const profile = productNamespace();
    const original = Buffer.from(JSON.stringify(profile));
    const binding = productNamespaceBinding(original);
    let payload = await writeArchive(join(root, 'valid'), platform, original, binding);
    assert.deepEqual(verifyDesktopProductNamespace(payload, platform), profile);
    const malformed = Buffer.from('{bad json');
    const incomplete = Buffer.from('{"version":1}');
    for (const [name, data, options, expectedBinding] of [
      ['missing', original, { missing: true }, binding],
      ['malformed', malformed, {}, productNamespaceBinding(malformed)],
      ['incomplete', incomplete, {}, productNamespaceBinding(incomplete)],
      ['different', Buffer.from(JSON.stringify({ ...profile, sourceRevision: 'c'.repeat(40) })), {}, binding],
      ['unbound', original, { noBinding: true }, binding],
    ]) {
      payload = await writeArchive(join(root, name), platform, data, expectedBinding, options);
      assert.throws(() => verifyDesktopProductNamespace(payload, platform), /Packaged product namespace is missing, invalid, or does not match/);
    }
  });
}

test('bundled main process reads and validates the packaged resource rather than an inlined object', async t => {
  const root = await mkdtemp(join(tmpdir(), 'product-namespace-main-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const resourceDir = join(root, 'out', 'main');
  await mkdir(resourceDir, { recursive: true });
  const path = join(resourceDir, 'product-namespace.json');
  const expected = productNamespace();
  const bytes = Buffer.from(JSON.stringify(expected));
  const binding = productNamespaceBinding(bytes);
  await writeFile(path, bytes);
  const built = await build({ entryPoints: [resolve('src/main/productNamespace.ts')], bundle: true, write: false,
    platform: 'node', format: 'cjs', external: ['electron'], define: { __PRODUCT_NAMESPACE_BINDING__: JSON.stringify(binding) } });
  const source = built.outputFiles[0].text;
  assert.equal(productNamespaceBindingFromBundle(Buffer.from(source)), binding);
  const nativeRequire = createRequire(import.meta.url);
  const load = (code = source) => {
    const module = { exports: {} };
    new Function('module', 'exports', 'require', code)(module, module.exports, name =>
      name === 'electron' ? { app: { isPackaged: true, getAppPath: () => root } } : nativeRequire(name));
    return module.exports.productNamespace;
  };
  assert.deepEqual(load(), expected);
  const unbound = await build({ entryPoints: [resolve('src/main/productNamespace.ts')], bundle: true, write: false,
    platform: 'node', format: 'cjs', external: ['electron'] });
  assert.throws(() => load(unbound.outputFiles[0].text), /packaged main bundle has no product namespace binding/);
  await writeFile(path, JSON.stringify({ ...expected, sourceRevision: 'd'.repeat(40) }));
  assert.throws(load, /Product namespace is missing, invalid, or does not match/);
  await rm(path);
  assert.throws(load, /Product namespace is missing, invalid, or does not match/);
});

test('source main process resolves the same generated namespace from the repository root', () => {
  const entry = pathToFileURL(resolve('src/main/productNamespace.ts')).href;
  const loader = resolve('../../packages/bridge-client/node_modules/tsx/dist/loader.mjs');
  const output = execFileSync(process.execPath, ['--import', loader, '--input-type=module', '-e',
    `import { productNamespace } from ${JSON.stringify(entry)}; console.log(JSON.stringify(productNamespace));`],
  { cwd: resolve('../..'), encoding: 'utf8' });
  assert.deepEqual(JSON.parse(output), productNamespace());
});

test('source main process does not pick a different checkout from the working directory', async t => {
  const other = await mkdtemp(join(tmpdir(), 'other-desktop-checkout-'));
  t.after(() => rm(other, { recursive: true, force: true }));
  const namespaceDir = join(other, '.native-cache', 'branding');
  await mkdir(namespaceDir, { recursive: true });
  await writeFile(join(other, 'package.json'), JSON.stringify({ name: 'lingxi-code-desktop' }));
  await writeFile(join(namespaceDir, 'product-namespace.json'), JSON.stringify({ ...productNamespace(), sourceRevision: 'e'.repeat(40) }));
  const entry = pathToFileURL(resolve('src/main/productNamespace.ts')).href;
  const loader = resolve('../../packages/bridge-client/node_modules/tsx/dist/loader.mjs');
  const output = execFileSync(process.execPath, ['--import', loader, '--input-type=module', '-e',
    `import { productNamespace } from ${JSON.stringify(entry)}; console.log(JSON.stringify(productNamespace));`],
  { cwd: other, encoding: 'utf8' });
  assert.deepEqual(JSON.parse(output), productNamespace());
});
