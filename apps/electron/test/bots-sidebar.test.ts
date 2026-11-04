import assert from 'node:assert/strict';
import test from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { DragDropProvider } from '@dnd-kit/react';
import type { SessionRowDto } from '@lingxi/bridge-client';

import type { BotUi } from '../src/shared/bots';
import { BotsController, type BotsState } from '../src/renderer/bridge/botsStore';
import { BotSessionsController } from '../src/renderer/bridge/botSessionsStore';
import { pendingBotPath } from '../src/renderer/bridge/botSession';
import { agentAvatarIndex } from '../src/renderer/components/AgentAvatar';
import { renderTeamAvatar } from '../src/renderer/components/TeamAvatar';
import { tr } from '../src/renderer/i18n/locale';
import { BotChatList, BotsSidebar, MenuItems, type BotsSidebarProps, type MenuTarget } from '../src/renderer/components/BotsSidebar';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import { pinEnglish } from './bot-ui-helpers';

(globalThis as { React?: typeof React }).React = React;
pinEnglish();

const noop = () => undefined;
const MINUTES = 60_000;
const NOW = Date.now();

function render(element: React.ReactElement): string {
  const error = console.error;
  console.error = (...args: unknown[]) => { if (!String(args[0]).includes('useLayoutEffect does nothing on the server')) error(...args); };
  try {
    return renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, React.createElement(DragDropProvider, null, element)));
  } finally {
    console.error = error;
  }
}

function bot(botId: string, overrides: Partial<BotUi> = {}): BotUi {
  return { botId, name: botId, description: '', createdAt: NOW - 600 * MINUTES, updatedAt: NOW, revision: 1, workspaces: [], ...overrides };
}

function chat(uuid: string, minutesAgo: number, title = uuid): SessionRowDto {
  return { uuid, title, modified_rfc3339: new Date(NOW - minutesAgo * MINUTES).toISOString(), message_count: 2, mode: 'code', path: `/transcripts/${uuid}.jsonl` } as SessionRowDto;
}

function botsState(change: Partial<BotsState> = {}): BotsState {
  return { availability: { available: true }, bots: [], loading: false, error: undefined, ...change };
}

/** The chats of Bots, read the way the sidebar reads them. */
async function sessionsOf(chats: Record<string, SessionRowDto[] | Error>): Promise<BotSessionsController> {
  const controller = new BotSessionsController({
    listSessions: async (botId) => {
      const answer = chats[botId];
      if (answer instanceof Error) throw answer;
      return { sessions: answer ?? [] };
    },
    onPromptSent: () => noop,
    onSessionShown: () => noop,
  });
  controller.show(Object.keys(chats));
  await new Promise<void>((resolve) => setImmediate(resolve));
  return controller;
}

function bridgeStub(overrides: Record<string, unknown> = {}, expanded: string[] = []): BotsSidebarProps['bridge'] {
  return {
    bootstrap: { runtimes: [], settings: { sidebar: { expandedBots: expanded } } },
    activeBotId: undefined,
    activeSession: undefined,
    sessionLoading: false,
    sessionRuntimeStatus: () => undefined,
    openBotSession: async () => ({ botId: '', projectPath: '', sessionId: '' }),
    updateSidebarPreferences: async () => undefined,
    ...overrides,
  } as unknown as BotsSidebarProps['bridge'];
}

async function sidebar(options: {
  bots?: BotUi[];
  state?: Partial<BotsState>;
  chats?: Record<string, SessionRowDto[] | Error>;
  expanded?: string[];
  bridge?: Record<string, unknown>;
  query?: string;
  chatVisible?: boolean;
} = {}): Promise<string> {
  const state = botsState({ bots: options.bots ?? [], ...options.state });
  const sessions = await sessionsOf(options.chats ?? {});
  const bots = { controller: new BotsController(undefined), state };
  return render(React.createElement(BotsSidebar, {
    bots, sessions, bridge: bridgeStub(options.bridge, options.expanded), query: options.query ?? '', chatVisible: options.chatVisible ?? true,
    onOpenChat: noop, onLeave: (go: () => void) => go(), onOpenBot: noop, onCreateBot: noop,
  }));
}

