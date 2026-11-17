import assert from 'node:assert/strict';
import test from 'node:test';
import { authorizedTaskReport, taskReportCsv } from '../../../packages/teams/task-report';
import type { TeamTaskView } from '../../../packages/teams/execution';
import { FakeTeams } from './support/fakeTeams';

const scope = { teamId: 'team-test', generation: 2 };
const task = (taskId: string, overrides: Partial<TeamTaskView> = {}): TeamTaskView => ({ taskId, conversationId: 'private-chat', sharedBotId: 'bot-test', requester: { uid: 'alice', generation: 2 }, state: 'completed', createdAt: 1, deadline: 2, epoch: 1, activeSpentMs: 3, ...overrides });

test('Authorized report traverses 103 pages, deduplicates task IDs and only exports this viewer and generation', async () => {
  const fake = new FakeTeams(); let calls = 0;
  fake.messages = async command => {
    assert.equal(command.action, 'listTasks');
    if (command.action !== 'listTasks') throw new Error('unexpected command');
    assert.equal(command.payload.conversationId, undefined);
    assert.deepEqual({ teamId: command.teamId, generation: command.generation }, scope);
    const page = command.payload.after ? Number(command.payload.after.replace('cursor-', '')) : 0; calls++;
    return { tasks: [task(`task-${page}`), task('duplicate'), task('someone-else', { requester: { uid: 'bob', generation: 2 } }), task('old-membership', { requester: { uid: 'alice', generation: 1 } })], next: page < 102 ? `cursor-${String(page + 1).padStart(3, '0')}` : null } as never;
  };
  const result = await authorizedTaskReport(fake.api, scope, 'alice', () => true, 123);
  assert.equal(calls, 103); assert.equal(result.tasks.length, 104); assert.equal(result.generatedAt, 123);
  assert.equal(result.totals.missingUsage, 104); assert.equal(result.totals.unpricedUsage, 104);
  assert.ok(result.tasks.every(row => !['someone-else', 'old-membership'].includes(row.taskId)));
});

test('Reports preserve Missing/Partial and keep complete priced costs separate per currency', async () => {
  const fake = new FakeTeams();
  const rows = [task('missing'), task('partial', { usage: { status: 'partial', inputTokens: 1, outputTokens: 2, costMicros: 500, currency: 'USD' } }), task('usd', { usage: { status: 'complete', inputTokens: 1, outputTokens: 2, costMicros: 200, currency: 'USD' } }), task('eur', { usage: { status: 'complete', inputTokens: 1, outputTokens: 2, costMicros: 300, currency: 'EUR' } }), task('unpriced', { usage: { status: 'complete', inputTokens: 1, outputTokens: 2 } }), task('unknown-currency', { usage: { status: 'complete', inputTokens: 1, outputTokens: 2, costMicros: 400 } }), task('zero', { usage: { status: 'complete', inputTokens: 0, outputTokens: 0, costMicros: 0, currency: 'USD' } })];
  fake.messages = async () => ({ tasks: rows, next: null }) as never;
  const result = await authorizedTaskReport(fake.api, scope, 'alice', () => true);
  assert.deepEqual(result.totals, { tasks: 7, missingUsage: 1, partialUsage: 1, completeUsage: 5, unpricedUsage: 3, completeCostMicrosByCurrency: { USD: 200, EUR: 300 } });
  assert.deepEqual(result.tasks.find(row => row.taskId === 'missing')?.usage, { status: 'missing' });
  assert.equal(result.tasks.find(row => row.taskId === 'partial')?.usage.status, 'partial');
  assert.equal('costMicros' in result.tasks.find(row => row.taskId === 'unpriced')!.usage, false);
});

test('A cyclic cursor or lost account/membership authority fails the whole export', async () => {
  const fake = new FakeTeams(); let calls = 0;
  fake.messages = async () => { calls++; return { tasks: [task('task')], next: calls % 2 ? 'cursor-a' : 'cursor-b' } as never; };
  await assert.rejects(authorizedTaskReport(fake.api, scope, 'alice', () => true), /TASK_REPORT_CURSOR_LOOP/);
  assert.equal(calls, 3);
  let current = true; calls = 0;
  fake.messages = async () => { calls++; current = false; return { tasks: [task('task')], next: 'next' } as never; };
  await assert.rejects(authorizedTaskReport(fake.api, scope, 'alice', () => current), /STALE_MEMBERSHIP/);
  assert.equal(calls, 1);
});

test('JSON and CSV allowlists exclude content and private identities; CSV cannot execute formulas', async () => {
  const fake = new FakeTeams();
  fake.messages = async () => ({ tasks: [{ ...task('=HYPERLINK("https://example.test")'), message: 'secret-message', document: { body: 'secret-resource' }, credentials: 'secret-key', usage: { status: 'partial', inputTokens: 1, outputTokens: 2, internal: 'secret-usage' } }], next: null }) as never;
  const report = await authorizedTaskReport(fake.api, scope, 'alice', () => true);
  const json = JSON.stringify(report), csv = taskReportCsv(report);
  for (const forbidden of ['secret-', 'private-chat', 'bot-test', 'alice', 'requester', 'conversationId']) { assert.equal(json.includes(forbidden), false); assert.equal(csv.includes(forbidden), false); }
  assert.match(csv, /"'=HYPERLINK\(""https:\/\/example.test""\)"/);
  assert.match(csv, /"partial","1","2","",""/);
});
