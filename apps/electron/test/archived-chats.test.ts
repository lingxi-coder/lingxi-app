import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { archivedChatRows, ArchivedChats } from '../src/renderer/components/settings/pages/ArchivedChats';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import { searchNav } from '../src/renderer/components/settings/nav';
(globalThis as { React?: typeof React }).React = React;
const records = Array.from({ length: 35 }, (_, index) => ({ projectPath: index % 2 ? '/project-one' : '/project-two', sessionId: `s${index}`, title: `Chat ${index}`, archivedAt: new Date(2026, 0, index + 1).toISOString() }));

test('archived chats includes all projects and records, newest first without mutating storage', () => {
  const rows = archivedChatRows(records, '');
  assert.equal(rows.length, 35);
  assert.equal(rows[0].sessionId, 's34');
  assert.equal(records[0].sessionId, 's0');
  assert.equal(new Set(rows.map((row) => row.projectPath)).size, 2);
});
test('search matches title, project or session id', () => {
  assert.equal(archivedChatRows(records, ' CHAT 34 ')[0].sessionId, 's34');
  assert.equal(archivedChatRows(records, 'project-one').length, 17);
  assert.equal(archivedChatRows(records, 's34').length, 1);
  assert.deepEqual(archivedChatRows(records, 'absent'), []);
  assert.equal(searchNav('archived')[0].id, 'archived-chats');
});
test('renders full archived list and an honest empty state', () => {
  const render = (archivedSessions: typeof records) => renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(false) }, React.createElement(ArchivedChats, { bridge: { bootstrap: { settings: { archivedSessions } } }, onClose() {} } as never)));
  assert.equal((render(records).match(/<li /g) ?? []).length, 35);
  assert.match(render([]), /暂无已归档会话/);
});
