import assert from 'node:assert/strict';
import test from 'node:test';
import vectors from '../../../packages/teams/contracts/bot-scope-vectors.json';
import { decodeBotScopes, effectiveBotScope, emptyBotScope, sameBotScopes, BOT_SCOPE_ENTRIES, type BotScopeEntry, type BotScopeNarrowing, type PublishedBotScope } from '../../../packages/teams/bot-scope';
import { executionCommand } from '../../../packages/teams/execution';
import { isBotScopeNarrowing, isMemberExecutionCommand, isPublishedBotScope } from '../../../packages/teams/execution-validation';
import { watchFirestoreMessages } from '../../../packages/teams/firestore';
import { emptyProjection, mergeConversation, mergeDirectory, type MessageSnapshot, type TeamConversation } from '../../../packages/teams/messages';

const scope = { teamId: 'test-team', generation: 1 }, uid = 'alice';
const row = (dataset: string, more: Partial<PublishedBotScope['datasets'][number]> = {}) => ({ source: 'orders', dataset, write: false, reportReview: false, ...more });
const published: PublishedBotScope = { localApps: [{ id: 'calendar', name: 'Calendar' }, { id: 'notes', name: 'Notes' }], datasets: [row('dataset_01', { write: true, reportReview: true }), row('dataset_02')] };

test('effective scope: the clients compute what the service computes, for every shared vector', () => {
  const cases = vectors.cases as Array<{ name: string; published: PublishedBotScope | null; narrowing: BotScopeEntry | null; effective: PublishedBotScope }>;
  assert.ok(cases.length >= 8);
  for (const { name, published: have, narrowing, effective } of cases) assert.deepEqual(effectiveBotScope(have ?? undefined, narrowing ?? undefined), effective, name);
});

test('effective scope: the published value is left alone and the result never aliases its lists', () => {
  const before = structuredClone(published);
  const all = effectiveBotScope(published, undefined);
  assert.deepEqual(published, before);
  all.localApps.length = 0; all.datasets.length = 0;
  assert.equal(published.localApps.length, 2); assert.equal(published.datasets.length, 2);
  assert.deepEqual(emptyBotScope(), { localApps: [], datasets: [] });
  assert.notEqual(emptyBotScope(), emptyBotScope());
});

test('decoding a Chat: well-formed entries are kept as plain ids, the rest is left out, and a Chat with none has none', () => {
  assert.deepEqual(decodeBotScopes({ bot_one: { revision: 2, localApps: ['notes'], datasets: [{ source: 'orders', dataset: 'dataset_02', extra: 1 }], stray: true }, bot_two: { revision: 1 } }),
    { bot_one: { revision: 2, localApps: ['notes'], datasets: [{ source: 'orders', dataset: 'dataset_02' }] }, bot_two: { revision: 1 } });
  assert.deepEqual(decodeBotScopes({ good: { revision: 1, localApps: [] }, zero: { revision: 0 }, fraction: { revision: 1.5 }, text: { revision: '1' }, none: {}, apps: { revision: 1, localApps: [7] },
    rows: { revision: 1, datasets: [{ source: 'orders' }] }, list: { revision: 1, datasets: 'all' }, array: [], nothing: null }), { good: { revision: 1, localApps: [] } });
  for (const value of [undefined, null, 0, 'scopes', [], {}, { bad: { revision: 0 } }]) assert.equal(decodeBotScopes(value), undefined, JSON.stringify(value));
});

test('comparing narrowings: the same entries are the same whatever their order, and any visible difference is not', () => {
  const a: Record<string, BotScopeEntry> = { one: { revision: 1, localApps: ['notes'] }, two: { revision: 3 } };
  assert.equal(sameBotScopes(undefined, undefined), true);
  assert.equal(sameBotScopes(a, { two: { revision: 3 }, one: { revision: 1, localApps: ['notes'] } }), true);
  for (const other of <(Record<string, BotScopeEntry> | undefined)[]>[undefined, {}, { one: a.one }, { ...a, two: { revision: 4 } }, { ...a, one: { revision: 1, localApps: ['calendar'] } }, { ...a, one: { revision: 1, localApps: ['notes'], datasets: [] } },
    { one: a.one, three: a.two }]) assert.equal(sameBotScopes(a, other), false, JSON.stringify(other));
  assert.equal(sameBotScopes({}, undefined), true, 'a Chat with no entries is a Chat with none');
});