test('the Bots are listed most recently used first under a heading that makes a new one, and a closed Bot lists no chats', async () => {
  const html = await sidebar({
    bots: [bot('a', { name: 'Alpha', lastActivityAt: NOW - 5 * MINUTES }), bot('b', { name: 'Beta', lastActivityAt: NOW - MINUTES }), bot('c', { name: 'Gamma' })],
    chats: { a: [chat('s1', 5)] },
    expanded: [],
  });
  assert.match(html, /<h2[^>]*>Bots<\/h2>/);
  assert.match(html, /aria-label="New Bot" title="New Bot"/);
  assert.ok(html.indexOf('Beta') < html.indexOf('Alpha') && html.indexOf('Alpha') < html.indexOf('Gamma'), 'by use');
  assert.equal([...html.matchAll(/aria-expanded="false"/g)].length >= 3, true, 'each Bot is a group that is closed');
  assert.ok(!html.includes('bot-chats-'), 'no chats are listed');
  assert.ok(!html.includes('data-session-id'));
});

const avatarsOf = (html: string) => [...html.matchAll(/data-agent-avatar="(\d+)"/g)].map((match) => Number(match[1]));

test('each Bot wears the avatar of its id, the one the Teams views draw for it whether they name it by its id or by its key', async () => {
  const ids = ['alpha', 'k3Qw9ZxLmN8vTb5RpYc2Ud'];
  const html = await sidebar({ bots: ids.map((id) => bot(id)), expanded: [] });
  assert.deepEqual(avatarsOf(html).sort((left, right) => left - right), ids.map(agentAvatarIndex).sort((left, right) => left - right));
  const drawn = (identity: Parameters<typeof renderTeamAvatar>[0]) => avatarsOf(render(React.createElement(React.Fragment, null, renderTeamAvatar(identity, 20))));
  for (const id of ids) {
    assert.deepEqual(drawn({ kind: 'bot', id, name: id }), [agentAvatarIndex(id)], 'by id');
    assert.deepEqual(drawn({ kind: 'bot', id: `bot:owner:${'h'.repeat(22)}:${id}`, name: id }), [agentAvatarIndex(id)], 'by key');
  }
  assert.equal(renderTeamAvatar({ kind: 'user', id: 'alice', name: 'Alice' }, 20), undefined, 'people get their initials');
});

test('an open Bot lists its chats, newest first, five to begin with and the rest behind Show more', async () => {
  const chats = [chat('s1', 50, 'Oldest'), chat('s2', 10, 'Second'), chat('s3', 1, 'Newest'), chat('s4', 20, 'Fourth'), chat('s5', 30, 'Fifth'), chat('s6', 40, 'Sixth'), chat('s7', 60, 'Seventh')];
  const html = await sidebar({ bots: [bot('a', { name: 'Alpha' })], chats: { a: chats }, expanded: ['a'] });
  assert.match(html, /aria-expanded="true" aria-controls="bot-chats-a"/);
  assert.match(html, /id="bot-chats-a"/);
  const shown = ['Newest', 'Second', 'Fourth', 'Fifth', 'Sixth'];
  const positions = shown.map((title) => html.indexOf(`>${title}<`));
  assert.ok(positions.every((position) => position >= 0), 'the five newest are there');
  assert.deepEqual([...positions].sort((left, right) => left - right), positions, 'in the order of time, not of the order read');
  assert.ok(!html.includes('>Oldest<') && !html.includes('>Seventh<'), 'the rest are not');
  assert.ok(html.includes(tr('desktop_chat_show_more', 2)));
  assert.ok(!html.includes(tr('chat_tool_show_less')));
});

