import assert from 'node:assert/strict';
import test from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

import type { BotReachUi, BotScopeUi, BotUi, BotsApi } from '../src/shared/bots';
import {
  BOT_SCOPE_MAX, botChanges, botCreateRequest, botDraftProblems, draftFromBot, emptyBotDraft, saveBot, sameScope, type BotDraft,
} from '../src/renderer/bridge/botForm';
import {
  BotScopeChoicesController, desktopScopeSources, scopeView, withApp, withDataset, withDatasetAccess,
  type BotScopeChoices, type BotScopeSources, type ScopeSource,
} from '../src/renderer/bridge/botScope';
import { problemText } from '../src/renderer/components/BotFields';
import { BotScopeField } from '../src/renderer/components/BotScopeField';
import { BotReachView } from '../src/renderer/components/BotReachList';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import { pinEnglish } from './bot-ui-helpers';

(globalThis as { React?: typeof React }).React = React;
pinEnglish();

const noop = () => undefined;

function bot(overrides: Partial<BotUi> = {}): BotUi {
  return { botId: 'b1', name: 'Scribe', description: '', createdAt: 1_000, updatedAt: 1_000, revision: 3, workspaces: [], sandbox: { enabled: true, allowedDomains: [] }, scope: { localApps: [], datasets: [] }, ...overrides };
}

const ORDERS = { source: 'orders-db', dataset: 'orders', write: false, reportReview: false };
const REFUNDS = { source: 'orders-db', dataset: 'refunds', write: true, reportReview: false };

function render(element: React.ReactElement): string {
  return renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, element));
}

// ---- The draft and what saving it sends ----

test('a new Bot starts with nothing in its scope and is made without one; a saved Bot\'s draft is its own copy of the scope', () => {
  assert.deepEqual(emptyBotDraft().scope, { localApps: [], datasets: [] });
  assert.ok(!('scope' in botCreateRequest({ ...emptyBotDraft(), name: 'New', scope: { localApps: ['sales'], datasets: [ORDERS] } })), 'the form that makes a Bot has no Scope tab');
  const saved = bot({ scope: { localApps: ['sales'], datasets: [ORDERS] } });
  const draft = draftFromBot(saved, '');
  assert.deepEqual(draft.scope, saved.scope);
  draft.scope.localApps.push('notes');
  draft.scope.datasets[0]!.write = true;
  assert.deepEqual(saved.scope, { localApps: ['sales'], datasets: [ORDERS] }, 'changing the draft does not change the Bot it came from');
});

test('what is in the scope is a change, in whatever order; going back to what was saved is not', () => {
  const saved = bot({ scope: { localApps: ['sales', 'notes'], datasets: [ORDERS, REFUNDS] } });
  const draft = (scope: BotScopeUi): BotDraft => ({ ...draftFromBot(saved, ''), scope });
  assert.deepEqual(botChanges(saved, '', draftFromBot(saved, '')).patch, {});
  assert.deepEqual(botChanges(saved, '', draft({ localApps: ['notes', 'sales'], datasets: [REFUNDS, ORDERS] })).patch, {}, 'the same entries in another order');
  assert.deepEqual(botChanges(saved, '', draft({ localApps: ['sales'], datasets: [ORDERS, REFUNDS] })).patch, { scope: { localApps: ['sales'], datasets: [ORDERS, REFUNDS] } });
  assert.deepEqual(botChanges(saved, '', draft({ localApps: ['sales', 'notes'], datasets: [ORDERS, { ...REFUNDS, write: false }] })).patch,
    { scope: { localApps: ['sales', 'notes'], datasets: [ORDERS, { ...REFUNDS, write: false }] } }, 'what is allowed in a dataset is part of the scope');
  assert.deepEqual(botChanges(saved, '', draft({ localApps: [], datasets: [] })).patch, { scope: { localApps: [], datasets: [] } }, 'an emptied scope is sent: the Bot may reach nothing');
  assert.deepEqual(botChanges(bot(), '', draftFromBot(bot(), '')).patch, {}, 'none saved, none chosen');
});

