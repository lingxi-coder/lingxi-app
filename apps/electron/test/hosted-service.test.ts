import test from 'node:test';
import assert from 'node:assert/strict';
import { createServiceChatReader, isServiceId, newServiceId, servicesTransport } from '../../../packages/teams/hosted-service';
import { TeamsApiError } from '../../../packages/teams';

const serviceId = 's_service_0001', conversationId = 'session001';
const scope = { teamId: serviceId, generation: 1, conversationId };
const pad = (n: number) => String(n).padStart(6, '0');
/** A session as the API's `sync` serves it: a change log and the current text of each message. */
function session() {
  const messages = new Map<string, Record<string, unknown>>(), changes: Array<{ seq: number; messageId: string }> = [], calls: any[] = [];
  let lastSeq = 0;
  const change = (messageId: string) => { changes.push({ seq: changes.length + 1, messageId }); return changes.length; };
  const api = {
    calls, changes, messages,
    say(text: string, bot = false) {
      const messageId = `message${pad(++lastSeq)}`, changeSeq = change(messageId);
      messages.set(messageId, { id: messageId, seq: lastSeq, changeSeq, changeId: `change${pad(changeSeq)}`, kind: 'text', text,
        author: bot ? { kind: 'shared_bot', sharedBotId: 'sharedbot001', displayName: 'Helper' } : { kind: 'user', uid: 'caller', displayName: 'Caller' },
        createdAt: new Date(1700000000000 + lastSeq).toISOString(), editedAt: null });
      return messageId;
    },
    edit(messageId: string, text: string) {
      const changeSeq = change(messageId);
      messages.set(messageId, { ...messages.get(messageId)!, text, changeSeq, changeId: `change${pad(changeSeq)}`, editedAt: new Date(1700000100000).toISOString() });
    },
    call: async (command: Record<string, unknown>) => {
      calls.push(command); const after = command.after as number, page = changes.filter(c => c.seq > after).slice(0, 100);
      return { conversation: { id: conversationId, lastSeq, lastChangeSeq: changes.length }, changes: page.map(c => ({ id: `change${pad(c.seq)}`, ...c })),
        messages: [...new Set(page.map(c => c.messageId))].map(id => messages.get(id)), hasMore: (page.at(-1)?.seq ?? after) < changes.length };
    },
  };
  return api;
}

test('a service id is its own id space', () => {
  assert.ok(isServiceId(newServiceId()));
  assert.ok(isServiceId('s_service_0001'));
  for (const value of ['team00001', 'f_pair_0001', 's_short', 'S_service_0001', 's_bad/id_0001', undefined, 7]) assert.equal(isServiceId(value), false);
});

test('a session is read through the API with the Team reader\'s requests and answers', async () => {
  const s = session(), reader = createServiceChatReader(s.call);
  const first = s.say('What should we pack?'); s.say('Passports.', true); s.say('And for the beach?');
  const recent = await reader.read({ ...scope, mode: 'recent', throughSeq: 3 });
  assert.deepEqual(recent.messages.map(m => [m.seq, m.author.kind, m.text]), [[1, 'user', 'What should we pack?'], [2, 'shared_bot', 'Passports.'], [3, 'user', 'And for the beach?']]);
  assert.deepEqual([recent.lastSeq, recent.lastChangeSeq, recent.hasMore], [3, 3, false]);
  assert.equal(recent.messages[0]!.createdAt, 1700000000001);
  // Every call names the service, never a Team.
  assert.ok(s.calls.every(c => c.action === 'sync' && c.serviceId === serviceId && c.generation === 1 && !('teamId' in c)));
  assert.deepEqual((await reader.read({ ...scope, mode: 'range', fromSeqExclusive: 1, toSeqInclusive: 3 })).messages.map(m => m.seq), [2, 3]);
  assert.deepEqual((await reader.read({ ...scope, mode: 'byId', messageId: first })).messages.map(m => m.text), ['What should we pack?']);
  assert.deepEqual((await reader.read({ ...scope, mode: 'byId', messageId: 'message999999' })).messages, []);
  const before = await reader.read({ ...scope, mode: 'before', beforeSeq: 3, limit: 1 });
  assert.deepEqual([before.messages.map(m => m.seq), before.hasMore], [[2], true]);
  // An edit is a change of an older message: the change read finds it, with its new text and time.
  s.edit(first, 'What should we pack for Lisbon?');
  const changed = await reader.read({ ...scope, mode: 'changes', afterChangeSeq: 3, throughMessageSeq: 3 });
  assert.deepEqual(changed.messages.map(m => [m.seq, m.text, m.editedAt]), [[1, 'What should we pack for Lisbon?', 1700000100000]]);
  assert.equal(changed.lastChangeSeq, 4);
  // What was followed is kept: a later read asks only for what came after it.
  assert.equal(s.calls.at(-1).after, 3);
  await assert.rejects(reader.read({ ...scope, mode: 'recent', throughSeq: 9 }), { message: 'INVALID_READ_CURSOR' });
  await assert.rejects(reader.read({ ...scope, mode: 'changes', afterChangeSeq: 9, throughMessageSeq: 3 }), { message: 'INVALID_READ_CURSOR' });
});