test('every chat is listed when all are asked for, with the way to fold them again', async () => {
  const chats = Array.from({ length: 7 }, (_, index) => chat(`s${index}`, index + 1, `Chat ${index}`));
  const sessions = await sessionsOf({ a: chats });
  const entry = sessions.getSnapshot().entries.a;
  const props = { bot: bot('a'), entry, query: '', matches: () => true, visibleChat: undefined, opening: undefined, bridge: bridgeStub(), onOpen: noop, onMenu: noop, onToggleAll: noop, onRetry: noop };
  const all = render(React.createElement(BotChatList, { ...props, all: true }));
  for (let index = 0; index < 7; index++) assert.ok(all.includes(`>Chat ${index}<`), `Chat ${index}`);
  assert.ok(all.includes(tr('chat_tool_show_less')));
  assert.ok(!all.includes(tr('desktop_chat_show_more', 2)));
  const some = render(React.createElement(BotChatList, { ...props, all: false }));
  assert.ok(!some.includes('>Chat 6<'));
  const few = render(React.createElement(BotChatList, { ...props, entry: { ...entry!, sessions: entry!.sessions!.slice(0, 5) }, all: false }));
  assert.ok(!few.includes(tr('chat_tool_show_less')) && !few.includes('Show more'), 'five need no fold');
});

test('the chat on screen is the marked row, and only while the chat view is the page', async () => {
  const active = { activeBotId: 'a', activeSession: { projectPath: '/data/bots/a/workspace', sessionId: 's2' } };
  const options = { bots: [bot('a')], chats: { a: [chat('s1', 5, 'First'), chat('s2', 1, 'Second')] }, expanded: ['a'], bridge: active };
  const shown = await sidebar({ ...options, chatVisible: true });
  assert.equal([...shown.matchAll(/aria-current="page"/g)].length, 1);
  assert.match(shown, /data-session-id="s2"[^>]*aria-current="page"|aria-current="page"[^>]*data-session-id="s2"/);
  const elsewhere = await sidebar({ ...options, chatVisible: false });
  assert.ok(!elsewhere.includes('aria-current="page"'), 'the Apps page is not a chat');
  const otherBot = await sidebar({ ...options, bots: [bot('a'), bot('z')], bridge: { activeBotId: 'z', activeSession: { projectPath: '/data/bots/z/workspace', sessionId: 's2' } } });
  assert.ok(!otherBot.includes('aria-current="page"'), 'a chat of another Bot does not mark this one\'s');
});

test('the Bot whose chat is on screen is open even when it has not been opened before', async () => {
  const html = await sidebar({
    bots: [bot('a')], chats: { a: [chat('s1', 5, 'First')] }, expanded: undefined as unknown as string[],
    bridge: { bootstrap: { runtimes: [], settings: { sidebar: {} } }, activeBotId: 'a', activeSession: { projectPath: '/data/bots/a/workspace', sessionId: 's1' } },
  });
  assert.match(html, /aria-expanded="true" aria-controls="bot-chats-a"/);
  assert.ok(html.includes('>First<'));
});

test('a chat just started, which the transcripts do not have yet, stands in its Bot\'s list', async () => {
  const active = { activeBotId: 'a', activeSession: { projectPath: pendingBotPath('a'), sessionId: 'fresh' } };
  const html = await sidebar({ bots: [bot('a')], chats: { a: [chat('s1', 5, 'First')] }, expanded: ['a'], bridge: active });
  assert.match(html, /data-session-id="fresh"/);
  assert.ok(html.indexOf('data-session-id="fresh"') < html.indexOf('data-session-id="s1"'), 'at the top');
  assert.match(html, />New chat</);
  assert.match(html, />just now</);

  const starting = await sidebar({ bots: [bot('a')], chats: { a: [chat('s1', 5, 'First')] }, expanded: ['a'], bridge: { activeBotId: 'a', activeSession: { projectPath: pendingBotPath('a'), sessionId: 'pending-new:4' } } });
  assert.ok(!starting.includes('pending-new'), 'a chat that has no id yet is not listed');
});

test('chats that have not been read, cannot be read, or are none are each said in words', async () => {
  const bots = [bot('a', { name: 'Alpha' })];
  const unread = await sidebar({ bots, chats: {}, expanded: ['a'] });
  assert.match(unread, /role="status"[^>]*>Loading chats…/);

  const failed = await sidebar({ bots, chats: { a: new Error('The Bot folder is missing.') }, expanded: ['a'] });
  assert.match(failed, /role="alert"[^>]*>Chats could not be loaded: The Bot folder is missing\./);
  assert.match(failed, />Retry</);
  assert.ok(!failed.includes('No chats yet'));

  const none = await sidebar({ bots, chats: { a: [] }, expanded: ['a'] });
  assert.match(none, />No chats yet</);
  assert.ok(!none.includes('Loading chats'));
});

