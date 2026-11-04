import assert from 'node:assert/strict';
import test from 'node:test';
import { registerHooks } from 'node:module';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

import type { CronJobDto, CronRequestDto } from '@lingxi/bridge-client';
import type { ScheduledScope } from '../src/shared/scheduled';
import { BotRoutinesController, botScopesKey, routineRow, routineScopeOf } from '../src/renderer/bridge/botRoutines';
import { jobStatus, nextRunLabel, scopeName } from '../src/renderer/bridge/scheduledPresentation';
import { tr } from '../src/renderer/i18n/locale';
import { BotRoutinesPanel } from '../src/renderer/components/BotPanels';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import { pinEnglish } from './bot-ui-helpers';

// Match the existing render tests under the root tsconfig's classic JSX loader.
(globalThis as { React?: typeof React }).React = React;
pinEnglish();

// The editor imports its stylesheet, which the test loader cannot read; as in the other render tests, it gets an empty one.
const cssHook = registerHooks({ load(url, context, nextLoad) {
  return url.endsWith('.css') ? { format: 'module', source: 'export {};', shortCircuit: true } : nextLoad(url, context);
} });
const { ScheduledTaskSetup } = await import('../src/renderer/components/ScheduledTaskSetup');
cssHook.deregister();

const noop = () => undefined;
/** Resolve on the next turn of the event loop, after every pending promise has settled. */
const settle = () => new Promise<void>((resolve) => setImmediate(resolve));

function render(element: React.ReactElement): string {
  return renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, element));
}

function job(patch: Partial<CronJobDto> & { status?: 'active' | 'paused' | 'completed'; statusReason?: string } = {}): CronJobDto {
  const { status, statusReason, ...rest } = patch;
  return {
    id: 'task-1', cron: '0 9 * * *', prompt: '# Morning brief\n\nSummarize the inbox.', recurring: true, durable: true, permanent: false, created_at: 1,
    automation: { version: 2, status: status ?? 'active', ...(statusReason ? { statusReason } : {}), model: 'a/b', reasoning: { type: 'automatic' }, runMode: 'new_session', notificationPolicy: 'all' },
    ...rest,
  };
}

const BOT_SCOPE: ScheduledScope = { id: '/data/bots/b1/workspace', label: 'Scribe', botId: 'b1' };
const SCOPES: ScheduledScope[] = [
  { id: 'global', label: 'No project' },
  { id: '/work/app', projectPath: '/work/app', label: 'app' },
  BOT_SCOPE,
];

// ---- Which scope is a Bot's ----

test('a Bot\'s routines are in the scope the scheduler lists for it, and in no other', () => {
  assert.equal(routineScopeOf(SCOPES, 'b1'), BOT_SCOPE.id);
  assert.equal(routineScopeOf(SCOPES, 'b2'), undefined, 'a Bot the scheduler does not list has no scope to read');
  assert.equal(routineScopeOf([], 'b1'), undefined);
  assert.equal(routineScopeOf([{ id: '/work/app' }], 'b1'), undefined);
});

test('the scopes are asked for again when a Bot is made, renamed or deleted, and not when only its use changes', () => {
  const bots = [{ botId: 'a', name: 'Alpha' }, { botId: 'b', name: 'Beta' }];
  const key = botScopesKey(bots);
  assert.equal(botScopesKey([...bots].reverse()), key, 'the order of the list does not matter');
  assert.equal(botScopesKey(bots.map((bot) => ({ ...bot, lastActivityAt: 5, description: 'other' }))), key, 'nor what the scopes do not show');
  assert.notEqual(botScopesKey([...bots, { botId: 'c', name: 'Gamma' }]), key, 'made');
  assert.notEqual(botScopesKey([bots[0]!, { botId: 'b', name: 'Beta 2' }]), key, 'renamed');
  assert.notEqual(botScopesKey([bots[0]!]), key, 'deleted');
  assert.notEqual(botScopesKey([{ botId: 'a', name: 'Alpha\nb' }]), botScopesKey([{ botId: 'a', name: 'Alpha' }, { botId: 'b', name: '' }]), 'a name cannot pass for two Bots');
});

test('a Bot\'s scope is named for the Bot and marked as a Bot\'s, so it is not taken for a project', () => {
  assert.equal(scopeName(BOT_SCOPE), 'Bot · Scribe');
  assert.equal(scopeName(SCOPES[1]!), 'app');
  assert.equal(scopeName(SCOPES[0]!), 'No project');
  assert.equal(scopeName({ label: 'Scribe (unavailable)' }), 'Scribe (unavailable)');
});

