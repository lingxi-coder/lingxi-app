import assert from 'node:assert/strict';
import test from 'node:test';
import { RemoteRpcError } from '@lingxi/bridge-client/remote';
import { knownBots } from '../src/remote/remoteClient';
import { BotHost, announce, answered, bot, open, quiet, reconnect, thread } from './support/botHost';
import { harness, settle, waitFor } from './support/remoteHarness';
import { drive } from './support/botHost';

// ─────────────────────────────────────────────────────────────────────────────
// Negotiation
// ─────────────────────────────────────────────────────────────────────────────

test('a desktop that has Bots is asked for them next to the sessions, and what it sends is cleaned', async () => {
  const h = await harness();
  const host = new BotHost(h.host);
  host.sessions = [{ session_id: 's1', title: 'Fix', project_label: 'app', status: 'idle', viewers: 0 }];
  host.bots = [
    bot('a', { name: 'Re‮search\ner', description: 'Reads​ the\u0007 papers', waiting_threads: 'many' as never }),
    bot('b', { name: 'x'.repeat(100), description: 'd'.repeat(300), last_activity_at: 12 }),
    null as never,
    { name: 'no id' } as never,
  ];
  const { client } = await open(h);
  await answered(h, client);
  await drive(h.clock, () => client.getState().sessions !== undefined, 'the sessions');

  assert.equal(host.requests('bots.watch').length, 1);
  assert.equal(host.requests('sessions.watch').length, 1);
  assert.deepEqual(host.botRequests(), ['bots.watch'], 'one question, and no chats asked for before a page shows them');
  const [a, b, ...others] = knownBots(client.getState());
  assert.equal(others.length, 0, 'an entry without an id is not a Bot');
  assert.equal(a!.name, 'Research er');
  assert.equal(a!.description, 'Reads the papers');
  assert.equal(a!.waiting_threads, 0);
  assert.equal(Array.from(b!.name).length, 60);
  assert.ok(b!.name.endsWith('…'));
  assert.equal(Array.from(b!.description).length, 160);
  assert.equal(b!.last_activity_at, 12);
  assert.equal(client.getState().sessions!.length, 1, 'the sessions are what they were');
});

test('bots.changed replaces the list, in the order the desktop sends it', async () => {
  const h = await harness();
  const host = new BotHost(h.host);
  host.bots = [bot('a'), bot('b')];
  const { client } = await open(h);
  await answered(h, client);
  assert.deepEqual(knownBots(client.getState()).map((entry) => entry.bot_id), ['a', 'b']);

  host.bots = [bot('c', { name: 'Zed' }), bot('b', { status: 'running' })];
  await announce(h, client, host);
  assert.deepEqual(knownBots(client.getState()).map((entry) => [entry.bot_id, entry.status]), [['c', 'idle'], ['b', 'running']]);

  host.bots = [];
  await announce(h, client, host);
  assert.deepEqual(client.getState().bots, [], 'a desktop with no Bot to show is not one that does not know Bots');
});

test('a desktop from before Bots answers unknown method: Bots are unsupported, and nothing more about them is asked', async () => {
  const h = await harness();
  const host = new BotHost(h.host);
  host.supported = false;
  host.sessions = [{ session_id: 's1', title: 'Fix', project_label: 'app', status: 'idle', viewers: 0 }];
  const { client } = await open(h);
  await answered(h, client);
  await drive(h.clock, () => client.getState().sessions !== undefined, 'the sessions');

  assert.equal(client.getState().bots, 'unsupported');
  assert.deepEqual(knownBots(client.getState()), []);
  assert.deepEqual(client.getState().sessions!.map((session) => session.session_id), ['s1'], 'the sessions are what they were');

  // Whatever a page might do next, none of it reaches the desktop.
  const store = client.botThreads('a');
  store.activate();
  store.refresh();
  store.connectionReady();
  await assert.rejects(client.createBotThread('a'), (error: unknown) => error instanceof RemoteRpcError && error.code === 'unknown_session');
  await quiet(h);
  assert.deepEqual(host.botRequests(), ['bots.watch']);
  assert.equal(store.getState().threads, undefined);
});

