import assert from 'node:assert/strict';
import test from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

import type { BotUi, BotsApi } from '../src/shared/bots';
import { BotEditorController, botEditorDirty } from '../src/renderer/bridge/botEditor';
import {
  BOT_ACCEPT_MAX, botChanges, botCreateRequest, botDraftProblems, botPeers, draftFromBot, emptyBotDraft, saveBot, type BotDraft,
} from '../src/renderer/bridge/botForm';
import { BotAcceptField, problemText } from '../src/renderer/components/BotFields';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import { pinEnglish } from './bot-ui-helpers';

(globalThis as { React?: typeof React }).React = React;
pinEnglish();

const noop = () => undefined;
const id = (letter: string) => letter.repeat(22);
const ME = id('M');
const ALICE = id('A');
const BOB = id('B');
const GONE = id('G');

function bot(overrides: Partial<BotUi> = {}): BotUi {
  return { botId: ME, name: 'Me', description: '', createdAt: 1_000, updatedAt: 1_000, revision: 1, workspaces: [], ...overrides };
}

const peers = (...ids: string[]) => new Set(ids);

function render(element: React.ReactElement): string {
  return renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, element));
}

// ---- The draft and what saving it sends ----

test('the draft starts from the saved list, and a new Bot starts with none and is made with none', () => {
  assert.deepEqual(emptyBotDraft().acceptFrom, []);
  assert.deepEqual(draftFromBot(bot({ acceptFrom: [ALICE, GONE] }), '').acceptFrom, [ALICE, GONE]);
  assert.deepEqual(draftFromBot(bot(), '').acceptFrom, [], 'absent is the same as empty');
  const saved = bot({ acceptFrom: [ALICE] });
  const draft = draftFromBot(saved, '');
  draft.acceptFrom.push(BOB);
  assert.deepEqual(saved.acceptFrom, [ALICE], 'the draft is its own copy');
  assert.ok(!('acceptFrom' in botCreateRequest({ ...emptyBotDraft(), name: 'New' })));
});

test('the Bots to choose among are the others on the list, and unknown until the list has this Bot in it', () => {
  assert.deepEqual([...botPeers([{ botId: ME }, { botId: ALICE }, { botId: BOB }], ME)!].sort(), [ALICE, BOB]);
  assert.deepEqual([...botPeers([{ botId: ME }], ME)!], []);
  assert.equal(botPeers([], ME), undefined, 'a list not read yet knows nothing');
  assert.equal(botPeers([{ botId: ALICE }], ME), undefined, 'nor a list that does not have this Bot');
});

test('choosing a Bot is a change, and so is choosing none; going back to what was saved is not', () => {
  const saved = bot({ acceptFrom: [ALICE] });
  const draft = (acceptFrom: string[]): BotDraft => ({ ...draftFromBot(saved, ''), acceptFrom });
  const known = peers(ALICE, BOB);
  assert.deepEqual(botChanges(saved, '', draft([ALICE]), known).patch, {});
  assert.deepEqual(botChanges(saved, '', draft([ALICE, BOB]), known).patch, { acceptFrom: [ALICE, BOB] });
  assert.deepEqual(botChanges(saved, '', draft([BOB]), known).patch, { acceptFrom: [BOB] });
  assert.deepEqual(botChanges(saved, '', draft([]), known).patch, { acceptFrom: [] }, 'an empty list is sent: no Bot may hand this one a task');
  const both = bot({ acceptFrom: [ALICE, BOB] });
  assert.deepEqual(botChanges(both, '', { ...draftFromBot(both, ''), acceptFrom: [BOB, ALICE] }, known).patch, {}, 'the same Bots in another order are no change');
  assert.deepEqual(botChanges(bot(), '', draft([]), known).patch, {}, 'none saved, none chosen');
});

test('the id of a Bot that is gone is no change of the owner\'s, is not sent back, and does not stop the list from being saved', () => {
  const saved = bot({ acceptFrom: [ALICE, GONE] });
  const known = peers(ALICE, BOB);
  const untouched = draftFromBot(saved, '');
  assert.deepEqual(botChanges(saved, '', untouched, known).patch, {}, 'a Bot that was deleted is not a reason to save');
  assert.deepEqual(botDraftProblems(untouched, known), {});
  assert.deepEqual(botChanges(saved, '', { ...untouched, acceptFrom: [ALICE, GONE, BOB] }, known).patch, { acceptFrom: [ALICE, BOB] }, 'when the list is saved anyway, the gone id is left out');
  assert.deepEqual(botChanges(saved, '', { ...untouched, acceptFrom: [GONE] }, known).patch, { acceptFrom: [] });
});

