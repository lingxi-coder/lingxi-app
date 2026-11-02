import assert from 'node:assert/strict';
import { test } from 'node:test';
import { sessionsInLogicalScope } from '../src/renderer/bridge/conversationCatalog';

test('workspace relocation preserves Global and project membership in both spaces', () => {
  const row = (uuid: string, mode: 'chat' | 'code') => ({ uuid, mode, title: uuid, path: `/transcripts/${uuid}`,
    modified_rfc3339: '', message_count: 1 });
  const globalChat = row('global-chat', 'chat'), projectChat = row('project-chat', 'chat');
  const globalCode = row('global-code', 'code'), undiscovered = row('unregistered', 'chat');
  const catalogs = {
    '/global': { sessions: [globalChat] }, // stale alias must not duplicate relocated owner
    '/project': { sessions: [globalChat, globalCode, undiscovered] },
    '/elsewhere': { sessions: [projectChat] },
  };
  const owners = [
    { sessionId: 'global-chat', mode: 'chat' as const, projectId: null, projectPath: '/project' },
    { sessionId: 'global-code', mode: 'code' as const, projectId: null, projectPath: '/project' },
    { sessionId: 'project-chat', mode: 'chat' as const, projectId: '/project', projectPath: '/elsewhere' },
  ];
  assert.deepEqual(sessionsInLogicalScope(catalogs, owners, 'chat', null, ['/global']).map(({ projectPath, session }) => [projectPath, session.uuid]),
    [['/project', 'global-chat']]);
  assert.deepEqual(sessionsInLogicalScope(catalogs, owners, 'code', null, ['/global']).map(({ projectPath, session }) => [projectPath, session.uuid]),
    [['/project', 'global-code']]);
  assert.deepEqual(sessionsInLogicalScope(catalogs, owners, 'chat', '/project', ['/global']).map(({ projectPath, session }) => [projectPath, session.uuid]),
    [['/project', 'unregistered'], ['/elsewhere', 'project-chat']]);
});
