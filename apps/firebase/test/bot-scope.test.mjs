import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { effectiveScope, emptyScope, narrowing, publishedScope, scopeSize, withinPublished } from '../src/bot-scope.mjs';
import { sharedBotCommand } from '../src/shared-bot-commands.mjs';

const command = (action, payload) => ({ schemaVersion: 1, commandId: 'command_0001', teamId: 'team_0001', generation: 1, action, payload });
const host = { deploymentId: 'deployment_01', hostId: 'host_0001', accountEpoch: 1 };
const publication = {
  sharedBotId: 'sharedbot_01', expectedRevision: 0, name: 'Assistant', description: '', usePolicy: { mode: 'all' },
  deployment: { ...host, sharedBotId: 'sharedbot_01', revision: 1, owner: { uid: 'owner', generation: 1 }, instanceId: 'instance_01',
    definitionDigest: 'a'.repeat(64), capabilitySnapshotDigest: 'b'.repeat(64), executionVersion: 2,
    capabilities: { durableWait: true, writeOperations: false, delegation: false }, enabled: true },
  inputs: [{ resourceId: 'persona_01', revision: 1, digest: 'a'.repeat(64), kind: 'persona', sourceAudience: { mode: 'team' } }],
  quotas: { team: 100, member: 20, bot: 100 },
};
const row = (dataset, more = {}) => ({ source: `source_${dataset}`, dataset, write: false, reportReview: false, ...more });
const published = {
  localApps: [{ id: 'calendar', name: 'Calendar' }, { id: 'notes', name: 'Notes' }],
  datasets: [row('d1', { write: true, reportReview: true }), row('d2')],
};

test('a published scope is ids, names and flags: bounded, with no repeated app or dataset and nothing else', () => {
  assert.deepEqual(publishedScope.parse(published), published);
  assert.deepEqual(publishedScope.parse(emptyScope), emptyScope);
  // Names are trimmed like every other name the service stores.
  assert.equal(publishedScope.parse({ localApps: [{ id: 'a', name: '  Calendar ' }], datasets: [] }).localApps[0].name, 'Calendar');
  const apps = count => Array.from({ length: count }, (_, i) => ({ id: `app${i}`, name: 'App' }));
  const datasets = count => Array.from({ length: count }, (_, i) => row(`d${i}`));
  publishedScope.parse({ localApps: apps(64), datasets: datasets(64) });
  for (const value of [
    undefined, null, [], {}, { localApps: [] }, { datasets: [] },
    { ...emptyScope, paths: ['/Users/owner'] },
    { localApps: apps(65), datasets: [] }, { localApps: [], datasets: datasets(65) },
    { localApps: [{ id: 'a', name: 'A' }, { id: 'a', name: 'Again' }], datasets: [] },
    { localApps: [], datasets: [row('d1'), row('d1', { source: 'other' })] },
    { localApps: [{ id: '', name: 'A' }], datasets: [] }, { localApps: [{ id: 'a b', name: 'A' }], datasets: [] }, { localApps: [{ id: 'a'.repeat(129), name: 'A' }], datasets: [] },
    { localApps: [{ id: 'a', name: '' }], datasets: [] }, { localApps: [{ id: 'a', name: '   ' }], datasets: [] }, { localApps: [{ id: 'a', name: 'x'.repeat(101) }], datasets: [] },
    { localApps: [{ id: 'a', name: 'bad\nname' }], datasets: [] }, { localApps: [{ id: 'a', name: 'A', path: '/tmp' }], datasets: [] },
    { localApps: [], datasets: [{ source: 's', dataset: 'd', write: true }] }, { localApps: [], datasets: [{ source: 's', dataset: 'd', write: 'yes', reportReview: false }] },
    { localApps: [], datasets: [row('d1', { source: 'has space' })] }, { localApps: [], datasets: [row('d'.repeat(65))] },
    { localApps: [], datasets: [{ ...row('d1'), token: 'secret' }] },
  ]) assert.throws(() => publishedScope.parse(value), JSON.stringify(value)?.slice(0, 80));
});

test('a narrowing is whole lists of ids: a list that is left out does not narrow, an empty one allows nothing', () => {
  for (const value of [{}, { localApps: [] }, { datasets: [] }, { localApps: ['calendar'], datasets: [{ source: 'source_d1', dataset: 'd1' }] }]) {
    assert.deepEqual(narrowing.parse(value), value);
  }
  for (const value of [
    null, [], { localApps: ['calendar', 'calendar'] }, { localApps: ['a b'] }, { localApps: [{ id: 'calendar' }] },
    { datasets: [{ source: 's', dataset: 'd' }, { source: 't', dataset: 'd' }] }, { datasets: [{ source: 's', dataset: 'd', write: true }] },
    { datasets: [{ dataset: 'd' }] }, { datasets: ['d'] }, { localApps: [], extra: [] },
    { localApps: Array.from({ length: 65 }, (_, i) => `app${i}`) },
  ]) assert.throws(() => narrowing.parse(value), JSON.stringify(value).slice(0, 80));
});