test('only the unknown-method answer means unsupported: any other failure leaves the list as it was', async () => {
  const h = await harness();
  const host = new BotHost(h.host);
  host.failWith.set('bots.watch', new RemoteRpcError('internal', 'internal error'));
  const { client } = await open(h);
  await drive(h.clock, () => host.requests('bots.watch').length === 1, 'bots.watch');
  await quiet(h);
  assert.equal(client.getState().bots, undefined);

  // `invalid_params` for another reason is not the answer of a desktop that has never heard of the method either.
  const second = await harness();
  const other = new BotHost(second.host);
  other.failWith.set('bots.watch', new RemoteRpcError('invalid_params', 'bad params'));
  const again = await open(second);
  await drive(second.clock, () => other.requests('bots.watch').length === 1, 'bots.watch');
  await quiet(second);
  assert.equal(again.client.getState().bots, undefined);
});

test('every connection asks again, so a desktop that was updated, or rolled back, in between is met as it is', async () => {
  const h = await harness();
  const host = new BotHost(h.host);
  host.supported = false;
  const { connection, client } = await open(h);
  await answered(h, client);
  assert.equal(client.getState().bots, 'unsupported');

  host.supported = true;
  host.bots = [bot('a')];
  await reconnect(h, connection);
  await drive(h.clock, () => Array.isArray(client.getState().bots), 'the Bots of the updated desktop');
  assert.deepEqual(knownBots(client.getState()).map((entry) => entry.bot_id), ['a']);
  assert.equal(host.requests('bots.watch').length, 2);

  host.supported = false;
  await reconnect(h, connection);
  await drive(h.clock, () => client.getState().bots === 'unsupported', 'the rolled-back desktop');
  assert.equal(host.requests('bots.watch').length, 3);
});

// ─────────────────────────────────────────────────────────────────────────────
// A Bot's chats
// ─────────────────────────────────────────────────────────────────────────────

test('a Bot\'s chats are listed when a page shows them, and again when the Bot, or the connection, changed', async () => {
  const h = await harness();
  const host = new BotHost(h.host);
  host.bots = [bot('a', { last_activity_at: 1 }), bot('b')];
  host.threads.set('a', [thread('t2', 'a', { title: 'Newer‮ one' }), thread('t1', 'a')]);
  const { connection, client } = await open(h);
  await answered(h, client);
  const listings = (): number => host.requests('bot.threads.list').length;
  const store = client.botThreads('a');
  assert.equal(client.botThreads('a'), store, 'one store per Bot');

  // Nobody shows the chats yet: nothing is asked.
  host.bots = [bot('a', { last_activity_at: 2 }), bot('b')];
  await announce(h, client, host);
  assert.equal(listings(), 0);

  store.activate();
  await drive(h.clock, () => store.getState().threads !== undefined, 'the chats');
  await quiet(h);
  assert.equal(listings(), 1);
  assert.deepEqual(host.requests('bot.threads.list')[0]!.params, { bot_id: 'a' });
  assert.deepEqual(store.getState().threads!.map((chat) => [chat.session_id, chat.title]), [['t2', 'Newer one'], ['t1', 'Chat t1']], 'in the desktop\'s order, and cleaned like any session');
  assert.equal(client.botOfThread('t1'), 'a');
  assert.equal(client.botOfThread('t2'), 'a');
  assert.equal(client.botOfThread('nobody'), undefined);

  // Changes that say nothing about the chats of this Bot.
  await announce(h, client, host);
  host.bots = [bot('a', { last_activity_at: 2, name: 'Renamed', description: 'does other things' }), bot('b')];
  await announce(h, client, host);
  host.bots = [bot('a', { last_activity_at: 2, name: 'Renamed', description: 'does other things' }), bot('b', { last_activity_at: 90, status: 'running', waiting_threads: 3 })];
  await announce(h, client, host);
  assert.equal(listings(), 1, 'the same list, a new name, and another Bot\'s activity change nothing here');

  // Each of the three things that do.
  host.bots = [bot('a', { last_activity_at: 3 }), bot('b')];
  await announce(h, client, host);
  assert.equal(listings(), 2, 'activity');
  host.bots = [bot('a', { last_activity_at: 3, status: 'running' }), bot('b')];
  await announce(h, client, host);
  assert.equal(listings(), 3, 'status');
  host.bots = [bot('a', { last_activity_at: 3, status: 'running', waiting_threads: 1 }), bot('b')];
  await announce(h, client, host);
  assert.equal(listings(), 4, 'waiting chats');

  // A reconnect lists them once more; the answer about the Bots that follows it adds nothing.
  await reconnect(h, connection);
  await drive(h.clock, () => listings() === 5, 'the listing after the reconnect');
  await quiet(h);
  assert.equal(listings(), 5);
  assert.equal(host.requests('bots.watch').length, 2);

  // A page that left is not served: nothing is asked for it any more.
  store.deactivate();
  host.bots = [bot('a', { last_activity_at: 4 }), bot('b')];
  await announce(h, client, host);
  await reconnect(h, connection);
  await quiet(h);
  assert.equal(listings(), 5);
});