test('while the Bots are not known the saved list is left alone, whatever the draft says', () => {
  const saved = bot({ acceptFrom: [ALICE] });
  const draft = { ...draftFromBot(saved, ''), acceptFrom: [] as string[], name: 'Renamed' };
  assert.deepEqual(botChanges(saved, '', draft, undefined).patch, { name: 'Renamed' });
  assert.deepEqual(botChanges(saved, '', draft).patch, { name: 'Renamed' });
});

test('too many Bots chosen is a problem, but the ids of Bots that are gone are not counted', () => {
  const many = Array.from({ length: BOT_ACCEPT_MAX + 1 }, (_, n) => `Bot${String(n).padStart(19, '0')}`);
  const draft = { ...emptyBotDraft(), name: 'Me', acceptFrom: many };
  assert.deepEqual(botDraftProblems(draft, new Set(many)), { accept: 'accept_too_many' });
  assert.deepEqual(botDraftProblems(draft, new Set(many.slice(0, 3))), {}, 'only three of them are Bots there are');
  assert.deepEqual(botDraftProblems({ ...draft, acceptFrom: many.slice(0, BOT_ACCEPT_MAX) }, new Set(many)), {});
  assert.equal(problemText('accept_too_many'), 'At most 64 Bots can be chosen.');
});

test('saving sends the list with the rest of the change, and a list that only had a gone id sends nothing', async () => {
  const updates: Array<Record<string, unknown>> = [];
  const api = {
    async update(_botId: string, _revision: number, patch: Record<string, unknown>) { updates.push(patch); return bot({ revision: 2 }); },
    persona: { async set() { /* unused */ } },
  } as unknown as Pick<BotsApi, 'update' | 'persona'>;
  const saved = bot({ acceptFrom: [ALICE, GONE] });
  const outcome = await saveBot(api, saved, '', { ...draftFromBot(saved, ''), name: 'Renamed', acceptFrom: [ALICE, BOB, GONE] }, peers(ALICE, BOB));
  assert.equal(outcome.ok, true);
  assert.deepEqual(updates, [{ name: 'Renamed', acceptFrom: [ALICE, BOB] }]);
  updates.length = 0;
  await saveBot(api, saved, '', { ...draftFromBot(saved, ''), name: 'Renamed again' }, peers(ALICE, BOB));
  assert.deepEqual(updates, [{ name: 'Renamed again' }], 'the list was not touched');
});

// ---- The editor ----

function editorApi(initial: BotUi) {
  const state = { bot: initial, updates: [] as Array<[number, Record<string, unknown>]> };
  const api = {
    async get() { return state.bot; },
    async update(_botId: string, revision: number, patch: Record<string, unknown>) {
      state.updates.push([revision, patch]);
      state.bot = { ...state.bot, ...patch, revision: state.bot.revision + 1 } as BotUi;
      return state.bot;
    },
    persona: { async get() { return ''; }, async set() { /* unused */ } },
  };
  return { api: api as unknown as Pick<BotsApi, 'get' | 'update' | 'persona'>, state };
}

test('the editor saves the list chosen among the Bots there are now, and is not made unsaved by a Bot that is gone', async () => {
  const { api, state } = editorApi(bot({ acceptFrom: [ALICE, GONE] }));
  let known: ReadonlySet<string> | undefined = peers(ALICE, BOB);
  const editor = new BotEditorController(api, ME, noop, () => known);
  await editor.load();
  assert.equal(botEditorDirty(editor.getSnapshot(), known), false, 'nothing was changed');
  editor.setDraft({ acceptFrom: [...editor.getSnapshot().draft.acceptFrom, BOB] });
  assert.equal(botEditorDirty(editor.getSnapshot(), known), true);
  await editor.save();
  assert.deepEqual(state.updates, [[1, { acceptFrom: [ALICE, BOB] }]]);
  assert.deepEqual(editor.getSnapshot().draft.acceptFrom, [ALICE, BOB], 'the draft is the saved Bot again');
  assert.equal(botEditorDirty(editor.getSnapshot(), known), false);

  // The list of Bots goes away (it is being read again): the list saved is not touched, and cannot be made to look changed.
  known = undefined;
  editor.setDraft({ acceptFrom: [] });
  assert.equal(botEditorDirty(editor.getSnapshot(), known), false);
  await editor.save();
  assert.equal(state.updates.length, 1);
});

