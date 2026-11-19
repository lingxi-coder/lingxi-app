import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import test from 'node:test';
import { createElement } from 'react';
import type { InboxItem } from '../../../packages/inbox/contracts';
import { agentGroup, ChatSidebar } from '../src/chat/ChatSidebar';
import { destinationOf, isOpen, sectionOf } from '../src/chat/routeFromInbox';

const renderer = createRequire(import.meta.url)('react-test-renderer');
const textOf = (node: any): string => typeof node === 'string' ? node : (node?.children ?? []).map(textOf).join('');

const row = (kind: 'team' | 'assistant' | 'service', sourceId: string, conversationId: string, more: Partial<InboxItem> = {}): InboxItem => ({
  schemaVersion: 1, key: `${kind}:${sourceId}:${conversationId}`, ref: { kind, sourceId, conversationId, generation: 1 }, category: kind,
  title: 'Title', preview: '', lastActivityAt: 0, sourceCursor: 0, sourceRevision: 0, readThrough: 0, unreadCount: 0, mentionCount: 0, pendingActionCount: 0,
  state: 'idle', manualUnread: false, manualUnreadRevision: 0, stateRevision: 0, offline: false, pinOrder: null, notificationMode: 'all', archived: false, ...more,
});

test('inbox rows go to their section and open in the workspace or on their own page', () => {
  const channel = row('team', 'T1', 'C1', { conversationKind: 'channel' });
  const botDm = row('team', 'T1', 'B1', { conversationKind: 'bot_dm' });
  const friend = row('team', 'f_abc', 'friend_chat', { category: 'friend', peerUid: 'Bob' });
  const service = row('service', 's_abcdef', 'S1');
  const computer = row('assistant', 'src', 'sess', { remoteHostId: 'AbC_-xYz0123456789AAAA' });
  assert.deepEqual([channel, botDm, friend, service, computer].map(sectionOf), ['teams', 'bots', 'friends', 'bots', 'computers']);
  assert.deepEqual(destinationOf(channel), { kind: 'chat', target: { kind: 'team', teamId: 'T1', conversationId: 'C1' } });
  assert.deepEqual(destinationOf(botDm), { kind: 'chat', target: { kind: 'team', teamId: 'T1', conversationId: 'B1' } });
  assert.deepEqual(destinationOf(friend), { kind: 'chat', target: { kind: 'friend', uid: 'Bob' } });
  assert.deepEqual(destinationOf(service), { kind: 'page', href: '/services/s_abcdef?session=S1' });
  assert.equal(destinationOf(computer)?.kind, 'page');
  assert.equal(destinationOf(row('assistant', 'src', 'sess')), undefined, 'a computer row without a host cannot open');
  assert.ok(isOpen(channel, { kind: 'team', teamId: 'T1', conversationId: 'C1' }));
  assert.ok(!isOpen(channel, { kind: 'team', teamId: 'T1', conversationId: 'C2' }));
  assert.ok(isOpen(friend, { kind: 'friend', uid: 'Bob' }));
});

test('older rows without a conversation kind stay under Teams', () => {
  assert.equal(sectionOf(row('team', 'T1', 'C1')), 'teams');
});

test('Agent chats are grouped by local day', () => {
  const now = new Date(2026, 10, 18, 15).getTime();
  assert.equal(agentGroup(new Date(2026, 10, 18, 1).getTime(), now), 'today');
  assert.equal(agentGroup(new Date(2026, 10, 17, 23).getTime(), now), 'yesterday');
  assert.equal(agentGroup(new Date(2026, 10, 13).getTime(), now), 'week');
  assert.equal(agentGroup(new Date(2026, 9, 1).getTime(), now), 'earlier');
});

test('the sidebar lists each section, marks the open chat and unread, and opens rows', () => {
  const opened: unknown[] = [];
  const rows = [row('team', 'T1', 'C1', { title: 'General', conversationKind: 'channel', unreadCount: 3 }), row('team', 'f_x', 'friend_chat', { title: 'Bob', category: 'friend', peerUid: 'Bob' })];
  let tree: any;
  renderer.act(() => { tree = renderer.create(createElement(ChatSidebar, {
    locale: 'en', theme: 'light', target: { kind: 'team', teamId: 'T1', conversationId: 'C1' }, rows, account: 'me@example.invalid',
    agentChats: [{ id: 'a1', title: 'Trip plan', updatedAt: Date.now() }],
    onNewChat() {}, onOpenAgent: (id: string) => opened.push(id), onOpenRow: (d: unknown) => opened.push(d), onOpen() {}, onLocaleChange() {}, onThemeToggle() {}, onSignOut() {},
  })); });
  const text = textOf(tree.toJSON());
  for (const part of ['Agent', 'Teams', 'Friends', 'General', 'Bob', 'Trip plan', 'Today']) assert.ok(text.includes(part), part);
  const current = tree.root.findAll((n: any) => n.type === 'button' && n.props['aria-current'] === 'page');
  assert.equal(current.length, 1);
  assert.ok(textOf(current[0]).includes('General'));
  assert.equal(tree.root.findAll((n: any) => n.props['aria-label'] === '3 unread').length, 1);
  const bob = tree.root.findAll((n: any) => n.type === 'button' && textOf(n).includes('Bob'))[0];
  renderer.act(() => bob.props.onClick());
  assert.deepEqual(opened, [{ kind: 'chat', target: { kind: 'friend', uid: 'Bob' } }]);
});