test('with no Bots the sidebar invites the owner to make one; before the host answers it says it is loading; a list that failed says why', async () => {
  const none = await sidebar({ bots: [] });
  assert.match(none, /No Bots yet/);
  assert.match(none, />Create your first Bot</);
  assert.ok(!none.includes('Loading Bots'));

  const loading = await sidebar({ bots: [], state: { availability: undefined } });
  assert.match(loading, /role="status"[^>]*>Loading Bots…/);
  assert.ok(!loading.includes('No Bots yet'), 'not mistaken for an empty list');

  const failed = await sidebar({ bots: [], state: { error: 'The Bots folder cannot be read.' } });
  assert.match(failed, /role="alert"[^>]*>\s*Bots could not be loaded\. The Bots folder cannot be read\./);
  assert.match(failed, />Retry</);
  assert.ok(!failed.includes('No Bots yet'));
});

test('searching keeps the Bots whose name matches, and the open Bots with a chat whose title does', async () => {
  const bots = [bot('a', { name: 'Alpha' }), bot('b', { name: 'Beta' }), bot('c', { name: 'Gamma' })];
  const chats = { a: [chat('a1', 5, 'Plan the launch'), chat('a2', 6, 'Weekly notes')], b: [chat('b1', 5, 'Launch party')], c: [chat('c1', 5, 'Lunch')] };

  const byName = await sidebar({ bots, chats, expanded: ['a', 'b'], query: 'alp' });
  assert.ok(byName.includes('Alpha') && !byName.includes('Beta') && !byName.includes('Gamma'));
  assert.ok(byName.includes('>Plan the launch<') && byName.includes('>Weekly notes<'), 'a Bot that matches by name shows its chats');

  const byChat = await sidebar({ bots, chats, expanded: ['a', 'b'], query: 'launch' });
  assert.ok(byChat.includes('Alpha') && byChat.includes('Beta') && !byChat.includes('Gamma'));
  assert.ok(byChat.includes('>Plan the launch<') && byChat.includes('>Launch party<'));
  assert.ok(!byChat.includes('>Weekly notes<'), 'only the chats that match');
  assert.ok(!byChat.includes(tr('desktop_chat_show_more', 1)), 'a search lists every match');

  const closed = await sidebar({ bots, chats, expanded: [], query: 'launch' });
  assert.ok(!closed.includes('Alpha'), 'the chats of a closed Bot are not searched');

  const nothing = await sidebar({ bots, chats, expanded: ['a'], query: 'zzz' });
  assert.match(nothing, /No matching Bots or chats\./);
  assert.ok(!nothing.includes('Alpha'));
  assert.ok(!(await sidebar({ bots, chats, expanded: ['a'], query: '' })).includes('No matching'));
});

test('each Bot has a hover button for a new chat and one for its menu, and the chats under it have neither pin nor archive', async () => {
  const html = await sidebar({ bots: [bot('a', { name: 'Alpha' })], chats: { a: [chat('s1', 5, 'First')] }, expanded: ['a'] });
  assert.match(html, /class="sidebar-row-action"[^>]*aria-label="New chat with Alpha"/);
  assert.match(html, /class="sidebar-row-action"[^>]*aria-haspopup="menu"[^>]*aria-expanded="false"[^>]*aria-label="Actions for Alpha"/);
  assert.equal([...html.matchAll(/class="sidebar-row-action"/g)].length, 2, 'two on the Bot and none on the chat');
  assert.ok(!html.includes('Pin ') && !html.includes('Archive'), 'a chat of a Bot is not the owner\'s to pin or archive from here');
});

test('while a chat is being opened no other can be started from the sidebar', async () => {
  const html = await sidebar({ bots: [bot('a', { name: 'Alpha' })], chats: {}, bridge: { sessionLoading: true } });
  assert.match(html, /class="sidebar-row-action"[^>]*disabled=""[^>]*aria-label="New chat with Alpha"/);
});

