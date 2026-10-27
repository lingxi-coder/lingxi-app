import assert from 'node:assert/strict';
import test from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

import type { BotMemoryEntryUi, BotSkillUi, BotUi, BotsApi } from '../src/shared/bots';
import { BotMemoryController } from '../src/renderer/bridge/botMemory';
import { BotSkillsController } from '../src/renderer/bridge/botSkills';
import { BotsController, type BotsState } from '../src/renderer/bridge/botsStore';
import { botDraftProblems, emptyBotDraft, permissionModeNeedsConfirmation } from '../src/renderer/bridge/botForm';
import { emptyDesktopState } from '../src/renderer/bridge/desktopState';
import { emptyRuntimeCenterState } from '../src/renderer/bridge/runtimeCenterState';
import type { UseBridge } from '../src/renderer/bridge/bridgeTypes.js';
import type { UseBots } from '../src/renderer/bridge/useBots';
import { tr } from '../src/renderer/i18n/locale';
import { BotSaveBar } from '../src/renderer/components/BotDetail';
import { BotFoldersField, BotGeneralFields, BotPermissionField, BotPersonaField } from '../src/renderer/components/BotFields';
import { BotChatsPanel, BotMemoryPanel, BotSkillsPanel } from '../src/renderer/components/BotPanels';
import { BotCreate, BotList, Bots, shownProblems } from '../src/renderer/components/Bots';
import { BetaSidebar, BetaTopBar } from '../src/renderer/components/BetaDesktop';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import { pinEnglish } from './bot-ui-helpers';

// Match the existing render tests under the root tsconfig's classic JSX loader.
(globalThis as { React?: typeof React }).React = React;
pinEnglish();

const noop = () => undefined;
const MINUTES = 60_000;

/** Render without the warning React prints for the sidebar's layout effect, which cannot run here. */
function render(element: React.ReactElement): string {
  const error = console.error;
  console.error = (...args: unknown[]) => { if (!String(args[0]).includes('useLayoutEffect does nothing on the server')) error(...args); };
  try {
    return renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, element));
  } finally {
    console.error = error;
  }
}

function bot(overrides: Partial<BotUi> = {}): BotUi {
  return { botId: 'b1', name: 'Scribe', description: 'Writes things down', createdAt: Date.now() - 60 * MINUTES, updatedAt: Date.now(), revision: 1, workspaces: [], ...overrides };
}

function state(change: Partial<BotsState> = {}): BotsState {
  return { availability: { available: true }, bots: [], loading: false, error: undefined, ...change };
}

const models = { references: [], details: [] };

// ---- The list ----

test('with no Bots the list invites the owner to create the first one', () => {
  const html = render(React.createElement(BotList, { state: state(), activityOf: () => 'idle', onNew: noop, onOpen: noop, onRetry: noop }));
  assert.match(html, /No Bots yet/);
  assert.match(html, /Create your first Bot/);
  assert.match(html, />New Bot</);
  assert.ok(!html.includes('bots-grid'));
});

test('the list says it is loading before the host has answered, and is not mistaken for an empty one', () => {
  const html = render(React.createElement(BotList, { state: state({ availability: undefined }), activityOf: () => 'idle', onNew: noop, onOpen: noop, onRetry: noop }));
  assert.match(html, /role="status"[^>]*>Loading Bots…/);
  assert.ok(!html.includes('No Bots yet'));
});

test('a list that could not be read says so, with the reason and a way to try again', () => {
  const html = render(React.createElement(BotList, { state: state({ error: 'The Bots folder cannot be read.' }), activityOf: () => 'idle', onNew: noop, onOpen: noop, onRetry: noop }));
  assert.match(html, /role="alert"[^>]*>Bots could not be loaded\. The Bots folder cannot be read\./);
  assert.match(html, />Retry</);
  assert.ok(!html.includes('No Bots yet'));
});

