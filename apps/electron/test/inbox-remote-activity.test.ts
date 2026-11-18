import assert from 'node:assert/strict';
import { test } from 'node:test';
import { SessionGateway } from '../src/main/remoteControl/sessionGateway';
import { RemoteCommands } from '../src/main/remoteControl/remoteCommands';
import { FakeRuntime, SESSION_A, gatewayFixture, rpcCode } from './remote-session-helpers';
import type { AssistantActivityPage } from '../src/shared/inbox';

const page: AssistantActivityPage = { items: [{ cursor: 11, resultUuids: [SESSION_A], kind: 'reply', at: 100 }], cursor: 11, baselineCursor: 0, hasMore: false };
function setup(activityPage: NonNullable<ConstructorParameters<typeof SessionGateway>[0]['activityPage']>) {
  const f = gatewayFixture();
  const gateway = new SessionGateway({ runtimes: f.runtimes, policy: f.policy,
    commands: new RemoteCommands({ allowAlwaysAllow: () => false, echo: () => {}, clock: f.clock }),
    titleOf: async () => 'Named conversation', activityPage, clock: f.clock, diagnostics: () => {} });
  f.enable(SESSION_A);
  f.runtimes.add(new FakeRuntime(SESSION_A)).resume();
  return { f, gateway, call: (method: string, params: unknown) => gateway.handle(f.phone, method, params, f.channel),
    close: () => { gateway.dispose(); f.gateway.dispose(); } };
}

test('semantic activity pages require an authorized current session attachment', async () => {
  const calls: number[] = [];
  const h = setup(async (ref, after) => { assert.equal(ref.sessionId, SESSION_A); calls.push(after); return page; });
  try {
    assert.notEqual(await rpcCode(h.call('session.activity', { session_id: SESSION_A, after: 10 })), undefined);
    assert.equal(calls.length, 0);
    await h.call('session.attach', { session_id: SESSION_A });
    assert.deepEqual(await h.call('session.activity', { session_id: SESSION_A, after: 10 }), page);
    assert.deepEqual(calls, [10]);
    assert.equal(await rpcCode(h.call('session.activity', { session_id: SESSION_A, after: -1 })), 'invalid_params');
    assert.equal(await rpcCode(h.call('session.activity', { session_id: SESSION_A, after: 1.5 })), 'invalid_params');
    assert.deepEqual(calls, [10]);
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
    assert.notEqual(await rpcCode(pending), undefined);
  } finally { h.close(); }
});