test('a Bot that was gone and is back has its chats listed again; one that is gone is not asked about', async () => {
  const h = await harness();
  const host = new BotHost(h.host);
  host.bots = [bot('a')];
  host.threads.set('a', [thread('t1', 'a')]);
  const { client } = await open(h);
  await answered(h, client);
  const store = client.botThreads('a');
  store.activate();
  await drive(h.clock, () => store.getState().threads !== undefined, 'the chats');
  await quiet(h);
  assert.equal(host.requests('bot.threads.list').length, 1);

  host.bots = [];
  await announce(h, client, host);
  assert.equal(host.requests('bot.threads.list').length, 1);
  assert.deepEqual(knownBots(client.getState()), []);

  host.bots = [bot('a')];
  await announce(h, client, host);
  assert.equal(host.requests('bot.threads.list').length, 2);
});

test('a change that arrives while the chats are being listed gets one more listing, not one each', async () => {
  const h = await harness();
  const host = new BotHost(h.host);
  host.bots = [bot('a')];
  host.threads.set('a', [thread('t1', 'a')]);
  const { client } = await open(h);
  await answered(h, client);
  let release!: () => void;
  host.holdThreads = new Promise<void>((resolve) => { release = resolve; });
  const store = client.botThreads('a');
  store.activate();
  await drive(h.clock, () => host.requests('bot.threads.list').length === 1, 'the first listing');

  host.bots = [bot('a', { last_activity_at: 1 })];
  await announce(h, client, host);
  host.bots = [bot('a', { last_activity_at: 2 })];
  await announce(h, client, host);
  store.refresh();
  assert.equal(host.requests('bot.threads.list').length, 1, 'one listing at a time');

  host.holdThreads = undefined;
  release();
  await drive(h.clock, () => host.requests('bot.threads.list').length === 2, 'the listing the changes asked for');
  await quiet(h);
  assert.equal(host.requests('bot.threads.list').length, 2);
  assert.deepEqual(store.getState().threads!.map((chat) => chat.session_id), ['t1']);
});

test('a listing that fails is shown as such and does not stop the next one; a dropped link is not a failure', async () => {
  const h = await harness();
  const host = new BotHost(h.host);
  host.bots = [bot('a')];
  host.threads.set('a', [thread('t1', 'a')]);
  host.failWith.set('bot.threads.list', new RemoteRpcError('internal', 'the chats could not be listed'));
  const { connection, client } = await open(h);
  await answered(h, client);
  const store = client.botThreads('a');
  store.activate();
  await drive(h.clock, () => store.getState().error !== undefined, 'the failure');
  assert.equal(store.getState().error!.code, 'internal');
  assert.equal(store.getState().threads, undefined);

  host.failWith.clear();
  store.refresh();
  await drive(h.clock, () => store.getState().threads !== undefined, 'the chats');
  assert.equal(store.getState().error, undefined);

  // The link dies under a listing: that is no error to show, and the reconnect asks again.
  host.holdThreads = new Promise<void>(() => undefined);
  store.refresh();
  await drive(h.clock, () => host.requests('bot.threads.list').length === 3, 'the listing that never answers');
  host.holdThreads = undefined;
  await reconnect(h, connection);
  await drive(h.clock, () => host.requests('bot.threads.list').length === 4, 'the listing after the reconnect');
  await quiet(h);
  assert.equal(store.getState().error, undefined);
});

// ─────────────────────────────────────────────────────────────────────────────
// Starting a chat
// ─────────────────────────────────────────────────────────────────────────────

test('a new chat is asked for with a fresh request id, and the Bot it belongs to is remembered', async () => {
  const h = await harness();
  const host = new BotHost(h.host);
  host.bots = [bot('a')];
  const { client } = await open(h);
  await answered(h, client);

  const first = await client.createBotThread('a');
  const second = await client.createBotThread('a');
  const creates = host.requests('bot.thread.create');
  assert.equal(creates.length, 2);
  for (const create of creates) {
    assert.deepEqual(Object.keys(create.params).sort(), ['bot_id', 'client_request_id']);
    assert.equal(create.params.bot_id, 'a');
    assert.ok(typeof create.params.client_request_id === 'string' && create.params.client_request_id.length > 0);
  }
  assert.notEqual(creates[0]!.params.client_request_id, creates[1]!.params.client_request_id);
  assert.notEqual(first, second);
  assert.equal(client.botOfThread(first), 'a');
  assert.equal(client.botOfThread(second), 'a');
});

