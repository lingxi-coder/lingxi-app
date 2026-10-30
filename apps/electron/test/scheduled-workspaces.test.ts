import assert from 'node:assert/strict';
import { mkdtemp, mkdir, rm, readdir, readFile, writeFile, symlink, realpath } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { createScheduledRunWorkspace } from '../src/main/scheduled-workspaces';

const sessionId = '11111111-2222-4333-8444-555555555555';
test('layout uses the supplied brand root and preserves memory across independent runs', async t => {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'scheduled-layout-')));
  t.after(() => rm(root, { recursive: true, force: true }));
  const home = join(root, '.another-brand');
  const first = await createScheduledRunWorkspace(home, '../../task', sessionId, '../晨间 简报', new Date(2026, 9, 30));
  assert.equal(first.directory, join(home, 'workspaces', '2026-10-30', `晨间-简报-${sessionId}`));
  assert.deepEqual((await readdir(first.directory)).sort(), ['outputs', 'work']);
  assert.ok(first.memory.startsWith(join(home, 'automations') + '/'));
  await writeFile(first.memory, 'Keep this task memory');
  const second = await createScheduledRunWorkspace(home, '../../task', '22222222-3333-4444-8555-666666666666', 'Other run', new Date(2026, 9, 31));
  assert.equal(second.memory, first.memory);
  assert.equal(await readFile(second.memory, 'utf8'), 'Keep this task memory');
  await assert.rejects(createScheduledRunWorkspace(home, 'task', sessionId, '../晨间 简报', new Date(2026, 9, 30)), { code: 'EEXIST' });
});

test('workspace root symlinks cannot redirect task output into unrelated directories', async t => {
  const root = await mkdtemp(join(tmpdir(), 'scheduled-symlink-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const home = join(root, '.another-brand');
  const outside = join(root, 'outside');
  await mkdir(home); await mkdir(outside);
  await symlink(outside, join(home, 'workspaces'), 'dir');
  await assert.rejects(createScheduledRunWorkspace(home, 'task', sessionId, 'Task'), /real directory/);
  assert.deepEqual(await readdir(outside), []);
});
