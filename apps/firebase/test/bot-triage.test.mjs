// Bot v3 B5a: what the service takes of a Bot's auto-reply log: the verdict the host records and the page the maintainer reads.
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { sharedBotCommand, triageActions, triageLogPage, triageReasons, triageSources } from '../src/shared-bot-commands.mjs';

const command = (action, payload) => ({ schemaVersion: 1, commandId: 'command_0001', teamId: 'team_0001', generation: 1, action, payload });
const host = { deploymentId: 'deployment_01', hostId: 'host_0001', accountEpoch: 1 };
const record = (more = {}) => command('recordTriage', { ...host, taskId: 'task_0001', action: 'decline', source: 'decision', ...more });
const list = (more = {}) => command('listTriage', { sharedBotId: 'sharedbot_01', ...more });

test('the codes of a verdict are the ones the clients send', () => {
  // The service does not import the contract of `packages/teams`; this keeps the two lists one list.
  const contract = readFileSync(new URL('../../../packages/teams/execution.ts', import.meta.url), 'utf8');
  const codes = name => [...contract.match(new RegExp(`export const ${name} = \\[([^\\]]*)\\] as const;`))[1].matchAll(/'([a-z_]+)'/gu)].map(match => match[1]);
  assert.deepEqual(codes('TRIAGE_ACTIONS'), triageActions);
  assert.deepEqual(codes('TRIAGE_SOURCES'), triageSources);
  assert.deepEqual(codes('TRIAGE_REASONS'), triageReasons);
});

test('a verdict is codes and a probability: every code is taken, anything else is refused', () => {
  for (const action of triageActions) for (const source of triageSources) assert.ok(sharedBotCommand.safeParse(record({ action, source })).success, `${action} ${source}`);
  for (const reason of triageReasons) assert.ok(sharedBotCommand.safeParse(record({ reason })).success, reason);
  for (const probability of [0, 0.3, 0.123456, 1]) assert.ok(sharedBotCommand.safeParse(record({ source: 'decision', probability })).success, String(probability));
  const refused = [
    { action: 'maybe' }, { action: undefined }, { source: 'model' }, { source: undefined }, { reason: 'the model said no' }, { reason: '' },
    { probability: -0.001 }, { probability: 1.001 }, { probability: Number.NaN }, { probability: Number.POSITIVE_INFINITY }, { probability: '0.5' }, { probability: null },
    // Nothing a model wrote is kept: there is no field for it.
    { text: 'I would rather not.' }, { explanation: 'x' }, { at: 1 },
    { taskId: 'short' }, { taskId: undefined }, { deploymentId: undefined }, { hostId: undefined }, { accountEpoch: 0 },
  ];
  for (const change of refused) assert.equal(sharedBotCommand.safeParse(record(change)).success, false, JSON.stringify(change));
});

test('the log of a Bot is asked for by its id, optionally one Chat, a cursor and at most a page of rows', () => {
  assert.equal(triageLogPage, 50);
  for (const more of [{}, { conversationId: 'chat_0001' }, { cursor: 'task_0001' }, { limit: 1 }, { limit: 50 }, { conversationId: 'chat_0001', cursor: 'task_0001', limit: 20 }]) {
    assert.ok(sharedBotCommand.safeParse(list(more)).success, JSON.stringify(more));
  }
  for (const more of [{ sharedBotId: undefined }, { sharedBotId: 'x' }, { limit: 0 }, { limit: 51 }, { limit: 1.5 }, { limit: '5' }, { cursor: 'x' }, { conversationId: 'x' },
    { teamId: 'team_0001' }, { includeHidden: true }, { deploymentId: host.deploymentId }]) {
    assert.equal(sharedBotCommand.safeParse(list(more)).success, false, JSON.stringify(more));
  }
});

test('a hosted Bot service lists neither command: its gate refuses what it does not list', () => {
  // The schema is shared with the service scope, whose gate (`hosted-bot-service.mjs`) names the commands a service takes.
  assert.doesNotMatch(readFileSync(new URL('../src/hosted-bot-service.mjs', import.meta.url), 'utf8'), /recordTriage|listTriage/u);
});