test('an editor made without the Bots to choose among never sends the list', async () => {
  const { api, state } = editorApi(bot({ acceptFrom: [ALICE] }));
  const editor = new BotEditorController(api, ME, noop);
  await editor.load();
  editor.setDraft({ acceptFrom: [], name: 'Renamed' });
  await editor.save();
  assert.deepEqual(state.updates, [[1, { name: 'Renamed' }]]);
});

// ---- The field ----

test('the other Bots are listed by name with what they do, the ones allowed are ticked, and what allowing means is said', () => {
  const others = [
    bot({ botId: BOB, name: 'Bob', description: 'Reviews code' }),
    bot({ botId: ALICE, name: 'Alice', description: '' }),
  ];
  const html = render(React.createElement(BotAcceptField, { draft: { ...emptyBotDraft(), acceptFrom: [BOB] }, problems: {}, peers: others, onChange: noop }));
  assert.match(html, /<legend class="bots-label">Tasks from other Bots<\/legend>/);
  assert.match(html, /Choose the Bots that may hand tasks to this one\./);
  assert.ok(html.indexOf('>Alice<') < html.indexOf('>Bob<'), 'by name');
  assert.match(html, /<input type="checkbox"[^>]*checked=""[^>]*\/><span class="bots-accept-name">Bob<\/span><span class="bots-muted bots-accept-description">Reviews code<\/span>/);
  assert.match(html, /<input type="checkbox"[^>]*\/><span class="bots-accept-name">Alice<\/span>/);
  assert.equal([...html.matchAll(/checked=""/g)].length, 1, 'only Bob is allowed');
  assert.match(html, /With none chosen, no Bot may hand this one a task\./);
  assert.match(html, /A task runs in a chat of this Bot, with its permissions and folders\./);
  assert.match(html, /Files that come with a task are copied into this Bot&#x27;s inbox folder\./);
  assert.match(html, /Other Bots on this computer can see this Bot&#x27;s name and description when they list Bots\./);
});

test('a Bot that is gone is not listed, and with no other Bots there is nothing to choose', () => {
  const html = render(React.createElement(BotAcceptField, { draft: { ...emptyBotDraft(), acceptFrom: [GONE] }, problems: {}, peers: [], onChange: noop }));
  assert.match(html, /There are no other Bots yet\./);
  assert.ok(!html.includes('type="checkbox"'));
  assert.ok(!html.includes('checked'));
});

test('before the list of Bots is read nothing can be chosen, and the field says so rather than that there are none', () => {
  const html = render(React.createElement(BotAcceptField, { draft: emptyBotDraft(), problems: {}, peers: undefined, onChange: noop }));
  assert.match(html, /The list of Bots has not loaded yet\./);
  assert.ok(!html.includes('There are no other Bots yet.'));
});

test('ticking and unticking change the list, keeping the order it was chosen in', () => {
  const others = [bot({ botId: ALICE, name: 'Alice' }), bot({ botId: BOB, name: 'Bob' })];
  const changes: Array<Partial<BotDraft>> = [];
  const element = React.createElement(BotAcceptField, { draft: { ...emptyBotDraft(), acceptFrom: [BOB] }, problems: {}, peers: others, onChange: (change: Partial<BotDraft>) => changes.push(change) });
  const tree = (element as unknown as { type: (props: unknown) => React.ReactElement }).type(element.props);
  // Find the checkboxes in the element tree without a DOM.
  const boxes: Array<{ checked: boolean; onChange(event: { target: { checked: boolean } }): void }> = [];
  const walk = (node: React.ReactNode): void => {
    if (!React.isValidElement(node)) return;
    const props = node.props as { type?: string; checked?: boolean; onChange?: never; children?: React.ReactNode };
    if (node.type === 'input' && props.type === 'checkbox') boxes.push(props as never);
    React.Children.forEach(props.children, walk);
  };
  walk(tree);
  assert.equal(boxes.length, 2);
  boxes[0]!.onChange({ target: { checked: true } });
  boxes[1]!.onChange({ target: { checked: false } });
  assert.deepEqual(changes, [{ acceptFrom: [BOB, ALICE] }, { acceptFrom: [] }]);
});

test('a list that is too long is a problem next to the field', () => {
  const html = render(React.createElement(BotAcceptField, { draft: emptyBotDraft(), problems: { accept: 'accept_too_many' }, peers: [], onChange: noop }));
  assert.match(html, /role="alert"[^>]*>At most 64 Bots can be chosen\./);
});
