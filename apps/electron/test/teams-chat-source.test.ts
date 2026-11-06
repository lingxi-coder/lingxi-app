import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createChatSourceReader, chatSourceMessage, type ChatReadRequest } from '../../../packages/teams/chat-source.js';

function fixture(count = 650) {
  const messages = Array.from({ length: count }, (_, index) => ({ id: `message_${index + 1}`, value: {
    seq: index + 1, changeSeq: index + 1, changeId: `change_${index + 1}`, text: 'x'.repeat(1024),
    author: { kind: 'user', uid: index % 2 ? 'alice' : 'bob', displayName: 'Same display name' }, createdAt: { toMillis: () => 1000 + index },
  } }));
  const chat = { memberUids: ['owner'], lastSeq: count, lastChangeSeq: count };
  const changes = messages.map(message => ({ id: message.value.changeId, value: { seq: message.value.changeSeq, messageId: message.id } }));
  let current = true, generation = 1, networkQueries = 0, evicted = false, serverGap = false, authorizations = 0;
  let afterPage: (() => void) | undefined;
  type Constraint = { key: string; args: unknown[] };
  type MockQuery = { path: string; constraints: Constraint[] };
  const sdk = {
    doc(_db: unknown, ...path: string[]) { return path.join('/'); },
    collection(_db: unknown, ...path: string[]) { return path.join('/'); },
    where(...args: unknown[]) { return { key: 'where', args }; },
    orderBy(...args: unknown[]) { return { key: 'orderBy', args }; },
    limit(...args: unknown[]) { return { key: 'limit', args }; },
    query(path: string, ...constraints: Constraint[]) { return { path, constraints }; },
    async getDocFromServer(path: string) {
      authorizations++;
      const value = path.endsWith('/members/owner') ? { generation } : path.includes('/messages/')
        ? messages.find(message => path.endsWith(`/${message.id}`))?.value : chat;
      const captured = value && { ...value };
      return { data: () => captured, exists: () => !!captured, id: path.split('/').at(-1) };
    },
    async getDocsFromServer(query: MockQuery) {
      networkQueries++; const result = page(query, serverGap), callback = afterPage;
      afterPage = undefined; callback?.(); return result;
    },
    async getDocsFromCache(query: MockQuery) { return page(query, evicted); },
  };
  function page(query: MockQuery, gap: boolean) {
    let values = (query.path.endsWith('/changes') ? changes : messages).filter(message => query.constraints.filter(c => c.key === 'where').every(c => {
      const [field, operator, operand] = c.args, value = message.value[field as 'seq'];
      return operator === '>' ? value > Number(operand) : operator === '<' ? value < Number(operand) : value <= Number(operand);
    }));
    const order = query.constraints.find(c => c.key === 'orderBy'); if (order?.args[1] === 'desc') values = values.toReversed();
    const size = Number(query.constraints.find(c => c.key === 'limit')?.args[0] ?? values.length);
    values = values.slice(0, size); if (gap) values = values.filter(value => value.value.seq % 2);
    const docs = values.map(value => { const captured = { ...value.value }; return { id: value.id, data: () => captured }; });
    return { size: docs.length, empty: !docs.length, docs };
  }
  const reader = createChatSourceReader(sdk as unknown as typeof import('firebase/firestore'), {} as import('firebase/firestore').Firestore, 'owner', () => current);
  const scope = { teamId: 'team_0001', generation: 1, conversationId: 'chat_0001' };
  return { reader, messages, scope, chat,
    state: () => ({ networkQueries, authorizations }), signOut() { current = false; }, revoke() { generation++; },
    evict() { evicted = true; }, createGap() { serverGap = true; },
    afterNextPage(callback: () => void) { afterPage = callback; },
    append() {
      const seq = ++chat.lastSeq, changeSeq = ++chat.lastChangeSeq;
      messages.push({ id: `message_${seq}`, value: { ...messages.at(-1)!.value, seq, changeSeq, changeId: `change_${changeSeq}`,
        createdAt: { toMillis: () => 1000 + seq }, text: 'A later message' } });
      changes.push({ id: `change_${changeSeq}`, value: { seq: changeSeq, messageId: `message_${seq}` } });
    },
    edit(seq: number, text: string, recalled = false) {
      const message = messages[seq - 1]!; chat.lastChangeSeq++;
      Object.assign(message.value, { text, recalled, editedAt: { toMillis: () => 3000 }, changeSeq: chat.lastChangeSeq, changeId: `edit_${chat.lastChangeSeq}` });
      changes.push({ id: `edit_${chat.lastChangeSeq}`, value: { seq: chat.lastChangeSeq, messageId: message.id } });
    } };
}
test('shared Chat source: recent 200 and 450-message delta preserve authors and source times without truncation', async () => {
  const f = fixture();
  const recent = await f.reader.read({ ...f.scope, mode: 'recent', throughSeq: 650 });
  assert.equal(recent.messages.length, 200); assert.equal(recent.messages[0]!.seq, 451); assert.equal(recent.messages.at(-1)!.createdAt, 1649);
  const delta = await f.reader.read({ ...f.scope, mode: 'range', fromSeqExclusive: 200, toSeqInclusive: 650 });
  assert.equal(delta.messages.length, 450); assert.ok(Buffer.byteLength(JSON.stringify(delta)) > 450 * 1024);
  assert.notDeepEqual(delta.messages[0]!.author, delta.messages[1]!.author);
  assert.equal(f.state().networkQueries, 4); f.reader.close();
});
test('shared Chat source: two Bots share in-flight work and SDK cache, with independent returned arrays', async () => {
  const f = fixture(); const request: ChatReadRequest = { ...f.scope, mode: 'range', fromSeqExclusive: 200, toSeqInclusive: 650 };
  const [first, second] = await Promise.all([f.reader.read(request), f.reader.read(request)]);
  assert.deepEqual(first, second); assert.notEqual(first.messages, second.messages); assert.equal(f.state().networkQueries, 3);
  first.messages.splice(0, 1); assert.equal(second.messages.length, 450);
  await f.reader.read(request); assert.equal(f.state().networkQueries, 3); assert.equal(f.state().authorizations, 8);
  f.reader.close();
});
test('shared Chat source: eviction refills the same SDK and a server gap never advances a consumer', async () => {
  const f = fixture(), request: ChatReadRequest = { ...f.scope, mode: 'range', fromSeqExclusive: 200, toSeqInclusive: 650 };
  await f.reader.read(request); f.evict();
  assert.equal((await f.reader.read(request)).messages.length, 450); assert.equal(f.state().networkQueries, 6);
  f.createGap(); await assert.rejects(f.reader.read(request), /MESSAGE_GAP/u); f.reader.close();
});
test('shared Chat source: history pages and exact ID lookup do not track or modify Bot cursors', async () => {
  const f = fixture();
  const first = await f.reader.read({ ...f.scope, mode: 'before', beforeSeq: 451 });
  assert.equal(first.messages[0]!.seq, 251); assert.equal(first.hasMore, true);
  const second = await f.reader.read({ ...f.scope, mode: 'before', beforeSeq: 251 });
  assert.equal(second.messages[0]!.seq, 51);
  const last = await f.reader.read({ ...f.scope, mode: 'before', beforeSeq: 51 });
  assert.equal(last.messages.length, 50); assert.equal(last.hasMore, false);
  assert.equal((await f.reader.read({ ...f.scope, mode: 'byId', messageId: 'message_17' })).messages[0]!.seq, 17);
  f.reader.close();
});
test('shared Chat source: revocation is rechecked before every cache read; account switch and closed ports reject', async () => {
  const f = fixture(), request: ChatReadRequest = { ...f.scope, mode: 'recent', throughSeq: 650 };
  await f.reader.read(request); f.revoke(); await assert.rejects(f.reader.read(request), /STALE_MEMBERSHIP/u);
  assert.equal(f.state().networkQueries, 1); f.signOut(); assert.throws(() => f.reader.read(request), /INVALID_SESSION/u); f.reader.close();
  const fresh = fixture(); fresh.reader.close(); assert.throws(() => fresh.reader.read(request), /INVALID_SESSION/u);
});
test('shared Chat source: original timestamps are mandatory, same-named authors retain IDs, and old Bot principals fail', () => {
  const f = fixture(), m = f.messages[0]!;
  assert.equal(chatSourceMessage(m.id, m.value).createdAt, 1000);
  assert.throws(() => chatSourceMessage(m.id, { ...m.value, createdAt: undefined }), /INVALID_SOURCE_TIME/u);
  assert.throws(() => chatSourceMessage(m.id, { ...m.value, author: { kind: 'bot', uid: 'owner', botKey: 'old' } }), /INVALID_DATA/u);
  assert.throws(() => chatSourceMessage(m.id, { ...m.value, recalled: true }), /INVALID_DATA/u);
  const human = chatSourceMessage(m.id, { ...m.value, author: { kind: 'user', uid: 'retained_human' } });
  assert.equal(human.author.displayName, 'retained_human'); f.reader.close();
});
test('shared Chat source: independent change watermark returns old edits and blank recalls without consuming future messages', async () => {
  const f = fixture(); f.edit(1, 'Corrected old detail'); f.edit(2, '', true); f.edit(600, 'Future for this task');
  const changed = await f.reader.read({ ...f.scope, mode: 'changes', afterChangeSeq: 650, throughMessageSeq: 300 });
  assert.deepEqual(changed.messages.map(message => message.seq), [1, 2]); assert.equal(changed.lastChangeSeq, 653);
  assert.equal(changed.messages[0]!.createdAt, 1000); assert.equal(changed.messages[0]!.editedAt, 3000);
  assert.equal(changed.messages[1]!.text, ''); assert.equal(changed.messages[1]!.recalled, true);
  const future = await f.reader.read({ ...f.scope, mode: 'range', fromSeqExclusive: 599, toSeqInclusive: 600 });
  assert.equal(future.messages[0]!.text, 'Future for this task'); f.reader.close();
});
test('shared Chat source: continued later appends do not starve a captured multi-page input window', async () => {
  const f = fixture(), append = () => { f.append(); f.afterNextPage(append); };
  f.afterNextPage(append);
  const result = await f.reader.read({ ...f.scope, mode: 'range', fromSeqExclusive: 200, toSeqInclusive: 650 });
  assert.equal(result.messages.length, 450); assert.equal(result.messages.at(-1)!.seq, 650);
  assert.equal(result.lastSeq, 650); assert.equal(result.lastChangeSeq, 650);
  assert.equal(f.chat.lastSeq, 653);
  f.afterNextPage(() => undefined);
  const next = await f.reader.read({ ...f.scope, mode: 'range', fromSeqExclusive: 650, toSeqInclusive: 653 });
  assert.deepEqual(next.messages.map(message => message.seq), [651, 652, 653]);
  f.reader.close();
});
test('shared Chat source: an edit mixed with later appends still invalidates the captured window', async () => {
  const f = fixture(), request: ChatReadRequest = { ...f.scope, mode: 'range', fromSeqExclusive: 200, toSeqInclusive: 650 };
  f.afterNextPage(() => { f.append(); f.edit(210, 'Corrected while reading'); });
  await assert.rejects(f.reader.read(request), /CHAT_CHANGED/u);
  const fresh = await f.reader.read(request);
  assert.equal(fresh.messages.length, 450);
  assert.equal(fresh.messages.find(message => message.seq === 210)!.text, 'Corrected while reading');
  f.reader.close();
});
