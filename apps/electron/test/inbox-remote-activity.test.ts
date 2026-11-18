import assert from 'node:assert/strict';
import { test } from 'node:test';
import { SessionGateway } from '../src/main/remoteControl/sessionGateway';
import { RemoteCommands } from '../src/main/remoteControl/remoteCommands';
import { FakeRuntime, SESSION_A, SESSION_B, gatewayFixture, rpcCode } from './remote-session-helpers';
import type { AssistantActivityPage } from '../src/shared/inbox';

const page: AssistantActivityPage = { items: [{ cursor: 11, resultUuids: [SESSION_A], kind: 'reply', at: 100 }], cursor: 11, baselineCursor: 0, hasMore: false };
function setup(activityPage: NonNullable<ConstructorParameters<typeof SessionGateway>[0]['activityPage']>) {
  const f = gatewayFixture();
  const gateway = new SessionGateway({ runtimes: f.runtimes, policy: f.policy,
    commands: new RemoteCommands({ allowAlwaysAllow: () => false, echo: () => {}, clock: f.clock }),
    titleOf: async () => 'Named conversation', activityPage, clock: f.clock, diagnostics: () => {} });
  f.enable(SESSION_A);
  const runtime = f.runtimes.add(new FakeRuntime(SESSION_A));
  runtime.resume();
  return { f, gateway, runtime, call: (method: string, params: unknown) => gateway.handle(f.phone, method, params, f.channel),
    close: () => { gateway.dispose(); f.gateway.dispose(); } };
}

test('semantic activity pages require an authorized current session attachment', async () => {
  const calls: number[] = [];
  const h = setup(async (ref, after) => { assert.equal(ref.sessionId, SESSION_A); calls.push(after); return page; });
  try {
    assert.equal(await rpcCode(h.call('session.activity', { session_id: SESSION_A, after: 10 })), 'invalid_params');
    assert.equal(calls.length, 0);
    await h.call('session.attach', { session_id: SESSION_A });
    assert.deepEqual(await h.call('session.activity', { session_id: SESSION_A, after: 10 }), page);
    assert.deepEqual(calls, [10]);
    assert.equal(await rpcCode(h.call('session.activity', { session_id: SESSION_A, after: -1 })), 'invalid_params');
    assert.equal(await rpcCode(h.call('session.activity', { session_id: SESSION_A, after: 1.5 })), 'invalid_params');
    assert.deepEqual(calls, [10]);
  } finally { h.close(); }
});

test('activity previews return only exact committed source text and omit private thinking fields', async () => {
  const h = setup(async (_ref, after) => { assert.equal(after, 10); return page; });
  h.runtime.resume([
    { role: 'user', transcript_uuid: SESSION_B, blocks: [{ type: 'text', text: 'Unrelated prompt' }] },
    { role: 'assistant', transcript_uuid: SESSION_A,
      blocks: [{ type: 'thinking', thinking: 'Reasoning', signature: 'private signature' }, { type: 'text', text: 'Result' }] },
  ]);
  try {
    assert.equal(await rpcCode(h.call('session.activityPreview', { session_id: SESSION_A, through: 11 })), 'invalid_params');
    await h.call('session.attach', { session_id: SESSION_A });
    const result = await h.call('session.activityPreview', { session_id: SESSION_A, through: 11 });
    assert.deepEqual(result, { available: true, messages: [{ role: 'assistant', transcript_uuid: SESSION_A,
      blocks: [{ type: 'text', text: 'Result' }] }] });
    assert.equal(await rpcCode(h.call('session.activityPreview', { session_id: SESSION_A, through: 0 })), 'invalid_params');
  } finally { h.close(); }
});

test('missing, non-assistant and oversized activity previews are unavailable without returning unrelated history', async () => {
  for (const message of [
    { role: 'assistant', transcript_uuid: SESSION_B, blocks: [{ type: 'text' as const, text: 'Another result' }] },
    { role: 'user', transcript_uuid: SESSION_A, blocks: [{ type: 'text' as const, text: 'User prompt' }] },
    { role: 'assistant', transcript_uuid: SESSION_A, blocks: [{ type: 'text' as const, text: 'x'.repeat(256 * 1024) }] },
  ]) {
    const h = setup(async () => page);
    h.runtime.resume([message]);
    try {
      await h.call('session.attach', { session_id: SESSION_A });
      assert.deepEqual(await h.call('session.activityPreview', { session_id: SESSION_A, through: 11 }), { available: false, messages: [] });
    } finally { h.close(); }
  }
});

test('a delayed activity preview cannot outlive sharing withdrawal', async () => {
  let finish!: (value: AssistantActivityPage) => void;
  const h = setup(() => new Promise(resolve => { finish = resolve; }));
  try {
    await h.call('session.attach', { session_id: SESSION_A });
    const pending = h.call('session.activityPreview', { session_id: SESSION_A, through: 11 });
    h.f.policy.setSessionEnabled({ projectPath: '/work/project-one', sessionId: SESSION_A }, false);
    finish(page);
    assert.equal(await rpcCode(pending), 'not_approved');
  } finally { h.close(); }
});

test('a delayed activity response cannot outlive sharing withdrawal', async () => {
  let finish!: (value: AssistantActivityPage) => void;
  const h = setup(() => new Promise(resolve => { finish = resolve; }));
  try {
    await h.call('session.attach', { session_id: SESSION_A });
    const pending = h.call('session.activity', { session_id: SESSION_A, after: 10 });
    h.f.policy.setSessionEnabled({ projectPath: '/work/project-one', sessionId: SESSION_A }, false);
    finish(page);
    assert.equal(await rpcCode(pending), 'not_approved');
  } finally { h.close(); }
});
