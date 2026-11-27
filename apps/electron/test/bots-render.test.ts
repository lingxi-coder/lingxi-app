import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

import type { BotMemoryEntryUi, BotSkillUi, BotUi, BotWorkspaceRequestUi, BotsApi, BotsAvailability } from '../src/shared/bots';
import { BotMemoryController } from '../src/renderer/bridge/botMemory';
import { BotSkillsController } from '../src/renderer/bridge/botSkills';
import { BotWorkspaceRequestsController } from '../src/renderer/bridge/botWorkspaceRequests';
import { BotLeaveGuard } from '../src/renderer/bridge/botLeaveGuard';
import { BotsController, type BotsState } from '../src/renderer/bridge/botsStore';
import { botDraftProblems, emptyBotDraft, permissionModeNeedsConfirmation, type BotDraft } from '../src/renderer/bridge/botForm';
import { emptyDesktopState } from '../src/renderer/bridge/desktopState';
import { emptyRuntimeCenterState } from '../src/renderer/bridge/runtimeCenterState';
import type { UseBridge } from '../src/renderer/bridge/bridgeTypes.js';
import type { UseBots } from '../src/renderer/bridge/useBots';
import { tr } from '../src/renderer/i18n/locale';
import { agentAvatarIndex } from '../src/renderer/components/AgentAvatar';
import { BotSaveBar } from '../src/renderer/components/BotDetail';
import { BotFoldersField, BotGeneralFields, BotPermissionField, BotPersonaField, BotSandboxAcknowledge, BotSandboxField } from '../src/renderer/components/BotFields';
import { BotMemoryPanel, BotSkillsPanel, BotWorkspaceRequestsPanel } from '../src/renderer/components/BotPanels';
import { BotCreate, BotList, Bots, shownProblems } from '../src/renderer/components/Bots';
import { BetaSidebar, BetaTopBar } from '../src/renderer/components/BetaDesktop';
import { Toggle } from '../src/renderer/components/settings/primitives';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { controlBorder } from '../src/renderer/theme/teamsTheme';
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

/** The avatars drawn in some markup, as the indexes `AgentAvatar` marks them with, in order. */
const avatarsOf = (html: string) => [...html.matchAll(/data-agent-avatar="(\d+)"/g)].map((match) => Number(match[1]));

function bot(overrides: Partial<BotUi> = {}): BotUi {
  return { botId: 'b1', name: 'Scribe', description: 'Writes things down', createdAt: Date.now() - 60 * MINUTES, updatedAt: Date.now(), revision: 1, workspaces: [], sandbox: { enabled: true, allowedDomains: [] }, scope: { localApps: [], datasets: [] }, autoReply: { policy: '', keywords: [], cooldownSec: 0, thresholds: { decline: 0.3, run: 0.75 }, approval: 'never' }, ...overrides };
}

/** What a computer that can give a Bot a sandbox says, and one that cannot. */
const READY: BotsAvailability = { available: true, sandbox: { available: true } };
const NO_SANDBOX: BotsAvailability = { available: true, sandbox: { available: false, reason: 'bots_sandbox_why_no_bwrap' } };

function state(change: Partial<BotsState> = {}): BotsState {
  return { availability: READY, bots: [], loading: false, error: undefined, ...change };
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
  assert.deepEqual(avatarsOf(html), ['b', 'c', 'a'].map(agentAvatarIndex), 'each card wears the avatar of its Bot\'s id, as in the sidebar and the Teams views');
  assert.ok(!/class="bot-card-avatar"[^>]*>[^<]/.test(html), 'and no letter stands in for it');
  assert.equal([...html.matchAll(/class="bot-card-avatar" aria-hidden="true"/g)].length, 3, 'the name beside it is what is read out');
});

// ---- Making a Bot ----

