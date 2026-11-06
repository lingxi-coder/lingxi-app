import assert from 'node:assert/strict';
import { test } from 'node:test';
import { renderToStaticMarkup } from 'react-dom/server';
import { createElement } from 'react';
import * as React from 'react';
import { localAppWorkspaceGroups } from '../src/renderer/bridge/localAppWorkspace';
import { LocalAppsSidebar } from '../src/renderer/components/LocalAppsSidebar';
import { patchSidebarPreferences } from '../src/main/host-utils';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import type { DesktopLocalApp } from '../src/shared/localApps';

(globalThis as any).React = React;

test('Apps groups only native authoring projects, keeps distinct drafts and exact transcript sources', () => {
  const source = (sessionId: string) => ({ name: 'Notes', projectPath: '/app', sessionId,
    source: { scope: 'repository' as const, transcriptPath: `/saved/${sessionId}.jsonl` } });
  const groups = localAppWorkspaceGroups([], [source('a'), source('b')], {
    '/app': { sessions: [{ uuid: 'a', title: 'First requirement', path: '/wrong/a.jsonl', mode: 'code' },
      { uuid: 'chat-mode', title: 'Unrelated Chat owner', path: '/wrong/chat.jsonl', mode: 'chat' }] },
    '/other/.apps/fake': { sessions: [{ uuid: 'fake', title: 'Not an app', path: '/fake.jsonl', mode: 'code' }] },
  }, [{ projectPath: '/app', sessionId: 'c', mode: 'code' }]);
  assert.equal(groups.length, 1);
  assert.equal(groups[0].projectPath, '/app');
  assert.deepEqual(groups[0].chats.map(chat => chat.sessionId), ['b', 'a', 'c']);
  assert.deepEqual(groups[0].chats[1].selection, source('a').source, 'catalog aliases cannot replace the native verified source');
  assert.equal(groups[0].chats[1].title, 'First requirement');
  const markup = renderToStaticMarkup(createElement(Theme.Provider, { value: tokens(false) }, createElement(LocalAppsSidebar, { groups, activeSessionId: 'a', query: '',
    chatVisible: true, libraryVisible: false, busy: false, error: '', onLibrary() {}, onCreate() {}, onOpen() {}, onEdit() {}, onNewChat() {}, onChat() {}, onRetry() {} })));
  assert.match(markup, /Apps and development chats/);
  assert.match(markup, /aria-current="page"/);
  assert.match(markup, /First requirement/);
  assert.doesNotMatch(markup, /Projects and sessions/);
});

test('moving a Global Code chat into an App cwd does not turn it into an App project session', () => {
  const global = { projectPath: '/app', projectId: null, sessionId: 'global', mode: 'code' as const };
  const groups = localAppWorkspaceGroups([], [{ name: 'Project', projectPath: '/app', sessionId: 'native',
    source: { scope: 'current', transcriptPath: '/native.jsonl' } }], {
    '/app': { sessions: [{ uuid: 'global', title: 'Global', path: '/global.jsonl', mode: 'code' }] },
  }, [global], [global]);
  assert.deepEqual(groups[0].chats.map(chat => chat.sessionId), ['native']);
});

test('apps sharing a source project keep their own identity and bound chats', () => {
  const app = (id: string, sessionId: string) => ({ id, name: id, authoring: {
    workspaceId: 'workspace', sourceDirectory: '/app', projectRef: { kind: 'desktopPath', path: '/app' }, authoringSessionId: sessionId,
  } } as DesktopLocalApp);
  const groups = localAppWorkspaceGroups([app('one', 'a'), app('two', 'b')], []);
  assert.deepEqual(groups.map(group => [group.app?.id, group.chats.map(chat => chat.sessionId)]), [['one', ['a']], ['two', ['b']]]);
});

test('native source ownership joins an installed App to its project even in Chat mode', () => {
  const groups = localAppWorkspaceGroups([{ id: 'one', name: 'Notes' } as DesktopLocalApp], [
    { appId: 'one', name: 'Project', projectPath: '/app', sessionId: 'a', source: { scope: 'current', transcriptPath: '/a.jsonl' } },
    { appId: 'one', name: 'Project', projectPath: '/app', sessionId: 'b', source: { scope: 'current', transcriptPath: '/b.jsonl' } },
  ]);
  assert.equal(groups.length, 1);
  assert.equal(groups[0].projectPath, '/app');
  assert.equal(groups[0].chats.length, 2);
});


test('Apps sidebar mode survives validated settings updates', () => {
  const first = patchSidebarPreferences(undefined, { mode: 'apps' }, []);
  assert.equal(first.mode, 'apps');
  assert.equal(patchSidebarPreferences(first, { chatSort: 'created' }, []).mode, 'apps');
});
