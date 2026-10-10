import assert from 'node:assert/strict';
import { appendFileSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execFileSync } from 'node:child_process';
import { test } from 'node:test';
import { inspectModBun, stageModBun, verifyModBun } from '../scripts/mod-bun.mjs';
import { DEFAULT_SECRET_CANARY, scanTreeForForbiddenContent } from '../scripts/package-support.mjs';

const configured = process.env.LINGXI_MOD_BUN_EXECUTABLE || 'bun';
let compiler;
try {
  compiler = inspectModBun(configured, process.platform, process.arch);
} catch (error) {
  if (process.env.CI || process.env.LINGXI_MOD_BUN_EXECUTABLE) throw error;
}

test('staged Bun compiler executes classic JSX with its recorded identity', { skip: !compiler && 'Bun is not installed' }, () => {
  const resources = mkdtempSync(join(tmpdir(), 'lingxi-mod-bun-'));
  try {
    const executable = stageModBun(resources, {
      ...compiler, noticesUrl: 'https://example.test/test-fixture-notices',
      notices: 'Test fixture for Bun MIT and JavaScriptCore notices identity.\n',
    });
    const manifest = verifyModBun(resources, process.platform, process.arch);
    assert.doesNotThrow(() => scanTreeForForbiddenContent(resources));
    assert.equal(manifest.version, compiler.version);
    assert.equal(manifest.revision, compiler.revision);
    // Run the copied compiler from a clean directory, without a globally installed runtime.
    const output = execFileSync(executable, ['--no-env-file', '--print', `
      const source = new Bun.Transpiler({loader:'tsx',macro:false}).transformSync(
        '/** @jsxRuntime classic */ /** @jsx h */ /** @jsxFrag Fragment */\\nexport default () => <Text>drawn</Text>'
      );
      const h = (type, props, ...children) => ({type,props,children});
      const Text = 'Text';
      const component = new Function('h','Text',source.replace('export default','return'))(h,Text);
      JSON.stringify(component());
    `], { cwd: resources, encoding: 'utf8', timeout: 15_000 });
    assert.deepEqual(JSON.parse(output), { type: 'Text', props: null, children: ['drawn'] });
    const original = readFileSync(executable);
    appendFileSync(executable, '\0');
    assert.throws(() => verifyModBun(resources, process.platform, process.arch), /recorded identity/);
    writeFileSync(executable, original);
    appendFileSync(join(resources, 'Bun-LICENSE.md'), 'changed');
    assert.throws(() => verifyModBun(resources, process.platform, process.arch), /recorded identity/);
  } finally {
    rmSync(resources, { recursive: true, force: true });
  }
});

test('packaging rejects Node as the Mod compiler', () => {
  assert.throws(() => inspectModBun(process.execPath, process.platform, process.arch), /working Bun executable/);
});

test('upstream compiler build prefixes do not permit developer paths or secret canaries', () => {
  const resources = mkdtempSync(join(tmpdir(), 'lingxi-mod-bun-scan-'));
  const path = join(resources, 'artifact');
  try {
    writeFileSync(path, '/Users/administrator/private/plugin.ts');
    assert.throws(() => scanTreeForForbiddenContent(resources), /absolute macOS user path/);
    writeFileSync(path, '/Users/runner/work/_temp/private/plugin.ts');
    assert.throws(() => scanTreeForForbiddenContent(resources), /absolute macOS user path/);
    writeFileSync(path, process.env.LINGXI_PACKAGE_SECRET_CANARY || DEFAULT_SECRET_CANARY);
    assert.throws(() => scanTreeForForbiddenContent(resources), /package secret canary/);
  } finally {
    rmSync(resources, { recursive: true, force: true });
  }
});