test('each Bot shows its name, description, whether it is working, and when it was last active; the busiest comes first', () => {
  const bots = [
    bot({ botId: 'a', name: 'Alpha', description: 'First helper', createdAt: 1 }),
    bot({ botId: 'b', name: 'Beta', description: 'Second helper', createdAt: 2, lastActivityAt: Date.now() - 5.5 * MINUTES }),
    bot({ botId: 'c', name: 'Gamma', description: '', createdAt: 3 }),
  ];
  const activity = { a: 'working', b: 'waiting', c: 'idle' } as const;
  const html = render(React.createElement(BotList, { state: state({ bots }), activityOf: (botId: string) => activity[botId as keyof typeof activity], onNew: noop, onOpen: noop, onRetry: noop }));
  assert.ok(html.indexOf('Beta') < html.indexOf('Gamma') && html.indexOf('Gamma') < html.indexOf('Alpha'), 'most recently used first');
  assert.match(html, /Second helper/);
  assert.match(html, /data-state="working">Working</);
  assert.match(html, /data-state="waiting">Needs you</);
  assert.match(html, /data-state="idle">Idle</);
  assert.match(html, /Active 5 minutes ago/);
  assert.match(html, /No chats yet/);
  assert.ok(!html.includes('No Bots yet'));
});

// ---- Making a Bot ----

test('the form that makes a Bot asks for each part, and does not scold an empty name before it is tried', () => {
  const html = render(React.createElement(BotCreate, { api: {} as BotsApi, models, pickFolder: async () => null, onCancel: noop, onCreated: noop }));
  for (const label of ['Name', 'Description', 'Model', 'Persona', 'Default permission mode', 'Folders']) assert.match(html, new RegExp(`>${label}<`), label);
  assert.match(html, />Create Bot</);
  assert.match(html, /← All Bots/);
  assert.ok(!html.includes('Give the Bot a name.'));
  assert.ok(!html.includes('role="alert"'));
});

test('a name left empty is only a problem once creating was tried, while a name that is wrong is one at once', () => {
  assert.deepEqual(shownProblems(botDraftProblems(emptyBotDraft()), false), {});
  assert.deepEqual(shownProblems(botDraftProblems(emptyBotDraft()), true), { name: 'name_required' });
  assert.deepEqual(shownProblems(botDraftProblems({ ...emptyBotDraft(), name: 'x'.repeat(81) }), false), { name: 'name_too_long' });
});

test('a problem with a field is written next to it and marks the field invalid', () => {
  const draft = { ...emptyBotDraft(), name: '', description: 'x'.repeat(501) };
  const html = render(React.createElement(BotGeneralFields, { draft, problems: botDraftProblems(draft), models, onChange: noop }));
  assert.match(html, /aria-invalid="true"/);
  assert.match(html, /role="alert"[^>]*>Give the Bot a name\./);
  assert.match(html, /role="alert"[^>]*>The description can have at most 500 characters\./);
  const persona = render(React.createElement(BotPersonaField, { draft: { ...emptyBotDraft(), persona: '界'.repeat(22_000) }, problems: { persona: 'persona_too_long' }, onChange: noop }));
  assert.match(persona, /The persona is too long \(64 KB at most\)\./);
});

test('the model can be chosen from the ones the engine lists, and a model that is gone is still shown', () => {
  const draft = { ...emptyBotDraft(), name: 'Scribe', model: 'builtin/retired-model' };
  const html = render(React.createElement(BotGeneralFields, { draft, problems: {}, models: { references: ['builtin/model-a', 'openai/model-b'], details: [] }, onChange: noop }));
  assert.match(html, /<option value="">Default model<\/option>/);
  assert.match(html, /<option value="builtin\/model-a">/);
  assert.match(html, /<option value="openai\/model-b">/);
  assert.match(html, /<option value="builtin\/retired-model"[^>]*selected[^>]*>builtin\/retired-model \(not available now\)<\/option>/);
});