test('only the four things of a dataset are sent, and a scope with the same entries twice is not the same as one without', () => {
  const saved = bot({ scope: { localApps: [], datasets: [ORDERS] } });
  const extra = { ...ORDERS, dataset: 'extra', label: 'left over from somewhere' } as typeof ORDERS;
  const { patch } = botChanges(saved, '', { ...draftFromBot(saved, ''), scope: { localApps: [], datasets: [ORDERS, extra] } });
  assert.deepEqual(patch, { scope: { localApps: [], datasets: [ORDERS, { source: 'orders-db', dataset: 'extra', write: false, reportReview: false }] } });
  assert.ok(!sameScope({ localApps: [], datasets: [ORDERS, ORDERS] }, { localApps: [], datasets: [ORDERS, REFUNDS] }));
  assert.ok(!sameScope({ localApps: [], datasets: [ORDERS, REFUNDS] }, { localApps: [], datasets: [ORDERS, ORDERS] }));
  assert.ok(sameScope({ localApps: ['a'], datasets: [] }, { localApps: ['a'], datasets: [] }));
  assert.ok(!sameScope({ localApps: ['a'], datasets: [] }, { localApps: ['b'], datasets: [] }));
});

test('too many Local Apps or too many datasets is a problem; the most there may be is not', () => {
  const apps = (count: number) => Array.from({ length: count }, (_, n) => `app-${n}`);
  const datasets = (count: number) => Array.from({ length: count }, (_, n) => ({ source: 's', dataset: `d${n}`, write: false, reportReview: false }));
  const draft = (scope: BotScopeUi): BotDraft => ({ ...emptyBotDraft(), name: 'Me', scope });
  assert.equal(BOT_SCOPE_MAX, 64);
  assert.deepEqual(botDraftProblems(draft({ localApps: apps(64), datasets: datasets(64) })), {});
  assert.deepEqual(botDraftProblems(draft({ localApps: apps(65), datasets: [] })), { scope: 'scope_too_many' });
  assert.deepEqual(botDraftProblems(draft({ localApps: [], datasets: datasets(65) })), { scope: 'scope_too_many' });
  assert.equal(problemText('scope_too_many'), 'At most 64 Local Apps and 64 datasets can be chosen.');
});

test('saving sends the scope with the revision the Bot was read at, and a scope that is too big is not sent', async () => {
  const updates: unknown[] = [];
  const api = {
    async update(botId: string, revision: number, patch: Record<string, unknown>) { updates.push([botId, revision, patch]); return bot({ revision: 4 }); },
    persona: { async set() { /* unused */ } },
  } as unknown as Pick<BotsApi, 'update' | 'persona'>;
  const saved = bot();
  const scope: BotScopeUi = { localApps: ['sales'], datasets: [ORDERS] };
  assert.equal((await saveBot(api, saved, '', { ...draftFromBot(saved, ''), scope })).ok, true);
  assert.deepEqual(updates, [['b1', 3, { scope }]]);
  const tooMany = { localApps: Array.from({ length: 65 }, (_, n) => `app-${n}`), datasets: [] };
  assert.equal((await saveBot(api, saved, '', { ...draftFromBot(saved, ''), scope: tooMany })).ok, false);
  assert.equal(updates.length, 1, 'nothing was sent');
});

// ---- The changes the page makes to the scope ----

test('an app is added once and taken away, and nothing else of the scope moves', () => {
  const scope: BotScopeUi = { localApps: ['sales'], datasets: [ORDERS] };
  assert.deepEqual(withApp(scope, 'notes', true), { localApps: ['sales', 'notes'], datasets: [ORDERS] });
  assert.equal(withApp(scope, 'sales', true), scope, 'already there: no change');
  assert.deepEqual(withApp(scope, 'sales', false), { localApps: [], datasets: [ORDERS] });
  assert.deepEqual(withApp(scope, 'other', false), scope);
  assert.deepEqual(scope, { localApps: ['sales'], datasets: [ORDERS] }, 'the scope it was given is not changed');
});

