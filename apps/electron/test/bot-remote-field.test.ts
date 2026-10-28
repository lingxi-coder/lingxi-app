import assert from 'node:assert/strict';
import test from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

import type { BotUi, BotsApi } from '../src/shared/bots';
import type { RemoteControlUiStatus } from '../src/shared/remoteControl';
import { BotEditorController, botEditorDirty } from '../src/renderer/bridge/botEditor';
import { botCreateRequest, draftFromBot, emptyBotDraft, type BotDraft } from '../src/renderer/bridge/botForm';
import { botRemoteAvailability, remoteAvailability } from '../src/renderer/bridge/remoteControl';
import { tr } from '../src/renderer/i18n/locale';
import { BotRemoteField } from '../src/renderer/components/BotFields';
import { Toggle } from '../src/renderer/components/settings/primitives';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import { pinEnglish } from './bot-ui-helpers';

(globalThis as { React?: typeof React }).React = React;
pinEnglish();

const noop = () => undefined;

function render(element: React.ReactElement): string {
  return renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, element));
}

const status = (patch: Partial<RemoteControlUiStatus> = {}): RemoteControlUiStatus => ({
  enabled: true, atStartup: false, managedDisabled: false, signedIn: true, state: 'online', connections: [], ...patch,
});

function bot(overrides: Partial<BotUi> = {}): BotUi {
  return { botId: 'b1', name: 'Scribe', description: '', createdAt: 1_000, updatedAt: 1_000, revision: 1, workspaces: [], ...overrides };
}

// ---- When the switch can be turned on ----

test('a Bot can be reached remotely only where Remote Control is usable and on for this computer, and each other case says why', () => {
  assert.deepEqual(botRemoteAvailability(status()), { available: true });
  assert.deepEqual(botRemoteAvailability(status({ state: 'starting' })), { available: true });
  assert.deepEqual(botRemoteAvailability(status({ state: 'error', error: 'offline' })), { available: true }, 'a connection that failed is not a switch that is off');
  assert.deepEqual(botRemoteAvailability(status({ enabled: false, state: 'online' })), { available: true }, 'a service that runs without the setting, as in development, is on');

  assert.deepEqual(botRemoteAvailability(null), remoteAvailability(null));
  assert.deepEqual(botRemoteAvailability(status({ managedDisabled: true })), remoteAvailability(status({ managedDisabled: true })));
  assert.deepEqual(botRemoteAvailability(status({ signedIn: false })), remoteAvailability(status({ signedIn: false })));

  const off = botRemoteAvailability(status({ enabled: false, state: 'off' }));
  assert.deepEqual(off, { available: false, reason: tr('bots_remote_computer_off') });
  assert.match(tr('bots_remote_computer_off'), /Remote control is off on this computer/);
  const reasons = [null, status({ managedDisabled: true }), status({ signedIn: false }), status({ enabled: false, state: 'off' })].map((value) => {
    const availability = botRemoteAvailability(value);
    return availability.available ? '' : availability.reason ?? '';
  });
  assert.equal(new Set(reasons).size, 4, 'each reason is its own words');
  assert.ok(reasons.every((reason) => reason.length > 0));
  assert.deepEqual(botRemoteAvailability(status({ managedDisabled: true, enabled: false, state: 'off' })), remoteAvailability(status({ managedDisabled: true })), 'the administrator\'s word comes before the computer\'s switch');
});

// ---- The draft and what saving it sends ----