test('a narrowing may only ask for what was published, dataset by dataset with its source', () => {
  assert.equal(withinPublished(published, {}), true);
  assert.equal(withinPublished(published, { localApps: [], datasets: [] }), true);
  assert.equal(withinPublished(published, { localApps: ['calendar'], datasets: [{ source: 'source_d1', dataset: 'd1' }] }), true);
  assert.equal(withinPublished(published, { localApps: ['mail'] }), false);
  assert.equal(withinPublished(published, { datasets: [{ source: 'source_d3', dataset: 'd3' }] }), false);
  // The same dataset through another source is not what was published.
  assert.equal(withinPublished(published, { datasets: [{ source: 'elsewhere', dataset: 'd1' }] }), false);
  // A Bot published without a scope has nothing to narrow.
  for (const none of [undefined, emptyScope]) {
    assert.equal(withinPublished(none, {}), true);
    assert.equal(withinPublished(none, { localApps: ['calendar'] }), false);
    assert.equal(withinPublished(none, { datasets: [{ source: 's', dataset: 'd' }] }), false);
  }
});

test('the effective scope is what was published cut down to the narrowing, with the published flags', () => {
  assert.deepEqual(effectiveScope(published, undefined), published);
  assert.deepEqual(effectiveScope(published, {}), published);
  assert.deepEqual(effectiveScope(published, { revision: 4 }), published, 'a lifted narrowing keeps only its revision');
  assert.deepEqual(effectiveScope(published, { localApps: ['notes'], datasets: [{ source: 'source_d2', dataset: 'd2' }] }),
    { localApps: [{ id: 'notes', name: 'Notes' }], datasets: [row('d2')] });
  assert.deepEqual(effectiveScope(published, { localApps: [], datasets: [] }), emptyScope, 'an empty list allows nothing');
  assert.deepEqual(effectiveScope(published, { localApps: ['calendar'] }).datasets, published.datasets, 'only the kind that is named is narrowed');
  // Narrowing never adds: what a later publication no longer has is gone, whatever the Chat still says.
  assert.deepEqual(effectiveScope({ localApps: [{ id: 'notes', name: 'Notes' }], datasets: [] }, { localApps: ['calendar', 'notes'], datasets: [{ source: 'source_d1', dataset: 'd1' }] }),
    { localApps: [{ id: 'notes', name: 'Notes' }], datasets: [] });
  // A narrowing cannot widen a flag: the write and report review flags are the published ones.
  assert.equal(effectiveScope(published, { datasets: [{ source: 'source_d2', dataset: 'd2' }] }).datasets[0].write, false);
  // A Bot published without a scope has none, and a narrowing written before it was republished does not give it one.
  for (const none of [undefined, emptyScope]) assert.deepEqual(effectiveScope(none, { localApps: ['calendar'] }), emptyScope);
  assert.equal(scopeSize(published), 4); assert.equal(scopeSize(undefined), 0); assert.equal(scopeSize(emptyScope), 0);
});

test('publishing takes an optional scope, and setting a Chat narrowing takes the whole value or null', () => {
  assert.equal(sharedBotCommand.parse(command('publishSharedBot', publication)).payload.scope, undefined);
  assert.deepEqual(sharedBotCommand.parse(command('publishSharedBot', { ...publication, scope: published })).payload.scope, published);
  assert.throws(() => sharedBotCommand.parse(command('publishSharedBot', { ...publication, scope: { localApps: [] } })));
  assert.throws(() => sharedBotCommand.parse(command('publishSharedBot', { ...publication, scope: { ...published, extra: 1 } })));
  const set = { conversationId: 'chat_0001', sharedBotId: 'sharedbot_01', expectedRevision: 0 };
  for (const scope of [null, {}, { localApps: [] }, { localApps: ['calendar'], datasets: [{ source: 'source_d1', dataset: 'd1' }] }]) {
    assert.deepEqual(sharedBotCommand.parse(command('setConversationBotScope', { ...set, scope })).payload.scope, scope);
  }
  for (const payload of [{ ...set }, { ...set, scope: undefined }, { ...set, scope: 'all' }, { ...set, scope: {}, extra: true },
    { conversationId: set.conversationId, expectedRevision: 0, scope: null }, { ...set, expectedRevision: -1, scope: null }, { ...set, expectedRevision: 1.5, scope: null },
    { ...set, sharedBotId: 'short', scope: null }]) {
    assert.throws(() => sharedBotCommand.parse(command('setConversationBotScope', payload)), JSON.stringify(payload));
  }
});

test('the command vectors say the same to the service as to the web client', () => {
  // They are not in `execution-vectors.json` yet: the native clients check every vector there, and take this command in their own phase.
  const { commands } = JSON.parse(readFileSync(new URL('../../../packages/teams/contracts/bot-scope-vectors.json', import.meta.url), 'utf8'));
  assert.ok(commands.length >= 8 && commands.every(vector => vector.input.action === 'setConversationBotScope'));
  for (const vector of commands) assert.equal(sharedBotCommand.safeParse(vector.input).success, vector.valid, vector.name);
});

test('the effective scope is the same one the clients compute: the shared vectors', () => {
  const { cases } = JSON.parse(readFileSync(new URL('../../../packages/teams/contracts/bot-scope-vectors.json', import.meta.url), 'utf8'));
  assert.ok(cases.length >= 8);
  for (const { name, published: have, narrowing: narrowed, effective } of cases) assert.deepEqual(effectiveScope(have ?? undefined, narrowed ?? undefined), effective, name);
});