test('a dataset is given read-only, and the same name in another source is another dataset', () => {
  const scope: BotScopeUi = { localApps: [], datasets: [REFUNDS] };
  const added = withDataset(scope, 'orders-db', 'orders', true);
  assert.deepEqual(added.datasets, [REFUNDS, ORDERS], 'nothing is allowed beyond reading until the owner says so');
  assert.equal(withDataset(added, 'orders-db', 'orders', true), added, 'already there: no change, and what was allowed in it is kept');
  assert.deepEqual(withDataset(scope, 'crm', 'refunds', true).datasets, [REFUNDS, { source: 'crm', dataset: 'refunds', write: false, reportReview: false }]);
  assert.deepEqual(withDataset(added, 'orders-db', 'refunds', false).datasets, [ORDERS]);
  assert.deepEqual(withDataset({ localApps: [], datasets: [REFUNDS, { ...REFUNDS, source: 'crm' }] }, 'crm', 'refunds', false).datasets, [REFUNDS], 'only the one of that source');
});

test('opening review cases is writing: allowing the one allows the other, and stopping the writing stops the cases', () => {
  const scope: BotScopeUi = { localApps: [], datasets: [ORDERS, { ...REFUNDS, source: 'crm' }] };
  const reviewing = withDatasetAccess(scope, 'orders-db', 'orders', { reportReview: true });
  assert.deepEqual(reviewing.datasets[0], { ...ORDERS, write: true, reportReview: true });
  assert.deepEqual(reviewing.datasets[1], scope.datasets[1], 'the other dataset is as it was');
  const stopped = withDatasetAccess(reviewing, 'orders-db', 'orders', { write: false });
  assert.deepEqual(stopped.datasets[0], ORDERS);
  const writing = withDatasetAccess(reviewing, 'orders-db', 'orders', { reportReview: false });
  assert.deepEqual(writing.datasets[0], { ...ORDERS, write: true, reportReview: false }, 'cases can be stopped and the writing kept');
  assert.deepEqual(withDatasetAccess(scope, 'orders-db', 'nothing-there', { write: true }), scope);
});

// ---- What the page shows ----

const ready = (overrides: Partial<BotScopeChoices> = {}): BotScopeChoices => ({
  apps: [{ id: 'sales', name: 'Sales Board' }, { id: 'notes', name: 'Notes' }],
  appsError: undefined,
  data: 'ready',
  sources: [{ source: 'orders-db', name: 'Orders', kind: 'postgres', datasets: [{ dataset: 'orders', access: 'edit' }, { dataset: 'refunds', access: 'read' }] }],
  ...overrides,
});

test('the installed apps are marked with the ones the Bot was given, and the given ones that are not installed are told apart from the ones that cannot be told', () => {
  const scope: BotScopeUi = { localApps: ['notes', 'ghost'], datasets: [] };
  const view = scopeView(ready(), scope);
  assert.deepEqual(view.apps, [{ id: 'sales', name: 'Sales Board', chosen: false }, { id: 'notes', name: 'Notes', chosen: true }]);
  assert.deepEqual(view.otherApps, [{ id: 'ghost', status: 'missing' }], 'it is not dropped: the owner removes it');
  const unknown = scopeView(ready({ apps: undefined, appsError: 'unavailable' }), scope);
  assert.deepEqual(unknown.apps, []);
  assert.deepEqual(unknown.otherApps, [{ id: 'notes', status: 'unverified' }, { id: 'ghost', status: 'unverified' }], 'with the list not read nothing is said to be missing');
});

test('the datasets are grouped by source with the Bot\'s choices marked; what the Bot has that the source lacks is gone, and a source that is gone has its datasets listed apart', () => {
  const gone = { source: 'orders-db', dataset: 'archived', write: true, reportReview: false };
  const orphan = { source: 'closed-db', dataset: 'ledger', write: false, reportReview: false };
  const view = scopeView(ready(), { localApps: [], datasets: [REFUNDS, gone, orphan] });
  assert.equal(view.groups.length, 1);
  assert.deepEqual(view.groups[0]!.datasets, [
    { dataset: 'orders', access: 'edit', chosen: false, write: false, reportReview: false, gone: false },
    { dataset: 'refunds', access: 'read', chosen: true, write: true, reportReview: false, gone: false },
    { dataset: 'archived', access: undefined, chosen: true, write: true, reportReview: false, gone: true },
  ]);
  assert.deepEqual(view.leftover, [{ ...orphan, status: 'missing' }]);
});

