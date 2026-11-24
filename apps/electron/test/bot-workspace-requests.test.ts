import { test } from 'node:test';
import assert from 'node:assert/strict';
import { join } from 'node:path';

import { DelegationError, type Caller } from '../src/main/bots/routerTools.js';
import {
  DECLINE_COOLDOWN_MS, MAX_PENDING_PER_BOT, MAX_REASON_CHARS, REQUEST_TTL_MS, WorkspaceRequests, type WorkspaceRequest,
} from '../src/main/bots/workspaceRequests.js';
import type { BotWorkspaceInput } from '../src/main/bots/store.js';
import { botFixture, SESSION } from './bot-helpers.js';

function setup(t: { after(fn: () => void): void }, options: { usable?: () => boolean } = {}) {
  const f = botFixture(t);
  const clock = { now: 1_000_000 };
  const grants: Array<[string, BotWorkspaceInput]> = [];
  const changes: string[][] = [];
  const announced: Array<[WorkspaceRequest, string]> = [];
  const warnings: string[] = [];
  let failGrant: Error | undefined;
  let sequence = 0;
  const requests = new WorkspaceRequests({
    store: f.store,
    grant: async (botId, input) => {
      grants.push([botId, input]);
      if (failGrant) throw failGrant;
      const record = f.store.get(botId);
      const held = record.workspaces.find((workspace) => workspace.path === f.store.workspace(input.path, input.mode).path);
      return held ? f.store.changeWorkspace(botId, held.id, { mode: input.mode }) : f.store.addWorkspace(botId, input);
    },
    usable: options.usable ?? (() => true),
    changed: (botIds) => { changes.push([...botIds]); },
    notify: (request, name) => { announced.push([request, name]); },
    now: () => clock.now,
    newId: () => `Req${String(++sequence).padStart(19, '0')}`,
    warn: (message) => warnings.push(message),
  });
  const bot = f.store.create({ name: 'Asker' });
  const caller: Caller = { botId: bot.bot_id, sessionId: SESSION };
  const signal = new AbortController().signal;
  const ask = (args: { path: string; mode?: 'read' | 'write'; reason?: string; waitSeconds?: number }, who: Caller = caller, abort: AbortSignal = signal) =>
    requests.request(who, { mode: 'read', reason: 'To look at it', waitSeconds: 0, ...args }, abort);
  return { ...f, requests, clock, grants, changes, announced, warnings, bot, caller, ask, failGrant: (error: Error | undefined) => { failGrant = error; } };
}

test('a Bot that asks for a folder only records the request: nothing is shared, the owner is told once, and the answer is pending', async (t) => {
  const s = setup(t);
  const repo = s.folder('repo');
  const view = await s.ask({ path: repo, mode: 'write', reason: 'To file the\nminutes\u0007 here' });
  assert.deepEqual(view, {
    request_id: 'Req0000000000000000001', path: repo, mode: 'write',
    status: { state: 'pending', message: view.status.message },
  });
  assert.match(view.status.message!, /owner has been asked on this computer/);
  assert.deepEqual(s.store.get(s.bot.bot_id).workspaces, [], 'the folder is not the Bot\'s until the owner says so');
  assert.deepEqual(s.grants, []);
  const [pending] = s.requests.pending(s.bot.bot_id);
  assert.deepEqual(pending, { id: 'Req0000000000000000001', botId: s.bot.bot_id, sessionId: SESSION, path: repo, mode: 'write', reason: 'To file the minutes here', createdAt: s.clock.now, state: 'pending' });
  assert.deepEqual(s.changes, [[s.bot.bot_id]]);
  assert.equal(s.announced.length, 1);
  assert.deepEqual([s.announced[0]![0].id, s.announced[0]![1]], ['Req0000000000000000001', 'Asker']);
  const again = await s.ask({ path: repo, mode: 'write' });
  assert.equal(again.request_id, view.request_id, 'one question, once');
  assert.equal(s.announced.length, 1);
  assert.equal(s.requests.pending().length, 1);
});

test('the folder asked for is checked as the owner\'s own would be, and the reason has to be given and is shortened', async (t) => {
  const s = setup(t);
  const refused = async (args: Parameters<typeof s.ask>[0], code: string, message: RegExp) => {
    await assert.rejects(s.ask(args), (error: unknown) => error instanceof DelegationError && error.code === code && message.test(error.message), JSON.stringify(args));
  };
  await refused({ path: join(s.root, 'nowhere') }, 'invalid', /does not exist/);
  await refused({ path: s.home }, 'invalid', /home folder/);
  await refused({ path: 'relative/folder' }, 'invalid', /./);
  await refused({ path: s.store.workspaceDir(s.bot.bot_id) }, 'invalid', /your own folder/);
  await refused({ path: s.folder('repo'), reason: '  \n ' }, 'invalid', /Say what you need the folder for/);
  assert.deepEqual(s.requests.pending(), [], 'nothing was recorded, and nobody was told');
  assert.equal(s.announced.length, 0);
  await s.ask({ path: s.folder('repo'), reason: 'x'.repeat(MAX_REASON_CHARS + 100) });
  assert.equal(s.requests.pending()[0]!.reason.length, MAX_REASON_CHARS);
});