// ---- Putting a task into words ----

test('a routine is shown with its title, schedule, status and next run', () => {
  const next = Date.UTC(2026, 9, 28, 9, 0, 0);
  const row = routineRow(job({ next_run_at: next }));
  assert.equal(row.id, 'task-1');
  assert.equal(row.title, 'Morning brief');
  assert.match(row.schedule, /^Daily at 09:00 \(.+\)$/);
  assert.equal(row.status, 'active');
  assert.equal(row.statusLabel, tr('cron_task_status_active'));
  assert.match(row.next, /^Next run /);
  assert.equal(row.openable, true);
});

test('a routine that is not running says why, or that it is paused', () => {
  assert.equal(routineRow(job({ status: 'paused', statusReason: 'The Bot is unavailable.' })).next, 'The Bot is unavailable.');
  const paused = routineRow(job({ status: 'paused' }));
  assert.equal(paused.next, tr('chat_status_paused'));
  assert.equal(paused.statusLabel, tr('chat_status_paused'));
  assert.equal(routineRow(job({ status: 'completed' })).status, 'completed');
  assert.equal(routineRow(job()).next, tr('desktop_scheduled_local'), 'an active routine with no time yet runs locally');
});

test('a task the engine made itself has no configuration and counts as active', () => {
  const bare = job();
  delete bare.automation;
  assert.equal(jobStatus(bare), 'active');
  assert.equal(routineRow(bare).title, 'Morning brief');
  assert.equal(nextRunLabel({ next_run_at: undefined }), tr('desktop_scheduled_local'));
});

test('a task that only lasts as long as a chat cannot be opened from the list', () => {
  assert.equal(routineRow(job({ durable: false })).openable, false);
  assert.equal(routineRow(job({ permanent: true })).openable, false);
});

// ---- Reading a Bot's routines ----

test('reading the routines asks the scheduler to list the Bot\'s scope', async () => {
  const requests: Array<[string, CronRequestDto]> = [];
  const controller = new BotRoutinesController(async (scopeId, request) => { requests.push([scopeId, request]); return [job()]; });
  assert.deepEqual(controller.getSnapshot(), { jobs: undefined, loading: false, error: undefined });
  const loading = controller.load(BOT_SCOPE.id);
  assert.equal(controller.getSnapshot().loading, true);
  await loading;
  assert.deepEqual(requests, [[BOT_SCOPE.id, { action: 'list' }]]);
  assert.deepEqual(controller.getSnapshot().jobs?.map((entry) => entry.id), ['task-1']);
  assert.equal(controller.getSnapshot().loading, false);
});

test('routines that could not be read are reported in words, and read again on request', async () => {
  let fail = true;
  const controller = new BotRoutinesController(async () => {
    if (fail) throw new Error("Error invoking remote method 'lingxi:scheduled': Error: This Bot is unavailable.");
    return [];
  });
  await controller.load(BOT_SCOPE.id);
  assert.equal(controller.getSnapshot().error, 'This Bot is unavailable.');
  assert.equal(controller.getSnapshot().jobs, undefined);
  fail = false;
  await controller.load(BOT_SCOPE.id);
  assert.deepEqual(controller.getSnapshot(), { jobs: [], loading: false, error: undefined });
});

test('an answer that a later reading overtook is dropped, whichever finishes last', async () => {
  const answers: Array<(jobs: CronJobDto[]) => void> = [];
  const controller = new BotRoutinesController(() => new Promise<CronJobDto[]>((resolve) => { answers.push(resolve); }));
  const first = controller.load(BOT_SCOPE.id);
  const second = controller.load(BOT_SCOPE.id);
  answers[1]!([job({ id: 'new' })]);
  await second;
  answers[0]!([job({ id: 'old' })]);
  await first;
  await settle();
  assert.deepEqual(controller.getSnapshot().jobs?.map((entry) => entry.id), ['new']);
  assert.equal(controller.getSnapshot().loading, false);
});

// ---- The panel ----

function panelAt(controller: BotRoutinesController, scopeId: string | undefined): string {
  return render(React.createElement(BotRoutinesPanel, { controller, scopeId, onNew: noop, onOpen: noop }));
}
const panel = (controller: BotRoutinesController): string => panelAt(controller, BOT_SCOPE.id);