test('a long session is followed page by page, and concurrent reads of it do not interleave', async () => {
  const s = session(), reader = createServiceChatReader(s.call);
  for (let i = 0; i < 250; i++) s.say(`message ${i}`);
  const [recent, range] = await Promise.all([reader.read({ ...scope, mode: 'recent', throughSeq: 250 }), reader.read({ ...scope, mode: 'range', fromSeqExclusive: 0, toSeqInclusive: 250 })]);
  assert.equal(recent.messages.length, 200); assert.equal(recent.hasMore, true); assert.equal(range.messages.length, 250);
  // Three pages for the first read; the second found everything followed and asked once for news.
  assert.deepEqual(s.calls.map(c => c.after), [0, 100, 200, 250]);
});

test('a session whose log or messages do not add up is refused, and read again from the start', async () => {
  const s = session(); s.say('one'); s.say('two');
  let broken: 'gap' | 'missing' | 'author' | undefined = 'gap';
  const reader = createServiceChatReader(async command => {
    const page = await s.call(command) as any;
    if (broken === 'gap') page.changes = page.changes.slice(1);
    if (broken === 'missing') page.messages = page.messages.slice(1);
    if (broken === 'author') page.messages[1] = { ...page.messages[1], author: { kind: 'system' } };
    return page;
  });
  await assert.rejects(reader.read({ ...scope, mode: 'recent', throughSeq: 2 }), { message: 'CHANGE_GAP' });
  broken = 'missing'; await assert.rejects(reader.read({ ...scope, mode: 'recent', throughSeq: 2 }), { message: 'MESSAGE_GAP' });
  broken = 'author'; await assert.rejects(reader.read({ ...scope, mode: 'recent', throughSeq: 2 }), { message: 'INVALID_DATA' });
  broken = undefined;
  assert.equal((await reader.read({ ...scope, mode: 'recent', throughSeq: 2 })).messages.length, 2);
  assert.equal(s.calls.at(-1).after, 0);
  // The API's refusal reaches the Bot's host as it is.
  const refused = createServiceChatReader(async () => { throw new TeamsApiError('SERVICE_WITHDRAWN', 410); });
  await assert.rejects(refused.read({ ...scope, mode: 'recent', throughSeq: 1 }), { code: 'SERVICE_WITHDRAWN' });
});

test('service commands go to /v1/services with the account token', async () => {
  const sent: Array<{ url: string; init: RequestInit }> = [];
  const services = servicesTransport('https://api.example', async () => 'token', (async (url: string, init: RequestInit) => {
    sent.push({ url, init });
    return JSON.parse(init.body as string).action === 'getService' ? new Response(JSON.stringify({ serviceId, availability: 'available' }))
      : new Response(JSON.stringify({ error: { code: 'PUBLISHER_REQUIRED' } }), { status: 403 });
  }) as typeof fetch);
  assert.deepEqual(await services({ action: 'getService', serviceId }), { serviceId, availability: 'available' });
  assert.equal(sent[0]!.url, 'https://api.example/v1/services');
  assert.equal((sent[0]!.init.headers as Record<string, string>).Authorization, 'Bearer token');
  await assert.rejects(services({ action: 'withdrawService', serviceId }), { code: 'PUBLISHER_REQUIRED', status: 403 });
});