test('a dataset of the same name in two sources is two datasets', () => {
  const sources: ScopeSource[] = [
    { source: 'a', name: 'A', kind: 'csv', datasets: [{ dataset: 'sheet', access: 'edit' }] },
    { source: 'b', name: 'B', kind: 'csv', datasets: [{ dataset: 'sheet', access: 'edit' }] },
  ];
  const view = scopeView(ready({ sources }), { localApps: [], datasets: [{ source: 'b', dataset: 'sheet', write: false, reportReview: false }] });
  assert.deepEqual(view.groups.map((group) => group.datasets.map((row) => row.chosen)), [[false], [true]]);
});

test('a source that could not be read lists only what the Bot has, none of it gone; sources that could not be read at all say nothing is missing', () => {
  const unreadable: ScopeSource = { source: 'orders-db', name: 'Orders', kind: 'postgres', datasets: undefined };
  const view = scopeView(ready({ sources: [unreadable] }), { localApps: [], datasets: [ORDERS] });
  assert.equal(view.groups[0]!.readable, false);
  assert.deepEqual(view.groups[0]!.datasets, [{ dataset: 'orders', access: undefined, chosen: true, write: false, reportReview: false, gone: false }]);
  for (const data of ['idle', 'loading', 'unavailable'] as const) {
    const unknown = scopeView(ready({ data, sources: [] }), { localApps: [], datasets: [ORDERS] });
    assert.deepEqual(unknown.groups, [], data);
    assert.deepEqual(unknown.leftover, [{ ...ORDERS, status: 'unverified' }], data);
  }
});

// ---- Reading what the owner has ----

function fakeSources(overrides: Partial<{ apps: () => Promise<Array<{ id: string; name: string }>>; list: () => Promise<never[]>; datasets: (scope: unknown) => Promise<unknown> }> = {}) {
  const asked: string[] = [];
  const personal = { kind: 'personal' } as const;
  const team = { kind: 'team', teamId: 't1', conversationId: 'c1', generation: 1 } as const;
  const rows = [
    { source: 'orders-db', scope: personal, name: 'Orders', kind: 'postgres', enabled: true, version: 1, requesters: [], grants: [] },
    { source: 'notes-csv', scope: personal, name: 'Notes', kind: 'csv', enabled: true, version: 1, requesters: [], grants: [] },
    { source: 'team-db', scope: team, name: 'Team', kind: 'mysql', enabled: true, version: 1, requesters: [], grants: [] },
  ];
  const sources: BotScopeSources = {
    localApps: overrides.apps ?? (async () => [{ id: 'sales', name: 'Sales Board' }]),
    data: {
      sources: (overrides.list ?? (async () => rows)) as never,
      datasets: (overrides.datasets ?? (async (scope: { kind: string }) => { asked.push(scope.kind); return scope.kind === 'personal' ? [{ dataset: 'orders', access: 'edit' }] : [{ dataset: 'shared', access: 'read' }]; })) as never,
    },
  };
  return { sources, asked };
}

test('opening the tab reads the apps and the sources, and what a scope has is read once however many sources are in it', async () => {
  const { sources, asked } = fakeSources();
  const controller = new BotScopeChoicesController(() => sources);
  assert.equal(controller.getSnapshot().data, 'idle');
  const loading = controller.load();
  assert.equal(controller.getSnapshot().data, 'loading');
  await loading;
  const state = controller.getSnapshot();
  assert.deepEqual(state.apps, [{ id: 'sales', name: 'Sales Board' }]);
  assert.equal(state.data, 'ready');
  assert.deepEqual(state.sources.map((source) => [source.source, source.datasets?.map((set) => set.dataset)]), [['orders-db', ['orders']], ['notes-csv', ['orders']], ['team-db', ['shared']]]);
  assert.deepEqual(asked.sort(), ['personal', 'team'], 'two sources of the personal scope, one read');
});

