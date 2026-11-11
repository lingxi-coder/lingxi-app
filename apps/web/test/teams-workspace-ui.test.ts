import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { createElement, type ReactElement, type ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { MESSAGE_BYTES, byteLength, canSendText, mentionAt, mentionCandidates, sendsOnEnter, snippet, withoutMention } from '../../../packages/teams/compose';
import type { SharedBotCard } from '../../../packages/teams/execution';
import type { TeamMember } from '../../../packages/teams';
import { visibleStart, UNREAD_CONTEXT, WINDOW_MAX, WINDOW_SIZE } from '../../../packages/teams/stream';
import type { MenuItem } from '../../../packages/teams/ui';
import { TeamsWorkspace, type TeamsSnapshot } from '../../../packages/teams/workspace';
import type { TeamMessage } from '../../../packages/teams/messages';
import { FakeTeams, botCard, conversation, memoryStorage, members, message, team, uid, until } from './support/fakeTeams';
import { loadTeamsModule } from './support/loadTeamsModule';

// The components import React, which shared packages cannot resolve on their own; see the loader.
const { composerLock } = loadTeamsModule<typeof import('../../../packages/teams/Composer')>('Composer');
const { TeamConversationView, conversationSubtitle } = loadTeamsModule<typeof import('../../../packages/teams/TeamConversationView')>('TeamConversationView');
const { DRAWER_DEFAULT, DRAWER_MAX, DRAWER_MIN, TeamDetailsDrawer, canManageMember, clampDrawerWidth, drawerTabs, drawerWidthForDrag, drawerWidthForKey } = loadTeamsModule<typeof import('../../../packages/teams/TeamDetailsDrawer')>('TeamDetailsDrawer');
const { TeamSidebar, connectionStatus } = loadTeamsModule<typeof import('../../../packages/teams/TeamSidebar')>('TeamSidebar');
const { AVATAR_COLORS, Avatar, PopupMenu, avatarColor, initials, nextEnabled, rovingIndex } = loadTeamsModule<typeof import('../../../packages/teams/ui')>('ui');

// ── Pure helpers ──

const luminance = (hex: string) => {
  const [r, g, b] = [1, 3, 5].map(i => parseInt(hex.slice(i, i + 2), 16) / 255).map(v => v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4) as [number, number, number];
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
};
const contrast = (a: string, b: string) => { const [x, y] = [luminance(a), luminance(b)].sort((m, n) => n - m) as [number, number]; return (x + 0.05) / (y + 0.05); };

test('avatars: the color follows the id, the initials follow the name, and every color carries white text at 4.5:1', () => {
  assert.equal(avatarColor('user-1'), avatarColor('user-1'));
  assert.ok(new Set(Array.from({ length: 60 }, (_, i) => avatarColor(`user-${i}`))).size >= 6, 'ids spread over the palette');
  for (const color of AVATAR_COLORS) assert.ok(contrast(color, '#ffffff') >= 4.5, `${color} on white is ${contrast(color, '#ffffff').toFixed(2)}`);
  assert.equal(initials('Ada Lovelace'), 'AL');
  assert.equal(initials('  grace  '), 'G');
  assert.equal(initials('灵犀 团队'), '灵');
  assert.equal(initials('한국어'), '한');
  assert.equal(initials('   '), '?');
  assert.equal(initials('😀 smile'), '😀S', 'a surrogate pair is one character');
});

test('Avatar draws initials, or the host\'s avatar when it supplies one', () => {
  const identity = { kind: 'bot' as const, id: 'b1', name: 'Fixture Bot' };
  const plain = renderToStaticMarkup(createElement(Avatar, { identity, size: 32 }));
  assert.match(plain, /class="tw-avatar"/); assert.match(plain, /data-kind="bot"/); assert.match(plain, />FB</); assert.match(plain, /aria-hidden="true"/);
  assert.match(plain, new RegExp(`background:${avatarColor('b1')}`));
  const custom = renderToStaticMarkup(createElement(Avatar, { identity, size: 32, render: () => createElement('img', { src: 'bot.png', alt: '' }) }));
  assert.match(custom, /tw-avatar-custom/); assert.match(custom, /<img src="bot.png"/); assert.doesNotMatch(custom, />FB</);
  const declined = renderToStaticMarkup(createElement(Avatar, { identity, size: 32, render: () => undefined }));
  assert.match(declined, />FB</, 'returning nothing keeps the initials');
});

test('menu and roving-focus helpers skip disabled items and wrap or stop as asked', () => {
  const items = [{ disabled: false }, { disabled: true }, { disabled: false }, { disabled: true }];
  assert.equal(nextEnabled(items, 0, 1), 2); assert.equal(nextEnabled(items, 2, 1), 0, 'wraps'); assert.equal(nextEnabled(items, 0, -1), 2);
  assert.equal(nextEnabled(items, -1, 1), 0); assert.equal(nextEnabled([{ disabled: true }], 0, 1), -1); assert.equal(nextEnabled([], 0, 1), -1);
  assert.equal(rovingIndex(5, 2, 'ArrowDown'), 3); assert.equal(rovingIndex(5, 4, 'ArrowDown'), 4, 'a list stops at its end');
  assert.equal(rovingIndex(5, 0, 'ArrowUp'), 0); assert.equal(rovingIndex(5, 2, 'Home'), 0); assert.equal(rovingIndex(5, 2, 'End'), 4);
  assert.equal(rovingIndex(5, -1, 'ArrowDown'), 0, 'from nothing, Down starts at the top'); assert.equal(rovingIndex(5, -1, 'ArrowUp'), 4, 'and Up at the bottom');
  assert.equal(rovingIndex(3, 2, 'ArrowRight', { prev: 'ArrowLeft', next: 'ArrowRight', wrap: true }), 0, 'tabs wrap');
  assert.equal(rovingIndex(3, 0, 'ArrowLeft', { prev: 'ArrowLeft', next: 'ArrowRight', wrap: true }), 2);
  assert.equal(rovingIndex(3, 0, 'ArrowDown', { prev: 'ArrowLeft', next: 'ArrowRight', wrap: true }), undefined, 'other keys do not move');
  assert.equal(rovingIndex(0, 0, 'ArrowDown'), undefined); assert.equal(rovingIndex(5, 2, 'a'), undefined);
});

test('PopupMenu is a menu: radio items say which is checked, disabled items are disabled, separators separate', () => {
  const items: MenuItem[] = [
    { id: 'a', label: 'Acme', radio: true, checked: true, onSelect() {} },
    { id: 'b', label: 'Beta', radio: true, checked: false, onSelect() {} },
    { id: 'c', label: 'Create', icon: 'plus', separated: true, onSelect() {} },
    { id: 'd', label: 'Delete', danger: true, disabled: true, onSelect() {} },
  ];
  const html = renderToStaticMarkup(createElement(PopupMenu, { label: 'Teams', items, onClose() {} }));
  assert.match(html, /role="menu"[^>]*aria-label="Teams"|aria-label="Teams"[^>]*role="menu"/);
  assert.equal((html.match(/role="menuitemradio"/g) ?? []).length, 2); assert.equal((html.match(/role="menuitem"/g) ?? []).length, 2);
  assert.match(html, /role="menuitemradio" aria-checked="true"[^>]*>/); assert.match(html, /role="menuitemradio" aria-checked="false"/);
  assert.equal((html.match(/role="separator"/g) ?? []).length, 1);
  assert.match(html, /tw-menu-item tw-danger"[^>]*disabled/);
  assert.equal((html.match(/tabindex="-1"/g) ?? []).length, 4, 'items are reached with arrow keys, not Tab');
});

test('composer helpers: Enter sends unless an input method is composing, @ completes Bots, text is bounded in bytes', () => {
  assert.ok(sendsOnEnter({ key: 'Enter', shiftKey: false }));
  assert.ok(!sendsOnEnter({ key: 'Enter', shiftKey: true }), 'Shift+Enter is a line break');
  assert.ok(!sendsOnEnter({ key: 'Enter', shiftKey: false, isComposing: true }), 'Enter that picks an input-method candidate');
  assert.ok(!sendsOnEnter({ key: 'Enter', shiftKey: false, keyCode: 229 }), 'the legacy composing key code');
  assert.ok(!sendsOnEnter({ key: 'a', shiftKey: false }));
  assert.deepEqual(mentionAt('hello @fi', 9), { start: 6, query: 'fi' });
  assert.deepEqual(mentionAt('@', 1), { start: 0, query: '' });
  assert.deepEqual(mentionAt('see (@b', 7), { start: 5, query: 'b' }, 'after a bracket');
  assert.deepEqual(mentionAt('请 @机器人', 6), { start: 2, query: '机器人' });
  assert.equal(mentionAt('mail me@example', 15), null, 'an address is not a mention');
  assert.equal(mentionAt('hello @fi there', 15), null, 'the caret has left the word');
  assert.deepEqual(mentionAt('a @b c', 4), { start: 2, query: 'b' }, 'only the text before the caret counts');
  assert.equal(withoutMention('hi @fi there', 3, 6), 'hi there', 'the double space it leaves is closed');
  assert.equal(withoutMention('@fi', 0, 3), ''); assert.equal(withoutMention('hi @fi', 3, 6), 'hi ');
  const a = botCard({ sharedBotId: 'a', name: 'Alpha' }), b = botCard({ sharedBotId: 'b', name: 'Beta', ready: false }), c = botCard({ sharedBotId: 'c', name: 'Gamma', permitted: false }), d = botCard({ sharedBotId: 'd', name: 'Delta', state: 'paused' });
  const group = { kind: 'group' as const, participants: ['user:alice', 'shared_bot:a', 'shared_bot:b', 'shared_bot:c', 'shared_bot:d'] };
  const names = (list: SharedBotCard[]) => list.map(bot => bot.name);
  assert.deepEqual(names(mentionCandidates([a, b, c, d], group, [], '')), ['Alpha', 'Beta'], 'usable Bots in the conversation');
  assert.deepEqual(names(mentionCandidates([a, b], group, ['a'], '')), ['Beta'], 'chosen ones are not offered twice');
  assert.deepEqual(names(mentionCandidates([a, b], group, [], 'BE')), ['Beta'], 'case-insensitive');
  assert.deepEqual(mentionCandidates([a], { kind: 'group', participants: ['user:alice'] }, [], ''), [], 'a Bot outside the conversation');
  assert.deepEqual(mentionCandidates([a], { ...group, kind: 'dm' }, [], ''), [], 'a dm has no explicit targets');
  assert.deepEqual(mentionCandidates([a], { ...group, kind: 'bot_dm' }, [], ''), [], 'a Bot dm asks its Bot automatically');
  assert.equal(mentionCandidates([a], { ...group, kind: 'channel' }, [], '').length, 1);
  assert.ok(canSendText('hi')); assert.ok(!canSendText('   \n')); assert.ok(!canSendText(''));
  assert.ok(canSendText('a'.repeat(MESSAGE_BYTES))); assert.ok(!canSendText('a'.repeat(MESSAGE_BYTES + 1)));
  assert.equal(byteLength('灵'), 3); assert.ok(!canSendText('灵'.repeat(Math.floor(MESSAGE_BYTES / 3) + 1)), 'the limit is bytes, not characters');
  assert.equal(snippet('  a\n\n b   c '), 'a b c'); assert.equal(snippet('x'.repeat(100), 10), `${'x'.repeat(10)}…`); assert.equal(snippet('灵犀团队', 2), '灵犀…');
});

test('composerLock: observers only read, and a Bot dm waits for a usable Bot', () => {
  const bot = botCard({ sharedBotId: 'a' });
  const base = { uid: 'alice', bots: [bot], botsLoaded: true, botsUnverified: false };
  const dm = { kind: 'dm' as const, participants: ['user:alice', 'user:bob'] };
  const botDm = { kind: 'bot_dm' as const, participants: ['user:alice', 'shared_bot:a'] };
  assert.equal(composerLock({ ...base, conversation: dm }, false), '');
  assert.equal(composerLock({ ...base, conversation: { kind: 'channel', participants: [] } }, false), '', 'a conversation without a participant list is open');
  assert.match(composerLock({ ...base, conversation: { kind: 'bot_dm', participants: ['user:bob', 'shared_bot:a'] } }, false), /only participants can send/);
  assert.match(composerLock({ ...base, conversation: { kind: 'bot_dm', participants: ['user:bob', 'shared_bot:a'] } }, true), /只有参与者可以发送/);
  assert.equal(composerLock({ ...base, conversation: botDm }, false), '');
  assert.match(composerLock({ ...base, bots: [botCard({ sharedBotId: 'a', permitted: false })], conversation: botDm }, false), /not available to you/);
  assert.match(composerLock({ ...base, bots: [botCard({ sharedBotId: 'a', state: 'paused' })], conversation: botDm }, false), /not available to you/);
  assert.match(composerLock({ ...base, bots: [], botsLoaded: false, conversation: botDm }, false), /Checking whether this Bot is available/);
  assert.match(composerLock({ ...base, bots: [], botsUnverified: true, conversation: botDm }, false), /could not be verified/);
  assert.match(composerLock({ ...base, bots: [], botsLoaded: true, botsUnverified: true, conversation: botDm }, true), /未能确认/);
  assert.equal(composerLock({ ...base, bots: [], botsLoaded: false, conversation: dm }, false), '', 'only a Bot dm needs the Bot list');
});

test('visibleStart: the newest window, widened for unread messages and for what the reader asked for', () => {
  const list = (count: number): TeamMessage[] => Array.from({ length: count }, (_, i) => message(i + 1, { author: { kind: 'user', uid: 'bob' } }));
  const all = list(250);
  assert.equal(visibleStart(list(30), { lastReadSeq: 30, uid }), 0);
  assert.equal(visibleStart(all, { lastReadSeq: 250, uid }), 250 - WINDOW_SIZE, 'the newest hundred');
  assert.equal(visibleStart(all, { lastReadSeq: 100, uid }), 100 - UNREAD_CONTEXT, 'back to the first unread, with context');
  assert.equal(visibleStart(all, { lastReadSeq: 0, uid }), 0, 'all unread and within the cap: everything');
  const big = list(600);
  assert.equal(visibleStart(big, { lastReadSeq: 0, uid }), 600 - WINDOW_MAX, 'but never past the cap by itself');
  assert.equal(visibleStart(big, { lastReadSeq: 600, uid, startId: 'message-10' }), 9, 'what the reader asked for is honored, even past the cap');
  assert.equal(visibleStart(big, { lastReadSeq: 0, uid, startId: 'message-10' }), 9);
  assert.equal(visibleStart(all, { lastReadSeq: 250, uid, startId: 'message-240' }), 150, 'asking for less than the default changes nothing');
  assert.equal(visibleStart(all, { lastReadSeq: 250, uid, startId: 'gone' }), 150, 'an unknown anchor is ignored');
  assert.equal(visibleStart(all, { lastReadSeq: 250, uid, size: 20 }), 230);
  const mine = all.map(value => ({ ...value, author: { kind: 'user' as const, uid } }));
  assert.equal(visibleStart(mine, { lastReadSeq: 0, uid }), 150, 'unread messages written by the reader do not widen the window');
  // New messages leave the start where it is, so the reader is not moved.
  const grown = [...all, message(251), message(252)];
  assert.equal(visibleStart(grown, { lastReadSeq: 252, uid, startId: 'message-100' }), 99);
});

test('drawer width and rights', () => {
  assert.equal(clampDrawerWidth(100), DRAWER_MIN); assert.equal(clampDrawerWidth(9999), DRAWER_MAX); assert.equal(clampDrawerWidth(333.6), 334);
  assert.equal(clampDrawerWidth(Number.NaN), DRAWER_DEFAULT); assert.equal(clampDrawerWidth(Infinity), DRAWER_DEFAULT);
  assert.deepEqual([DRAWER_MIN, DRAWER_DEFAULT, DRAWER_MAX], [280, 360, 480]);
  assert.equal(drawerWidthForDrag(360, 800, 760), 400, 'dragging the left edge left widens');
  assert.equal(drawerWidthForDrag(360, 800, 900), 280, 'and right narrows, down to the minimum');
  assert.equal(drawerWidthForDrag(360, 800, 0), 480);
  assert.equal(drawerWidthForKey(360, 'ArrowLeft'), 376); assert.equal(drawerWidthForKey(360, 'ArrowRight'), 344);
  assert.equal(drawerWidthForKey(360, 'ArrowLeft', true), 408); assert.equal(drawerWidthForKey(470, 'ArrowLeft', true), 480);
  assert.equal(drawerWidthForKey(360, 'Home'), 280); assert.equal(drawerWidthForKey(360, 'End'), 480); assert.equal(drawerWidthForKey(360, 'a'), undefined);
  assert.deepEqual(drawerTabs('owner'), ['members', 'invites', 'bots']); assert.deepEqual(drawerTabs('admin'), ['members', 'invites', 'bots']); assert.deepEqual(drawerTabs('member'), ['members', 'bots']);
  const person = (id: string, role: TeamMember['role']) => ({ uid: id, role });
  assert.ok(canManageMember('owner', 'alice', person('bob', 'member'))); assert.ok(canManageMember('owner', 'alice', person('bob', 'admin')));
  assert.ok(!canManageMember('owner', 'alice', person('alice', 'owner')), 'not yourself'); assert.ok(!canManageMember('owner', 'alice', person('cy', 'owner')), 'not another owner');
  assert.ok(canManageMember('admin', 'alice', person('bob', 'member'))); assert.ok(!canManageMember('admin', 'alice', person('bob', 'admin')), 'an admin cannot manage admins');
  assert.ok(!canManageMember('member', 'alice', person('bob', 'member')));
});

test('connectionStatus: errors first, then offline, then connected; nothing while loading or without a team', () => {
  const base = { loading: false, selectedTeamId: 'team-aaaa', loadError: '', offline: false, messagingConnecting: false, messagingError: '' };
  assert.equal(connectionStatus({ ...base, loading: true, loadError: 'X' }, false), undefined);
  assert.equal(connectionStatus({ ...base, selectedTeamId: '' }, false), undefined);
  assert.equal(connectionStatus({ ...base, selectedTeamId: '', loadError: 'SERVICE_UNAVAILABLE' }, false)?.action, 'refresh', 'a failed team list can be retried even with no team');
  assert.deepEqual(connectionStatus(base, false), { kind: 'ok', label: 'Teams connected', detail: 'Teams is connected. Bot host availability is shown in each conversation.' });
  assert.equal(connectionStatus({ ...base, offline: true, messagingConnecting: true }, false)?.kind, 'connecting');
  assert.equal(connectionStatus({ ...base, offline: true, messagingConnecting: true }, true)?.label, '正在连接…');
  assert.equal(connectionStatus({ ...base, offline: true, messagingConnecting: true, messagingError: 'SERVICE_UNAVAILABLE' }, false)?.kind, 'offline');
  const offline = connectionStatus({ ...base, offline: true }, false)!;
  assert.equal(offline.kind, 'offline'); assert.equal(offline.action, 'reconnect'); assert.match(offline.detail, /Reconnect to confirm access/);
  assert.match(connectionStatus({ ...base, offline: true, messagingError: 'STORAGE_FAILED' }, false)!.detail, /storage/i, 'the controller\'s own failure is explained');
  const failed = connectionStatus({ ...base, offline: true, loadError: 'MEMBERSHIP_REQUIRED' }, true)!;
  assert.equal(failed.kind, 'error'); assert.match(failed.detail, /你已不在此团队/); assert.equal(failed.label, '无法刷新团队');
});

test('the default palette meets contrast in light and dark, and the stylesheet keeps its size and focus promises', () => {
  const css = readFileSync(new URL('../../../packages/teams/workspace.css', import.meta.url), 'utf8');
  const palette = (block: string): Record<string, string> => Object.fromEntries([...block.matchAll(/--tw-([a-z-]+):\s*var\(--teams-[a-z-]+,\s*(#[0-9a-fA-F]{6})\)/g)].map(match => [match[1]!, match[2]!]));
  const dark = css.indexOf('@media (prefers-color-scheme: dark)');
  const schemes = { light: palette(css.slice(css.indexOf('.tw-scope {'), dark)), dark: palette(css.slice(dark, css.indexOf('.tw-scope, .tw-scope *'))) };
  const text: [string, string][] = [
    ['fg', 'bg'], ['fg', 'sidebar-bg'], ['fg', 'surface'], ['fg', 'hover'], ['fg', 'selected'],
    ['muted', 'bg'], ['muted', 'sidebar-bg'], ['muted', 'surface'], ['muted', 'hover'], ['muted', 'selected'],
    ['accent-fg', 'accent'], ['accent-text', 'bg'], ['accent-text', 'sidebar-bg'], ['accent-text', 'surface'], ['accent-text', 'hover'], ['accent-text', 'selected'],
    ['danger', 'bg'], ['danger', 'sidebar-bg'], ['danger', 'surface'], ['danger', 'hover'], ['badge-fg', 'badge-bg'], ['online', 'bg'], ['online', 'surface'],
  ];
  const parts: [string, string][] = [['border', 'bg'], ['border', 'sidebar-bg'], ['border', 'surface'], ['border', 'hover'], ['focus', 'bg'], ['focus', 'sidebar-bg'], ['focus', 'surface'], ['focus', 'hover'], ['online', 'hover'], ['accent', 'bg'], ['accent', 'surface']];
  for (const [name, colors] of Object.entries(schemes)) {
    assert.ok(Object.keys(colors).length >= 16, `${name} defines its tokens`);
    for (const [foreground, background] of text) assert.ok(contrast(colors[foreground]!, colors[background]!) >= 4.5, `${name}: ${foreground} on ${background} is ${contrast(colors[foreground]!, colors[background]!).toFixed(2)}`);
    for (const [foreground, background] of parts) assert.ok(contrast(colors[foreground]!, colors[background]!) >= 3, `${name}: ${foreground} against ${background} is ${contrast(colors[foreground]!, colors[background]!).toFixed(2)}`);
  }
  assert.deepEqual(Object.keys(schemes.light).sort(), Object.keys(schemes.dark).sort(), 'both schemes define the same tokens');
  const sizes = [...css.matchAll(/font-size:\s*(\d+(?:\.\d+)?)px/g)].map(match => Number(match[1]));
  assert.ok(sizes.length > 10 && Math.min(...sizes) >= 11, `no text below 11px (smallest ${Math.min(...sizes)})`);
  assert.match(css, /\.tw-scope\s*\{[^}]*font:\s*13px\//, 'body text is 13px');
  assert.match(css, /\.tw-row-preview\s*\{[^}]*font-size:\s*12px/, 'previews are 12px');
  assert.match(css, /\.tw-scope button, \.tw-dialog button\s*\{[^}]*min-height:\s*28px/, 'controls are at least 28px tall');
  assert.match(css, /\.tw-scope \.tw-icon-btn[^{]*\{[^}]*width:\s*28px/, 'icon buttons are 28px wide');
  assert.match(css, /\.tw-scope :focus-visible[^{]*\{[^}]*outline:\s*2px solid var\(--tw-focus\)/, 'keyboard focus is outlined');
  assert.match(css, /prefers-reduced-motion:\s*reduce\)\s*\{[^}]*transition:\s*none/, 'motion can be turned off');
  for (const hidden of ['.tw-row-more', '.tw-msg-actions', '.tw-msg-hover-time']) {
    assert.match(css, new RegExp(`${hidden.replace('.', '\\.')}[^{]*\\{[^}]*opacity:\\s*0`), `${hidden} starts hidden`);
    assert.match(css, new RegExp(`:focus-within [^{]*${hidden.replace('.', '\\.')}|\\.tw-row:focus-within ${hidden.replace('.', '\\.')}`), `${hidden} also shows on focus-within`);
  }
  assert.doesNotMatch(css, /display:\s*none/, 'nothing is removed from the accessibility tree by CSS');
});

// ── Rendering ──

const NOW = new Date(2026, 10, 2, 12, 0, 0).getTime();
const at = (minute: number) => new Date(2026, 10, 2, 11, minute).toISOString();

async function open(setup: (fake: FakeTeams) => void = () => {}) {
  const fake = new FakeTeams();
  const bot = botCard();
  fake.teams = [team('team-aaaa', { name: 'Acme', role: 'owner' }), team('team-bbbb', { name: 'Beta', generation: 2 })];
  fake.bots = [bot];
  fake.conversations.set('team-aaaa', [
    conversation('chan-general', { title: 'general', participants: ['user:alice', 'user:bob', `shared_bot:${bot.sharedBotId}`], lastSeq: 7, lastReadSeq: 3, lastMessage: { id: 'message-7', seq: 7, at: at(40), preview: 'Ship it', author: { kind: 'user', uid: 'bob' } } }),
    conversation('dm-bob', { kind: 'dm', title: 'Alice, Bob', lastSeq: 1, lastReadSeq: 1, lastMessage: { id: 'dm-1', seq: 1, at: at(10), preview: 'Lunch?', author: { kind: 'user', uid: 'alice' } } }),
    conversation('bot-chat', { kind: 'bot_dm', title: 'Fixture Bot', participants: ['user:alice', `shared_bot:${bot.sharedBotId}`], lastSeq: 2, lastReadSeq: 2 }),
    conversation('group-1', { kind: 'group', title: 'Alice, Bob, Owner', participants: ['user:alice', 'user:bob', 'user:owner', `shared_bot:${bot.sharedBotId}`] }),
  ]);
  fake.conversations.set('team-bbbb', [conversation('chan-b', { lastSeq: 4, lastReadSeq: 1 })]);
  fake.threads.set('chan-general', [
    message(1, { text: 'Morning', createdAt: at(0) }),
    message(2, { text: 'Standup in five', createdAt: at(1) }),
    message(3, { text: 'Notes are in the doc', createdAt: at(2) }),
    message(4, { text: 'Thanks <script>alert(1)</script>', author: { kind: 'user', uid: 'alice' }, createdAt: at(20) }),
    message(5, { text: 'Anytime', replyTo: 'message-4', editedAt: at(22), createdAt: at(21) }),
    message(6, { text: '', recalled: true, createdAt: at(30) }),
    message(7, { text: 'Ship it', createdAt: at(40) }),
  ]);
  setup(fake);
  const storage = memoryStorage();
  const workspace = new TeamsWorkspace(fake.api, uid, { storage: () => storage, environment: new EventTarget(), refreshDelay: 5 });
  const snapshot = (): TeamsSnapshot => workspace.getSnapshot();
  await until(() => snapshot().conversations.length === (fake.conversations.get('team-aaaa')?.length ?? 0) && snapshot().botsLoaded, 'the team to load');
  return { fake, workspace, snapshot, storage };
}
/** A workspace for an account with no teams (or whatever `setup` makes of the fake). */
async function bare(setup: (fake: FakeTeams) => void = () => {}) {
  const fake = new FakeTeams(); setup(fake);
  const workspace = new TeamsWorkspace(fake.api, uid, { storage: memoryStorage, environment: new EventTarget(), refreshDelay: 5 });
  await until(() => !workspace.getSnapshot().loading, 'the team list');
  return { fake, workspace, snapshot: (): TeamsSnapshot => workspace.getSnapshot() };
}
const html = (element: ReactElement) => renderToStaticMarkup(element);
const count = (text: string, pattern: RegExp) => (text.match(pattern) ?? []).length;
const sidebar = (workspace: TeamsWorkspace, snapshot: TeamsSnapshot, more: Record<string, unknown> = {}) => createElement(TeamSidebar, { workspace, snapshot, zh: false, now: NOW, ...more } as never);
const view = (workspace: TeamsWorkspace, snapshot: TeamsSnapshot, more: Record<string, unknown> = {}) => createElement(TeamConversationView, { workspace, snapshot, zh: false, now: NOW, ...more } as never);
const drawer = (workspace: TeamsWorkspace, snapshot: TeamsSnapshot, more: Record<string, unknown> = {}) => createElement(TeamDetailsDrawer, { workspace, snapshot, zh: false, onClose() {}, ...more } as never);
const rowTags = (markup: string) => [...markup.matchAll(/<button[^>]*data-row="([^"]+)"[^>]*>/g)].map(match => ({ id: match[1]!, tag: match[0] }));

test('sidebar: team switcher, three sections with their add buttons, rows with previews and unread badges', async context => {
  const { workspace, snapshot } = await open(); context.after(() => workspace.close());
  const markup = html(sidebar(workspace, snapshot()));
  assert.match(markup, /<nav class="tw-scope tw-sidebar" aria-label="Teams">/);
  assert.match(markup, /class="tw-switcher" aria-haspopup="menu" aria-expanded="false"/); assert.match(markup, /Acme/);
  assert.match(markup, /class="tw-switcher"[^]*?aria-label="3 unread"[^]*?<\/button>/, 'Beta has 3 unread messages, which the switcher points out');
  for (const heading of ['Channels', 'Direct messages', 'Bot DMs']) assert.match(markup, new RegExp(`<h3 id="tw-section-[a-zA-Z]+">${heading}</h3>`));
  for (const add of ['New channel', 'New message', 'Message a Bot']) assert.match(markup, new RegExp(`aria-label="${add}"`));
  assert.match(markup, /class="tw-row-title">general</); assert.match(markup, /class="tw-row-title">Bob</, 'a dm is named after the other person, not its stored title');
  assert.match(markup, /class="tw-row-title">Fixture Bot</); assert.match(markup, /class="tw-row-title">Bob, Owner</, 'a group without a chosen name lists the others');
  assert.match(markup, /class="tw-row-preview">Bob: Ship it</, 'channels name the sender'); assert.match(markup, /class="tw-row-preview">You: Lunch\?</);
  assert.match(markup, /class="tw-chan"[^>]*>#</, 'channels have a # mark');
  assert.match(markup, /role="img" aria-label="4 unread">4</, 'unread = lastSeq - lastReadSeq');
  assert.match(markup, /<time class="tw-row-time" dateTime="[^"]+">11:40/, 'a row shows its newest message time'); assert.doesNotMatch(markup, /aria-label="0 unread"/);
  assert.match(markup, /data-unread="true" data-kind="channel"/); assert.match(markup, /Teams connected/);
  assert.equal(count(markup, /class="tw-row-more"/g), 1, 'the row menu exists where it has something to offer: only the unread row, for "Mark as read"');
});

test('sidebar: roving tabindex gives the list one tab stop, the open conversation if there is one', async context => {
  const { workspace, snapshot } = await open(); context.after(() => workspace.close());
  const first = rowTags(html(sidebar(workspace, snapshot())));
  assert.equal(first.length, 4); assert.deepEqual(first.filter(row => /tabindex="0"/.test(row.tag)).map(row => row.id), [first[0]!.id], 'the first row when none is open');
  workspace.selectConversation('dm-bob'); await until(() => snapshot().selectedConversationId === 'dm-bob', 'selection');
  const second = rowTags(html(sidebar(workspace, snapshot())));
  assert.deepEqual(second.filter(row => /tabindex="0"/.test(row.tag)).map(row => row.id), ['dm-bob']);
  assert.deepEqual(second.filter(row => /aria-current="page"/.test(row.tag)).map(row => row.id), ['dm-bob']);
});

test('sidebar: unread rows have a row menu, zh text, and details when the host offers them', async context => {
  const { workspace, snapshot } = await open(); context.after(() => workspace.close());
  const unreadOnly = html(sidebar(workspace, snapshot(), { onOpenDetails() {} }));
  assert.equal(count(unreadOnly, /class="tw-row-more"/g), 4, 'with details every row has the menu');
  const zh = html(sidebar(workspace, snapshot(), { zh: true }));
  for (const text of ['频道', '私信', 'Bot 对话', '新建频道', '与 Bot 对话', '已连接']) assert.ok(zh.includes(text), text);
  assert.match(zh, /aria-label="4 条未读"/); assert.match(zh, /class="tw-row-preview">你：Lunch\?</);
  // The details panel has one button, the conversation header's. The sidebar reaches the panel through the team menu (an entry that
  // needs a host to open it, driven in teams-workspace-interaction.test.ts) and has no second always-visible button for it.
  const withHost = html(sidebar(workspace, snapshot(), { onOpenSettings() {} }));
  assert.doesNotMatch(withHost, /Team members &amp; settings/, 'no button of its own, even when a host can open the panel');
  assert.doesNotMatch(html(sidebar(workspace, snapshot())), /Team members &amp; settings/);
  assert.equal(count(/<header class="tw-sidebar-head">[^]*?<\/header>/.exec(withHost)?.[0] ?? '', /<button/g), 1, 'the team switcher is the only control in its row');
});

test('sidebar: avatars come from the host when it renders them, from initials otherwise', async context => {
  const { workspace, snapshot } = await open(); context.after(() => workspace.close());
  const asked: string[] = [];
  const renderAvatar = (identity: { kind: string; id: string; name: string }): ReactNode => { asked.push(`${identity.kind}:${identity.name}`); return identity.kind === 'bot' ? createElement('img', { src: 'bot.png', alt: '' }) : undefined; };
  const markup = html(sidebar(workspace, snapshot(), { renderAvatar }));
  assert.ok(asked.includes('bot:Fixture Bot') && asked.includes('user:Bob'));
  assert.match(markup, /tw-avatar-custom[^>]*>\s*<img src="bot.png"/); assert.match(markup, />B<\/span>/, 'a person without a custom avatar gets initials');
});

test('sidebar: empty, loading, offline and failed states', async context => {
  const none = await bare(); context.after(() => none.workspace.close());
  const empty = html(sidebar(none.workspace, none.snapshot()));
  assert.match(empty, /A shared place for your team/); assert.match(empty, /Create a team/); assert.match(empty, /Join with an invite/); assert.match(empty, /readable by the platform/);
  assert.doesNotMatch(empty, /tw-sections/); assert.match(empty, /No team/);

  const slow = new FakeTeams(); slow.list = () => new Promise(() => {});
  const waiting = new TeamsWorkspace(slow.api, uid, { storage: memoryStorage, environment: new EventTarget() }); context.after(() => waiting.close());
  assert.match(html(sidebar(waiting, waiting.getSnapshot())), /Loading teams…/);
  assert.match(html(view(waiting, waiting.getSnapshot())), /Loading…/);

  const { fake, workspace, snapshot } = await open(); context.after(() => workspace.close());
  const [watch] = fake.open('team-aaaa'); assert.ok(watch);
  fake.emit(watch, { kind: 'directory', fromCache: true, conversations: fake.conversations.get('team-aaaa')! });
  await until(() => snapshot().offline, 'offline');
  const offline = html(sidebar(workspace, snapshot()));
  assert.match(offline, /data-kind="offline"/); assert.match(offline, /Offline · saved messages/); assert.match(offline, />Reconnect</); assert.match(offline, /role="status"/);
  fake.list = async () => { throw new Error('SERVICE_UNAVAILABLE'); };
  await workspace.refresh(); await until(() => snapshot().loadError === 'SERVICE_UNAVAILABLE', 'the failed refetch');
  const failed = html(sidebar(workspace, snapshot()));
  assert.match(failed, /data-kind="error"/); assert.match(failed, /Could not refresh teams/); assert.match(failed, />Retry</); assert.match(failed, /Acme/, 'the last list stays');
});

test('sidebar: a long list shows its first fifty rows and "Show more", and always the open conversation', async context => {
  const base = new Date(2026, 10, 1, 8, 0, 0).getTime();
  const { workspace, snapshot } = await open(fake => {
    fake.conversations.set('team-aaaa', Array.from({ length: 60 }, (_, i) => conversation(`c-${String(i).padStart(2, '0')}`, {
      title: `channel ${i}`, lastSeq: 1, lastReadSeq: 1, lastMessage: { id: `m-${i}`, seq: 1, at: new Date(base + (100 - i) * 1000).toISOString(), preview: 'x', author: { kind: 'user', uid: 'bob' } },
    })));
  });
  context.after(() => workspace.close());
  const rows = () => rowTags(html(sidebar(workspace, snapshot()))).map(row => row.id);
  assert.equal(rows().length, 50); assert.deepEqual([rows()[0], rows()[49]], ['c-00', 'c-49'], 'newest first');
  assert.match(html(sidebar(workspace, snapshot())), />Show 10 more</);
  workspace.selectConversation('c-59'); await until(() => snapshot().selectedConversationId === 'c-59', 'selection');
  assert.equal(rows().length, 60, 'the open conversation is never hidden behind "Show more"'); assert.doesNotMatch(html(sidebar(workspace, snapshot())), /Show \d+ more/);
});

test('sidebar: a query narrows every section to the conversations whose name or newest message contains it, and says when there are none', async context => {
  const { workspace, snapshot } = await open(); context.after(() => workspace.close());
  const ids = (query?: string) => rowTags(html(sidebar(workspace, snapshot(), query === undefined ? {} : { query }))).map(row => row.id).sort();
  const everything = ['bot-chat', 'chan-general', 'dm-bob', 'group-1'];
  assert.deepEqual(ids(), everything, 'no query lists them all');
  assert.deepEqual(ids(''), everything); assert.deepEqual(ids('   '), everything, 'a blank query is no query');
  assert.deepEqual(ids('GENERAL'), ['chan-general'], 'by name, in any case');
  assert.deepEqual(ids('  fixture '), ['bot-chat'], 'surrounding space is ignored');
  assert.deepEqual(ids('ship it'), ['chan-general'], 'by the newest message');
  assert.deepEqual(ids('lunch'), ['dm-bob']);
  assert.deepEqual(ids('owner'), ['group-1'], 'by the name a group is shown under');
  const none = html(sidebar(workspace, snapshot(), { query: 'zzz-no-such-thing' }));
  assert.deepEqual(rowTags(none), []); assert.match(none, /<p class="tw-empty-line" role="status">No matching conversations\.<\/p>/);
  assert.doesNotMatch(none, /No channels yet|No messages yet|No Bot chats yet/, 'a section that has nothing to show for the query does not say it is empty');
  for (const add of ['New channel', 'New message', 'Message a Bot']) assert.match(none, new RegExp(`aria-label="${add}"`), 'and the sections keep their buttons');
  assert.doesNotMatch(html(sidebar(workspace, snapshot(), { query: 'general' })), /No matching conversations/);
  assert.match(html(sidebar(workspace, snapshot(), { query: '搜索', zh: true })), /没有匹配的会话。/);
});

test('sidebar: while a query is set every match is listed, with no "Show more"', async context => {
  const base = new Date(2026, 10, 1, 8, 0, 0).getTime();
  const { workspace, snapshot } = await open(fake => {
    fake.conversations.set('team-aaaa', Array.from({ length: 60 }, (_, i) => conversation(`c-${String(i).padStart(2, '0')}`, {
      title: `channel ${i}`, lastSeq: 1, lastReadSeq: 1, lastMessage: { id: `m-${i}`, seq: 1, at: new Date(base + (100 - i) * 1000).toISOString(), preview: 'x', author: { kind: 'user', uid: 'bob' } },
    })));
  });
  context.after(() => workspace.close());
  const markup = (query?: string) => html(sidebar(workspace, snapshot(), query === undefined ? {} : { query }));
  assert.equal(rowTags(markup()).length, 50); assert.match(markup(), />Show 10 more</);
  assert.equal(rowTags(markup('channel')).length, 60, 'all sixty match'); assert.doesNotMatch(markup('channel'), /Show \d+ more/);
  assert.deepEqual(rowTags(markup('channel 5')).map(row => row.id).sort(), ['c-05', 'c-50', 'c-51', 'c-52', 'c-53', 'c-54', 'c-55', 'c-56', 'c-57', 'c-58', 'c-59'], 'including the ones past the first fifty');
});

test('conversation view: header, stream with day and unread dividers, grouped runs, quotes, recalls and escaping', async context => {
  const { workspace, snapshot } = await open(); context.after(() => workspace.close());
  assert.match(html(view(workspace, snapshot())), /Team assistants/);
  workspace.selectConversation('chan-general'); await until(() => snapshot().messages.length === 7, 'the thread');
  const markup = html(view(workspace, snapshot(), { detailsOpen: false, onToggleDetails() {} }));
  const header = /<header class="tw-conv-head">[^]*?<\/header>/.exec(markup)?.[0] ?? '';
  assert.match(header, /class="tw-chan"[^>]*>#</, 'a channel\'s header has the # tile'); assert.match(header, /<h2>general<\/h2>/, 'and its title is the name alone'); assert.match(header, /<p>2 members · 1 Bot<\/p>/, 'Bots are not counted as members');
  assert.equal(count(header.replace(/<[^>]*>/g, ''), /#/g), 1, 'one #, the tile\'s: the title does not repeat it');
  assert.match(markup, /class="tw-platform" title="[^"]*readable by the platform[^"]*">Platform readable/);
  assert.match(markup, /role="log" aria-label="Messages" tabindex="0"/);
  assert.equal(count(markup, /class="tw-date"/g), 1); assert.match(markup, /class="tw-date"><span>Today<\/span>/);
  assert.equal(count(markup, /data-unread-divider/g), 1); assert.match(markup, /<span>New messages<\/span>/);
  assert.ok(markup.indexOf('data-unread-divider') < markup.indexOf('data-message-id="message-5"') && markup.indexOf('data-unread-divider') > markup.indexOf('data-message-id="message-4"'),
    'the divider sits before the first unread message written by someone else (alice\'s own message 4 does not anchor it)');
  assert.equal(count(markup, /data-grouped="true"/g), 2, 'messages 2 and 3 continue message 1');
  assert.equal(count(markup, /class="tw-msg"/g), 7);
  assert.match(markup, /data-message-id="message-1" data-message-seq="1"/);
  assert.match(markup, /<b class="tw-msg-author">Bob<\/b>/); assert.match(markup, /<b class="tw-msg-author">Alice<\/b>/);
  assert.match(markup, /<blockquote class="tw-quote"><b>Alice<\/b> Thanks &lt;script&gt;alert\(1\)&lt;\/script&gt;<\/blockquote>/, 'the quote names the author and is escaped');
  assert.doesNotMatch(markup, /<script>alert/, 'message text is never markup'); assert.match(markup, /Thanks &lt;script&gt;/);
  assert.match(markup, /<em>Message recalled<\/em>/); assert.match(markup, /\(edited\)/);
  assert.equal(count(markup, /aria-label="Reply"/g), 6, 'every message that is not recalled can be answered');
  assert.equal(count(markup, /aria-label="Edit"/g), 1, 'only the viewer\'s own'); assert.equal(count(markup, /aria-label="Recall"/g), 1);
  assert.match(markup, /<button type="button" disabled=""[^>]*>Mark as read|>Mark as read<\/button>/); assert.doesNotMatch(markup, /Mark as read<\/button>[^]*disabled=""[^]*Mark as read/);
  assert.match(markup, /aria-pressed="false" aria-label="Team details"/);
  assert.match(markup, /<textarea[^>]*aria-label="Message"[^>]*placeholder="Message #general"/); assert.match(markup, /Type @ to ask a Bot/);
  assert.match(markup, /aria-label="Ask a Bot \(@\)"/); assert.match(markup, /class="tw-primary tw-send" disabled="" aria-label="Send"/, 'nothing to send yet');
  assert.doesNotMatch(markup, /tw-jump/, 'pinned to the newest message: no jump button');
});

test('conversation view: only a shared Bot\'s messages go through the host\'s Markdown renderer; people\'s stay plain text', async context => {
  const { fake, workspace, snapshot } = await open(); context.after(() => workspace.close());
  const bot = { kind: 'shared_bot' as const, displayName: 'Fixture Bot', sharedBotId: 'assistant-fixture' };
  fake.threads.set('chan-general', [
    message(1, { text: '**typed** by a person', author: { kind: 'user', uid: 'bob' }, createdAt: at(0) }),
    message(2, { text: '## Plan\n- [ ] one', author: bot, createdAt: at(1) }),
    message(3, { text: '**gone**', author: bot, recalled: true, createdAt: at(2) }),
    message(4, { text: 'edited **reply**', author: bot, editedAt: at(4), createdAt: at(3) }),
  ]);
  workspace.selectConversation('chan-general'); await until(() => snapshot().messages.length === 4, 'the thread');
  const calls: string[] = [];
  const renderMarkdown = (text: string) => { calls.push(text); return createElement('div', { className: 'host-md' }, text); };
  const markup = html(view(workspace, snapshot(), { renderMarkdown }));
  assert.deepEqual(calls, ['## Plan\n- [ ] one', 'edited **reply**'], 'the person\'s and the recalled message never reach the renderer');
  assert.match(markup, /<p class="tw-msg-body">\*\*typed\*\* by a person<\/p>/, 'a person\'s text is a plain paragraph');
  assert.match(markup, /<div class="tw-msg-body tw-md" data-markdown="true"><div class="host-md">## Plan\n- \[ \] one<\/div><\/div>/);
  assert.match(markup, /class="tw-edited"> \(edited\)<\/small>/, 'the edited marker stays');
  assert.match(markup, /<em>Message recalled<\/em>/, 'the recalled marker stays');
  const plain = html(view(workspace, snapshot()));
  assert.doesNotMatch(plain, /data-markdown/, 'without a host renderer a Bot message is plain text, as before');
  assert.match(plain, /<p class="tw-msg-body">## Plan/);
});

test('conversation view: someone else\'s Bot dm is read-only; a Bot dm needs its Bot; a dm and a group differ in what they ask', async context => {
  const { fake, workspace, snapshot } = await open(fake => {
    fake.conversations.get('team-aaaa')!.push(conversation('others-bot-chat', { kind: 'bot_dm', title: 'Fixture Bot', participants: ['user:bob', `shared_bot:${botCard().sharedBotId}`], lastSeq: 1, lastReadSeq: 1 }));
    fake.threads.set('others-bot-chat', [message(1, { text: 'Hi bot' })]);
  });
  context.after(() => workspace.close());
  await until(() => snapshot().conversations.length === 5, 'five conversations');
  workspace.selectConversation('others-bot-chat'); await until(() => snapshot().messages.length === 1, 'their thread');
  const observer = html(view(workspace, snapshot()));
  assert.match(observer, /only participants can send messages/); assert.doesNotMatch(observer, /<textarea/); assert.doesNotMatch(observer, /aria-label="Reply"/);
  assert.match(observer, /Owner: Owner|Online · Owner/); assert.match(observer, /<h2>Fixture Bot · Bob<\/h2>/, 'the Bot\'s owner sees whose chat it is');
  workspace.selectConversation('bot-chat'); await until(() => snapshot().selectedConversationId === 'bot-chat', 'bot chat');
  const own = html(view(workspace, snapshot()));
  assert.match(own, /<textarea[^>]*placeholder="Message Fixture Bot"/); assert.match(own, /Messages ask this Bot automatically/); assert.doesNotMatch(own, /Ask a Bot \(@\)/, 'a Bot dm asks its Bot by itself');
  assert.match(own, /Online · Owner: Owner/); assert.match(own, /<h2>Fixture Bot<\/h2>/); assert.doesNotMatch(own, /tw-chan/, 'a Bot dm has its Bot\'s picture, not the channel #');
  fake.bots = [botCard({ permitted: false })]; await workspace.refreshBots(); await until(() => snapshot().bots[0]?.permitted === false, 'revoked access');
  const revoked = html(view(workspace, snapshot()));
  assert.match(revoked, /This Bot is not available to you right now/); assert.doesNotMatch(revoked, /<textarea/);
  fake.bots = []; fake.messages = async () => { throw new Error('SERVICE_UNAVAILABLE'); }; await workspace.refreshBots(); await until(() => snapshot().botsUnverified, 'unverified');
  assert.match(html(view(workspace, snapshot())), /Bot availability could not be verified/);
  workspace.selectConversation('dm-bob'); await until(() => snapshot().selectedConversationId === 'dm-bob', 'dm');
  const dm = html(view(workspace, snapshot()));
  assert.match(dm, /<h2>Bob<\/h2>/); assert.match(dm, /placeholder="Message Bob"/); assert.doesNotMatch(dm, /Ask a Bot/); assert.match(dm, /Enter to send, Shift\+Enter for a new line/); assert.match(dm, /No messages yet|Loading messages/);
});

test('conversation view: pending messages show their state and can be discarded; offline and errors explain themselves', async context => {
  const { fake, workspace, snapshot } = await open(); context.after(() => workspace.close());
  workspace.selectConversation('chan-general'); await until(() => snapshot().messages.length === 7, 'the thread');
  const originalMessages = fake.messages;
  fake.messages = async () => { throw new Error('SERVICE_UNAVAILABLE'); };
  await snapshot().messaging!.queue('create', 'Queued while offline'); await until(() => snapshot().pending.length === 1, 'the pending message');
  const markup = html(view(workspace, snapshot()));
  assert.match(markup, /class="tw-msg tw-pending"/); assert.match(markup, /Queued while offline/); assert.match(markup, />Discard</);
  assert.match(markup, /Pending · sends when connected|Waiting for server confirmation/);
  fake.messages = originalMessages;
  await until(() => snapshot().pending.length === 0, 'the retry to land');
  const [watch] = fake.open('team-aaaa', 'chan-general'); assert.ok(watch);
  fake.emit(watch, { kind: 'directory', fromCache: true, conversations: fake.conversations.get('team-aaaa')! });
  await until(() => snapshot().offline, 'offline');
  const offline = html(view(workspace, snapshot()));
  assert.match(offline, /data-kind="offline"/); assert.match(offline, /Saved on this device\. Reconnect to confirm access/);
  assert.match(offline, /Offline: messages are saved and sent when you reconnect/); assert.match(offline, /disabled=""[^>]*>Mark as read/, 'marking read needs a connection');
  const connecting = html(view(workspace, { ...snapshot(), messagingConnecting: true, messagingError: '' }));
  assert.match(connecting, /data-kind="connecting"/);
  assert.match(connecting, /messages will send after server confirmation/);
  assert.doesNotMatch(connecting, /data-kind="offline"|Offline:|>Reconnect</);
});

test('conversation view: no team, no conversations, a pending creation', async context => {
  const none = await bare(); context.after(() => none.workspace.close());
  assert.match(html(view(none.workspace, none.snapshot())), /Create or join a team/);
  const { fake, workspace, snapshot } = await open(fake => { fake.conversations.set('team-aaaa', []); });
  context.after(() => workspace.close());
  assert.match(html(view(workspace, snapshot())), /Team assistants/);
  fake.messages = async () => { throw new Error('SERVICE_UNAVAILABLE'); };
  await workspace.createConversation('channel', 'planning', ['bob']); await until(() => !!snapshot().creation, 'the pending creation');
  const markup = html(view(workspace, snapshot()));
  assert.match(markup, /Creating conversation…/); assert.match(markup, /planning/); assert.match(markup, />Discard</);
});

test('conversation subtitle names who is in a conversation or whether its Bot is reachable', () => {
  const members3 = members('alice', 'bob', 'owner');
  const snap = (bots: SharedBotCard[]) => ({ bots, directory: members3, uid });
  const bot = botCard({ sharedBotId: 'a', maintainer: { uid: 'bob', generation: 1 } });
  const botDm = conversation('b', { kind: 'bot_dm', participants: ['user:alice', 'shared_bot:a'] });
  assert.equal(conversationSubtitle(botDm, snap([bot]), false), 'Online · Owner: Bob');
  assert.equal(conversationSubtitle(botDm, snap([{ ...bot, ready: false }]), false), 'Host unavailable · Owner: Bob');
  assert.equal(conversationSubtitle(botDm, snap([{ ...bot, state: 'paused' }]), true), '已停用 · 所有者: Bob');
  assert.equal(conversationSubtitle(botDm, snap([]), false), '', 'nothing to say about a Bot that is not listed');
  assert.equal(conversationSubtitle(conversation('d', { kind: 'dm' }), snap([]), false), 'Direct message');
  assert.equal(conversationSubtitle(conversation('g', { kind: 'group', participants: ['user:alice', 'user:bob', 'user:owner'] }), snap([]), true), '3 位成员');
  const mixed = conversation('m', { kind: 'group', participants: ['user:alice', 'user:bob', 'user:carol', 'user:owner', 'shared_bot:one'] });
  assert.equal(conversationSubtitle(mixed, snap([]), false), '4 members · 1 Bot', 'Bots are not members');
  assert.equal(conversationSubtitle(mixed, snap([]), true), '4 位成员 · 1 个 Bot');
  assert.equal(conversationSubtitle(conversation('m2', { kind: 'channel', participants: ['user:alice', 'user:bob', 'shared_bot:one', 'shared_bot:two'] }), snap([]), false), '2 members · 2 Bots');
  assert.equal(conversationSubtitle(conversation('c', { participants: [] }), snap([]), false), '');
});

test('drawer: leave or delete first, then tabs by role, with a resizable separator and a close button', async context => {
  const list = members('alice', 'bob', 'owner'); list[1]!.role = 'admin';
  const { fake, workspace, snapshot } = await open(fake => {
    fake.details.set('team-aaaa', { team: team('team-aaaa', { name: 'Acme', role: 'owner' }), members: list, invites: [
      { id: 'invite-1', role: 'member', expiresAt: '2026-11-09T00:00:00.000Z', uses: 1, maxUses: 5, revoked: false },
      { id: 'invite-2', role: 'admin', expiresAt: '2026-11-09T00:00:00.000Z', uses: 0, maxUses: 1, revoked: true },
    ] });
  });
  context.after(() => workspace.close());
  await until(() => !!snapshot().detail, 'detail');
  const markup = html(drawer(workspace, snapshot()));
  assert.match(markup, /<aside class="tw-scope tw-drawer" role="complementary" aria-label="Team details" style="width:360px">/);
  assert.match(markup, /role="separator" aria-orientation="vertical"[^>]*aria-valuemin="280" aria-valuemax="480" aria-valuenow="360" tabindex="0"/);
  assert.ok(markup.indexOf('Delete team') > 0 && markup.indexOf('Delete team') < markup.indexOf('role="tablist"'), 'delete is above the tabs');
  assert.doesNotMatch(markup, /Leave team/); assert.match(markup, /<h2 tabindex="-1">Acme<\/h2>/); assert.match(markup, /aria-label="Close details"/);
  assert.equal(count(markup, /role="tab"/g), 3); assert.match(markup, /role="tab"[^>]*aria-selected="true"[^>]*tabindex="0"[^>]*>Members</); assert.equal(count(markup, /tabindex="-1" class="tw-tab">(Invites|Bots)</g), 2);
  assert.match(markup, /role="tabpanel"[^>]*aria-labelledby="[^"]*-tab-members"/);
  assert.equal(count(markup, />Remove</g), 2, 'the owner can remove the other two'); assert.match(markup, />Make member</); assert.match(markup, />Make admin</);
  assert.match(markup, /Alice<\/b><small>You · Owner/); assert.match(markup, /readable by the platform/);
  const invites = html(drawer(workspace, snapshot(), { tab: 'invites' }));
  assert.match(invites, /Create invite link/); assert.match(invites, /<option value="admin">Admin<\/option>/, 'an owner can invite admins');
  assert.match(invites, /Member · 1\/5/); assert.match(invites, />Revoke</); assert.match(invites, /Revoked/); assert.equal(count(invites, />Revoke</g), 1);
  fake.execute = async () => ({ code: 'A'.repeat(32) });
  await workspace.execute({ action: 'invite', teamId: 'team-aaaa', requestId: 'r', role: 'member', maxUses: 1, ttlSeconds: 3600 }); await until(() => !!snapshot().inviteLink, 'link');
  const shared = html(drawer(workspace, snapshot(), { tab: 'invites' }));
  assert.match(shared, new RegExp(`value="https://lingxios.net/open/team/join\\?code=${'A'.repeat(32)}"`)); assert.match(shared, /Copy<\/button>/);
  const bots = html(drawer(workspace, snapshot(), { tab: 'bots' }));
  assert.match(bots, /Fixture Bot/); assert.match(bots, /Message assistant/); assert.match(bots, /class="tw-embed"/);
  assert.match(html(drawer(workspace, snapshot(), { width: 9999 })), /style="width:480px"/); assert.match(html(drawer(workspace, snapshot(), { defaultWidth: 300 })), /style="width:300px"/);
  assert.match(html(drawer(workspace, snapshot(), { zh: true })), /删除团队/);
});

test('drawer: a plain member can leave and sees no Invites tab, even when asked for it', async context => {
  const { workspace, snapshot } = await open(); context.after(() => workspace.close());
  workspace.selectTeam('team-bbbb'); await until(() => snapshot().detail?.team.id === 'team-bbbb' && snapshot().botsLoaded, 'team B');
  const markup = html(drawer(workspace, snapshot(), { tab: 'invites' }));
  assert.match(markup, />Leave team</); assert.doesNotMatch(markup, /Delete team/); assert.equal(count(markup, /role="tab"/g), 2); assert.doesNotMatch(markup, />Invites</);
  assert.match(markup, /aria-selected="true"[^>]*>Members</, 'falls back to Members'); assert.doesNotMatch(markup, /Create invite link/); assert.equal(count(markup, />Remove</g), 0);
});

test('drawer: hidden detail after an invalidation shows loading, a failed load can be retried, no team renders nothing', async context => {
  const { fake, workspace, snapshot } = await open(); context.after(() => workspace.close());
  fake.detail = () => new Promise(() => {});
  fake.invalidate(); assert.equal(snapshot().detail, null);
  assert.match(html(drawer(workspace, snapshot())), /Loading…/);
  const none = await bare(); context.after(() => none.workspace.close());
  assert.equal(html(drawer(none.workspace, none.snapshot())), '', 'no team, nothing to show');
});
