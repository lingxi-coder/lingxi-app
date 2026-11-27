import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import ts from 'typescript';
import { FriendApiError } from '../../../packages/teams/friends';

// Objects made inside the module's context have another realm's prototypes; what is compared is their data.
const plain = <T>(value: T): T => JSON.parse(JSON.stringify(value ?? null));

// `lingxi:friends` the way the main process installs it, with Electron and the service replaced by fakes.
function install() {
  const handlers = new Map<string, (event: unknown, input: unknown) => Promise<unknown>>(), removed: string[] = [];
  const source = ts.transpileModule(readFileSync(new URL('../src/main/teams/friendsIpc.ts', import.meta.url), 'utf8'), { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText;
  const exported: { installFriendsIpc(service: unknown, windows: () => unknown[]): () => void } = {} as never;
  vm.runInNewContext(source, {
    exports: exported,
    // The errors the fakes throw must be the module's `Error`s too.
    Error, AbortController,
    require: (id: string) => {
      if (id === 'electron') return { ipcMain: { handle: (name: string, handler: never) => handlers.set(name, handler), removeHandler: (name: string) => removed.push(name) } };
      if (id.includes('packages/teams/friends')) return { FriendApiError };
      throw new Error(id);
    },
  });
  const calls: unknown[][] = [];
  let outcome: (...args: unknown[]) => unknown = () => ({ ok: true });
  const events = { rows: new Set<(event: { leaseId: string; rows: unknown[] }) => void>(), failed: new Set<(event: { leaseId: string; code: string }) => void>() };
  const record = (name: string) => (...args: unknown[]) => { calls.push([name, ...args]); return outcome(...args); };
  const service = {
    profile: record('profile'), execute: record('execute'), readProfile: record('readProfile'), watch: record('watch'), releaseWatches: record('releaseWatches'), close: record('close'),
    onRows: (callback: never) => { events.rows.add(callback); return () => events.rows.delete(callback); },
    onFailed: (callback: never) => { events.failed.add(callback); return () => events.failed.delete(callback); },
  };
  const sent: unknown[][] = [];
  const window = (id: number) => {
    const webContents = Object.assign(new EventEmitter(), { id, mainFrame: { frame: id }, send: (...args: unknown[]) => sent.push(plain([id, ...args])) });
    return { webContents, isDestroyed: () => false, from: { sender: webContents, senderFrame: webContents.mainFrame } };
  };
  const windows = [window(7), window(8)];
  const stop = exported.installFriendsIpc(service, () => windows);
  const invoke = async (input: unknown, from = windows[0]!.from) => plain(await handlers.get('lingxi:friends')!(from, input)) as { value?: unknown; error?: string };
  return { invoke, calls, sent, events, windows, stop, removed, respond(next: typeof outcome) { outcome = next; }, fail(error: Error) { outcome = () => { throw error; }; } };
}

test('only a main frame of one of the app\'s windows may call, and it must name the account', async () => {
  const { invoke, calls } = install();
  const stranger = { sender: new EventEmitter(), senderFrame: {} };
  await assert.rejects(invoke({ type: 'profile', uid: 'alice' }, stranger as never), /Invalid sender/u);
  const frame = install();
  await assert.rejects(frame.invoke({ type: 'profile', uid: 'alice' }, { sender: frame.windows[0]!.webContents, senderFrame: {} } as never), /Invalid sender/u, 'a subframe is not the window');
  for (const input of [undefined, null, 'profile', 7]) await assert.rejects(invoke(input), /Invalid Friends command/u);
  for (const uid of [undefined, '', 7, 'u'.repeat(129)]) await assert.rejects(invoke({ type: 'profile', uid }), /Invalid account/u);
  assert.deepEqual(calls, []);
});

test('a profile, an action and a friend\'s profile are the service\'s, for the account the renderer names', async () => {
  const { invoke, calls, fail } = install();
  assert.deepEqual(await invoke({ type: 'profile', uid: 'alice' }), { value: { ok: true } });
  assert.deepEqual(await invoke({ type: 'execute', uid: 'alice', command: { action: 'accept', otherUid: 'bob' } }), { value: { ok: true } });
  assert.deepEqual(await invoke({ type: 'execute', uid: 'alice', command: { action: 'offerTeam', teamId: 'team-alpha', friendUid: 'bob', requestId: 'request-id-1' } }), { value: { ok: true } });
  assert.deepEqual(await invoke({ type: 'readProfile', uid: 'alice', friendUid: 'bob' }), { value: { ok: true } });
  assert.deepEqual(calls, [['profile', 'alice'], ['execute', 'alice', { action: 'accept', otherUid: 'bob' }],
    ['execute', 'alice', { action: 'offerTeam', teamId: 'team-alpha', friendUid: 'bob', requestId: 'request-id-1' }], ['readProfile', 'alice', 'bob']]);
  calls.length = 0;
  for (const input of [{ type: 'execute', uid: 'alice' }, { type: 'execute', uid: 'alice', command: 'accept' }, { type: 'readProfile', uid: 'alice', friendUid: 7 }, { type: 'delete', uid: 'alice' }, { uid: 'alice' }]) {
    assert.deepEqual(await invoke(input), { error: 'INVALID_INPUT' }, JSON.stringify(input));
  }
  assert.deepEqual(calls, []);
  fail(new Error('HANDLE_TAKEN'));
  assert.deepEqual(await invoke({ type: 'profile', uid: 'alice' }), { error: 'HANDLE_TAKEN' }, 'a code crosses the bridge as it is');
  fail(new Error('connect ECONNREFUSED 127.0.0.1:443'));
  assert.deepEqual(await invoke({ type: 'profile', uid: 'alice' }), { error: 'SERVICE_UNAVAILABLE' }, 'anything else is a service failure, never its text');
  fail(new Error('handle taken'));
  assert.deepEqual(await invoke({ type: 'profile', uid: 'alice' }), { error: 'SERVICE_UNAVAILABLE' });
});

test('search cancellation is scoped to the originating window and account, and rate metadata crosses IPC', async () => {
  const { invoke, respond, windows, fail, stop } = install();
  let signal!: AbortSignal;
  respond((_uid, _command, rawSignal) => new Promise((_resolve, reject) => {
    signal = rawSignal as AbortSignal;
    signal.addEventListener('abort', () => reject(new Error('ABORTED')), { once: true });
  }));
  const search = { type: 'searchHandles', requestId: 'search-0001', uid: 'alice', query: 'ali' };
  const cancel = { type: 'cancelSearch', requestId: search.requestId, uid: search.uid };
  const pending = invoke(search);
  await invoke(cancel, windows[1]!.from);
  await invoke({ ...cancel, uid: 'bob' });
  assert.equal(signal.aborted, false);
  await invoke(cancel);
  assert.equal(signal.aborted, true);
  assert.deepEqual(await pending, { error: 'ABORTED' });

  fail(new FriendApiError('RATE_LIMITED', 429, 625));
  assert.deepEqual(await invoke(search), { error: 'RATE_LIMITED', retryAfterMs: 625 });
  respond(() => ({ ok: true }));
  stop();
});

test('a watch is leased to the window that asked, validated, and released with it or by name', async () => {
  const { invoke, calls, windows } = install();
  for (const leaseId of [undefined, 'short', 'has space!', 'x'.repeat(65)]) assert.deepEqual(await invoke({ type: 'watch', uid: 'alice', collection: 'friends', leaseId }), { error: 'INVALID_INPUT' }, String(leaseId));
  for (const collection of [undefined, 'teams', 'profiles', 'friends/bob', 'teamOffer', 'users/alice/teamOffers']) assert.deepEqual(await invoke({ type: 'watch', uid: 'alice', collection, leaseId: 'lease-0001' }), { error: 'INVALID_INPUT' }, String(collection));
  assert.deepEqual(calls, []);

  await invoke({ type: 'watch', uid: 'alice', collection: 'friends', leaseId: 'lease-0001' });
  await invoke({ type: 'watch', uid: 'alice', collection: 'blocks', leaseId: 'lease-0002' });
  await invoke({ type: 'watch', uid: 'alice', collection: 'teamOffers', leaseId: 'lease-0003' });
  await invoke({ type: 'unwatch', uid: 'alice', leaseId: 'lease-0001' });
  assert.deepEqual(calls, [['watch', '7:lease-0001:', 'alice', 'friends'], ['watch', '7:lease-0002:', 'alice', 'blocks'], ['watch', '7:lease-0003:', 'alice', 'teamOffers'],
    ['releaseWatches', '7:lease-0001:']]);

  calls.length = 0;
  windows[0]!.webContents.emit('did-start-navigation', {}, 'https://example.com', true, true);
  assert.deepEqual(calls, [], 'an in-place navigation keeps the watches');
  windows[0]!.webContents.emit('did-start-navigation', {}, 'https://example.com', false, false);
  assert.deepEqual(calls, [], 'so does a frame');
  windows[0]!.webContents.emit('did-start-navigation', {}, 'https://example.com', false, true);
  assert.deepEqual(calls, [['releaseWatches', '7:']], 'a page that is left takes the window\'s watches with it');
  calls.length = 0;
  windows[0]!.webContents.emit('destroyed');
  assert.deepEqual(calls, [['releaseWatches', '7:']], 'and so does a window that closes');
});

test('rows and failures go to the window that holds the lease, under the id it chose', async () => {
  const { invoke, sent, events, windows } = install();
  await invoke({ type: 'watch', uid: 'alice', collection: 'friends', leaseId: 'lease-0001' }, windows[1]!.from);
  for (const callback of events.rows) callback({ leaseId: '8:lease-0001:', rows: [{ uid: 'bob' }] });
  for (const callback of events.failed) callback({ leaseId: '8:lease-0001:', code: 'INVALID_SESSION' });
  assert.deepEqual(sent, [[8, 'lingxi:friends:rows', { rows: [{ uid: 'bob' }], leaseId: 'lease-0001' }], [8, 'lingxi:friends:failed', { code: 'INVALID_SESSION', leaseId: 'lease-0001' }]]);
});

test('stopping removes the handler, the sender hooks and the service', async () => {
  const { invoke, stop, removed, calls, events, windows } = install();
  await invoke({ type: 'watch', uid: 'alice', collection: 'friends', leaseId: 'lease-0001' });
  assert.equal(windows[0]!.webContents.listenerCount('destroyed'), 1);
  stop();
  assert.deepEqual(removed, ['lingxi:friends']);
  assert.equal(windows[0]!.webContents.listenerCount('destroyed'), 0);
  assert.equal(events.rows.size + events.failed.size, 0);
  assert.deepEqual(calls.at(-1), ['close']);
});