test('a scope that cannot be read leaves its sources unreadable and the rest as they are; a list of sources that cannot be read is not a list', async () => {
  const partly = fakeSources({ datasets: async (scope) => { if ((scope as { kind: string }).kind === 'team') throw new Error('offline'); return [{ dataset: 'orders', access: 'edit' }]; } });
  const controller = new BotScopeChoicesController(() => partly.sources);
  await controller.load();
  assert.equal(controller.getSnapshot().data, 'ready');
  assert.deepEqual(controller.getSnapshot().sources.map((source) => source.datasets === undefined), [false, false, true]);

  const failing = fakeSources({ list: async () => { throw new Error('no service'); } });
  const broken = new BotScopeChoicesController(() => failing.sources);
  await broken.load();
  assert.equal(broken.getSnapshot().data, 'unavailable');
  assert.deepEqual(broken.getSnapshot().sources, []);
  assert.deepEqual(broken.getSnapshot().apps, [{ id: 'sales', name: 'Sales Board' }], 'the apps do not depend on it');
});

test('without an account there are no sources to read, and apps that cannot be read are unknown, not none', async () => {
  const withoutAccount = new BotScopeChoicesController(() => ({ localApps: async () => [{ id: 'sales', name: 'Sales Board' }] }));
  await withoutAccount.load();
  assert.equal(withoutAccount.getSnapshot().data, 'unavailable');

  const noApps = new BotScopeChoicesController(() => ({ localApps: async () => { throw new Error('Error invoking remote method: host is down'); } }));
  await noApps.load();
  assert.equal(noApps.getSnapshot().apps, undefined);
  assert.ok(noApps.getSnapshot().appsError, 'it says it failed');
});

test('a read that is overtaken by a newer one does not overwrite it', async () => {
  let release!: (apps: Array<{ id: string; name: string }>) => void;
  const slow = new Promise<Array<{ id: string; name: string }>>((resolve) => { release = resolve; });
  let calls = 0;
  const controller = new BotScopeChoicesController(() => ({ localApps: () => (++calls === 1 ? slow : Promise.resolve([{ id: 'new', name: 'New' }])) }));
  const first = controller.load();
  await controller.load();
  release([{ id: 'old', name: 'Old' }]);
  await first;
  assert.deepEqual(controller.getSnapshot().apps, [{ id: 'new', name: 'New' }]);
});

test('the desktop sources are the Local App host and the data service of the signed-in account', async () => {
  const calls: unknown[] = [];
  const host = {
    localApps: { list: async () => [{ id: 'sales', name: 'Sales Board', summary: 'x', revision: 2 }] },
    teams: {
      localDataSource: async (uid: string, request: unknown) => { calls.push(['sources', uid, request]); return [{ source: 'orders-db', scope: { kind: 'personal' }, name: 'Orders', kind: 'postgres' }]; },
      data: async (uid: string, request: unknown) => { calls.push(['data', uid, request]); return { datasets: [{ dataset: 'orders', access: 'edit' }] }; },
    },
  };
  const sources = desktopScopeSources(host as never, 'u1');
  assert.deepEqual(await sources.localApps(), [{ id: 'sales', name: 'Sales Board' }], 'an id and a name; nothing else leaves the page');
  assert.equal((await sources.data!.sources()).length, 1);
  assert.deepEqual(await sources.data!.datasets({ kind: 'personal' }), [{ dataset: 'orders', access: 'edit' }]);
  assert.deepEqual(calls, [['sources', 'u1', { action: 'list' }], ['data', 'u1', { scope: { kind: 'personal' }, command: { action: 'datasets' } }]]);

  assert.equal(desktopScopeSources(host as never, undefined).data, undefined, 'no account, no data');
  assert.equal(desktopScopeSources({ localApps: host.localApps } as never, 'u1').data, undefined, 'no data service on this host');
  await assert.rejects(() => desktopScopeSources(undefined, 'u1').localApps(), /unavailable/);
  const odd = desktopScopeSources({ ...host, teams: { ...host.teams, localDataSource: async () => ({ running: true }) } } as never, 'u1');
  await assert.rejects(() => odd.data!.sources(), /could not be read/, 'an answer that is not a list is no list');
});

// ---- The page ----

function field(scope: BotScopeUi, choices: BotScopeChoices, extra: { disabled?: boolean; problems?: Record<string, string> } = {}): string {
  const draft: BotDraft = { ...emptyBotDraft(), name: 'Scribe', scope };
  return render(React.createElement(BotScopeField, { draft, problems: (extra.problems ?? {}) as never, choices, disabled: extra.disabled, onChange: noop }));
}