test('what is sent: a published scope and a narrowing are checked the way the service checks them', () => {
  const apps = (count: number) => Array.from({ length: count }, (_, i) => ({ id: `app${i}`, name: 'App' }));
  const sets = (count: number) => Array.from({ length: count }, (_, i) => row(`d${i}`));
  for (const good of [published, { localApps: [], datasets: [] }, { localApps: apps(BOT_SCOPE_ENTRIES), datasets: sets(BOT_SCOPE_ENTRIES) }]) assert.equal(isPublishedBotScope(good), true);
  for (const bad of [undefined, null, [], {}, { localApps: [] }, { datasets: [] }, { ...published, paths: [] },
    { localApps: apps(BOT_SCOPE_ENTRIES + 1), datasets: [] }, { localApps: [], datasets: sets(BOT_SCOPE_ENTRIES + 1) },
    { localApps: [{ id: 'a', name: 'A' }, { id: 'a', name: 'B' }], datasets: [] }, { localApps: [], datasets: [row('d'), row('d', { source: 'other' })] },
    { localApps: [{ id: 'a b', name: 'A' }], datasets: [] }, { localApps: [{ id: 'a', name: '  ' }], datasets: [] }, { localApps: [{ id: 'a', name: 'x'.repeat(101) }], datasets: [] },
    { localApps: [{ id: 'a', name: 'A', path: '/tmp' }], datasets: [] }, { localApps: [], datasets: [{ source: 's', dataset: 'd', write: true }] },
    { localApps: [], datasets: [{ source: 's', dataset: 'd', write: 'yes', reportReview: false }] }, { localApps: [], datasets: [{ ...row('d'), token: 'x' }] }]) {
    assert.equal(isPublishedBotScope(bad), false, JSON.stringify(bad)?.slice(0, 80));
  }
  for (const good of [{}, { localApps: [] }, { datasets: [] }, { localApps: ['calendar'], datasets: [{ source: 'orders', dataset: 'dataset_01' }] }]) assert.equal(isBotScopeNarrowing(good), true, JSON.stringify(good));
  for (const bad of [null, [], 'all', { localApps: ['a', 'a'] }, { localApps: ['a b'] }, { localApps: [{ id: 'a' }] }, { datasets: [{ source: 's', dataset: 'd' }, { source: 't', dataset: 'd' }] },
    { datasets: [{ source: 's', dataset: 'd', write: true }] }, { datasets: ['d'] }, { stray: [] }]) assert.equal(isBotScopeNarrowing(bad), false, JSON.stringify(bad));
});

test('setting a Chat narrowing: the web client says what the service says about every shared vector', () => {
  const rows = vectors.commands as Array<{ name: string; valid: boolean; input: unknown }>;
  assert.ok(rows.length >= 8);
  for (const row of rows) assert.equal(isMemberExecutionCommand(row.input), row.valid, row.name);
});

test('setting a Chat narrowing: the command takes the whole value or null and nothing else', () => {
  const payload = { conversationId: 'test-chat', sharedBotId: 'shared-123', expectedRevision: 0 };
  for (const narrowing of [null, {}, { localApps: ['calendar'] }] as Array<BotScopeNarrowing | null>) {
    const command = executionCommand(scope, 'setConversationBotScope', { ...payload, scope: narrowing }, 'command001');
    assert.deepEqual(command, { schemaVersion: 1, commandId: 'command001', ...scope, action: 'setConversationBotScope', payload: { ...payload, scope: narrowing } });
    assert.equal(isMemberExecutionCommand(command), true);
  }
  assert.throws(() => executionCommand(scope, 'setConversationBotScope', { ...payload, scope: { localApps: ['a', 'a'] } }, 'command001'), /INVALID_INPUT/);
  assert.throws(() => executionCommand(scope, 'setConversationBotScope', { ...payload, expectedRevision: -1, scope: null }, 'command001'), /INVALID_INPUT/);
  assert.equal(isMemberExecutionCommand({ schemaVersion: 1, commandId: 'command001', ...scope, action: 'setConversationBotScope', payload }), false, 'the scope key is required');
  assert.equal(isMemberExecutionCommand({ schemaVersion: 1, commandId: 'command001', ...scope, action: 'setConversationBotScope', payload: { ...payload, scope: null, extra: 1 } }), false);
});

