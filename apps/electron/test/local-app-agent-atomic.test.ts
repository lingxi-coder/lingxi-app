import { test } from 'node:test';
import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { atomicJson } from '../src/main/local-apps/store.ts';

test('atomic JSON does not rename an aborted temporary write over the original', async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-atomic-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const target = join(root, 'record.json');
  await fs.writeFile(target, '{"original":true}');
  const writable = fs as any;
  const writeFile = writable.writeFile, rename = writable.rename;
  let release!: () => void, entered!: () => void, renameCalls = 0;
  const blocked = new Promise<void>(resolve => { release = resolve; });
  const waiting = new Promise<void>(resolve => { entered = resolve; });
  writable.writeFile = async (...args: any[]) => {
    const result = await writeFile(...args);
    if (typeof args[0] === 'string' && args[0].startsWith(`${target}.`) && args[0].endsWith('.tmp')) { entered(); await blocked; }
    return result;
  };
  writable.rename = async (...args: any[]) => { renameCalls++; return rename(...args); };
  const controller = new AbortController();
  try {
    const pending = assert.rejects(atomicJson(target, { original: false }, controller.signal), /revoked/);
    await waiting;
    controller.abort(new Error('revoked'));
    release(); await pending;
    assert.equal(renameCalls, 0);
    assert.equal(await fs.readFile(target, 'utf8'), '{"original":true}');
    assert.deepEqual(await fs.readdir(root), ['record.json']);
  } finally {
    release(); writable.writeFile = writeFile; writable.rename = rename;
  }
});
