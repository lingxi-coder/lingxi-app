import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { resolveDesktopAuthoringBinding } from '../src/main/local-apps/authoring.ts';

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

test('Desktop authoring rejects chat sessions and Code sessions whose cwd differs from the source Project', async t => {
  const projectPath = await directory(t, 'project');
  const otherPath = await directory(t, 'other');
  await assert.rejects(resolveDesktopAuthoringBinding({ find: async () => ({ uuid: sessionId, mode: 'chat', workspace_path: projectPath } as never) },
    { projectPath, sessionId }), /Code session/);
  await assert.rejects(resolveDesktopAuthoringBinding({ find: async () => ({ uuid: sessionId, mode: 'code', workspace_path: otherPath } as never) },
    { projectPath, sessionId }), /dedicated Code Project cwd/);
  await assert.rejects(resolveDesktopAuthoringBinding({ find: async () => undefined }, { projectPath, sessionId }), /Code session/);
});
