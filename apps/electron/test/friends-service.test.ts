import assert from 'node:assert/strict';
import test, { type TestContext } from 'node:test';
import { FriendsService } from '../src/main/teams/friends';

// A Firestore Timestamp as the mapper reads it.
const stamp = (ms: number) => ({ toDate: () => new Date(ms) });
interface Doc { id: string; data(): Record<string, unknown> }
const card = (id: string, more: Record<string, unknown> = {}): Doc => ({ id, data: () => ({ state: 'friends', displayName: id.toUpperCase(), since: stamp(1000), ...more }) });

/** The account of the window, which a test can hand to another person. */
function fixture(context: TestContext) {
  let uid: string | undefined = 'alice', changed: (() => void) | undefined;
  const account = { snapshot: () => ({ user: uid ? { uid } : null }), getIdToken: async () => 'synthetic-token',
    onChanged(fn: () => void) { changed = fn; return () => { changed = undefined; }; } };
  const listeners: { path: string; next(snapshot: { docs: Doc[] }): void; error(error: Error): void; stopped: boolean }[] = [];
  const profiles = new Map<string, Record<string, unknown>>();
  const reads: string[] = [];
  // The Teams service's session: `db` is opaque to the service, the SDK is the only thing it calls.
  const sdk = {
    collection: (_db: unknown, ...path: string[]) => ({ path: path.join('/') }),
    doc: (_db: unknown, ...path: string[]) => ({ path: path.join('/') }),
    getDoc: async (ref: { path: string }) => { reads.push(ref.path); return { data: () => profiles.get(ref.path) }; },
    onSnapshot: (ref: { path: string }, next: never, error: never) => {
      const listener = { path: ref.path, next, error, stopped: false };
      listeners.push(listener);
      return () => { listener.stopped = true; };
    },
  };
  let session: Promise<{ db: unknown; sdk: typeof sdk }> = Promise.resolve({ db: {}, sdk });
  const teams = { firestore: async (expected: string) => { if (expected !== uid) throw new Error('INVALID_SESSION'); return session as never; } };
  const service = new FriendsService(account, teams as never);
  context.after(() => service.close());
  const rows: { leaseId: string; rows: unknown[] }[] = [], failed: { leaseId: string; code: string }[] = [];
  service.onRows(event => rows.push(event)); service.onFailed(event => failed.push(event));
  return {
    service, listeners, rows, failed, profiles, reads,
    signIn(next: string | undefined) { uid = next; changed?.(); },
    openingSession(promise: typeof session) { session = promise; },
    sdk,
  };
}

test('the profile and every action go to /v1/friends with the account\'s token and nothing else of it', async context => {
  const calls: { url: string; method: string; authorization: string; body?: unknown }[] = [];
  context.mock.method(globalThis, 'fetch', async (url: string, init: RequestInit) => {
    calls.push({ url, method: init.method!, authorization: (init.headers as Record<string, string>).Authorization!, ...(init.body ? { body: JSON.parse(String(init.body)) } : {}) });
    return Response.json({ ok: true });
  });
  const { service } = fixture(context);
  await service.profile('alice');
  await service.execute('alice', { action: 'lookup', handle: 'sam' });
  await service.execute('alice', { action: 'request', via: 'handle', handle: 'sam', requestId: 'request-id-1', note: 'hi' });
  assert.deepEqual(calls, [
    { url: 'https://lingxios.net/v1/friends/profile', method: 'GET', authorization: 'Bearer synthetic-token' },
    { url: 'https://lingxios.net/v1/friends', method: 'POST', authorization: 'Bearer synthetic-token', body: { action: 'lookup', handle: 'sam' } },
    { url: 'https://lingxios.net/v1/friends', method: 'POST', authorization: 'Bearer synthetic-token', body: { action: 'request', via: 'handle', handle: 'sam', requestId: 'request-id-1', note: 'hi' } },
  ]);
});

test('an action the service does not know is refused before anything is sent, and a server error arrives as its code', async context => {
  let sent = 0, answer = Response.json({ error: { code: 'HANDLE_TAKEN' } }, { status: 409 });
  context.mock.method(globalThis, 'fetch', async () => { sent++; return answer; });
  const { service } = fixture(context);
  for (const action of ['delete', 'teams', '', 'toString']) await assert.rejects(service.execute('alice', { action } as never), /INVALID_INPUT/u, action);
  await assert.rejects(service.execute('alice', undefined as never), /INVALID_INPUT/u);
  assert.equal(sent, 0);
  await assert.rejects(service.execute('alice', { action: 'profile', handle: 'taken' }), /^Error: HANDLE_TAKEN$/u);
  answer = Response.json({ nothing: true }, { status: 503 });
  await assert.rejects(service.profile('alice'), /^Error: SERVICE_UNAVAILABLE$/u, 'an answer without a code is a service failure');
});

