import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { test } from 'node:test';
import { SessionRuntime } from '../src/main/bridge.js';
import { SessionRuntimeManager } from '../src/main/sessionRuntimeManager.js';

const ref = { projectPath: '/resume-owner', sessionId: '11111111-2222-4333-8444-555555555555', mode: 'code' as const };
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
  client.emit('event', { type: 'session_resumed', session_id: ref.sessionId, mode: ref.mode, messages: [] });
  client.emit('event', { type: 'model_changed', model: 'anthropic/claude-sonnet-4-5' });
}
async function expired(promise: Promise<void>) {
  // Keep the test loop alive while exercising the production unref'ed timer.
  await Promise.all([assert.rejects(promise, /timed out resuming/), new Promise((done) => setTimeout(done, 40))]);
}

test('prior background credentials cannot confirm the next resume after its owner times out', async (t) => {
  const credential = deferred<string | undefined>();
  const runtime = new SessionRuntime({ ...ref, expectedSessionMode: ref.mode, sessionResumeTimeoutMs: 20,
    launchConfig: () => ({ workspace: ref.projectPath, trusted: true }), resolveProviderAccounts: async () => { await credential.promise; return undefined; } });
  t.after(() => runtime.dispose());
  const client = wire(runtime);
  const hydration = runtime.refreshProviderAccounts();
  const first = runtime.resumeOwnedSession();
  await expired(first);
  let settled = false;
  const second = runtime.resumeOwnedSession().then(() => { settled = true; });
  const pending = (runtime as any).pendingSessionResume;
  credential.resolve(undefined);
  await hydration;
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
    resolveProviderAccounts: async () => { await credential.promise; return {
      providerAccounts: { storeScope: 'fixture', configRevision: 0, accounts: [], health: [] }, providerAccountCredentials: {},
    }; } });
  t.after(() => manager.dispose());
  const runtime = await manager.ensure(ref, false);
  const internal = runtime as any;
  wire(runtime);
  internal.sessionHasHistory = true;
  const transcriptPath = `${ref.projectPath}/transcript.jsonl`;
  runtime.rememberTranscriptSelection(transcriptPath);
  const resumeSignals: AbortSignal[] = [];
  let launches = 0;
  let secondClient: ReturnType<typeof wire> | undefined;
  let priorHydrationRejected: Promise<void> | undefined;
  const secondSent = deferred<void>();
  internal.startInternal = async () => {
    ++internal.generation;
    const client = wire(runtime);
    launches++;
    if (launches === 1) priorHydrationRejected = assert.rejects(runtime.refreshProviderAccounts(), /interrupted/);
    Object.assign(client, {
      resumeSessionFromTranscript: async (sessionId: string, selectedPath: string, mode: string, signal: AbortSignal) => {
        assert.equal(sessionId, ref.sessionId);
        assert.equal(selectedPath, transcriptPath);
        assert.equal(mode, ref.mode);
        resumeSignals.push(signal);
        if (launches !== 1) { secondClient = client; secondSent.resolve(); }
      },
    });
  };
  await expired(manager.restart(ref));
  assert.equal(resumeSignals[0].aborted, true);
  let settled = false;
  const second = manager.restart(ref).then(() => { settled = true; });
  await secondSent.promise;
  const pending = internal.pendingSessionResume;
  assert.equal(resumeSignals.length, 2);
  assert.equal(resumeSignals[1].aborted, false);
  credential.resolve(undefined);
  await priorHydrationRejected;
  await new Promise((done) => setImmediate(done));
  assert.equal(settled, false);
  assert.strictEqual(internal.pendingSessionResume, pending);
  resumeProof(secondClient!);
  await second;
});
