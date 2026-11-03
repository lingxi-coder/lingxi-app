import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { resolveDesktopAuthoringBinding } from '../src/main/local-apps/authoring.ts';
import { resolveActiveDraftSession } from '../src/main/sessionRuntimeManager.ts';

const sessionId = '12345678-1234-4234-8234-123456789abc';

async function directory(t: { after(fn: () => Promise<void>): void }, name: string): Promise<string> {
  const path = await fs.mkdtemp(join(tmpdir(), `desktop-authoring-${name}-`));
  t.after(() => fs.rm(path, { recursive: true, force: true }));
  return path;
}

test('Desktop authoring resolves only the real Code session cwd matching its selected Project', async t => {
  const projectPath = await directory(t, 'project');
  const canonicalPath = await fs.realpath(projectPath);
  const findCalls: unknown[] = [];
  const result = await resolveDesktopAuthoringBinding({ find: async (...args: unknown[]) => {
    findCalls.push(args);
    return { uuid: sessionId, mode: 'code', workspace_path: projectPath } as never;
  } }, { projectPath, sessionId });
  assert.deepEqual(findCalls, [[projectPath, sessionId]]);
  assert.deepEqual(result, { sourceDirectory: canonicalPath, projectRef: { kind: 'desktopPath', path: canonicalPath }, authoringSessionId: sessionId });
});

test('Desktop authoring rejects Chat and enforces the source Project cwd', async t => {
  const projectPath = await directory(t, 'project');
  const otherPath = await directory(t, 'other');
  await assert.rejects(resolveDesktopAuthoringBinding({ find: async () => ({ uuid: sessionId, mode: 'chat', workspace_path: projectPath } as never) },
    { projectPath, sessionId }), /local_app_development_requires_code|Code/);
  await assert.rejects(resolveDesktopAuthoringBinding({ find: async () => ({ uuid: sessionId, mode: 'code', workspace_path: otherPath } as never) },
    { projectPath, sessionId }), /dedicated Project cwd/);
  await assert.rejects(resolveDesktopAuthoringBinding({ find: async () => undefined }, { projectPath, sessionId }), /session/);
});

test('a host-created Code draft binds before its first prompt persists a catalog row', async t => {
  const projectPath = await directory(t, 'draft-project');
  const canonicalPath = await fs.realpath(projectPath);
  const result = await resolveDesktopAuthoringBinding({ find: async () => undefined }, { projectPath, sessionId }, {
    projectPath, sessionId, mode: 'code', engineCwd: projectPath,
  });
  assert.deepEqual(result, { sourceDirectory: canonicalPath, projectRef: { kind: 'desktopPath', path: canonicalPath }, authoringSessionId: sessionId });
});

test('unpersisted authoring drafts require Code and the matching locator and cwd', async t => {
  const projectPath = await directory(t, 'project');
  const otherPath = await directory(t, 'other');
  const noCatalogRow = { find: async () => undefined };
  const locator = { projectPath, sessionId };
  const codeDraft = { projectPath, sessionId, mode: 'code' as const, engineCwd: projectPath };
  await assert.rejects(resolveDesktopAuthoringBinding(noCatalogRow, locator, { ...codeDraft, mode: 'chat' }), /local_app_development_requires_code|Code/);
  await assert.rejects(resolveDesktopAuthoringBinding(noCatalogRow, locator, { ...codeDraft, sessionId: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa' }), /session/);
  await assert.rejects(resolveDesktopAuthoringBinding(noCatalogRow, locator, { ...codeDraft, projectPath: otherPath }), /dedicated Project cwd/);
  await assert.rejects(resolveDesktopAuthoringBinding(noCatalogRow, locator, { ...codeDraft, engineCwd: otherPath }), /dedicated Project cwd/);
  await assert.rejects(resolveDesktopAuthoringBinding(noCatalogRow, locator), /session/);
});

test('a persisted invalid or mismatched row cannot fall back to a live draft', async t => {
  const projectPath = await directory(t, 'project');
  const otherPath = await directory(t, 'other');
  const liveDraft = { projectPath, sessionId, mode: 'code' as const, engineCwd: projectPath };
  const locator = { projectPath, sessionId };
  await assert.rejects(resolveDesktopAuthoringBinding({ find: async () => ({
    uuid: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', mode: 'chat', workspace_path: projectPath,
  } as never) }, locator, liveDraft), /session/);
  await assert.rejects(resolveDesktopAuthoringBinding({ find: async () => ({
    uuid: sessionId, mode: 'code', workspace_path: otherPath,
  } as never) }, locator, liveDraft), /dedicated Project cwd/);
});

test('a pending Local App receipt verifies its exact recorded transcript before registration', async t => {
  const projectPath = await directory(t, 'exact-source');
  const selected = { scope: 'repository' as const, transcriptPath: join(projectPath, 'a', `${sessionId}.jsonl`) };
  const other = join(projectPath, 'b', `${sessionId}.jsonl`);
  const live = { projectPath, sessionId, mode: 'code' as const, engineCwd: projectPath };
  let path = selected.transcriptPath;
  const catalog = { find: async () => { throw new Error('must not find by UUID'); },
    select: async (_project: string, _id: string, selection: typeof selected) => {
      assert.deepEqual(selection, selected); return { uuid: sessionId, mode: 'code', workspace_path: projectPath, path } as never;
    } };
  assert.equal((await resolveDesktopAuthoringBinding(catalog, { projectPath, sessionId }, live, selected)).authoringSessionId, sessionId);
  path = other;
  await assert.rejects(resolveDesktopAuthoringBinding(catalog, { projectPath, sessionId }, live, selected), /source changed/);
  await assert.rejects(resolveDesktopAuthoringBinding({ find: async () => undefined, select: async () => undefined },
    { projectPath, sessionId }, live, selected), /source changed/);
});

test('only the exact active connected draft runtime can supply a missing catalog row', () => {
  const ref = { projectPath: '/project', sessionId };
  const runtime = { ...ref, sessionMode: 'code' as const, engineWorkspacePath: '/project', connectionState: { status: 'connected' } } as never;
  assert.deepEqual(resolveActiveDraftSession(ref, ref, ref, runtime, sessionId), {
    ...ref, mode: 'code', engineCwd: '/project',
  });
  assert.equal(resolveActiveDraftSession(ref, undefined, ref, runtime, sessionId), undefined);
  assert.equal(resolveActiveDraftSession(ref, ref, undefined, runtime, sessionId), undefined);
  assert.equal(resolveActiveDraftSession(ref, ref, ref, undefined, sessionId), undefined);
  assert.deepEqual(resolveActiveDraftSession(ref, ref, ref, { ...runtime, sessionMode: 'chat' }, sessionId), { ...ref, mode: 'chat', engineCwd: '/project' });
  assert.equal(resolveActiveDraftSession(ref, ref, ref, { ...runtime, sessionId: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa' }, sessionId), undefined);
  assert.equal(resolveActiveDraftSession(ref, ref, ref, { ...runtime, engineWorkspacePath: undefined }, sessionId), undefined);
  assert.equal(resolveActiveDraftSession(ref, ref, ref, { ...runtime, connectionState: { status: 'idle' } }, sessionId), undefined);
  assert.equal(resolveActiveDraftSession(ref, ref, ref, runtime, null), undefined);
});