test('a call is for the account that made it: another account, a token fetched for a replaced account and a late answer are all refused', async context => {
  let uid = 'alice', changed: (() => void) | undefined, release!: (value: string) => void, sent = 0;
  const token = new Promise<string>(resolve => { release = resolve; });
  const account = { snapshot: () => ({ user: { uid } }), getIdToken: () => token, onChanged(fn: () => void) { changed = fn; return () => undefined; } };
  context.mock.method(globalThis, 'fetch', async () => { sent++; return Response.json({ ok: true }); });
  const service = new FriendsService(account, { firestore: async () => { throw new Error('unused'); } } as never);
  context.after(() => service.close());
  await assert.rejects(service.execute('bob', { action: 'rotateCode' }), /INVALID_SESSION/u, 'a call for someone else');
  const operation = service.execute('alice', { action: 'rotateCode' });
  uid = 'bob'; changed?.(); release('synthetic-token');
  await assert.rejects(operation, /INVALID_SESSION/u);
  assert.equal(sent, 0, 'no request is sent with the replacement account\'s token');
  await assert.rejects(service.profile('alice'), /INVALID_SESSION/u, 'and the first account stays out');
});

test('a response that comes after the account changed is discarded', async context => {
  let uid = 'alice', changed: (() => void) | undefined, release!: (value: Response) => void, started!: () => void;
  const sending = new Promise<void>(resolve => { started = resolve; });
  const response = new Promise<Response>(resolve => { release = resolve; });
  const account = { snapshot: () => ({ user: { uid } }), getIdToken: async () => 'synthetic-token', onChanged(fn: () => void) { changed = fn; return () => undefined; } };
  context.mock.method(globalThis, 'fetch', async () => { started(); return response; });
  const service = new FriendsService(account, { firestore: async () => { throw new Error('unused'); } } as never);
  context.after(() => service.close());
  const operation = service.profile('alice');
  await sending; uid = 'bob'; changed?.(); release(Response.json({ uid: 'alice', displayName: 'Alice' }));
  await assert.rejects(operation, /INVALID_SESSION/u);
});

test('watching: the cards and the blocks of the account arrive as plain values, a card that breaks the contract is left out', async context => {
  const { service, listeners, rows } = fixture(context);
  await service.watch('1:cards:', 'alice', 'friends');
  await service.watch('1:blocks:', 'alice', 'blocks');
  assert.deepEqual(listeners.map(listener => listener.path), ['users/alice/friends', 'users/alice/blocks']);
  const pointer = { id: 'm1', seq: 4, at: stamp(5000), author: { kind: 'user', uid: 'bob' }, preview: 'Hello' };
  listeners[0]!.next({ docs: [
    card('bob', { handle: 'bobby', teamId: 'f_' + 'a'.repeat(40), generation: 2, lastSeq: 4, lastReadSeq: 1, lastChangeSeq: 4, lastMessage: pointer }),
    card('carol', { state: 'incoming', note: 'Hi, it is Carol', teamId: undefined }),
    card('broken', { state: 'strangers' }),
    card('nameless', { displayName: undefined }),
  ] });
  listeners[1]!.next({ docs: [{ id: 'dave', data: () => ({ at: stamp(9000), displayName: 'Dave', handle: 'dave' }) }, { id: 'erin', data: () => ({ at: stamp(2000) }) }] });
  assert.deepEqual(rows, [
    { leaseId: '1:cards:', rows: [
      { uid: 'bob', state: 'friends', displayName: 'BOB', handle: 'bobby', since: 1000, teamId: 'f_' + 'a'.repeat(40), generation: 2, lastSeq: 4, lastReadSeq: 1, lastChangeSeq: 4,
        lastMessage: { id: 'm1', seq: 4, at: new Date(5000).toISOString(), author: { kind: 'user', uid: 'bob' }, preview: 'Hello' } },
      { uid: 'carol', state: 'incoming', displayName: 'CAROL', note: 'Hi, it is Carol', since: 1000 },
    ] },
    { leaseId: '1:blocks:', rows: [{ uid: 'dave', at: 9000, displayName: 'Dave', handle: 'dave' }, { uid: 'erin', at: 2000, displayName: '' }] },
  ]);
});

