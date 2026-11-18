import assert from 'node:assert/strict';
import { test } from 'node:test';
import { AssistantInboxReader, assistantInboxActivity, type AssistantInboxActivity } from '../src/renderer/bridge/inboxAssistantReading';
import type { InboxItem, InboxMutation } from '../../../packages/inbox/contracts';

const SESSION = '11111111-1111-4111-8111-111111111111';
const FIRST = '22222222-2222-4222-8222-222222222222';
const SECOND = '33333333-3333-4333-8333-333333333333';
function row(patch: Partial<InboxItem> = {}): InboxItem {
  return { schemaVersion: 1, key: `assistant:device:${SESSION}`, ref: { kind: 'assistant', sourceId: 'device', conversationId: SESSION, generation: 1 },
    category: 'assistant', title: 'Report', preview: '', lastActivityAt: 1, sourceCursor: 1, sourceRevision: 1, readThrough: 0,
    unreadCount: 1, mentionCount: 0, pendingActionCount: 0, state: 'completed', manualUnread: false, manualUnreadRevision: 2,
    stateRevision: 1, offline: false, pinOrder: null, notificationMode: 'all', archived: false, ...patch };
}
function harness() {
  let now = 0, next = 0;
  const timers = new Map<number, { at: number; callback: () => void }>();
  const sent: InboxMutation[] = [];
  const reader = new AssistantInboxReader(async command => { sent.push(command); }, error => { throw error; }, {
    now: () => now,
    schedule(callback, delay) { const id = ++next; timers.set(id, { at: now + delay, callback }); return id as unknown as ReturnType<typeof setTimeout>; },
    cancel(timer) { timers.delete(timer as unknown as number); },
  });
  const advance = (delay: number) => {
    const until = now + delay;
    for (;;) {
      const due = [...timers].filter(([, timer]) => timer.at <= until).sort((a, b) => a[1].at - b[1].at || a[0] - b[0])[0];
      if (!due) break;
      timers.delete(due[0]); now = due[1].at; due[1].callback();
    }
    now = until;
  };
  const activity: AssistantInboxActivity = { sourceId: 'device', cursor: 1, baselineCursor: 0, latestResultUuids: [FIRST] };
  const observe = (patch: Partial<Parameters<AssistantInboxReader['observe']>[0]> = {}) => reader.observe({ item: row(), activity,
    position: { sessionKey: SESSION, visibleMessageUuids: [FIRST], preview: false }, foreground: true, ...patch });
  return { reader, sent, advance, observe, activity };
}

test('activity rejects row-count proxies and malformed source metadata', () => {
  assert.equal(assistantInboxActivity(JSON.stringify({ message_count: 900, modified_rfc3339: '2026-01-01' })), null);
  assert.equal(assistantInboxActivity(JSON.stringify({ sourceId: '../elsewhere', cursor: 1, baselineCursor: 0 })), null);
  assert.equal(assistantInboxActivity(JSON.stringify({ sourceId: 'device', cursor: 1, baselineCursor: 2 })), null);
  assert.deepEqual(assistantInboxActivity(JSON.stringify({ sourceId: 'device', cursor: 2, baselineCursor: 1, latestResultUuids: [FIRST, '/private/prompt'] }))?.latestResultUuids, [FIRST]);
});

test('requires 300ms of actual committed content, not list or focus metadata', () => {
  const h = harness();
  h.observe(); h.advance(299); assert.equal(h.sent.length, 0);
  h.advance(1); assert.equal(h.sent.length, 1);
  const { operationId, ...command } = h.sent[0] as Extract<InboxMutation, { action: 'markRead' }>;
  assert.match(operationId, /^[0-9a-f-]{36}$/);
  assert.deepEqual(command, { action: 'markRead', key: row().key, generation: 1, through: 1, manualUnreadRevision: 2 });
  h.observe(); h.advance(2000); assert.equal(h.sent.length, 1);
});