test('the draft starts from the saved Bot, a new Bot starts with the switch off, and the editor saves a change of it with the revision it read', async () => {
  assert.equal(emptyBotDraft().remote, false);
  assert.ok(!('remote' in botCreateRequest({ ...emptyBotDraft(), name: 'New' })));
  assert.equal(draftFromBot(bot(), '').remote, false);
  assert.equal(draftFromBot(bot({ remote: true }), '').remote, true);

  const state = { bot: bot(), updates: [] as Array<[number, Record<string, unknown>]> };
  const api = {
    async get() { return state.bot; },
    async update(_botId: string, revision: number, patch: Record<string, unknown>) {
      state.updates.push([revision, patch]);
      state.bot = { ...state.bot, ...(patch['remote'] === false ? { remote: undefined } : patch), revision: state.bot.revision + 1 } as BotUi;
      return state.bot;
    },
    persona: { async get() { return ''; }, async set() { /* unused */ } },
  } as unknown as Pick<BotsApi, 'get' | 'update' | 'persona'>;
  const editor = new BotEditorController(api, 'b1', noop);
  await editor.load();
  assert.equal(botEditorDirty(editor.getSnapshot()), false);
  editor.setDraft({ remote: true });
  assert.equal(botEditorDirty(editor.getSnapshot()), true);
  await editor.save();
  assert.deepEqual(state.updates, [[1, { remote: true }]]);
  assert.equal(editor.getSnapshot().draft.remote, true, 'the draft is the saved Bot again');
  assert.equal(botEditorDirty(editor.getSnapshot()), false);
  editor.setDraft({ remote: false });
  assert.equal(botEditorDirty(editor.getSnapshot()), true);
  await editor.save();
  assert.deepEqual(state.updates[1], [2, { remote: false }]);
  assert.equal(editor.getSnapshot().draft.remote, false);
});

// ---- The field ----

const switchTag = (html: string): string => /<button[^>]*role="switch"[^>]*>/.exec(html)![0];
const draft = (remote: boolean): BotDraft => ({ ...emptyBotDraft(), remote });

test('the field says what the switch lets devices do, and shows it on or off as the draft has it', () => {
  const off = render(React.createElement(BotRemoteField, { draft: draft(false), availability: { available: true }, onChange: noop }));
  assert.match(off, /Reach this Bot from your phone and browser/);
  assert.match(off, /Your approved devices can see this Bot&#x27;s chats, start new ones, answer its questions and stop its turns\./);
  assert.match(switchTag(off), /aria-checked="false"/);
  assert.match(switchTag(off), /aria-label="Reach this Bot from your phone and browser"/);
  assert.ok(!/disabled=""/.test(switchTag(off)), 'it can be turned on');
  assert.ok(!off.includes('role="status"'), 'nothing is wrong, so nothing is said');

  const on = render(React.createElement(BotRemoteField, { draft: draft(true), availability: { available: true }, onChange: noop }));
  assert.match(switchTag(on), /aria-checked="true"/);
});

test('where Remote Control is not available the switch cannot be turned on and says why, but one that is on can always be turned off', () => {
  const reason = 'Remote control is off on this computer. Turn it on in Settings first.';
  const unavailable = { available: false as const, reason };
  const stuckOff = render(React.createElement(BotRemoteField, { draft: draft(false), availability: unavailable, onChange: noop }));
  assert.match(switchTag(stuckOff), /disabled=""/);
  assert.match(stuckOff, /<span role="status"[^>]*>Remote control is off on this computer\. Turn it on in Settings first\.<\/span>/);

  const turnOffable = render(React.createElement(BotRemoteField, { draft: draft(true), availability: unavailable, onChange: noop }));
  assert.ok(!/disabled=""/.test(switchTag(turnOffable)), 'turning off is always allowed');
  assert.match(switchTag(turnOffable), /aria-checked="true"/);
  assert.match(turnOffable, /role="status"/, 'and the reason is still said');
});

test('while the Bot is being saved the switch cannot be moved either way', () => {
  for (const remote of [false, true]) {
    const html = render(React.createElement(BotRemoteField, { draft: draft(remote), availability: { available: true }, disabled: true, onChange: noop }));
    assert.match(switchTag(html), /disabled=""/, String(remote));
  }
});

test('moving the switch changes only the draft\'s remote setting', () => {
  const changes: Array<Partial<BotDraft>> = [];
  const element = React.createElement(BotRemoteField, { draft: draft(false), availability: { available: true }, onChange: (change: Partial<BotDraft>) => changes.push(change) });
  const tree = (element as unknown as { type: (props: unknown) => React.ReactElement }).type(element.props);
  // Find the switch in the element tree without a DOM.
  const switches: Array<{ value: boolean; onChange(value: boolean): void }> = [];
  const walk = (node: React.ReactNode): void => {
    if (!React.isValidElement(node)) return;
    const props = node.props as { value?: boolean; children?: React.ReactNode };
    if (node.type === Toggle) switches.push(props as never);
    React.Children.forEach(props.children, walk);
  };
  walk(tree);
  assert.equal(switches.length, 1);
  switches[0]!.onChange(true);
  assert.deepEqual(changes, [{ remote: true }]);
});
