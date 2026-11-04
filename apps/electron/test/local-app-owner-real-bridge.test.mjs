import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { test } from 'node:test';
import { build } from 'esbuild';

const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
const binary = process.env.LINGXI_BRIDGE_SERVER_BIN;
test('real Bridge keeps a parked Local App Code owner and recovers its exact durable anchor', { skip: !binary, timeout: 120_000 }, async () => {
  const temp = await mkdtemp(join(tmpdir(), 'lingxi-local-app-owner-'));
  const entry = join(root, 'test/fixtures/local-app-owner-real-bridge.ts');
  const bundle = join(temp, 'main.cjs');
  let child;
  try {
    await build({ entryPoints: [entry], outfile: bundle, bundle: true, platform: 'node', format: 'cjs', external: ['electron'],
      define: { 'import.meta.url': JSON.stringify(pathToFileURL(entry).href) } });
    const env = { ...process.env, LINGXI_TEST_USER_DATA: temp, LINGXI_CONFIG_DIR: join(temp, 'brand'), LINGXI_BRIDGE_SERVER_BIN: binary };
    delete env.ELECTRON_RUN_AS_NODE;
    child = spawn(createRequire(import.meta.url)('electron'), [bundle], { cwd: root, env, stdio: ['ignore', 'pipe', 'pipe'] });
    let out = '', err = '';
    child.stdout.on('data', chunk => { out += chunk; }); child.stderr.on('data', chunk => { err += chunk; });
    const code = await new Promise((resolveCode, reject) => {
      const timer = setTimeout(() => { child.kill('SIGTERM'); reject(new Error(`Bridge owner fixture timed out\n${err}\n${out}`)); }, 105_000);
      child.once('error', error => { clearTimeout(timer); reject(error); });
      child.once('close', (value, signal) => { clearTimeout(timer); resolveCode(value ?? signal); });
    });
    assert.equal(code, 0, `Bridge owner fixture exited ${code}\n${err}\n${out}`);
    const line = out.trim().split('\n').find(value => value.startsWith('OWNER_RESULT:'));
    assert.ok(line, `No owner evidence\n${err}\n${out}`);
    assert.deepEqual(JSON.parse(line.slice('OWNER_RESULT:'.length)), {
      codeToChatParked: true, fixedHeader: true, title: 'Reading garden', exactSource: true, coldRecovery: true,
    });
  } finally {
    if (child && child.exitCode === null) child.kill('SIGTERM');
    await rm(temp, { recursive: true, force: true });
  }
});