const base: TeamConversation = { id: 'test-chat', kind: 'group', title: 'Fixture', participants: ['user:alice', 'shared_bot:bot_one'], lastSeq: 2, lastChangeSeq: 2, lastReadSeq: 0 };
test('conversation merge: an unchanged narrowing keeps the row, a changed one replaces it', () => {
  const narrowed = { ...base, botScopes: { bot_one: { revision: 1, localApps: ['notes'] } } };
  assert.equal(mergeConversation(narrowed, { ...narrowed, botScopes: { bot_one: { revision: 1, localApps: ['notes'] } } }), narrowed);
  assert.equal(mergeConversation(base, { ...base }), base);
  for (const next of <(Record<string, BotScopeEntry> | undefined)[]>[{ bot_one: { revision: 2, localApps: ['notes'] } }, { bot_one: { revision: 1, localApps: [] } }, { bot_one: { revision: 1 } }, { bot_one: narrowed.botScopes.bot_one, bot_two: { revision: 1 } }, undefined]) {
    const merged = mergeConversation(narrowed, { ...base, ...(next ? { botScopes: next } : {}) });
    assert.notEqual(merged, narrowed, JSON.stringify(next));
    assert.deepEqual(merged.botScopes, next);
  }
  assert.deepEqual(mergeConversation(base, narrowed).botScopes, narrowed.botScopes);
  const state = mergeDirectory(emptyProjection(uid, scope), [narrowed], true);
  assert.equal(mergeDirectory(state, [{ ...narrowed, botScopes: { bot_one: { revision: 1, localApps: ['notes'] } } }], true), state);
});

type Listener = { path: string; next(snapshot: unknown): void };
function watch() {
  const listeners: Listener[] = [];
  const target = (kind: string, path: unknown[]) => ({ kind, path: path.filter(part => typeof part === 'string').join('/') });
  const sdk = {
    collection: (_db: unknown, ...path: string[]) => target('collection', path), doc: (_db: unknown, ...path: string[]) => target('doc', path),
    query: (source: { path: string }) => ({ ...source, kind: 'query' }), where: () => ({}), limit: () => ({}), orderBy: () => ({}),
    onSnapshot: (source: { kind: string; path: string }, _options: unknown, next: (snapshot: unknown) => void) => { listeners.push({ path: `${source.kind}:${source.path}`, next }); return () => {}; },
  } as unknown as typeof import('firebase/firestore');
  const received: MessageSnapshot[] = [];
  const stop = watchFirestoreMessages(sdk, {} as never, uid, scope, '', value => received.push(value), () => true);
  const on = (path: string) => listeners.find(value => value.path === path)!;
  on('doc:teams/test-team/members/alice').next({ data: () => ({ generation: 1 }), metadata: { fromCache: false } });
  const document = (id: string, value: Record<string, unknown>) => ({ id, data: () => ({ kind: 'group', title: id, participants: ['user:alice'], lastSeq: 2, lastChangeSeq: 2, lastMessage: null, ...value }) });
  const directory = (docs: unknown[]) => on('query:teams/test-team/conversations').next({ metadata: { fromCache: false }, docChanges: () => [], docs });
  return { received, directory, document, stop };
}

test('Firestore listener: each Chat row carries the narrowings the document holds, as plain ids, and a Chat without any has no key', () => {
  const { received, directory, document, stop } = watch();
  directory([
    document('narrowed', { botScopes: { bot_one: { revision: 3, localApps: ['notes'], datasets: [{ source: 'orders', dataset: 'dataset_02' }] }, bot_two: { revision: 1 }, broken: { revision: 0 } } }),
    document('plain', {}), document('junk', { botScopes: 'oops' }),
  ]);
  stop();
  const rows = new Map(received.find((value): value is Extract<MessageSnapshot, { kind: 'directory' }> => value.kind === 'directory')!.conversations.map(value => [value.id, value]));
  assert.deepEqual(rows.get('narrowed')!.botScopes, { bot_one: { revision: 3, localApps: ['notes'], datasets: [{ source: 'orders', dataset: 'dataset_02' }] }, bot_two: { revision: 1 } });
  assert.ok(!('botScopes' in rows.get('plain')!)); assert.ok(!('botScopes' in rows.get('junk')!));
  assert.ok(!received.some(value => value.kind === 'error'));
});