test('what the Bot has already is not asked of the owner again, but more than it has is', async (t) => {
  const s = setup(t);
  const repo = s.folder('repo');
  await s.store.addWorkspace(s.bot.bot_id, { path: repo, mode: 'read' });
  const same = await s.ask({ path: repo, mode: 'read' });
  assert.deepEqual(same, { request_id: '', path: repo, mode: 'read', status: { state: 'approved', message: 'You have this folder already.' } });
  assert.equal(s.requests.pending().length, 0);
  await s.ask({ path: repo, mode: 'write' });
  assert.equal(s.requests.pending().length, 1, 'write is more than read');
  const other = s.folder('other');
  s.store.addWorkspace(s.bot.bot_id, { path: other, mode: 'write' });
  const less = await s.ask({ path: other, mode: 'read' });
  assert.equal(less.status.state, 'approved');
  assert.equal(less.mode, 'write', 'the answer says what the Bot has');
});

test('a request for more takes the place of a smaller one for the same folder, and a smaller one waits behind a larger', async (t) => {
  const s = setup(t);
  const repo = s.folder('repo');
  const small = await s.ask({ path: repo, mode: 'read' });
  const large = await s.ask({ path: repo, mode: 'write' });
  assert.notEqual(small.request_id, large.request_id);
  assert.deepEqual(s.requests.pending().map((request) => [request.id, request.mode]), [[large.request_id, 'write']], 'the owner is not asked twice for one folder');
  const behind = await s.ask({ path: repo, mode: 'read' });
  assert.equal(behind.request_id, large.request_id, 'what is asked already covers it');
});

test('the owner approves: the Bot is given the folder as asked, the answer reaches a Bot that waits, and a request is answered once', async (t) => {
  const s = setup(t);
  const repo = s.folder('repo');
  const waiting = s.ask({ path: repo, mode: 'write', waitSeconds: 30 });
  await new Promise((resolve) => setImmediate(resolve));
  const [request] = s.requests.pending(s.bot.bot_id);
  s.changes.length = 0;
  const decided = await s.requests.decide(s.bot.bot_id, request!.id, 'approve');
  assert.equal(decided.state, 'approved');
  assert.deepEqual(s.grants, [[s.bot.bot_id, { path: repo, mode: 'write' }]]);
  assert.deepEqual(s.store.get(s.bot.bot_id).workspaces.map((workspace) => [workspace.path, workspace.mode]), [[repo, 'write']]);
  const answer = await waiting;
  assert.equal(answer.status.state, 'approved');
  assert.match(answer.status.message!, /yours from your next turn/);
  assert.deepEqual(s.requests.pending(), []);
  assert.deepEqual(s.changes, [[s.bot.bot_id]], 'the window is told once');
  await assert.rejects(s.requests.decide(s.bot.bot_id, request!.id, 'approve'), /answered already, or has expired/);
  assert.equal(s.grants.length, 1);
});

test('the owner declines: nothing is given, the Bot is told, and it is not let ask for the same folder at once again', async (t) => {
  const s = setup(t);
  const repo = s.folder('repo');
  const waiting = s.ask({ path: repo, mode: 'write', waitSeconds: 30 });
  await new Promise((resolve) => setImmediate(resolve));
  const [request] = s.requests.pending(s.bot.bot_id);
  await s.requests.decide(s.bot.bot_id, request!.id, 'deny');
  assert.equal((await waiting).status.state, 'denied');
  assert.deepEqual(s.grants, []);
  assert.deepEqual(s.store.get(s.bot.bot_id).workspaces, []);
  const again = await s.ask({ path: repo, mode: 'write' });
  assert.equal(again.status.state, 'denied');
  assert.match(again.status.message!, /Do not ask again/);
  assert.equal(s.requests.pending().length, 0, 'no new question for the owner');
  assert.equal(s.announced.length, 1);
  // A request for less than what was declined is another question.
  assert.equal((await s.ask({ path: repo, mode: 'read' })).status.state, 'pending');
  s.clock.now += DECLINE_COOLDOWN_MS + 1;
  assert.equal((await s.ask({ path: repo, mode: 'write' })).status.state, 'pending', 'and after a while the Bot may ask again');
});

test('a folder declined for reading is declined for writing too', async (t) => {
  const s = setup(t);
  const repo = s.folder('repo');
  const { request_id } = await s.ask({ path: repo, mode: 'read' });
  await s.requests.decide(s.bot.bot_id, request_id, 'deny');
  assert.equal((await s.ask({ path: repo, mode: 'write' })).status.state, 'denied');
  assert.equal((await s.ask({ path: repo, mode: 'read' })).status.state, 'denied');
});

