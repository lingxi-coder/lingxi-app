import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { ProjectSessionCatalog } from '../src/main/session-catalog';

function fakeChild(stdoutBody: string) {
  const child = new EventEmitter() as EventEmitter & {
    stdout: EventEmitter;
    stderr: EventEmitter;
    kill(): boolean;
  };
  child.stdout = new EventEmitter();
  child.stderr = new EventEmitter();
  child.kill = () => true;
  queueMicrotask(() => {
    child.stdout.emit('data', Buffer.from(stdoutBody));
    child.emit('close', 0, null);
  });
  return child;
}

test('project catalog uses the allowlisted environment and validates session UUIDs', async () => {
  const project = mkdtempSync(join(tmpdir(), 'lingxi-catalog-project-'));
  let spawnedEnv: NodeJS.ProcessEnv | undefined;
  const validId = '11111111-2222-4333-8444-555555555555';
  const catalog = new ProjectSessionCatalog({
    serverBin: '/bridge-server',
    spawnProcess: ((_bin: string, _args: readonly string[], options: { env?: NodeJS.ProcessEnv }) => {
      spawnedEnv = options.env;
      return fakeChild(JSON.stringify({
        version: 1,
        sessions: [{ uuid: validId, title: 'Session', modified_rfc3339: '', message_count: 1, path: 'session.jsonl' }],
      }));
    }) as any,
  });
  const previousSecret = process.env['ANTHROPIC_API_KEY'];
  process.env['ANTHROPIC_API_KEY'] = 'must-not-cross-catalog-boundary';
  try {
    const result = await catalog.list(project);
    assert.equal(result.sessions[0]?.uuid, validId);
    assert.equal(spawnedEnv?.['ANTHROPIC_API_KEY'], undefined);
  } finally {
    if (previousSecret === undefined) delete process.env['ANTHROPIC_API_KEY'];
    else process.env['ANTHROPIC_API_KEY'] = previousSecret;
    rmSync(project, { recursive: true, force: true });
  }
});

test('project catalog rejects malformed session identities', async () => {
  const project = mkdtempSync(join(tmpdir(), 'lingxi-catalog-invalid-'));
  const catalog = new ProjectSessionCatalog({
    serverBin: '/bridge-server',
    spawnProcess: (() => fakeChild(JSON.stringify({
      version: 1,
      sessions: [{ uuid: 'not-a-uuid', title: 'Bad', modified_rfc3339: '', message_count: 1, path: 'bad.jsonl' }],
    }))) as any,
  });
  try {
    await assert.rejects(catalog.list(project), /invalid session row/);
  } finally {
    rmSync(project, { recursive: true, force: true });
  }
});

test('project catalog keeps the UI list bounded but supports exact lookup beyond the bound', async () => {
  const project = mkdtempSync(join(tmpdir(), 'lingxi-catalog-exact-'));
  const rows = Array.from({ length: 201 }, (_, index) => ({
    uuid: `00000000-0000-4${String(index).padStart(3, '0')}-8000-${String(index).padStart(12, '0')}`,
    title: `Session ${index}`,
    modified_rfc3339: '',
    message_count: 1,
    path: `session-${index}.jsonl`,
  }));
  const catalog = new ProjectSessionCatalog({
    serverBin: '/bridge-server',
    spawnProcess: (() => fakeChild(JSON.stringify({ version: 1, sessions: rows }))) as any,
  });
  const outsideBound = rows[200].uuid;
  try {
    const listed = await catalog.list(project);
    assert.equal(listed.sessions.length, 200);
    assert.equal(listed.sessions.some((session) => session.uuid === outsideBound), false);
    assert.equal((await catalog.find(project, outsideBound))?.title, 'Session 200');
  } finally {
    rmSync(project, { recursive: true, force: true });
  }
});

test('project catalog preserves validated zero-count rows and applies the sidebar limit to catalog output', async () => {
  const project = mkdtempSync(join(tmpdir(), 'lingxi-catalog-resumable-'));
  const emptyAnchor = {
    uuid: '11111111-2222-4333-8444-555555555556',
    title: 'New mobile session',
    modified_rfc3339: '2026-08-26T00:00:00Z',
    message_count: 0,
    path: 'empty.jsonl',
  };
  const metadataOnly = Array.from({ length: 200 }, (_, index) => ({
    uuid: `10000000-0000-4${String(index).padStart(3, '0')}-8000-${String(index).padStart(12, '0')}`,
    title: 'Control Rename',
    modified_rfc3339: `2026-08-26T00:${String(index % 60).padStart(2, '0')}:00Z`,
    message_count: 0,
    path: `metadata-${index}.jsonl`,
  }));
  const resumable = {
    uuid: '22222222-3333-4444-8555-666666666666',
    title: 'Inspect the session catalog',
    modified_rfc3339: '2026-08-25T00:00:00Z',
    message_count: 2,
    path: 'conversation.jsonl',
  };
  const catalog = new ProjectSessionCatalog({
    serverBin: '/bridge-server',
    spawnProcess: (() => fakeChild(JSON.stringify({
      version: 1,
      sessions: [emptyAnchor, ...metadataOnly, resumable],
    }))) as any,
  });
  try {
    const listed = await catalog.list(project);
    assert.equal(listed.sessions[0]?.uuid, emptyAnchor.uuid);
    assert.equal(listed.sessions.length, 200);
    assert.equal((await catalog.find(project, emptyAnchor.uuid))?.message_count, 0);
    assert.equal((await catalog.find(project, resumable.uuid))?.title, resumable.title);
  } finally {
    rmSync(project, { recursive: true, force: true });
  }
});