test('the form that makes a Bot asks for each part, and does not scold an empty name before it is tried', () => {
  const html = render(React.createElement(BotCreate, { api: {} as BotsApi, models, pickFolder: async () => null, sandbox: { available: true }, onCancel: noop, onCreated: noop }));
  for (const label of ['Name', 'Description', 'Model', 'Persona', 'Default permission mode', 'Folders']) {
    assert.match(html, new RegExp(`>${label}<`), label);
  }
  assert.match(html, />Create Bot</);
  assert.match(html, /aria-label="All Bots"/);
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

test('Bot model choices distinguish subscription and API billing without changing model references', () => {
  const references = ['openai/gpt-6-luna', 'openai-chatgpt/gpt-6-luna'];
  const choices = { references, details: references.map(reference => ({ reference, display_name: 'GPT-6 Luna' })) };
  for (const reference of references) {
    const html = render(React.createElement(BotGeneralFields, { draft: { ...emptyBotDraft(), model: reference }, problems: {}, models: choices, onChange: noop }));
    const options = [...html.matchAll(/<option value="([^"]+)"([^>]*)>([^<]*)<\/option>/g)];
    assert.deepEqual(options.map(option => option[1]).sort(), [...references].sort());
    assert.equal(options.find(option => option[1] === 'openai-chatgpt/gpt-6-luna')?.[3], 'GPT-6 Luna · ChatGPT subscription');
    assert.equal(options.find(option => option[1] === 'openai/gpt-6-luna')?.[3], 'GPT-6 Luna · API key');
    assert.deepEqual(options.filter(option => option[2]!.includes('selected')).map(option => option[1]), [reference]);
    assert.match(html, /<optgroup[^>]*label="OpenAI · ChatGPT subscription"/);
    assert.match(html, /<optgroup[^>]*label="OpenAI · API key"/);
  }
});

test('new Bots explicitly ask for approval, while an unset mode is shown as the app default', () => {
  const html = render(React.createElement(BotPermissionField, { draft: emptyBotDraft(), onChange: noop }));
  for (const label of ['Ask for approval', 'Accept edits', 'Plan mode', 'Approve for me', 'Full access']) assert.match(html, new RegExp(`>${label}</option>`), label);
  assert.match(html, /Don(&#x27;|')t ask<\/option>/);
  assert.match(html, /<option value="default" selected="">Ask for approval<\/option>/);
  assert.match(html, /never changes your own preference/);

  const plan = render(React.createElement(BotPermissionField, { draft: { ...emptyBotDraft(), permissionMode: 'plan' }, onChange: noop }));
  assert.match(plan, /<option value="plan" selected="">Plan mode<\/option>/);
  const unset = render(React.createElement(BotPermissionField, { draft: { ...emptyBotDraft(), permissionMode: '' }, onChange: noop }));
  assert.match(unset, /<option value="" selected="">App default<\/option>/);
  assert.match(unset, /configured mode/);

  assert.equal(permissionModeNeedsConfirmation('bypassPermissions'), true);
  for (const mode of ['', 'default', 'acceptEdits', 'plan', 'auto', 'dontAsk'] as const) assert.equal(permissionModeNeedsConfirmation(mode), false, mode);
});

test('the sandbox is a switch that is on for a Bot that has it, with the hosts it may reach listed under it and a way to take each away', () => {
  const draft = { ...emptyBotDraft(), sandbox: { enabled: true, allowedDomains: ['example.com', '*.example.org'] } };
  const html = render(React.createElement(BotSandboxField, { draft, problems: {}, availability: { available: true }, onChange: noop }));
  assert.match(html, /<button type="button" role="switch" class="settings-toggle"[^>]*aria-checked="true"[^>]*aria-label="Sandbox for shell commands"/);
  assert.ok(!/role="switch"[^>]*disabled/.test(html), 'it can be turned off');
  assert.ok(!html.includes('role="status"'), 'a computer that can give one has nothing to explain');
  assert.match(html, /<legend class="bots-label">Hosts the commands may reach<\/legend>/);
  assert.match(html, /class="bots-row-title mono">example\.com</);
  assert.match(html, /class="bots-row-title mono">\*\.example\.org</);
  assert.match(html, /aria-label="Remove example\.com"/);
  assert.match(html, /aria-label="Remove \*\.example\.org"/);
  assert.match(html, /<input[^>]*placeholder="example\.com or \*\.example\.com"[^>]*aria-label="Host to allow"/);
  assert.match(html, /<button type="button" disabled="">Add<\/button>/, 'nothing typed, nothing to add');
  assert.ok(!html.includes('No hosts'));
  assert.ok(!html.includes('role="dialog"'), 'nothing is asked until the sandbox is switched off');

  const none = render(React.createElement(BotSandboxField, { draft: emptyBotDraft(), problems: {}, availability: { available: true }, onChange: noop }));
  assert.match(none, /No hosts: every connection a command in the sandbox makes needs your approval first\./);
  assert.ok(!none.includes('aria-label="Remove '));
});

/** The switch of a sandbox field, found in what the field draws while it is being drawn, since there is no DOM to click in. */
function sandboxSwitch(draft: BotDraft, changes: Array<Partial<BotDraft>>): { value: boolean; onChange(value: boolean): void } {
  const found: Array<{ value: boolean; onChange(value: boolean): void }> = [];
  const walk = (node: React.ReactNode): void => {
    if (!React.isValidElement(node)) return;
    const props = node.props as { value?: boolean; children?: React.ReactNode };
    if (node.type === Toggle) found.push(props as never);
    React.Children.forEach(props.children, walk);
  };
  const Probe = () => {
    walk(BotSandboxField({ draft, problems: {}, availability: { available: true }, onChange: (change: Partial<BotDraft>) => changes.push(change) }));
    return null;
  };
  render(React.createElement(Probe));
  assert.equal(found.length, 1);
  return found[0]!;
}

test('the sandbox is turned on at once, and turned off only once the owner has been asked, which the draft does not wait for', () => {
  const changes: Array<Partial<BotDraft>> = [];
  const on = sandboxSwitch({ ...emptyBotDraft(), sandbox: { enabled: true, allowedDomains: ['example.com'] } }, changes);
  assert.equal(on.value, true);
  on.onChange(false);
  assert.deepEqual(changes, [], 'turning it off changes nothing until the question is answered');
  const off = sandboxSwitch({ ...emptyBotDraft(), sandbox: { enabled: false, allowedDomains: ['example.com'] } }, changes);
  assert.equal(off.value, false);
  off.onChange(true);
  assert.deepEqual(changes, [{ sandbox: { enabled: true, allowedDomains: ['example.com'] } }], 'turning it on is not asked about, and keeps the hosts the Bot had');
});

test('a Bot without the sandbox shows the switch off and no hosts to edit, which would reach nothing', () => {
  const draft = { ...emptyBotDraft(), sandbox: { enabled: false, allowedDomains: ['kept.example.com'] } };
  const html = render(React.createElement(BotSandboxField, { draft, problems: {}, availability: { available: true }, onChange: noop }));
  assert.match(html, /role="switch"[^>]*aria-checked="false"/);
  assert.ok(!html.includes('Hosts the commands may reach'));
  assert.ok(!html.includes('kept.example.com'));
  assert.ok(!html.includes('role="dialog"'));
});

test('where this computer has no sandbox the switch cannot be turned on, and says why; a Bot that has it on can still be turned off', () => {
  const off = render(React.createElement(BotSandboxField, { draft: { ...emptyBotDraft(), sandbox: { enabled: false, allowedDomains: [] } }, problems: {}, availability: { available: false, reason: 'bots_sandbox_why_no_bwrap' }, onChange: noop }));
  assert.match(off, /role="switch"[^>]*disabled=""[^>]*aria-checked="false"/);
  assert.match(off, /role="status" class="bots-row-text bots-sandbox-why">bubblewrap \(bwrap\) is not installed on this computer\./);
  const on = render(React.createElement(BotSandboxField, { draft: emptyBotDraft(), problems: {}, availability: { available: false, reason: 'bots_sandbox_why_wsl1' }, onChange: noop }));
  assert.ok(!/role="switch"[^>]*disabled/.test(on), 'turning it off is always allowed');
  assert.match(on, /aria-checked="true"/);
  assert.match(on, /This computer runs WSL 1, which cannot run the sandbox\. WSL 2 can\./);
  const saving = render(React.createElement(BotSandboxField, { draft: emptyBotDraft(), problems: {}, availability: { available: true }, disabled: true, onChange: noop }));
  assert.match(saving, /role="switch"[^>]*disabled=""/, 'nothing changes while a save is under way');
  assert.match(saving, /<fieldset class="bots-fields bots-sandbox-hosts" disabled="">/);
});

test('more hosts than may be allowed is said next to the hosts', () => {
  const html = render(React.createElement(BotSandboxField, { draft: emptyBotDraft(), problems: { sandbox: 'sandbox_too_many' }, availability: { available: true }, onChange: noop }));
  assert.match(html, /role="alert"[^>]*>At most 64 hosts can be allowed\./);
});

test('the form that makes a Bot on a computer with no sandbox asks the owner to say they know, and one with a sandbox asks nothing of the kind', () => {
  const props = { api: {} as BotsApi, models, pickFolder: async () => null, onCancel: noop, onCreated: noop };
  const without = render(React.createElement(BotCreate, { ...props, sandbox: { available: false, reason: 'bots_sandbox_why_platform' } }));
  assert.match(without, /<span class="bots-row-title">Sandbox for shell commands<\/span>/);
  assert.match(without, /This device can&#x27;t isolate this Bot\./);
  assert.match(without, /This operating system does not support the sandbox\./);
  assert.match(without, /role="switch"[^>]*disabled=""[^>]*aria-checked="false"[^>]*aria-label="Sandbox for shell commands"/, 'the switch is shown off and cannot be turned on');
  assert.match(without, /<input type="checkbox"[^>]*\/><span>I understand that on this computer this Bot&#x27;s shell commands will not be confined to its folders\.<\/span>/);
  assert.ok(!/<input type="checkbox"[^>]*checked/.test(without), 'unticked until the owner ticks it');
  assert.ok(!without.includes('Tick the box'), 'not scolded before it is tried');
  assert.ok(without.indexOf('Folders') < without.indexOf('Sandbox for shell commands') && without.indexOf('Sandbox for shell commands') < without.indexOf('Create Bot'), 'between the folders and the button');

  const withSandbox = render(React.createElement(BotCreate, { ...props, sandbox: { available: true } }));
  assert.ok(!withSandbox.includes('Sandbox for shell commands'));
  assert.ok(!withSandbox.includes('type="checkbox"'));
});

test('the acknowledgement is asked for again, with the box marked, once creating was tried without it', () => {
  const tried = render(React.createElement(BotSandboxAcknowledge, { reason: 'bots_sandbox_why_no_sandbox_exec', acknowledged: false, missing: true, onChange: noop }));
  assert.match(tried, /<input type="checkbox" aria-invalid="true"/);
  assert.match(tried, /role="alert"[^>]*>Tick the box to create a Bot without the sandbox\./);
  assert.match(tried, /This Mac has no sandbox-exec, which the sandbox needs\./);
  const ticked = render(React.createElement(BotSandboxAcknowledge, { reason: 'bots_sandbox_why_no_sandbox_exec', acknowledged: true, missing: true, onChange: noop }));
  assert.match(ticked, /<input type="checkbox" checked=""/);
  assert.ok(!ticked.includes('Tick the box') && !ticked.includes('aria-invalid'));
  const busy = render(React.createElement(BotSandboxAcknowledge, { reason: 'bots_sandbox_why_platform', acknowledged: false, missing: false, disabled: true, onChange: noop }));
  assert.match(busy, /<fieldset class="bots-fields bots-sandbox" disabled="">/);
});

test('a Bot whose sandbox is on, on a computer that has none, says its chats cannot start and leads to the setting', async () => {
  const bots = await pageBots({ kind: 'detail', botId: 'a' }, [bot({ botId: 'a', name: 'Alpha' })], NO_SANDBOX);
  const html = render(React.createElement(Bots, { visible: true, bots, api: {} as BotsApi, bridge: bridgeStub(), onOpenChat: noop, onNewRoutine: noop, onOpenRoutine: noop }));
  assert.match(html, /role="alert" class="bots-problem">This device can&#x27;t isolate this Bot\. The sandbox is on for this Bot, and this device cannot provide one, so its chats cannot start\. Turn the sandbox off to use the Bot here\.<button type="button">Sandbox settings<\/button>/);

  const off = await pageBots({ kind: 'detail', botId: 'a' }, [bot({ botId: 'a', name: 'Alpha', sandbox: { enabled: false, allowedDomains: [] } })], NO_SANDBOX);
  assert.ok(!render(React.createElement(Bots, { visible: true, bots: off, api: {} as BotsApi, bridge: bridgeStub(), onOpenChat: noop, onNewRoutine: noop, onOpenRoutine: noop })).includes('isolate this Bot'), 'a Bot that has none to give up has nothing to say');

  const ready = await pageBots({ kind: 'detail', botId: 'a' }, [bot({ botId: 'a', name: 'Alpha' })]);
  assert.ok(!render(React.createElement(Bots, { visible: true, bots: ready, api: {} as BotsApi, bridge: bridgeStub(), onOpenChat: noop, onNewRoutine: noop, onOpenRoutine: noop })).includes('isolate this Bot'), 'nor has a computer that has one');
});

test('a request for the Bot\'s sandbox settings opens at the Sandbox tab, which is then selected and drops the way to itself', async () => {
  const bots = await pageBots({ kind: 'detail', botId: 'a', sandboxRequest: 1 }, [bot({ botId: 'a', name: 'Alpha' })], NO_SANDBOX);
  const html = render(React.createElement(Bots, { visible: true, bots, api: {} as BotsApi, bridge: bridgeStub(), onOpenChat: noop, onNewRoutine: noop, onOpenRoutine: noop }));
  assert.match(html, /role="tab" id="[^"]+" aria-selected="true" aria-controls="[^"]+" tabindex="0"><span class="bots-tab-icon"[^>]*>.*?<\/span>Sandbox<\/button>/);
  assert.ok(!html.includes('>Sandbox settings<'), 'the setting is already in view');
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

test('a folder can be given a name, and a Bot that exists offers a chat in its own folder and in each folder it has saved', () => {
  const draft = { ...emptyBotDraft(), workspaces: [
    { path: '/work/notes', mode: 'read' as const, label: 'Notes' }, { path: '/work/site', mode: 'write' as const }, { path: '/work/new', mode: 'read' as const },
  ] };
  const saved = [{ id: 'wNotes', path: '/work/notes', mode: 'read' as const, addedAt: 1, label: 'Notes' }, { id: 'wSite', path: '/work/site', mode: 'write' as const, addedAt: 2 }];
  const html = render(React.createElement(BotFoldersField, { draft, problems: {}, pickFolder: async () => null, saved, onNewChat: noop, onChange: noop }));
  assert.match(html, /<input[^>]*class="bots-folder-name"[^>]*value="Notes"[^>]*>/, 'the name the owner gave');
  assert.match(html, /<input[^>]*placeholder="site"[^>]*aria-label="Name of folder \/work\/site"/, 'without one, the folder\'s own name is what shows through');
  assert.match(html, /<li class="bots-folder-own"><span class="bots-folder-name-text">Own workspace<\/span>/);
  assert.ok(html.includes('aria-label="New chat in Own workspace"'));
  assert.ok(html.includes('aria-label="New chat in Notes"'));
  assert.ok(html.includes('aria-label="New chat in site"'));
  assert.ok(!html.includes('New chat in new'), 'a folder that is not saved yet has no chat: the Bot does not have it');
  // Where the Bot is not made yet there is nothing to start a chat of.
  const unmade = render(React.createElement(BotFoldersField, { draft, problems: {}, pickFolder: async () => null, onChange: noop }));
  assert.ok(!unmade.includes('New chat in') && !unmade.includes('bots-folder-own'));
  // A chat is not started while another is being opened.
  assert.equal([...render(React.createElement(BotFoldersField, { draft, problems: {}, pickFolder: async () => null, saved, chatDisabled: true, onNewChat: noop, onChange: noop })).matchAll(/class="bots-folder-chat"[^>]*disabled=""|disabled=""[^>]*class="bots-folder-chat"/g)].length, 3);
  const tooLong = render(React.createElement(BotFoldersField, { draft, problems: { folders: 'folders_label_too_long' }, pickFolder: async () => null, onChange: noop }));
  assert.match(tooLong, /role="alert"[^>]*>A folder&#x27;s name can have up to 80 characters\./);
});

test('what a Bot asked for is shown with the reason and the access, and can be allowed or denied; with nothing asked nothing is shown', async () => {
  const asked: BotWorkspaceRequestUi[] = [
    { requestId: 'r1', botId: 'b1', botName: 'Scribe', path: '/work/specs', mode: 'write', reason: 'to update the specs it was asked about', createdAt: Date.now() - 5 * MINUTES },
    { requestId: 'r2', botId: 'b2', botName: 'Other', path: '/work/else', mode: 'read', reason: 'another Bot\'s', createdAt: Date.now() },
  ];
  const controller = new BotWorkspaceRequestsController({ list: async () => asked, decide: async () => bot(), onChanged: () => noop } as unknown as BotsApi['workspaceRequests'], noop);
  await controller.load();
  const html = render(React.createElement(BotWorkspaceRequestsPanel, { controller, botId: 'b1', onApproved: noop }));
  assert.match(html, /<h2[^>]*>Folder requests<\/h2>/);
  assert.match(html, /<span class="mono">\/work\/specs<\/span><span class="bots-tag">Read and write<\/span>/);
  assert.ok(html.includes('to update the specs it was asked about'));
  assert.ok(html.includes('Asked 5 minutes ago'));
  assert.ok(html.includes('aria-label="Allow: /work/specs"') && html.includes('aria-label="Deny: /work/specs"'));
  assert.ok(!html.includes('/work/else'), 'only this Bot\'s requests');
  const none = render(React.createElement(BotWorkspaceRequestsPanel, { controller, botId: 'b3', onApproved: noop }));
  assert.equal(none, '');
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
    scheduledScopes: [{ id: '/data/bots/a/workspace', label: 'Alpha', botId: 'a' }],
    manageScheduled: async () => [],
    setBotsKeepAwake: async () => undefined,
    ...extra,
  } as unknown as Pick<UseBridge, 'bootstrap' | 'desktop' | 'sessionLoading' | 'sessionRuntimeStatus' | 'openBotSession' | 'deleteBot' | 'pickBotFolder' | 'scheduledScopes' | 'manageScheduled' | 'setBotsKeepAwake'>;
}

async function pageBots(view: UseBots['view'], bots: BotUi[], availability: BotsAvailability = READY): Promise<UseBots> {
  const api: Pick<BotsApi, 'availability' | 'list'> = { availability: async () => availability, list: async () => bots };
  const controller = new BotsController(api);
  await controller.start();
  return { controller, state: controller.getSnapshot(), available: true, view, setView: noop, guard: new BotLeaveGuard(noop) };
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
  const html = render(React.createElement(Bots, { visible: true, bots, api: {} as BotsApi, bridge: bridgeStub(runtimes, { sessionRuntimeStatus: live }), onOpenChat: noop, onNewRoutine: noop, onOpenRoutine: noop }));
  assert.match(html, /aria-label="Bots"/);
  assert.match(html, /Alpha[\s\S]*?data-state="working">Working</);
  assert.match(html, /Beta[\s\S]*?data-state="waiting">Needs you</);
  assert.match(html, /Gamma[\s\S]*?data-state="idle">Idle</);
});

test('a Bot opens to its general settings, with the routines and tasks listed first, and a chat can be started or the Bot deleted; its chats are in the sidebar', async () => {
  const bots = await pageBots({ kind: 'detail', botId: 'a' }, [bot({ botId: 'a', name: 'Alpha', description: 'First helper' })]);
  const html = render(React.createElement(Bots, { visible: true, bots, api: {} as BotsApi, bridge: bridgeStub(), onOpenChat: noop, onNewRoutine: noop, onOpenRoutine: noop }));
  assert.match(html, /<h1>Alpha<\/h1>/);
  assert.match(html, /<div class="bots-detail-identity"><span class="bots-avatar" aria-hidden="true"><img [^>]*data-agent-avatar="\d+"[^>]*\/><\/span><div class="bots-detail-title"><h1>Alpha<\/h1>/, 'its avatar stands beside the name');
  assert.deepEqual(avatarsOf(html), [agentAvatarIndex('a')], 'the one the list shows for it');
  assert.match(html, /First helper/);
  for (const tab of ['Routines', 'Tasks', 'General', 'Persona', 'Memory', 'Skills', 'Folders', 'Permissions', 'Sandbox', 'Scope']) assert.match(html, new RegExp(`</span>${tab}</button>`), tab);
  assert.ok(html.indexOf('>Routines</button>') < html.indexOf('>Tasks</button>') && html.indexOf('>Tasks</button>') < html.indexOf('>General</button>'), 'the routines and the tasks come before the settings');
  assert.ok(!html.includes('aria-current'), 'the sections are tabs, not links to pages');
  assert.ok(!html.includes('>Chats</button>'), 'the chats of a Bot are listed under it in the sidebar, not here');
  assert.ok(!html.includes('Loading chats'));
  assert.match(html, />New chat</);
  assert.match(html, />Delete Bot</);
  assert.ok(!html.includes('role="dialog"'), 'nothing is asked until deletion is started');
});

test('the sections of a Bot are a tab list: one tab selected and in the tab order, each naming the panel it controls, the panel naming the selected tab', async () => {
  const bots = await pageBots({ kind: 'detail', botId: 'a' }, [bot({ botId: 'a', name: 'Alpha' })]);
  const html = render(React.createElement(Bots, { visible: true, bots, api: {} as BotsApi, bridge: bridgeStub(), onOpenChat: noop, onNewRoutine: noop, onOpenRoutine: noop }));
  const list = /<div class="bots-tabs" role="tablist" aria-labelledby="([^"]+)" aria-orientation="horizontal">/.exec(html);
  assert.ok(list && html.includes(`id="${list[1]}">Bot settings</p>`));
  const tabs = [...html.matchAll(/<button type="button" role="tab" id="([^"]+)" aria-selected="(true|false)" aria-controls="([^"]+)" tabindex="(-?\d+)"[^>]*><span class="bots-tab-icon" aria-hidden="true">.*?<\/span>([^<]+)<\/button>/g)]
    .map((match) => ({ id: match[1]!, selected: match[2], controls: match[3]!, tabindex: match[4], label: match[5] }));
  assert.deepEqual(tabs.map((tab) => tab.label), ['Routines', 'Tasks', 'General', 'Persona', 'Memory', 'Skills', 'Folders', 'Permissions', 'Sandbox', 'Scope', 'Auto-reply', 'Package &amp; Market']);
  assert.deepEqual(tabs.filter((tab) => tab.selected === 'true').map((tab) => tab.label), ['General'], 'one is selected');
  assert.deepEqual(tabs.filter((tab) => tab.tabindex === '0').map((tab) => tab.label), ['General'], 'and it is the one Tab reaches: the arrow keys go to the others');
  assert.ok(tabs.every((tab) => tab.selected === 'true' || tab.tabindex === '-1'));
  assert.equal(new Set(tabs.map((tab) => tab.id)).size, tabs.length, 'each has an id of its own');
  const panel = /<div class="bots-tabpanel" role="tabpanel" id="([^"]+)" aria-labelledby="([^"]+)" tabindex="0">/.exec(html);
  assert.ok(panel, 'the tab list has a panel');
  assert.ok(tabs.every((tab) => tab.controls === panel![1]), 'every tab controls it');
  assert.equal(panel![2], tabs.find((tab) => tab.selected === 'true')!.id, 'and it is labelled by the selected one');
  assert.ok(html.indexOf('role="tablist"') < html.indexOf('role="tabpanel"'));
});

test('a request to delete the Bot comes up as the question whether to, and nothing is deleted until it is answered', async () => {
  const deleted: string[] = [];
  const bridge = bridgeStub([], { deleteBot: async (botId: string) => { deleted.push(botId); } });
  const bots = await pageBots({ kind: 'detail', botId: 'a', deleteRequest: 1 }, [bot({ botId: 'a', name: 'Alpha' })]);
  const html = render(React.createElement(Bots, { visible: true, bots, api: {} as BotsApi, bridge, onOpenChat: noop, onNewRoutine: noop, onOpenRoutine: noop }));
  assert.match(html, /role="dialog"[^>]*aria-label="Delete “Alpha”\?"/);
  assert.match(html, /desktop-dialog-action--primary">Delete Bot</, 'to be confirmed, with the words of the page\'s own button');
  assert.deepEqual(deleted, []);

  const plain = await pageBots({ kind: 'detail', botId: 'a' }, [bot({ botId: 'a', name: 'Alpha' })]);
  assert.ok(!render(React.createElement(Bots, { visible: true, bots: plain, api: {} as BotsApi, bridge, onOpenChat: noop, onNewRoutine: noop, onOpenRoutine: noop })).includes('role="dialog"'));
});

test('a way out of unsaved edits comes up as the question whether to give them up, and goes nowhere until it is answered', async () => {
  const bots = await pageBots({ kind: 'detail', botId: 'a' }, [bot({ botId: 'a', name: 'Alpha' })]);
  const page = () => render(React.createElement(Bots, { visible: true, bots, api: {} as BotsApi, bridge: bridgeStub(), onOpenChat: noop, onNewRoutine: noop, onOpenRoutine: noop }));
  const gone: string[] = [];
  assert.ok(!page().includes('role="dialog"'), 'nothing is asked while no one is leaving');

  bots.guard.leave(() => { gone.push('with nothing to lose'); });
  assert.deepEqual(gone, ['with nothing to lose']);
  assert.ok(!page().includes('role="dialog"'), 'a way out with no edits to lose is simply taken');

  bots.guard.hold(true);
  bots.guard.leave(() => { gone.push('with edits'); });
  const html = page();
  assert.match(html, /role="dialog"[^>]*aria-label="Discard your changes\?"/);
  assert.match(html, /This Bot has changes that are not saved\./);
  assert.match(html, /desktop-dialog-action--primary">Discard changes</, 'to be confirmed, with the words of giving the edits up');
  assert.deepEqual(gone, ['with nothing to lose'], 'the owner has not answered');

  bots.guard.cancel();
  assert.ok(!page().includes('role="dialog"'), 'answered no: the question is gone and the page stays');
  assert.deepEqual(gone, ['with nothing to lose']);
});

test('while another chat is opening the Bot\'s chats cannot be started', async () => {
  const bots = await pageBots({ kind: 'detail', botId: 'a' }, [bot({ botId: 'a', name: 'Alpha' })]);
  const html = render(React.createElement(Bots, { visible: true, bots, api: {} as BotsApi, bridge: bridgeStub([], { sessionLoading: true }), onOpenChat: noop, onNewRoutine: noop, onOpenRoutine: noop }));
  assert.match(html, /<button type="button" data-variant="primary" disabled="">New chat<\/button>/);
});

test('the page shows the form that makes a Bot when asked to', async () => {
  const bots = await pageBots({ kind: 'create' }, []);
  const html = render(React.createElement(Bots, { visible: true, bots, api: {} as BotsApi, bridge: bridgeStub(), onOpenChat: noop, onNewRoutine: noop, onOpenRoutine: noop }));
  assert.match(html, /<h1>New Bot<\/h1>/);
  assert.match(html, />Create Bot</);
});

test('what the Bots need of this Mac to go on working is offered below the Bots once there are some, and not before', () => {
  const background = React.createElement('section', null, 'Background settings');
  const props = { activityOf: () => 'idle' as const, onNew: noop, onOpen: noop, onRetry: noop, background };
  assert.ok(!render(React.createElement(BotList, { state: state(), ...props })).includes('Background settings'), 'no Bots, nothing to keep running');
  assert.ok(!render(React.createElement(BotList, { state: state({ availability: undefined }), ...props })).includes('Background settings'));
  const html = render(React.createElement(BotList, { state: state({ bots: [bot()] }), ...props }));
  assert.ok(html.indexOf('bots-grid') >= 0 && html.indexOf('bots-grid') < html.indexOf('Background settings'), 'under the Bots');
});

test('the page offers keeping the Mac awake and opening at login below the Bots, and the first follows the setting', async () => {
  const bots = await pageBots({ kind: 'list' }, [bot({ botId: 'a', name: 'Alpha' })]);
  const page = (extra: Record<string, unknown> = {}) => render(React.createElement(Bots, { visible: true, bots, api: {} as BotsApi, bridge: bridgeStub([], extra), onOpenChat: noop, onNewRoutine: noop, onOpenRoutine: noop }));
  const keepAwake = (html: string) => /<button[^>]*role="switch"[^>]*aria-label="Keep this Mac awake while Bots are working"[^>]*>/.exec(html)?.[0] ?? '';
  const off = page();
  assert.match(off, /Running in the background/);
  assert.match(off, /Open LingXi in the background when you log in/);
  assert.match(off, /Available in the installed app\./, 'a window with no host to ask is not in the installed app');
  assert.ok(off.indexOf('Alpha') < off.indexOf('Running in the background'));
  assert.match(keepAwake(off), /aria-checked="false"/);
  assert.match(keepAwake(page({ bootstrap: { runtimes: [], settings: { botsKeepAwake: true } } })), /aria-checked="true"/);
  assert.match(keepAwake(page({ bootstrap: { runtimes: [], settings: { botsKeepAwake: false } } })), /aria-checked="false"/);
  assert.match(keepAwake(page({ bootstrap: { runtimes: [], settings: {} } })), /aria-checked="false"/, 'never set is off');
});

test('a Bot\'s own page and the form that makes a Bot do not carry the settings of the list', async () => {
  for (const view of [{ kind: 'detail', botId: 'a' }, { kind: 'create' }] as const) {
    const bots = await pageBots(view, [bot({ botId: 'a', name: 'Alpha' })]);
    const html = render(React.createElement(Bots, { visible: true, bots, api: {} as BotsApi, bridge: bridgeStub(), onOpenChat: noop, onNewRoutine: noop, onOpenRoutine: noop }));
    assert.ok(!html.includes('Running in the background'), view.kind);
  }
});

// ---- Outlines ----

const channelOf = (value: number) => { const c = value / 255; return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4; };
const luminanceOf = (hex: string) => 0.2126 * channelOf(Number.parseInt(hex.slice(1, 3), 16)) + 0.7152 * channelOf(Number.parseInt(hex.slice(3, 5), 16)) + 0.0722 * channelOf(Number.parseInt(hex.slice(5, 7), 16));
const contrastOf = (a: string, b: string) => { const [light, dark] = [luminanceOf(a), luminanceOf(b)].sort((x, y) => y - x) as [number, number]; return (light + 0.05) / (dark + 0.05); };

test('the page outlines its controls at 3:1 against the page in both palettes, with the same derivation Teams uses, and keeps the hairline for dividers', async () => {
  for (const dark of [true, false]) {
    const t = tokens(dark);
    const bots = await pageBots({ kind: 'list' }, [bot({ botId: 'a', name: 'Alpha' })]);
    const element = React.createElement(Bots, { visible: true, bots, api: {} as BotsApi, bridge: bridgeStub(), onOpenChat: noop, onNewRoutine: noop, onOpenRoutine: noop });
    const html = renderToStaticMarkup(React.createElement(Theme.Provider, { value: t }, element));
    const variable = (name: string) => new RegExp(`${name}:([^;"]+)`).exec(html)?.[1];
    const outline = variable('--bots-border')!;
    assert.equal(outline, controlBorder(t), `${dark ? 'dark' : 'light'}: the derivation is the one of --teams-border`);
    assert.match(outline, /^#[0-9a-f]{6}$/, 'a plain color, which contrast can be measured on');
    assert.equal(variable('--bots-line'), t.border, `${dark ? 'dark' : 'light'}: the hairline is what dividers keep`);
    // What the outline sits on: the page, the fill of a field or a card, and a card that is hovered.
    for (const [name, surface] of [['page', t.stageBg], ['surface', t.surface], ['hover', t.surfaceHover]] as const) {
      assert.ok(contrastOf(outline, surface) >= 3, `${dark ? 'dark' : 'light'} outline on the ${name} is ${contrastOf(outline, surface).toFixed(2)}:1`);
    }
  }
});

test('in the stylesheets of the page controls keep the 3:1 outline, and frames, dividers and tags the hairline', () => {
  const css = ['Bots.css', 'BotCreation.css'].map((name) => readFileSync(join(process.cwd(), 'src/renderer/components', name), 'utf8')).join('\n').replace(/\/\*[\s\S]*?\*\//g, '');
  const rules = [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].map((match) => ({ selectors: match[1]!.split(',').map((part) => part.trim()), body: match[2]! }));
  /** The color a rule gives the line it draws, in `border` or `border-top`, from the first rule for the selector that draws one. */
  const lineOf = (selector: string) => rules.filter((rule) => rule.selectors.includes(selector)).map((rule) => /\bborder(?:-top)?:\s*[^;]*var\((--bots-[a-z]+)\)/.exec(rule.body)?.[1]).find((variable) => variable !== undefined);
  for (const control of ['.bots-page button', '.bots-page input', '.bot-create-mode', '.bots-accept-list label']) assert.equal(lineOf(control), '--bots-border', control);
  for (const frame of ['.bots-avatar', '.bots-row', '.bots-tag', '.bots-viewer', '.bots-folder-list li', '.bots-savebar', '.bot-create-intro']) assert.equal(lineOf(frame), '--bots-line', frame);
});

// ---- The window around the page ----

function sidebarBridge(overrides: Record<string, unknown> = {}) {
  return { bootstrap: null, sessionLoading: false, desktop: emptyDesktopState(), activeSession: undefined, activeBotId: undefined, ...overrides } as unknown as UseBridge;
}

const railOf = (html: string) => /<nav[^>]*desktop-nav-rail[\s\S]*?<\/nav>/.exec(html)?.[0] ?? '';
const buttonLabels = (html: string) => [...html.matchAll(/<button[^>]*aria-label="([^"]+)"/g)].map((match) => match[1]);
const markedLabels = (html: string) => buttonLabels([...railOf(html).matchAll(/<button\b[^>]*>/g)]
  .map(([button]) => button)
  .filter(button => button.includes('data-active="true"') || (button.includes('desktop-rail-workspace') && button.includes('aria-current="page"')))
  .join(''));
/** The rail buttons that a screen reader is told are the current page, which are the ones that are drawn marked. */
const currentLabels = (html: string) => [...railOf(html).matchAll(/<button[^>]*aria-current="page"[^>]*aria-label="([^"]+)"/g)].map((match) => match[1]);

test('the sidebar offers Bots only where the host said they can be used, and only once', () => {
  const without = render(React.createElement(BetaSidebar, { bridge: sidebarBridge(), onOpenSettings: noop }));
  assert.ok(!without.includes('aria-label="Bots"'));
  assert.ok(!without.includes('>Bots<'));
  const refused = render(React.createElement(BetaSidebar, { bridge: sidebarBridge(), onOpenSettings: noop, botsAvailable: false }));
  assert.ok(!refused.includes('Bots'));

  const offered = render(React.createElement(BetaSidebar, { bridge: sidebarBridge(), onOpenSettings: noop, botsAvailable: true }));
  assert.match(offered, /class="desktop-rail-button" aria-label="Bots" title="Bots"/);
  assert.ok(!offered.includes('<span>Bots</span>') && !offered.includes('<span>Teams</span>'), 'Home does not repeat the rail\'s buttons');
  assert.ok(!offered.includes('<span>Apps</span>') && !offered.includes('<span>Scheduled</span>'), 'Home does not repeat Apps or Scheduled from the rail');
});

test('the rail has Messages, Projects, Bots, Teams, Friends and Apps, then a divider, then the pages', () => {
  const rail = railOf(render(React.createElement(BetaSidebar, { bridge: sidebarBridge(), onOpenSettings: noop, botsAvailable: true })));
  const divider = rail.indexOf('desktop-rail-divider');
  assert.ok(divider > 0);
  assert.deepEqual(buttonLabels(rail.slice(0, divider)), ['Messages', 'Projects', 'Bots', 'Teams', 'Friends', 'Apps']);
  assert.deepEqual(buttonLabels(rail.slice(divider)), ['Scheduled tasks', 'Review changes', 'Settings']);

  const noBots = railOf(render(React.createElement(BetaSidebar, { bridge: sidebarBridge(), onOpenSettings: noop })));
  assert.deepEqual(buttonLabels(noBots.slice(0, noBots.indexOf('desktop-rail-divider'))), ['Messages', 'Projects', 'Teams', 'Friends', 'Apps']);
});

test('the rail marks the selected workspace or the scheduled tasks page', () => {
  const sidebar = (props: Record<string, unknown> = {}) => render(React.createElement(BetaSidebar, { bridge: sidebarBridge(), onOpenSettings: noop, botsAvailable: true, ...props }));
  assert.deepEqual(markedLabels(sidebar()), ['Messages']);
  assert.deepEqual(markedLabels(sidebar({ sidebarMode: 'home', homeView: 'projects' })), ['Projects']);
  assert.deepEqual(markedLabels(sidebar({ sidebarMode: 'bots' })), ['Bots']);
  assert.deepEqual(markedLabels(sidebar({ sidebarMode: 'teams' })), ['Teams']);
  assert.deepEqual(markedLabels(sidebar({ sidebarMode: 'friends' })), ['Friends']);

  assert.deepEqual(markedLabels(sidebar({ sidebarMode: 'bots', apps: true })), ['Bots', 'Apps']);
  assert.deepEqual(markedLabels(sidebar({ sidebarMode: 'teams', scheduled: true })), ['Scheduled tasks'], 'Cron owns its sidebar, so the mode is not marked behind it');
  assert.deepEqual(markedLabels(sidebar({ sidebarMode: 'home', apps: true })), ['Messages', 'Apps']);
});

test('the rail tells assistive technology which page it is on with aria-current, for exactly the buttons it draws marked', () => {
  const sidebar = (props: Record<string, unknown> = {}) => render(React.createElement(BetaSidebar, { bridge: sidebarBridge(), onOpenSettings: noop, botsAvailable: true, ...props }));
  for (const props of [{}, { sidebarMode: 'home', homeView: 'projects' }, { sidebarMode: 'bots' }, { sidebarMode: 'teams' }, { sidebarMode: 'friends' }, { sidebarMode: 'bots', apps: true }, { sidebarMode: 'teams', scheduled: true }, { sidebarMode: 'friends', changes: true }, { scheduled: true }]) {
    const html = sidebar(props);
    assert.deepEqual(currentLabels(html), markedLabels(html), JSON.stringify(props));
  }
  assert.deepEqual(currentLabels(sidebar({ sidebarMode: 'bots' })), ['Bots']);
  assert.deepEqual(currentLabels(sidebar({ sidebarMode: 'friends', apps: true })), ['Friends', 'Apps']);
  assert.deepEqual(currentLabels(sidebar({ sidebarMode: 'teams', scheduled: true })), ['Scheduled tasks'], 'a page that owns the sidebar is the current one, and the mode behind it is not');
  const bots = railOf(sidebar({ sidebarMode: 'bots' }));
  assert.match(bots, /class="desktop-rail-button" data-active="true" aria-current="page" aria-label="Bots"/, 'the mark that CSS draws from stays with it');
  assert.equal([...bots.matchAll(/aria-current="page"/g)].length, 1, 'one button of the rail is the current page');
});

test('the rail selects the requested workspace and keeps the chat fallback for a standalone sidebar', () => {
  const source = readFileSync(join(process.cwd(), 'src/renderer/components/BetaDesktop.tsx'), 'utf8');
  for (const mode of ['bots', 'teams', 'friends']) assert.match(source, new RegExp(`onClick=\\{\\(\\) => selectMode\\('${mode}'\\)\\}`), mode);
  for (const view of ['chats', 'projects']) assert.match(source, new RegExp(`onClick=\\{\\(\\) => selectHomeView\\('${view}'\\)\\}`), view);
  assert.match(source, /if \(onHomeViewChange\) onHomeViewChange\(view\);\s+else selectMode\('home'\);/);
  assert.match(source, /if \(onSelectSidebarMode\) onSelectSidebarMode\(mode\);\s+else if \(mode === 'home'\) onOpenChat\?\.\(\);/);
});

test('the split sidebar mounts project actions when Projects is opened', () => {
  const seen: unknown[][] = [];
  const modePanel = (mode: string, search: string) => { seen.push([mode, search]); return React.createElement('div', { id: 'mode-panel' }, `panel for ${mode}`); };
  const sidebar = (sidebarMode: 'home' | 'bots' | 'teams' | 'friends', homeView: 'chats' | 'projects' = 'chats') => render(React.createElement(BetaSidebar, {
    bridge: sidebarBridge(), onOpenSettings: noop, botsAvailable: true, sidebarMode, modePanel,
    homePanel: React.createElement('div', { id: 'messages-panel' }), homeView, onHomeViewChange: noop,
  }));

  const messages = sidebar('home');
  assert.match(messages, /id="messages-panel"/);
  assert.doesNotMatch(messages, /mode-panel|sidebar-new-chat/);
  const projects = sidebar('home', 'projects');
  assert.deepEqual(seen, []);
  assert.match(projects, /sidebar-new-chat/);
  assert.match(projects, new RegExp(`aria-label="${tr('desktop_projects_sessions')}"`));

  for (const mode of ['bots', 'teams', 'friends'] as const) {
    const html = sidebar(mode);
    assert.match(html, new RegExp(`<div id="mode-panel">panel for ${mode}</div>`));
    assert.ok(!html.includes('sidebar-new-chat'), mode);
    assert.ok(!html.includes(`aria-label="${tr('desktop_projects_sessions')}"`), mode);
  }
  assert.deepEqual(seen, [['bots', ''], ['teams', ''], ['friends', '']], 'told the mode, and what the search box holds');
});

test('the sidebar is hidden when the parent says so, and keeps its own state when the parent says nothing', () => {
  const sidebar = (props: Record<string, unknown> = {}) => render(React.createElement(BetaSidebar, { bridge: sidebarBridge(), onOpenSettings: noop, ...props }));
  assert.ok(!sidebar().includes('data-collapsed'));
  assert.ok(!sidebar({ sidebarCollapsed: false }).includes('data-collapsed'));
  const hidden = sidebar({ sidebarCollapsed: true });
  assert.match(hidden, /class="desktop-navigation" data-collapsed="true"/);
  assert.match(hidden, /aria-label="Show sidebar"[^>]*aria-expanded="false"/);
  assert.match(sidebar({ sidebarCollapsed: false }), /aria-label="Hide sidebar"[^>]*aria-expanded="true"/);
});

test('a Bot\'s chat is titled with the Bot, not with a folder path, and offers the way back to the Bot', () => {
  const bridge = { bootstrap: null, sessionLoading: false, desktop: emptyDesktopState(), runtimeCenter: emptyRuntimeCenterState(), activeSession: { projectPath: '/data/bots/a/workspace', sessionId: 's1' } } as unknown as UseBridge;
  const html = render(React.createElement(BetaTopBar, { bridge, runtimeCenterOpen: false, onToggleRuntimeCenter: noop, bot: { botId: 'scribe-id', name: 'Scribe', onOpen: noop } }));
  assert.match(html, /class="desktop-topbar-title"[^>]*>Scribe<\/span>/);
  assert.match(html, /aria-label="Open Bot"/);
  assert.ok(!html.includes('/data/bots'), 'the Bot\'s private folder is not shown');
  assert.deepEqual(avatarsOf(html), [agentAvatarIndex('scribe-id')], 'the title wears the Bot\'s own avatar, not a generic robot');
  assert.match(html, /aria-label="Open Bot"[^>]*><img [^>]*aria-hidden="true"/, 'the avatar is the Open Bot button\'s face; the button carries the label');

  const projectBridge = { ...bridge, activeSession: { ...bridge.activeSession!, projectId: '/data/bots/a/workspace', mode: 'code' as const } };
  const project = render(React.createElement(BetaTopBar, { bridge: projectBridge, runtimeCenterOpen: false, onToggleRuntimeCenter: noop }));
  assert.ok(!project.includes('Open Bot'));
  assert.deepEqual(avatarsOf(project), [], 'a project\'s chat has its folder, no avatar');
  assert.match(project, /\/data\/bots\/a\/workspace/, 'a project chat still shows its folder');
});