test('a Bot at work or waiting for the owner is marked, and a Bot at rest is not', async () => {
  const runtime = (sessionId: string, botId: string, turnActive: boolean, pendingInteractions = 0) => ({ sessionId, botId, projectPath: `/data/bots/${botId}/workspace`, turnActive, pendingInteractions, pendingAskUserQuestions: 0 });
  const html = await sidebar({
    bots: [bot('a', { name: 'Alpha' }), bot('b', { name: 'Beta' }), bot('c', { name: 'Gamma' })],
    bridge: { bootstrap: { runtimes: [runtime('r-a', 'a', true), runtime('r-b', 'b', false, 1)], settings: { sidebar: { expandedBots: [] } } } },
  });
  assert.match(html, /Alpha[\s\S]*?role="img" aria-label="Working"/);
  assert.match(html, /Beta[\s\S]*?role="img" aria-label="Needs you"/);
  assert.equal([...html.matchAll(/role="img"/g)].length, 2, 'Gamma is at rest');
});

test('a Bot\'s menu offers a new chat, its settings and its deletion; a chat\'s offers to open it', () => {
  const bot: MenuTarget = { kind: 'bot', botId: 'a', name: 'Alpha' };
  const chat: MenuTarget = { kind: 'chat', botId: 'a', sessionId: 's1', title: 'First', transcriptPath: '/chosen/s1.jsonl' };
  const items = (target: MenuTarget, busy = false) => render(React.createElement(MenuItems, { target, busy, danger: '#ff453a', onNewChat: noop, onSettings: noop, onDelete: noop, onOpen: noop }));
  const forBot = items(bot);
  const labels = [...forBot.matchAll(/role="menuitem"[^>]*>(?:<svg[\s\S]*?<\/svg>)? ?([^<]+)</g)].map((match) => match[1]);
  assert.deepEqual(labels, ['New chat', 'Bot settings', 'Delete Bot…']);
  assert.match(forBot, /role="menuitem" style="color:#ff453a"/, 'the deletion looks like one');
  const forChat = items(chat);
  assert.equal([...forChat.matchAll(/role="menuitem"/g)].length, 1);
  assert.match(forChat, /<\/svg> Open<\/button>/);

  const busy = items(bot, true);
  assert.equal([...busy.matchAll(/disabled=""/g)].length, 1, 'only the new chat waits for the one being opened');
  assert.match(items(chat, true), /disabled=""/);
});

test('the menu\'s deletion asks to delete and deletes nothing itself', () => {
  const calls: unknown[][] = [];
  const record = (name: string) => (...args: unknown[]) => { calls.push([name, ...args]); };
  const elements = (element: React.ReactNode): Array<React.ReactElement<{ role?: string; onClick?: () => void; children?: React.ReactNode }>> => {
    const found: Array<React.ReactElement<{ role?: string; onClick?: () => void; children?: React.ReactNode }>> = [];
    React.Children.forEach(element, (child) => {
      if (!React.isValidElement(child)) return;
      const props = child.props as { role?: string; onClick?: () => void; children?: React.ReactNode };
      if (props.role === 'menuitem') found.push(child as never);
      else found.push(...elements(props.children));
    });
    return found;
  };
  const props = { busy: false, danger: 'red', onNewChat: record('new'), onSettings: record('settings'), onDelete: record('delete'), onOpen: record('open') };
  const forBot = elements(MenuItems({ ...props, target: { kind: 'bot', botId: 'a', name: 'Alpha' } }));
  assert.equal(forBot.length, 3);
  forBot[2]!.props.onClick!();
  assert.deepEqual(calls, [['delete', 'a']]);
  forBot[0]!.props.onClick!();
  forBot[1]!.props.onClick!();
  assert.deepEqual(calls.slice(1), [['new', 'a'], ['settings', 'a']]);
  calls.length = 0;
  elements(MenuItems({ ...props, target: { kind: 'chat', botId: 'a', sessionId: 's1', title: 'First', transcriptPath: '/chosen/s1.jsonl' } }))[0]!.props.onClick!();
  assert.deepEqual(calls, [['open', 'a', 's1', '/chosen/s1.jsonl']]);
});
