import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

import { badgeCount } from '../../../packages/teams/friends';
import { AccountSignInContext } from '../src/renderer/bridge/useAccountState';
import { emptyDesktopState } from '../src/renderer/bridge/desktopState';
import { modeForSettledChat, pageForMode, reconcileSidebarMode, restoredSidebarMode, SidebarModeSync, type AppPage } from '../src/renderer/bridge/sidebarMode';
import { unreadBadgeText } from '../src/renderer/bridge/teamsUnread';
import type { UseBridge } from '../src/renderer/bridge/bridgeTypes.js';
import { BetaSidebar } from '../src/renderer/components/BetaDesktop';
import { FriendsSignIn } from '../src/renderer/components/FriendsSignIn';
import { navigationCatalog } from '../src/renderer/i18n/navigationCatalog';
import { tr } from '../src/renderer/i18n/locale';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import { pinEnglish } from './bot-ui-helpers';

(globalThis as { React?: typeof React }).React = React;
pinEnglish();

const vectors = JSON.parse(readFileSync(new URL('../../../packages/teams/fixtures/friends-vectors.json', import.meta.url), 'utf8'));
const noop = () => undefined;

/** Render without the warning React prints for the sidebar's layout effect, which cannot run here. */
function render(element: React.ReactElement): string {
  const error = console.error;
  console.error = (...args: unknown[]) => { if (!String(args[0]).includes('useLayoutEffect does nothing on the server')) error(...args); };
  try { return renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, element)); } finally { console.error = error; }
}
const sidebar = (props: Record<string, unknown> = {}) => render(React.createElement(BetaSidebar, { bridge: { bootstrap: null, sessionLoading: false, desktop: emptyDesktopState(), activeSession: undefined, activeBotId: undefined } as unknown as UseBridge, onOpenSettings: noop, ...props } as never));
const rail = (html: string) => /<nav[^>]*desktop-nav-rail[\s\S]*?<\/nav>/.exec(html)?.[0] ?? '';
const friendsButton = (html: string) => /<button[^>]*desktop-rail-friends[^>]*>[\s\S]*?<\/button>/.exec(rail(html))?.[0] ?? '';

// ── The rail ──

test('the rail badge is the vectors\' count in the rail\'s own rule: nothing at 0, the number up to 99, 99+ from 100', () => {
  assert.ok(vectors.badge.length >= 8);
  for (const row of vectors.badge) assert.equal(unreadBadgeText(badgeCount(row.cards, [], vectors.offers.nowMs)), row.expectText ?? '', row.note);
});

test('team invitations that are still live count on the rail badge too, and an expired one does not (D28)', () => {
  assert.ok(vectors.offers.badge.length >= 10);
  for (const row of vectors.offers.badge) {
    const offers = row.offers.map((expiresAt: number) => ({ expiresAt }));
    assert.equal(unreadBadgeText(badgeCount(row.cards, offers, vectors.offers.nowMs)), row.expectText ?? '', row.note);
  }
});

test('the Friends button sits after Teams and before Apps, is named with the exact count, and carries the capped digits', () => {
  assert.equal(tr('friends_title'), 'Friends');
  assert.equal(tr('desktop_friends_rail_unread', 3), 'Friends, 3 unread');
  const none = friendsButton(sidebar());
  assert.match(none, /aria-label="Friends"/); assert.ok(!none.includes('desktop-rail-count'));
  const some = friendsButton(sidebar({ friendsUnread: 7 }));
  assert.match(some, /aria-label="Friends, 7 unread"/); assert.match(some, /class="desktop-rail-count" data-kind="unread" aria-hidden="true">7</);
  const many = friendsButton(sidebar({ friendsUnread: 250 }));
  assert.match(many, /aria-label="Friends, 250 unread"/, 'the label says what the digits cannot'); assert.match(many, />99\+</);
  const buttons = [...rail(sidebar()).matchAll(/<button[^>]*aria-label="([^"]+)"/g)].map(match => match[1]);
  assert.ok(buttons.indexOf('Friends') === buttons.indexOf('Teams') + 1 && buttons.indexOf('Apps') === buttons.indexOf('Friends') + 1, buttons.join(', '));
});