test('an approval that cannot be carried out leaves the request to be answered again', async (t) => {
  const s = setup(t);
  const repo = s.folder('repo');
  const { request_id } = await s.ask({ path: repo, mode: 'write' });
  s.failGrant(new Error('The folder is gone.'));
  await assert.rejects(s.requests.decide(s.bot.bot_id, request_id, 'approve'), /folder is gone/);
  assert.equal(s.requests.pending().length, 1, 'still waiting for an answer');
  s.failGrant(undefined);
  assert.equal((await s.requests.decide(s.bot.bot_id, request_id, 'approve')).state, 'approved');
});

test('the owner of one Bot cannot answer another Bot\'s request, and a decision is approve or deny', async (t) => {
  const s = setup(t);
  const other = s.store.create({ name: 'Other' });
  const { request_id } = await s.ask({ path: s.folder('repo') });
  await assert.rejects(s.requests.decide(other.bot_id, request_id, 'approve'), /answered already, or has expired/);
  await assert.rejects(s.requests.decide(s.bot.bot_id, 'nothing', 'approve'), /answered already, or has expired/);
  await assert.rejects(s.requests.decide(s.bot.bot_id, request_id, 'maybe' as 'approve'), /invalid decision/);
  assert.equal(s.requests.pending(other.bot_id).length, 0);
  assert.equal(s.requests.pending(s.bot.bot_id).length, 1);
  assert.deepEqual(s.grants, []);
});

test('a Bot has only so many questions open, and a question nobody answered is forgotten after a day', async (t) => {
  const s = setup(t);
  for (let index = 0; index < MAX_PENDING_PER_BOT; index += 1) await s.ask({ path: s.folder(`repo-${index}`) });
  await assert.rejects(s.ask({ path: s.folder('one-more') }), (error: unknown) => error instanceof DelegationError && error.code === 'busy');
  const waiting = s.ask({ path: s.folder('repo-0'), waitSeconds: 30 });
  s.clock.now += REQUEST_TTL_MS + 1;
  assert.deepEqual(s.requests.pending(), []);
  assert.equal((await waiting).request_id.length > 0, true);
  await assert.rejects(s.requests.decide(s.bot.bot_id, 'Req0000000000000000001', 'approve'), /answered already, or has expired/);
  assert.equal((await s.ask({ path: s.folder('repo-0') })).status.state, 'pending', 'and it may be asked again');
});

test('a Bot that waits is let go when the call ends, and a Bot that is deleted takes its questions with it', async (t) => {
  const s = setup(t);
  const controller = new AbortController();
  const waiting = s.ask({ path: s.folder('repo'), waitSeconds: 30 }, s.caller, controller.signal);
  await new Promise((resolve) => setImmediate(resolve));
  controller.abort();
  assert.equal((await waiting).status.state, 'pending', 'the call ends with the question still open');
  const second = s.ask({ path: s.folder('other'), waitSeconds: 30 });
  await new Promise((resolve) => setImmediate(resolve));
  s.changes.length = 0;
  s.requests.dropBot(s.bot.bot_id);
  assert.equal((await second).status.state, 'expired', 'the waiting call of a deleted Bot hears the question is gone');
  assert.deepEqual(s.requests.pending(), []);
  assert.deepEqual(s.changes, [[s.bot.bot_id]], 'the window stops showing them');
  s.changes.length = 0;
  s.requests.dropBot(s.bot.bot_id);
  assert.deepEqual(s.changes, [], 'nothing to say about a Bot with none');
});

test('a Bot that cannot ask gets a reason: Bots are off, it is being deleted, or it is not a Bot of this computer', async (t) => {
  let usable = true;
  const s = setup(t, { usable: () => usable });
  usable = false;
  await assert.rejects(s.ask({ path: s.folder('repo') }), (error: unknown) => error instanceof DelegationError && error.code === 'unavailable');
  usable = true;
  await assert.rejects(s.ask({ path: s.folder('repo') }, { botId: 'Bot0000000000000000999', sessionId: SESSION }), (error: unknown) => error instanceof DelegationError && error.code === 'unavailable');
  assert.equal(s.announced.length, 0);
  assert.deepEqual(s.warnings, []);
});

test('a notification that cannot be shown does not stop the request', async (t) => {
  const f = botFixture(t);
  const warnings: string[] = [];
  const requests = new WorkspaceRequests({
    store: f.store, grant: async () => { throw new Error('unused'); }, usable: () => true, changed: () => undefined,
    notify: () => { throw new Error('no notifications'); }, warn: (message) => warnings.push(message),
  });
  const bot = f.store.create({ name: 'Quiet' });
  const view = await requests.request({ botId: bot.bot_id, sessionId: SESSION }, { path: f.folder('repo'), mode: 'read', reason: 'x', waitSeconds: 0 }, new AbortController().signal);
  assert.equal(view.status.state, 'pending');
  assert.equal(requests.pending().length, 1);
  assert.deepEqual(warnings, ['a folder request could not be announced: no notifications']);
});