test('a watch that fails is reported once and is over; rows after it are not sent', async context => {
  const { service, listeners, rows, failed } = fixture(context);
  await service.watch('1:cards:', 'alice', 'friends');
  listeners[0]!.error(new Error('permission-denied'));
  assert.deepEqual(failed, [{ leaseId: '1:cards:', code: 'SERVICE_UNAVAILABLE' }]);
  assert.equal(listeners[0]!.stopped, true);
  listeners[0]!.next({ docs: [card('bob')] }); listeners[0]!.error(new Error('again'));
  assert.deepEqual(rows, []); assert.equal(failed.length, 1);
  await service.watch('2:cards:', 'alice', 'friends');
  assert.equal(listeners.length, 2, 'the renderer starts a new one under a new lease');
});

test('leases are released by prefix, and a lease released before the session opened never listens', async context => {
  const { service, listeners, rows, openingSession, sdk } = fixture(context);
  await service.watch('1:a:', 'alice', 'friends');
  await service.watch('1:b:', 'alice', 'blocks');
  await service.watch('2:a:', 'alice', 'friends');
  service.releaseWatches('1:');
  assert.deepEqual(listeners.map(listener => listener.stopped), [true, true, false], 'a window\'s watches go together');
  listeners[0]!.next({ docs: [card('bob')] }); listeners[2]!.next({ docs: [card('carol')] });
  assert.deepEqual(rows.map(row => row.leaseId), ['2:a:'], 'only the lease that is left');

  let open!: (value: { db: unknown; sdk: typeof sdk }) => void;
  openingSession(new Promise(resolve => { open = resolve; }));
  const pending = service.watch('3:a:', 'alice', 'friends');
  service.releaseWatches('3:');
  open({ db: {}, sdk }); await pending;
  assert.equal(listeners.length, 3, 'the released lease started nothing');
});

test('another account ends every watch and tells each lease, so no rows of the first account reach the second', async context => {
  const { service, listeners, rows, failed, signIn } = fixture(context);
  await service.watch('1:cards:', 'alice', 'friends');
  await service.watch('1:blocks:', 'alice', 'blocks');
  signIn('bob');
  assert.deepEqual(listeners.map(listener => listener.stopped), [true, true]);
  assert.deepEqual(failed.map(event => event.code), ['INVALID_SESSION', 'INVALID_SESSION']);
  assert.deepEqual(failed.map(event => event.leaseId).sort(), ['1:blocks:', '1:cards:']);
  listeners[0]!.next({ docs: [card('stale')] });
  assert.deepEqual(rows, []);
  await assert.rejects(service.watch('2:cards:', 'alice', 'friends'), /INVALID_SESSION/u, 'the old account cannot watch');
  await service.watch('2:cards:', 'bob', 'friends');
  assert.equal(listeners.at(-1)!.path, 'users/bob/friends');
  signIn(undefined);
  assert.equal(listeners.at(-1)!.stopped, true, 'signing out ends the watch too');
  assert.deepEqual(failed.at(-1), { leaseId: '2:cards:', code: 'INVALID_SESSION' });
});

test('a watch whose account changed while the session was opening starts nothing', async context => {
  const { service, listeners, signIn, openingSession, sdk } = fixture(context);
  let open!: (value: { db: unknown; sdk: typeof sdk }) => void;
  openingSession(new Promise(resolve => { open = resolve; }));
  const pending = service.watch('1:cards:', 'alice', 'friends');
  signIn('bob');
  open({ db: {}, sdk }); await pending;
  assert.deepEqual(listeners, []);
});

test('a friend\'s profile is read once and only its name and username leave the main process', async context => {
  const { service, profiles, reads } = fixture(context);
  profiles.set('profiles/bob', { displayName: 'Bob B', handle: 'bobby', email: 'bob@example.com', findByEmail: true });
  profiles.set('profiles/nameless', { findByEmail: true });
  assert.deepEqual(await service.readProfile('alice', 'bob'), { displayName: 'Bob B', handle: 'bobby' });
  assert.deepEqual(await service.readProfile('alice', 'nameless'), {});
  assert.equal(await service.readProfile('alice', 'missing'), null);
  for (const friend of ['', 'a/b', 'a b', '../x', 'x'.repeat(129)]) assert.equal(await service.readProfile('alice', friend), null, friend);
  assert.deepEqual(reads, ['profiles/bob', 'profiles/nameless', 'profiles/missing'], 'a malformed uid is never a path');
  await assert.rejects(service.readProfile('bob', 'bob'), /INVALID_SESSION/u);
});

test('closing ends the watches and stops following the account', async context => {
  const { service, listeners, failed } = fixture(context);
  await service.watch('1:cards:', 'alice', 'friends');
  service.close();
  assert.equal(listeners[0]!.stopped, true);
  assert.deepEqual(failed, [{ leaseId: '1:cards:', code: 'INVALID_SESSION' }], 'the watch is told before the listeners are dropped');
  assert.doesNotThrow(() => service.close());
});