/** Every switch of the markup, in order. */
function switches(html: string): Array<{ label: string; on: boolean; disabled: boolean }> {
  return [...html.matchAll(/<button type="button" role="switch" class="settings-toggle"( disabled="")? aria-checked="(true|false)" aria-label="([^"]*)"/g)]
    .map((match) => ({ label: match[3]!, on: match[2] === 'true', disabled: match[1] !== undefined }));
}

test('the Scope tab lists the installed apps with their first letter and a switch each, on for the ones the Bot was given', () => {
  const html = field({ localApps: ['notes'], datasets: [] }, ready());
  assert.match(html, /The most this Bot can use of your Local Apps and datasets\./);
  assert.match(html, /<legend class="bots-label">Local Apps<\/legend>/);
  assert.match(html, /<span class="bots-scope-avatar" aria-hidden="true">S<\/span><span class="bots-row-title">Sales Board<\/span>/);
  assert.match(html, /<span class="bots-scope-avatar" aria-hidden="true">N<\/span><span class="bots-row-title">Notes<\/span>/);
  assert.deepEqual(switches(html).filter((entry) => ['Sales Board', 'Notes'].includes(entry.label)).map((entry) => [entry.label, entry.on]), [['Sales Board', false], ['Notes', true]]);
});

test('an app the Bot was given that is not installed is greyed with a way to remove it, and is not a switch', () => {
  const html = field({ localApps: ['ghost-app'], datasets: [] }, ready());
  assert.match(html, /<li class="bots-row" data-gone=""><div class="bots-row-main"><span class="bots-row-title mono">ghost-app<\/span><span class="bots-row-text">Not installed on this computer\.<\/span><\/div>/);
  assert.match(html, /aria-label="Remove ghost-app"/);
  assert.ok(!switches(html).some((entry) => entry.label === 'ghost-app'));
});

test('while the apps cannot be told, the Bot\'s apps are shown as they are, not as missing, and the page says the list has not loaded', () => {
  const html = field({ localApps: ['sales'], datasets: [] }, ready({ apps: undefined }));
  assert.match(html, /The list of Local Apps has not loaded yet\./);
  assert.match(html, /<li class="bots-row"><div class="bots-row-main"><span class="bots-row-title mono">sales<\/span><span class="bots-row-text">Could not check whether it is installed\.<\/span>/);
  assert.ok(!html.includes('data-gone'), 'nothing is greyed on a guess');
  assert.match(field({ localApps: [], datasets: [] }, ready({ apps: [] })), /No Local Apps are installed on this computer\./);
});

test('the datasets are grouped by their source, the page says plainly that the Bot cannot reach them by itself yet, and a dataset taken has what it may do beside it', () => {
  const html = field({ localApps: [], datasets: [{ ...ORDERS, write: true }] }, ready());
  assert.match(html, /<legend class="bots-label">Datasets<\/legend>/);
  assert.match(html, /On this computer the Bot cannot reach data sources by itself yet\. For now this only records what it may be given\./);
  assert.match(html, /<h3 class="bots-scope-source"><span>Orders<\/span><span class="bots-tag">PostgreSQL<\/span><\/h3>/);
  assert.deepEqual(switches(html).filter((entry) => !['Sales Board', 'Notes'].includes(entry.label)).map((entry) => [entry.label, entry.on, entry.disabled]), [
    ['May write: orders', true, false],
    ['May open review cases: orders', false, false],
    ['orders', true, false],
    ['refunds', false, false],
  ], 'only the dataset that was taken has the two things beside it');
});

test('what the owner can only read cannot be written to or opened for review through the Bot, unless it already is', () => {
  const html = field({ localApps: [], datasets: [{ ...REFUNDS, write: false }] }, ready());
  assert.match(html, /You can only read this dataset\./);
  const refunds = switches(html).filter((entry) => entry.label.endsWith(': refunds'));
  assert.deepEqual(refunds.map((entry) => [entry.label, entry.on, entry.disabled]), [['May write: refunds', false, true], ['May open review cases: refunds', false, true]]);
  const already = switches(field({ localApps: [], datasets: [{ ...REFUNDS, write: true, reportReview: true }] }, ready())).filter((entry) => entry.label.endsWith(': refunds'));
  assert.deepEqual(already.map((entry) => entry.disabled), [false, false], 'what is allowed already can be taken back');
});