test('the rail label is catalogued in all five languages with the count in it', () => {
  const entry = navigationCatalog['desktop_friends_rail_unread'] as Record<string, string>;
  assert.deepEqual(Object.keys(entry).sort(), ['en', 'ja', 'ko', 'zh-Hans', 'zh-Hant']);
  for (const [locale, text] of Object.entries(entry)) assert.equal(text.match(/%1\$d/g)?.length, 1, locale);
});

test('in Friends mode the sidebar shows the Friends panel and none of its own search, because the list has a filter of its own', () => {
  const modePanel = (mode: string) => React.createElement('div', { id: 'mode-panel' }, `panel for ${mode}`);
  const friends = sidebar({ sidebarMode: 'friends', modePanel });
  assert.match(friends, /panel for friends/); assert.match(friendsButton(friends), /data-active="true"/);
  assert.ok(!friends.includes(`aria-label="${tr('desktop_search_chats')}"`));
  assert.ok(sidebar({ sidebarMode: 'teams', modePanel }).includes(`aria-label="${tr('desktop_search_chats')}"`), 'the other modes keep it');
  assert.ok(!friendsButton(sidebar({ sidebarMode: 'teams', modePanel })).includes('data-active'));
});

// ── The mode and the page ──

test('Friends goes to its page from wherever the window is, and a page that is Friends is the Friends mode', () => {
  const pages: AppPage[] = ['chat', 'scheduled', 'apps', 'changes', 'bots', 'teams', 'friends'];
  for (const page of pages) {
    for (const botChat of [true, false]) assert.equal(pageForMode('friends', { page, botChat }), 'friends', `${page}/${botChat}`);
    assert.equal(reconcileSidebarMode('home', 'friends', page !== 'bots'), 'friends');
  }
  assert.equal(reconcileSidebarMode('teams', 'friends', true), 'friends');
  assert.equal(reconcileSidebarMode('friends', 'teams', true), 'teams');
  for (const page of ['chat', 'scheduled', 'changes'] as const) assert.equal(reconcileSidebarMode('friends', page, true), 'friends', `${page} retains the sidebar`);
  assert.equal(reconcileSidebarMode('friends', 'chat', false), 'friends', 'and so do Bots that cannot be used');
});

test('a settled chat does not take the person out of Friends, except an app\'s own chat', () => {
  for (const botChat of [true, false]) assert.equal(modeForSettledChat('friends', 'chat', botChat), 'friends');
  assert.equal(modeForSettledChat('friends', 'scheduled', true), 'friends');
  assert.equal(modeForSettledChat('friends', 'chat', false, true), 'apps');
});

test('the window opens in Friends when it was left there, once, and only while it is still where it opened', () => {
  assert.equal(restoredSidebarMode('friends', false), 'friends');
  assert.equal(new SidebarModeSync().open({ saved: 'friends', page: 'chat', mode: 'home', botsAvailable: false }), 'friends');
  assert.equal(new SidebarModeSync().open({ saved: 'friends', page: 'teams', mode: 'teams', botsAvailable: true }), undefined, 'a window a link has moved stays where it is');
  const sync = new SidebarModeSync();
  assert.equal(sync.open({ saved: 'home', page: 'chat', mode: 'home', botsAvailable: true }), undefined);
  assert.equal(sync.toSave('friends'), 'friends');
  assert.equal(sync.toSave('friends'), undefined);
});

// ── Signed out ──

test('signed out, the sidebar and the page say friends_signin, and offer to sign in when the window can', () => {
  const sentence = 'Sign in to your LingXi account to add friends and chat.';
  const compact = render(React.createElement(FriendsSignIn, { compact: true }));
  assert.match(compact, /<h2[^>]*>Friends<\/h2>/); assert.ok(compact.includes(sentence));
  assert.ok(!compact.includes('<button'), 'with nothing to sign in with there is no button');
  const page = render(React.createElement(AccountSignInContext.Provider, { value: noop }, React.createElement(FriendsSignIn)));
  assert.match(page, /<h2[^>]*>Friends<\/h2>/); assert.ok(page.includes(sentence));
  assert.match(page, new RegExp(`<button[^>]*>${tr('desktop_shell_sign_in')}</button>`));
});
