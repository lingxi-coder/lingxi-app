import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { test } from 'node:test';
import { SessionRuntime } from '../src/main/bridge.js';
import { SessionRuntimeManager } from '../src/main/sessionRuntimeManager.js';

const ref = { projectPath: '/resume-owner', sessionId: '11111111-2222-4333-8444-555555555555' };
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}
function wire(runtime: SessionRuntime) {
  const internal = runtime as any;
  const client = new EventEmitter() as EventEmitter & { sendCommand(command: any): void; close(): void };
  client.sendCommand = () => undefined;
  client.close = () => undefined;
  internal.activeWorkspace = ref.projectPath;
  internal.activeWorkspaceTrusted = true;
  internal.client = client;
  internal.credentialRoutingSettings = {};
  internal.wireClient(client, internal.generation);
  internal.setState({ status: 'connected' });
  return client;
}
function resumeProof(client: EventEmitter) {
  client.emit('event', { type: 'session_resumed', session_id: ref.sessionId, messages: [] });
  client.emit('event', { type: 'model_changed', model: 'anthropic/claude-sonnet-4-5' });
}
async function expired(promise: Promise<void>) {
  // Keep the test loop alive while exercising the production unref'ed timer.
  await Promise.all([assert.rejects(promise, /timed out resuming/), new Promise((done) => setTimeout(done, 40))]);
}

test('a timed-out resume credential completion cannot resolve its next unconfirmed resume', async (t) => {
  const credential = deferred<string | undefined>();
  const runtime = new SessionRuntime({ ...ref, sessionResumeTimeoutMs: 20,
    launchConfig: () => ({ workspace: ref.projectPath, trusted: true }), resolveProviderCredential: () => credential.promise });
  t.after(() => runtime.dispose());
  const client = wire(runtime);
  const first = runtime.resumeOwnedSession();
  resumeProof(client);
  await expired(first);
  let settled = false;
  const second = runtime.resumeOwnedSession().then(() => { settled = true; });
  const pending = (runtime as any).pendingSessionResume;
  credential.resolve(undefined);
  await new Promise((done) => setImmediate(done));
  assert.equal(settled, false);
  assert.strictEqual((runtime as any).pendingSessionResume, pending);
  assert.equal(pending.sessionResumed, false);
  resumeProof(client);
  await second;
});

test('a prior restart broker load cannot reject a newer generation resume', async (t) => {
  const credential = deferred<string | undefined>();
  const manager = new SessionRuntimeManager({ sessionResumeTimeoutMs: 20,
    launchConfig: (owned) => ({ workspace: owned.projectPath, sessionId: owned.sessionId, trusted: true }),
    resolveProviderCredential: () => credential.promise });
  t.after(() => manager.dispose());
  const runtime = await manager.ensure(ref, false);
  const internal = runtime as any;
  wire(runtime);
  internal.sessionHasHistory = true;
  let launches = 0;
  let secondClient: ReturnType<typeof wire> | undefined;
  const secondSent = deferred<void>();
  internal.startInternal = async () => {
    ++internal.generation;
    const client = wire(runtime);
    launches++;
    client.sendCommand = (command) => {
      if (command.type !== 'resume_session') return;
      if (launches === 1) resumeProof(client);
      else { secondClient = client; secondSent.resolve(); }
    };
  };
  await expired(manager.restart(ref));
  let settled = false;
  const second = manager.restart(ref).then(() => { settled = true; });
  await secondSent.promise;
  const pending = internal.pendingSessionResume;
  credential.resolve(undefined);
  await new Promise((done) => setImmediate(done));
  assert.equal(settled, false);
  assert.strictEqual(internal.pendingSessionResume, pending);
  resumeProof(secondClient!);
  await second;
});