test('every permission mode can be the default, asking is the one chosen when none was, and only Full access is confirmed', () => {
  const html = render(React.createElement(BotPermissionField, { draft: emptyBotDraft(), onChange: noop }));
  for (const label of ['Ask for approval', 'Accept edits', 'Plan mode', 'Approve for me', 'Full access']) assert.match(html, new RegExp(`>${label}</option>`), label);
  assert.match(html, /Don(&#x27;|')t ask<\/option>/);
  assert.match(html, /<option value="" selected="">Ask for approval<\/option>/);
  assert.match(html, /never changes your own preference/);

  const plan = render(React.createElement(BotPermissionField, { draft: { ...emptyBotDraft(), permissionMode: 'plan' }, onChange: noop }));
  assert.match(plan, /<option value="plan" selected="">Plan mode<\/option>/);

  assert.equal(permissionModeNeedsConfirmation('bypassPermissions'), true);
  for (const mode of ['', 'default', 'acceptEdits', 'plan', 'auto', 'dontAsk'] as const) assert.equal(permissionModeNeedsConfirmation(mode), false, mode);
});

test('folders show their access and can be removed; a Bot with none works only in its own workspace', () => {
  const none = render(React.createElement(BotFoldersField, { draft: emptyBotDraft(), problems: {}, pickFolder: async () => null, onChange: noop }));
  assert.match(none, /No folders yet\. The Bot works only in its own workspace\./);
  assert.match(none, />Add folder…</);

  const draft = { ...emptyBotDraft(), workspaces: [{ path: '/work/notes', mode: 'read' as const }, { path: '/work/site', mode: 'write' as const }] };
  const html = render(React.createElement(BotFoldersField, { draft, problems: {}, pickFolder: async () => null, onChange: noop }));
  assert.match(html, /aria-label="Access to \/work\/notes"/);
  assert.match(html, /aria-label="Remove folder \/work\/site"/);
  assert.match(html, /<option value="write" selected="">Read and write<\/option>/);
  assert.match(html, /<option value="read" selected="">Read only<\/option>/);

  const full = { ...emptyBotDraft(), workspaces: Array.from({ length: 16 }, (_, index) => ({ path: `/work/${index}`, mode: 'read' as const })) };
  assert.match(render(React.createElement(BotFoldersField, { draft: full, problems: {}, pickFolder: async () => null, onChange: noop })), /<button type="button" disabled="">Add folder…<\/button>/);
  const duplicated = render(React.createElement(BotFoldersField, { draft, problems: { folders: 'folders_duplicate' }, pickFolder: async () => null, onChange: noop }));
  assert.match(duplicated, /role="alert"[^>]*>The same folder is listed twice\./);
});

// ---- Saving ----

test('a Bot that changed elsewhere is said to have, with a way to reload, and cannot be saved over', () => {
  const html = render(React.createElement(BotSaveBar, { state: { save: { kind: 'conflict' } }, dirty: true, problems: false, onSave: noop, onReload: noop }));
  assert.match(html, /role="alert"[^>]*>This Bot was changed somewhere else\. Reload to see the latest version; your edits here have not been saved\./);
  assert.match(html, />Reload</);
  assert.match(html, /<button type="button" data-variant="primary" disabled="">Save<\/button>/);
});

test('the save bar says what saving came to, and is enabled only for a change that can be saved', () => {
  const bar = (save: Parameters<typeof BotSaveBar>[0]['state']['save'], dirty: boolean, problems = false) =>
    render(React.createElement(BotSaveBar, { state: { save }, dirty, problems, onSave: noop, onReload: noop }));
  assert.match(bar({ kind: 'idle' }, true), /Unsaved changes/);
  assert.match(bar({ kind: 'idle' }, true), /<button type="button" data-variant="primary">Save<\/button>/);
  assert.match(bar({ kind: 'idle' }, false), /<button type="button" data-variant="primary" disabled="">Save<\/button>/);
  assert.match(bar({ kind: 'idle' }, true, true), /disabled=""/);
  assert.match(bar({ kind: 'saved' }, false), /Saved\./);
  assert.match(bar({ kind: 'saving' }, true), /disabled=""[^>]*>Saving…/);
  assert.match(bar({ kind: 'failed', message: 'The disk is full.' }, true), /role="alert"[^>]*>Could not save: The disk is full\./);
});

// ---- Chats ----

test('the chats of a Bot are listed with when they were last used, and a chat that is being opened says so', () => {
  const sessions = [
    { uuid: 's1', title: 'Plan the launch', modified_rfc3339: new Date(Date.now() - 5.5 * MINUTES).toISOString(), message_count: 4, mode: 'code', path: '' },
    { uuid: 's2', title: '  ', modified_rfc3339: new Date(Date.now() - 3.5 * 3_600_000).toISOString(), message_count: 1, mode: 'code', path: '' },
  ];
  const html = render(React.createElement(BotChatsPanel, { chats: { status: 'ready', sessions }, opening: undefined, disabled: false, onOpen: noop, onRetry: noop }));
  assert.match(html, /Plan the launch/);
  assert.match(html, /5 minutes ago · 4 messages/);
  assert.ok(html.includes(tr('remote_untitled')), 'a chat without a title is still a row');

  const opening = render(React.createElement(BotChatsPanel, { chats: { status: 'ready', sessions }, opening: 's1', disabled: true, onOpen: noop, onRetry: noop }));
  assert.match(opening, /aria-busy="true"/);
  assert.match(opening, /Opening…/);
  assert.match(opening, /disabled=""/);
});

test('a Bot without chats, and chats that cannot be read, are each said in words', () => {
  const empty = render(React.createElement(BotChatsPanel, { chats: { status: 'ready', sessions: [] }, opening: undefined, disabled: false, onOpen: noop, onRetry: noop }));
  assert.match(empty, /No chats with this Bot yet\./);
  const failed = render(React.createElement(BotChatsPanel, { chats: { status: 'failed', message: 'The Bot folder is missing.' }, opening: undefined, disabled: false, onOpen: noop, onRetry: noop }));
  assert.match(failed, /role="alert"[^>]*>Chats could not be loaded: The Bot folder is missing\./);
  assert.match(failed, />Retry</);
  assert.match(render(React.createElement(BotChatsPanel, { chats: { status: 'loading' }, opening: undefined, disabled: false, onOpen: noop, onRetry: noop })), /Loading chats…/);
});

// ---- Memory ----

function memoryController(files: Record<string, string>, entries: BotMemoryEntryUi[]) {
  const api: BotsApi['memory'] = {
    async list() { return entries; },
    async read(_botId, name) { return files[name]!; },
    async write() {},
    async delete() {},
  };
  return new BotMemoryController(api, 'b1');
}

const ENTRIES: BotMemoryEntryUi[] = [
  { name: 'user-role.md', description: 'Who the owner is', type: 'user', size: 40, modifiedAt: Date.now() - 5.5 * MINUTES },
  { name: 'style.md', size: 20, modifiedAt: Date.now() - 3.5 * 3_600_000 },
];

test('memory lists each entry with its description, type and time, and offers to view and delete it', async () => {
  const controller = memoryController({}, ENTRIES);
  await controller.load();
  const html = render(React.createElement(BotMemoryPanel, { controller }));
  assert.match(html, /user-role\.md/);
  assert.match(html, /Who the owner is/);
  assert.match(html, /class="bots-tag">user</);
  assert.match(html, /Changed 5 minutes ago/);
  assert.match(html, /aria-label="View: style\.md"/);
  assert.match(html, /aria-label="Delete: style\.md"/);
  assert.ok(!html.includes('role="dialog"'), 'nothing is asked until a deletion is started');
});

test('an empty memory, and a memory that cannot be read, are each said in words', async () => {
  const empty = memoryController({}, []);
  await empty.load();
  assert.match(render(React.createElement(BotMemoryPanel, { controller: empty })), /Nothing remembered yet\./);
  const broken = new BotMemoryController({ list: async () => { throw new Error('The memory folder is missing.'); } } as unknown as BotsApi['memory'], 'b1');
  await broken.load();
  assert.match(render(React.createElement(BotMemoryPanel, { controller: broken })), /role="alert"[^>]*>The memory could not be read: The memory folder is missing\./);
});

test('an entry can be read and then edited in place', async () => {
  const controller = memoryController({ 'style.md': 'Short answers.' }, ENTRIES);
  await controller.load();
  await controller.open('style.md');
  const viewing = render(React.createElement(BotMemoryPanel, { controller }));
  assert.match(viewing, /<pre class="bots-memory-content">Short answers\.<\/pre>/);
  assert.match(viewing, />Edit</);

  controller.edit();
  controller.setDraft('Short answers, no emoji.');
  const editing = render(React.createElement(BotMemoryPanel, { controller }));
  assert.match(editing, /<textarea[^>]*aria-label="Memory text"[^>]*>Short answers, no emoji\.<\/textarea>/);
  assert.match(editing, />Save</);
  assert.ok(!editing.includes('bots-memory-content'));

  controller.setDraft('bad \0 text');
  assert.match(render(React.createElement(BotMemoryPanel, { controller })), /This memory contains characters that cannot be saved\./);
});

test('deleting a memory asks in the app first and says that chats already had keep what they learned', async () => {
  const controller = memoryController({}, ENTRIES);
  await controller.load();
  controller.askDelete('style.md');
  const html = render(React.createElement(BotMemoryPanel, { controller }));
  assert.match(html, /role="dialog"[^>]*aria-label="Delete this memory\?"/);
  assert.match(html, /“style\.md” will be removed from this Bot&#x27;s memory\./);
  assert.match(html, /Chats the Bot has already had may still contain what it learned there/);
  assert.match(html, />Cancel</);
  assert.match(html, />Delete memory</);
});

// ---- Skills ----

const OWNER_SKILLS: BotSkillUi[] = [{ name: 'commit', description: 'Write a commit message' }, { name: 'review' }, { name: 'translate' }];

function skillsController(present: BotSkillUi[], importable: BotSkillUi[]) {
  const api: BotsApi['skills'] = {
    async list() { return present; },
    async listImportable() { return importable; },
    async importFromUser(_botId, name) { return { name }; },
    async delete() {},
  };
  return new BotSkillsController(api, 'b1');
}

test('a Bot\'s skills are listed, and the owner\'s own are offered for import except the ones it already has', async () => {
  const controller = skillsController([{ name: 'review', description: 'Review a diff' }], OWNER_SKILLS);
  await controller.load();
  const list = render(React.createElement(BotSkillsPanel, { controller }));
  assert.match(list, /Review a diff/);
  assert.match(list, />Import from my skills…</);
  assert.ok(!list.includes('role="dialog"'));

  await controller.openImport();
  const importing = render(React.createElement(BotSkillsPanel, { controller }));
  assert.match(importing, /role="dialog"[^>]*aria-label="Import a skill"/);
  assert.match(importing, /Copies one of your skills into this Bot\./);
  assert.match(importing, /Write a commit message/);
  assert.match(importing, /aria-label="Import: commit"/);
  assert.match(importing, /aria-label="Import: translate"/);
  assert.ok(!importing.includes('aria-label="Import: review"'), 'the Bot already has it');
});

test('with nothing left to import, the import says so; and a Bot without skills says so too', async () => {
  const controller = skillsController([], []);
  await controller.load();
  assert.match(render(React.createElement(BotSkillsPanel, { controller })), /No skills yet\./);
  await controller.openImport();
  assert.match(render(React.createElement(BotSkillsPanel, { controller })), /None of your skills are left to import\./);
});

test('deleting a skill asks first and says the owner\'s own skills are not affected', async () => {
  const controller = skillsController([{ name: 'review' }], []);
  await controller.load();
  controller.askDelete('review');
  const html = render(React.createElement(BotSkillsPanel, { controller }));
  assert.match(html, /aria-label="Delete this skill\?"/);
  assert.match(html, /Your own skills are not affected\./);
  assert.match(html, />Delete skill</);
});

// ---- The page ----

function bridgeStub(runtimes: unknown[] = [], extra: Record<string, unknown> = {}) {
  return {
    bootstrap: { runtimes },
    desktop: emptyDesktopState(),
    sessionLoading: false,
    sessionRuntimeStatus: () => undefined,
    openBotSession: async () => ({ botId: 'a', projectPath: '/data/bots/a/workspace', sessionId: 's1' }),
    deleteBot: async () => undefined,
    pickBotFolder: async () => null,
    ...extra,
  } as unknown as Pick<UseBridge, 'bootstrap' | 'desktop' | 'sessionLoading' | 'sessionRuntimeStatus' | 'openBotSession' | 'deleteBot' | 'pickBotFolder'>;
}

async function pageBots(view: UseBots['view'], bots: BotUi[]): Promise<UseBots> {
  const api: Pick<BotsApi, 'availability' | 'list'> = { availability: async () => ({ available: true }), list: async () => bots };
  const controller = new BotsController(api);
  await controller.start();
  return { controller, state: controller.getSnapshot(), available: true, view, setView: noop };
}

test('the page shows which Bots are working from the chats the host is running', async () => {
  const bots = await pageBots({ kind: 'list' }, [bot({ botId: 'a', name: 'Alpha' }), bot({ botId: 'b', name: 'Beta' }), bot({ botId: 'c', name: 'Gamma' })]);
  const runtimes = [
    { sessionId: 's-a', botId: 'a', projectPath: '/data/bots/a/workspace', turnActive: true, pendingInteractions: 0, pendingAskUserQuestions: 0 },
    { sessionId: 's-b', botId: 'b', projectPath: '/data/bots/b/workspace', turnActive: false, pendingInteractions: 0, pendingAskUserQuestions: 0 },
    { sessionId: 's-p', projectPath: '/work/app', turnActive: true, pendingInteractions: 0, pendingAskUserQuestions: 0 },
  ];
  // The window has since heard that Beta is waiting for an answer: that is fresher than the summary.
  const live = (sessionId: string) => sessionId === 's-b' ? { turnActive: true, pendingInteractions: 1 } : undefined;
  const html = render(React.createElement(Bots, { visible: true, bots, api: {} as BotsApi, bridge: bridgeStub(runtimes, { sessionRuntimeStatus: live }), onOpenChat: noop }));
  assert.match(html, /aria-label="Bots"/);
  assert.match(html, /Alpha[\s\S]*?data-state="working">Working</);
  assert.match(html, /Beta[\s\S]*?data-state="waiting">Needs you</);
  assert.match(html, /Gamma[\s\S]*?data-state="idle">Idle</);
});

test('a Bot opens to its chats first, with the sections of its settings, and a chat can be started or the Bot deleted', async () => {
  const bots = await pageBots({ kind: 'detail', botId: 'a' }, [bot({ botId: 'a', name: 'Alpha', description: 'First helper' })]);
  const html = render(React.createElement(Bots, { visible: true, bots, api: {} as BotsApi, bridge: bridgeStub(), onOpenChat: noop }));
  assert.match(html, /<h1>Alpha<\/h1>/);
  assert.match(html, /First helper/);
  for (const tab of ['Chats', 'General', 'Persona', 'Memory', 'Skills', 'Folders', 'Permissions']) assert.match(html, new RegExp(`<button type="button"[^>]*>${tab}</button>`), tab);
  assert.match(html, /aria-current="page">Chats</);
  assert.match(html, />New chat</);
  assert.match(html, />Delete Bot</);
  assert.match(html, /Loading chats…/);
  assert.ok(!html.includes('role="dialog"'), 'nothing is asked until deletion is started');
});

test('while another chat is opening the Bot\'s chats cannot be started', async () => {
  const bots = await pageBots({ kind: 'detail', botId: 'a' }, [bot({ botId: 'a', name: 'Alpha' })]);
  const html = render(React.createElement(Bots, { visible: true, bots, api: {} as BotsApi, bridge: bridgeStub([], { sessionLoading: true }), onOpenChat: noop }));
  assert.match(html, /<button type="button" data-variant="primary" disabled="">New chat<\/button>/);
});

test('the page shows the form that makes a Bot when asked to', async () => {
  const bots = await pageBots({ kind: 'create' }, []);
  const html = render(React.createElement(Bots, { visible: true, bots, api: {} as BotsApi, bridge: bridgeStub(), onOpenChat: noop }));
  assert.match(html, /<h1>New Bot<\/h1>/);
  assert.match(html, />Create Bot</);
});

// ---- The window around the page ----

function sidebarBridge(overrides: Record<string, unknown> = {}) {
  return { bootstrap: null, sessionLoading: false, desktop: emptyDesktopState(), activeSession: undefined, activeBotId: undefined, ...overrides } as unknown as UseBridge;
}

test('the sidebar offers Bots only where the host said they can be used', () => {
  const without = render(React.createElement(BetaSidebar, { bridge: sidebarBridge(), onOpenSettings: noop }));
  assert.ok(!without.includes('aria-label="Bots"'));
  assert.ok(!without.includes('>Bots<'));
  const refused = render(React.createElement(BetaSidebar, { bridge: sidebarBridge(), onOpenSettings: noop, botsAvailable: false }));
  assert.ok(!refused.includes('Bots'));

  const offered = render(React.createElement(BetaSidebar, { bridge: sidebarBridge(), onOpenSettings: noop, botsAvailable: true }));
  assert.match(offered, /class="desktop-rail-button" aria-label="Bots" title="Bots"/);
  assert.match(offered, /<span>Bots<\/span>/);
});

test('with the Bots page open the sidebar marks it, and Home is not marked', () => {
  const home = tr('desktop_home');
  const closed = render(React.createElement(BetaSidebar, { bridge: sidebarBridge(), onOpenSettings: noop, botsAvailable: true }));
  assert.match(closed, new RegExp(`data-active="true" aria-label="${home}"`));
  assert.ok(!/aria-current="page"[^>]*><svg[^>]*><rect x="4" y="8"/.test(closed));

  const open = render(React.createElement(BetaSidebar, { bridge: sidebarBridge(), onOpenSettings: noop, botsAvailable: true, bots: true }));
  assert.ok(!new RegExp(`data-active="true" aria-label="${home}"`).test(open), 'Home is not the page');
  assert.match(open, /data-active="true" aria-label="Bots"/);
  assert.match(open, /aria-current="page"[^>]*><svg[^>]*><rect x="4" y="8"/);
});

test('a Bot\'s chat is titled with the Bot, not with a folder path, and offers the way back to the Bot', () => {
  const bridge = { bootstrap: null, sessionLoading: false, desktop: emptyDesktopState(), runtimeCenter: emptyRuntimeCenterState(), activeSession: { projectPath: '/data/bots/a/workspace', sessionId: 's1' } } as unknown as UseBridge;
  const html = render(React.createElement(BetaTopBar, { bridge, runtimeCenterOpen: false, onToggleRuntimeCenter: noop, bot: { name: 'Scribe', onOpen: noop } }));
  assert.match(html, />Scribe<\/div>/);
  assert.match(html, />Bot chat<\/div>/);
  assert.match(html, /aria-label="Open Bot"/);
  assert.ok(!html.includes('/data/bots'), 'the Bot\'s private folder is not shown');

  const project = render(React.createElement(BetaTopBar, { bridge, runtimeCenterOpen: false, onToggleRuntimeCenter: noop }));
  assert.ok(!project.includes('Open Bot'));
  assert.match(project, /\/data\/bots\/a\/workspace/, 'a project chat still shows its folder');
});