test('hidden, mismatched, preview or not-yet-rendered content cannot become read', () => {
  for (const patch of [
    { foreground: false },
    { activity: { sourceId: 'another-device', cursor: 1, baselineCursor: 0, latestResultUuids: [FIRST] } },
    { position: { sessionKey: SESSION, visibleMessageUuids: [FIRST], preview: true } },
    { position: { sessionKey: SESSION, visibleMessageUuids: [SECOND], preview: false } },
    { item: row({ sourceCursor: 2 }) },
  ]) {
    const h = harness(); h.observe(patch); h.advance(1000); assert.equal(h.sent.length, 0);
  }
  const h = harness(); h.observe(); h.advance(200); h.observe({ foreground: false }); h.advance(1000);
  assert.equal(h.sent.length, 0);
});

test('noncontiguous results become read only when the intervening result is seen', () => {
  const h = harness(), activity = { ...h.activity, cursor: 2, latestResultUuids: [SECOND] };
  const entries = [{ cursor: 1, resultUuids: [FIRST] }, { cursor: 2, resultUuids: [SECOND] }];
  h.observe({ item: row({ sourceCursor: 2 }), activity, entries,
    position: { sessionKey: SESSION, visibleMessageUuids: [SECOND], preview: false } });
  h.advance(300); assert.equal(h.sent.length, 0);
  h.observe({ item: row({ sourceCursor: 2 }), activity, entries });
  h.advance(300); assert.equal(h.sent.length, 1);
  assert.equal((h.sent[0] as { through: number }).through, 2);
});

test('flushes confirmed positions on leave and retains the original manual-unread revision', () => {
  const h = harness(); h.observe(); h.advance(300);
  h.observe({ item: row({ sourceCursor: 2, readThrough: 1, manualUnread: true, manualUnreadRevision: 9 }),
    activity: { ...h.activity, cursor: 2, latestResultUuids: [SECOND] },
    position: { sessionKey: SESSION, visibleMessageUuids: [SECOND], preview: false } });
  h.advance(300); assert.equal(h.sent.length, 1);
  h.reader.leave(); assert.equal(h.sent.length, 2);
  assert.equal((h.sent[1] as { manualUnreadRevision: number }).manualUnreadRevision, 2);
  assert.equal((h.sent[1] as { through: number }).through, 2);
});

test('explicit jump-latest confirms earlier results after seeing the latest committed result', () => {
  const h = harness();
  h.observe({ item: row({ sourceCursor: 7 }), activity: { ...h.activity, cursor: 7 },
    position: { sessionKey: SESSION, visibleMessageUuids: [FIRST], preview: true, explicitLatest: true } });
  h.advance(300); assert.equal((h.sent[0] as { through: number }).through, 7);
});

test('a jump confirms only its frozen cursor and cannot cover later unseen results, even after a blur', () => {
  const h = harness();
  h.observe({ position: { sessionKey: SESSION, visibleMessageUuids: [FIRST], preview: true, explicitLatest: true, latestIntent: 1 } });
  h.advance(300); assert.equal(h.sent.length, 1);
  h.reader.leave();
  h.observe({ item: row({ sourceCursor: 4, readThrough: 1 }), activity: { ...h.activity, cursor: 4, latestResultUuids: [SECOND] },
    position: { sessionKey: SESSION, visibleMessageUuids: [SECOND], preview: true, explicitLatest: true, latestIntent: 1 } });
  h.advance(1500); assert.equal(h.sent.length, 1, 'the old jump must not skip unseen ordinals 2 and 3');
  h.observe({ item: row({ sourceCursor: 4, readThrough: 1 }), activity: { ...h.activity, cursor: 4, latestResultUuids: [SECOND] },
    position: { sessionKey: SESSION, visibleMessageUuids: [SECOND], preview: true, explicitLatest: true, latestIntent: 2 } });
  h.advance(300); assert.equal((h.sent[1] as { through: number }).through, 4);
});