test('a dataset the source no longer has is greyed with a way to remove it; one whose source is gone is listed apart', () => {
  const html = field({ localApps: [], datasets: [{ source: 'orders-db', dataset: 'archived', write: false, reportReview: false }, { source: 'closed-db', dataset: 'ledger', write: false, reportReview: false }] }, ready());
  assert.match(html, /<li class="bots-row" data-gone=""><div class="bots-row-main"><span class="bots-row-title">archived<\/span><span class="bots-row-text">No longer available\.<\/span>/);
  assert.match(html, /aria-label="Remove archived"/);
  assert.match(html, /<h3 class="bots-scope-source">Other datasets<\/h3>/);
  assert.match(html, /<span class="bots-row-title">closed-db \/ ledger<\/span><span class="bots-row-text">Its data source is gone\.<\/span>/);
  assert.match(html, /aria-label="Remove closed-db \/ ledger"/);
  assert.ok(!switches(html).some((entry) => entry.label.includes('archived') || entry.label.includes('ledger')), 'what is gone has no switches');
});

test('the page says when the sources are loading, cannot be read, are none, are unreadable or are empty, and does not call anything gone on those grounds', () => {
  const given = { localApps: [], datasets: [ORDERS] } as BotScopeUi;
  assert.match(field(given, ready({ data: 'loading', sources: [] })), /Your data sources are loading\./);
  assert.match(field(given, ready({ data: 'idle', sources: [] })), /Your data sources are loading\./);
  const unavailable = field(given, ready({ data: 'unavailable', sources: [] }));
  assert.match(unavailable, /Your data sources could not be read\. What you chose for this Bot is shown as it is\./);
  assert.match(unavailable, /<span class="bots-row-title">orders-db \/ orders<\/span><span class="bots-row-text">Could not check whether it still exists\.<\/span>/);
  assert.ok(!unavailable.includes('data-gone'));
  assert.deepEqual(switches(unavailable).filter((entry) => entry.label === 'orders-db / orders').map((entry) => entry.on), [true], 'it can still be switched off');
  assert.match(field({ localApps: [], datasets: [] }, ready({ sources: [] })), /You have no data sources yet\./);
  assert.match(field({ localApps: [], datasets: [] }, ready({ sources: [{ source: 'orders-db', name: 'Orders', kind: 'postgres', datasets: undefined }] })), /The datasets of this source could not be read\./);
  assert.match(field({ localApps: [], datasets: [] }, ready({ sources: [{ source: 'orders-db', name: 'Orders', kind: 'postgres', datasets: [] }] })), /This source has no datasets you can use\./);
});

test('too many entries is said as an alert, and while the Bot is saving every control is off', () => {
  assert.match(field({ localApps: [], datasets: [] }, ready(), { problems: { scope: 'scope_too_many' } }), /<p class="bots-problem" role="alert">At most 64 Local Apps and 64 datasets can be chosen\.<\/p>/);
  const saving = field({ localApps: ['sales', 'ghost'], datasets: [ORDERS] }, ready(), { disabled: true });
  assert.equal(saving.match(/<fieldset class="bots-fields bots-scope-section" disabled="">/g)?.length, 2, 'the apps and the datasets');
  assert.ok(switches(saving).every((entry) => entry.disabled));
  assert.ok(!field({ localApps: [], datasets: [] }, ready()).includes('disabled=""'));
});

test('the page of an app or a source names the Bots that reach it, and says when there are none', () => {
  const bots: BotReachUi[] = [{ botId: 'a', name: 'Alpha' }, { botId: 'b', name: 'Beta' }];
  const app = render(React.createElement(BotReachView, { target: { kind: 'localApp', appId: 'sales' }, bots }));
  assert.match(app, /<h3 class="bots-reach-title">Bots that can use this app<\/h3><ul class="bots-reach-list"><li>Alpha<\/li><li>Beta<\/li><\/ul>/);
  const none = render(React.createElement(BotReachView, { target: { kind: 'dataSource', source: 'orders-db' }, bots: [] }));
  assert.match(none, /<h3 class="bots-reach-title">Bots that can reach this data source<\/h3><p class="bots-reach-none">No Bot has this in its scope\.<\/p>/);
});
