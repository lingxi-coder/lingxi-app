import assert from 'node:assert/strict';
import test from 'node:test';
import {
  alertEnabled,
  applyRead,
  badge,
  command,
  delaySeconds,
  emptyRow,
  messageEvent
} from '../src/inbox-model.mjs';
import { eligible } from '../src/inbox-delivery.mjs';
import { createTeamsHandler } from '../src/teams.mjs';

const ref = { kind: 'team', sourceId: 'team', conversationId: 'chat', generation: 1 };
test('manual unread is independently versioned; reads do not resolve required actions', () => {
  const r = { ...emptyRow(ref), readThrough: 5, manualUnread: true, manualUnreadRevision: 3, pendingActionCount: 1 };
  assert.equal(applyRead(r, 3, 2).readThrough, 5);
  assert.equal(applyRead(r, 8, 2).manualUnread, true);
  assert.equal(applyRead(r, 8, 3).manualUnread, false);
  assert.equal(badge(applyRead(r, 8, 3)), true);
});
test('count incoming user-visible items and distinguish balanced DM, mention, reply and ordinary chat', () => {
  const m = { author: { kind: 'user', uid: 'peer' }, seq: 3, kind: 'text' };
  for (const excluded of [
    { ...m, recalled: true },
    { ...m, kind: 'tool' },
    { ...m, author: { kind: 'system' } },
    { ...m, author: { kind: 'user', uid: 'me' } }
  ])
    assert.equal(messageEvent(excluded, 'me', {}), null);
  for (const [input, conversation, reply, kind] of [
    [m, {}, null, 'message'],
    [m, { kind: 'dm' }, null, 'dm'],
    [{ ...m, mentions: ['user:me'] }, {}, null, 'mention'],
    [m, {}, { author: { kind: 'user', uid: 'me' } }, 'reply']
  ])
    assert.equal(messageEvent(input, 'me', conversation, reply).kind, kind);
  assert.equal(alertEnabled('mentions', 'message'), false);
  assert.equal(alertEnabled('mentions', 'dm'), true);
  assert.equal(delaySeconds('completion'), 5);
  assert.equal(delaySeconds('approval'), 6);
});
test('an actual read cancels delivery; manual unread, a different notice or an old read do not', () => {
  const row = { ...emptyRow(ref), readThrough: 3, manualUnread: true, actualReadRevision: 4 };
  const event = {
    generation: 1,
    kind: 'dm',
    through: 3,
    createdReadRevision: 4,
    deliverUntil: 5000,
    expiresAt: 5000
  };
  assert.equal(eligible(event, row, 1000), true);
  assert.equal(eligible(event, { ...row, actualReadRevision: 5 }, 1000), false);
  assert.equal(eligible({ ...event, through: 4 }, { ...row, actualReadRevision: 5 }, 1000), true);
  assert.equal(eligible(event, row, 5000), false);
  assert.equal(eligible(event, { ...row, ref: { ...ref, generation: 2 } }, 1000), false);
});
test('strict Assistant publisher rejects private metadata, transcript inputs and filesystem labels', () => {
  const input = {
    action: 'assistantPublish',
    sourceId: 'abc',
    sourceSecret: 'a'.repeat(32),
    conversationId: 'session',
    generation: 1,
    baseline: 0,
    sourceRevision: 1,
    sourceCursor: 1,
    lastActivityAt: 1,
    state: 'completed',
    pendingActionCount: 0,
    previewKind: 'taskResult'
  };
  assert.ok(command.safeParse(input).success);
  for (const field of [
    { title: '/Users/name/secret' },
    { sourceLabel: 'C:\\Windows\\x' },
    { prompt: 'x' },
    { preview: 'x' },
    { tool: 'x' },
    { events: [] }
  ])
    assert.equal(command.safeParse({ ...input, ...field }).success, false);
});
test('inboxApi authenticates revoked-token verifier, enforces origin, uid and payload path', async () => {
  let invoked = 0;
  const handler = createTeamsHandler({
    inbox: {
      execute: async (uid, body) => {
        invoked++;
        return { uid, action: body.action };
      }
    },
    verifyToken: async token => {
      if (token !== 'valid') throw Error();
      return { uid: 'self' };
    },
    origins: ['https://allowed']
  });
  const req = (token, origin) =>
    new Request('https://local/v1/inbox', {
      method: 'POST',
      headers: { Origin: origin, Authorization: 'Bearer ' + token },
      body: '{"action":"list"}'
    });
  assert.equal((await handler(req('valid', 'https://foreign'))).status, 403);
  assert.equal((await handler(req('bad', 'https://allowed'))).status, 401);
  assert.deepEqual(await (await handler(req('valid', 'https://allowed'))).json(), { uid: 'self', action: 'list' });
  assert.equal(invoked, 1);
});

test('operator delivery rollout uses a stable hashed cohort while preserving the unaffiliated default', async () => {
  const { inboxNotificationEnabled } = await import('../src/inbox-rollout.mjs');
  const { createHash } = await import('node:crypto');
  const allowed = createHash('sha256').update('alice').digest('hex');
  assert.equal(inboxNotificationEnabled('alice', {}), true);
  assert.equal(inboxNotificationEnabled('bob', { INBOX_NOTIFY_ROLLOUT_PERCENT: 'bad' }), false);
  assert.equal(inboxNotificationEnabled('alice', { INBOX_NOTIFY_ALLOW_SHA256: allowed }), true);
  assert.equal(inboxNotificationEnabled('bob', { INBOX_NOTIFY_ALLOW_SHA256: allowed }), false);
  assert.equal(
    inboxNotificationEnabled('bob', { INBOX_NOTIFY_ROLLOUT_PERCENT: '20' }),
    inboxNotificationEnabled('bob', { INBOX_NOTIFY_ROLLOUT_PERCENT: '20' })
  );
});