test('a link that drops with the request on it gets one more request with the same id, and the same chat', async () => {
  const h = await harness();
  const host = new BotHost(h.host);
  host.bots = [bot('a')];
  const { client } = await open(h);
  await answered(h, client);

  host.dropping = { method: 'bot.thread.create', times: 1 };
  let created: string | undefined;
  const pending = client.createBotThread('a').then((sessionId) => { created = sessionId; });
  await drive(h.clock, () => created !== undefined, 'the chat');
  await pending;

  const [first, second] = host.requests('bot.thread.create');
  assert.equal(first!.params.client_request_id, second!.params.client_request_id);
  assert.notEqual(first!.connId, second!.connId, 'the second went over the new link');
  assert.equal(host.created.size, 1, 'the desktop made one chat');
  assert.equal(created, 'chat-1');
  assert.equal(client.botOfThread('chat-1'), 'a');
});

test('the request is repeated once, not again and again', async () => {
  const h = await harness();
  const host = new BotHost(h.host);
  host.bots = [bot('a')];
  const { client } = await open(h);
  await answered(h, client);

  host.dropping = { method: 'bot.thread.create', times: Infinity }; // every link it goes over dies under it
  let failure: unknown;
  const pending = client.createBotThread('a').then(() => undefined, (error: unknown) => { failure = error; });
  await drive(h.clock, () => failure !== undefined, 'the failure');
  await pending;
  assert.ok(failure instanceof RemoteRpcError && failure.code === 'closed');
  assert.equal(host.requests('bot.thread.create').length, 2);
  for (let second = 0; second < 120; second++) {
    await settle();
    h.clock.advance(1000);
  }
  assert.equal(host.requests('bot.thread.create').length, 2);
});

test('a link that does not come back in time ends the wait with an error, and nothing more is sent', async () => {
  const h = await harness();
  const host = new BotHost(h.host);
  host.bots = [bot('a')];
  const { connection, client } = await open(h);
  await answered(h, client);

  host.dropping = { method: 'bot.thread.create', times: 1 };
  h.host.mode = 'silent'; // the desktop does not answer the viewer's next attempts to dial
  const started = h.clock.now();
  let failure: unknown;
  const pending = client.createBotThread('a').then(() => undefined, (error: unknown) => { failure = error; });
  await waitFor(() => connection.state.phase === 'reconnecting', 'the dropped link');
  h.clock.advance(30_000);
  await waitFor(() => failure !== undefined, 'the reconnect wait deadline');
  await pending;
  assert.ok(failure instanceof RemoteRpcError && failure.code === 'closed');
  assert.ok(h.clock.now() - started >= 30_000, 'it waited for the link before it gave up');
  assert.notEqual(connection.state.phase, 'ready');
  assert.equal(host.requests('bot.thread.create').length, 1);
});

test('what the desktop refuses surfaces as it came, with no second request', async () => {
  const h = await harness();
  const host = new BotHost(h.host);
  host.bots = [bot('a')];
  const { client } = await open(h);
  await answered(h, client);

  for (const code of ['not_remote_enabled', 'unknown_session', 'internal', 'invalid_params'] as const) {
    host.failWith.set('bot.thread.create', new RemoteRpcError(code, 'refused'));
    const before = host.requests('bot.thread.create').length;
    await assert.rejects(client.createBotThread('a'), (error: unknown) => error instanceof RemoteRpcError && error.code === code);
    assert.equal(host.requests('bot.thread.create').length, before + 1, code);
  }
  assert.equal(client.botOfThread('chat-1'), undefined);

  host.failWith.clear();
  assert.equal(await client.createBotThread('a'), 'chat-1', 'the next try works');
});

test('a chat of the desktop that has none to give is not made up', async () => {
  const h = await harness();
  const host = new BotHost(h.host);
  host.bots = [bot('a')];
  const { client } = await open(h);
  await answered(h, client);
  h.host.requestHandler = async (method) => (method === 'bot.thread.create' ? { session_id: '' } : { bots: host.bots });
  await assert.rejects(client.createBotThread('a'), (error: unknown) => error instanceof RemoteRpcError && error.code === 'internal');
  assert.equal(client.botOfThread(''), undefined);
});
