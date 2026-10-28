import assert from 'node:assert/strict';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

import { buildFreshBridgeClient, syncBuiltBridgeClient } from '../scripts/bridge-client-dist.mjs';

function fixture() {
  const root = mkdtempSync(join(tmpdir(), 'desktop-sdk-copy-'));
  const source = join(root, 'source');
  const installed = join(root, 'installed');
  mkdirSync(join(source, 'dist'), { recursive: true });
  writeFileSync(join(source, 'package.json'), JSON.stringify({
    name: '@local/sdk', version: '1.0.0', exports: { '.': './dist/index.js' }, dependencies: { helper: '1.0.0' },
  }));
  writeFileSync(join(source, 'dist', 'index.js'), 'export const revision = "fresh";\n');
  return { root, source, installed };
}

test('packaging refreshes a copied SDK from the just-built output', () => {
  const { root, source, installed } = fixture();
  try {
    cpSync(source, installed, { recursive: true });
    writeFileSync(join(installed, 'dist', 'index.js'), 'stale');
    writeFileSync(join(installed, 'dist', 'removed.js'), 'obsolete');
    mkdirSync(join(source, 'dist', 'nested'));
    writeFileSync(join(source, 'dist', 'nested', 'types.d.ts'), 'export {};\n');
    syncBuiltBridgeClient(source, installed);
    assert.equal(readFileSync(join(installed, 'dist', 'index.js'), 'utf8'), readFileSync(join(source, 'dist', 'index.js'), 'utf8'));
    assert.equal(readFileSync(join(installed, 'dist', 'nested', 'types.d.ts'), 'utf8'), 'export {};\n');
    assert.throws(() => readFileSync(join(installed, 'dist', 'removed.js')), { code: 'ENOENT' });
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('packaging leaves a linked SDK and its source output intact', () => {
  const { root, source, installed } = fixture();
  try {
    symlinkSync(source, installed, process.platform === 'win32' ? 'junction' : 'dir');
    syncBuiltBridgeClient(source, installed);
    assert.equal(readFileSync(join(source, 'dist', 'index.js'), 'utf8'), 'export const revision = "fresh";\n');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('packaging cannot fall back to an old copy when fresh output is missing', () => {
  const { root, source, installed } = fixture();
  try {
    cpSync(source, installed, { recursive: true });
    writeFileSync(join(installed, 'dist', 'index.js'), 'stale');
    rmSync(join(source, 'dist', 'index.js'));
    assert.throws(() => syncBuiltBridgeClient(source, installed), { code: 'ENOENT' });
    assert.equal(readFileSync(join(installed, 'dist', 'index.js'), 'utf8'), 'stale');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('a fresh build removes outputs belonging to deleted sources before copying', () => {
  const { root, source, installed } = fixture();
  try {
    cpSync(source, installed, { recursive: true });
    writeFileSync(join(source, 'dist', 'removed.js'), 'output from a deleted source');
    writeFileSync(join(installed, 'dist', 'removed.js'), 'output from a deleted source');
    buildFreshBridgeClient(source, installed, () => {
      assert.throws(() => readFileSync(join(source, 'dist', 'removed.js')), { code: 'ENOENT' });
      mkdirSync(join(source, 'dist'));
      writeFileSync(join(source, 'dist', 'index.js'), 'rebuilt');
    });
    assert.equal(readFileSync(join(installed, 'dist', 'index.js'), 'utf8'), 'rebuilt');
    assert.throws(() => readFileSync(join(installed, 'dist', 'removed.js')), { code: 'ENOENT' });
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('same-version copies with stale exports or dependencies fail before the build', () => {
  for (const change of [
    { exports: { '.': './dist/old.js' } },
    { dependencies: { helper: '2.0.0' } },
  ]) {
    const { root, source, installed } = fixture();
    try {
      cpSync(source, installed, { recursive: true });
      const manifest = JSON.parse(readFileSync(join(installed, 'package.json'), 'utf8'));
      writeFileSync(join(installed, 'package.json'), JSON.stringify({ ...manifest, ...change }));
      let built = false;
      assert.throws(() => buildFreshBridgeClient(source, installed, () => { built = true; }), /manifest differs.*npm ci/);
      assert.equal(built, false);
      assert.equal(readFileSync(join(source, 'dist', 'index.js'), 'utf8'), 'export const revision = "fresh";\n');
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  }
});

test('packaging refuses a mismatched copy or a link to another SDK', () => {
  const { root, source, installed } = fixture();
  try {
    cpSync(source, installed, { recursive: true });
    writeFileSync(join(installed, 'package.json'), JSON.stringify({ name: '@local/sdk', version: '0.0.1' }));
    assert.throws(() => syncBuiltBridgeClient(source, installed), /manifest differs/);
    assert.equal(readFileSync(join(installed, 'dist', 'index.js'), 'utf8'), 'export const revision = "fresh";\n');
    rmSync(installed, { recursive: true, force: true });
    const other = join(root, 'other');
    cpSync(source, other, { recursive: true });
    symlinkSync(other, installed, process.platform === 'win32' ? 'junction' : 'dir');
    assert.throws(() => syncBuiltBridgeClient(source, installed), /unexpected location/);
    assert.equal(readFileSync(join(other, 'dist', 'index.js'), 'utf8'), 'export const revision = "fresh";\n');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