test('a Bot the scheduler does not list has routines that cannot be made yet', () => {
  const html = panelAt(new BotRoutinesController(async () => []), undefined);
  assert.match(html, /Routines are not available for this Bot right now\./);
  assert.match(html, /<button type="button" disabled="">New routine<\/button>/);
  assert.ok(!html.includes('Loading routines'));
});

test('the panel explains what a routine is, and offers to make one', async () => {
  const controller = new BotRoutinesController(async () => []);
  await controller.load(BOT_SCOPE.id);
  const html = panel(controller);
  assert.match(html, /A routine is a task this Bot runs on a schedule/);
  assert.match(html, /<button type="button">New routine<\/button>/);
  assert.match(html, /No routines yet\./);
});

test('before the first answer the panel says it is loading, and a failure says why with a way to try again', async () => {
  const waiting = new BotRoutinesController(() => new Promise<CronJobDto[]>(() => undefined));
  void waiting.load(BOT_SCOPE.id);
  assert.match(panel(waiting), /role="status"[^>]*>Loading routines…/);
  const broken = new BotRoutinesController(async () => { throw new Error('The scheduler stopped.'); });
  await broken.load(BOT_SCOPE.id);
  const html = panel(broken);
  assert.match(html, /role="alert"[^>]*>Routines could not be loaded: The scheduler stopped\./);
  assert.match(html, />Retry</);
  assert.ok(!html.includes('No routines yet'));
});

test('each routine shows its title, schedule, status and next run, and opens when clicked', async () => {
  const controller = new BotRoutinesController(async () => [
    job({ id: 'a', next_run_at: Date.UTC(2026, 9, 28, 9, 0, 0) }),
    job({ id: 'b', status: 'paused', cron: '30 17 * * 1-5', prompt: '# Evening tidy\n\nFile the notes.' }),
    job({ id: 'c', durable: false, prompt: '# Session only\n\nGone with the chat.' }),
  ]);
  await controller.load(BOT_SCOPE.id);
  const html = panel(controller);
  assert.match(html, /aria-label="Routines"/);
  assert.match(html, /Morning brief<span class="bots-tag" data-status="active">Active<\/span>/);
  assert.match(html, /Daily at 09:00 \(/);
  assert.match(html, /Next run /);
  assert.match(html, /Evening tidy<span class="bots-tag" data-status="paused">Paused<\/span>/);
  assert.match(html, /Weekdays at 17:30 \(/);
  assert.match(html, /<button type="button" class="bots-chat-row" disabled="">[^]*Session only/, 'a task that is not kept cannot be opened');
  assert.equal((html.match(/class="bots-chat-row"/g) ?? []).length, 3);
  assert.ok(!html.includes('No routines yet'));
});

// ---- The Scheduled page names the Bot ----

function setup(scopeId: string): string {
  return render(React.createElement(ScheduledTaskSetup, {
    initial: { title: '', schedule: '', description: '' }, scopeId, scopes: SCOPES, sessions: [], models: [], defaultModel: 'a/b',
    loadContext: async () => ({ models: [], currentModel: 'a/b', sessions: [] }), onBack: noop, onSave: async () => undefined, onDirtyChange: noop,
  }));
}

test('the editor offers a Bot\'s workspace as "Bot · Name", selected when the routine is made for the Bot, with a note on what that means', () => {
  const html = setup(BOT_SCOPE.id);
  assert.match(html, new RegExp(`<option value="${BOT_SCOPE.id.replace(/\//g, '\\/')}" selected="">Bot · Scribe</option>`));
  assert.match(html, /<option value="\/work\/app">app<\/option>/, 'a project is still just its name');
  assert.match(html, /This task runs as the Bot, in its own workspace/);
  assert.ok(!html.includes(tr('desktop_scheduled_no_project')), 'it is not a task of no project');
});

test('the editor says nothing of a Bot for a project or for no project', () => {
  for (const scopeId of ['global', '/work/app']) {
    const html = setup(scopeId);
    assert.ok(!html.includes('runs as the Bot'), scopeId);
    assert.match(html, /Bot · Scribe/, 'the Bot is still among the places to run in');
  }
  assert.match(setup('global'), new RegExp(tr('desktop_scheduled_no_project').slice(0, 20)));
});
