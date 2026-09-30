import assert from 'node:assert/strict';
import { test } from 'node:test';
import { SessionRuntime } from '../src/main/bridge.js';
import { DiagnosticBuffer } from '../src/main/host-utils.js';

const sessionId = '11111111-2222-4333-8444-555555555555';
const request = {
  identity: { id: 'admitted-recording', generation: 1, service_epoch: 1 },
  owner: { type: 'session', session_id: sessionId },
  max_payload_bytes: 1024,
  operation: { type: 'start_recording', sample_rate_hz: 16_000, format: 'wav' },
};
for (const cancellationFails of [false, true]) {
  test(`failed native recording result delivery observes owner rollback after ${cancellationFails ? 'failing' : 'successful'} cancellation`, async () => {
    const diagnostics = new DiagnosticBuffer();
    const operations: unknown[] = [];
    const audio = {
      getCapabilities: () => ({ service_epoch: 1, supported_operations: [], max_payload_bytes: 1024 }),
      onEvent: () => () => undefined,
      executeAudioRequest: async () => ({ type: 'recording_started', handle: 'fake-recording' }),
      cancelAudioRequest: async (identity: unknown) => {
        operations.push({ cancel: identity });
        if (cancellationFails) throw new Error('cancel failed token=fake-cancel-value');
      },
      endAudioOwner: async (owner: unknown) => {
        operations.push({ end: owner });
        throw new Error('native audio owner could not be ended api_key=fake-owner-value');
      },
    };
    const runtime = new SessionRuntime({ sessionId, projectPath: '/tmp', launchConfig: () => ({ workspace: '/tmp', trusted: true }), audioService: audio as any, diagnostics });
    const client = { sendCommand: () => { throw new Error('result transport closed'); } };
    (runtime as any).client = client;
    await (runtime as any).dispatchAudioRequest(client, (runtime as any).generation, request);
    await new Promise((resolve) => setImmediate(resolve));
    assert.deepEqual(operations, [{ cancel: request.identity }, { end: request.owner }]);
    const report = diagnostics.snapshot().map((row) => row.message).join('\n');
    assert.match(report, /recording owner teardown rollback failed/);
    assert.equal(report.includes('fake-owner-value'), false);
    if (cancellationFails) {
      assert.match(report, /cancellation rollback failed/);
      assert.equal(report.includes('fake-cancel-value'), false);
    }
    // No real transport or native owner exists. Detach the fake before normal disposal.
    (runtime as any).client = null;
    await runtime.dispose();
  });
}

test('a synchronous cancellation rollback error does not suppress admitted recording owner teardown', async () => {
  let ownerCleanup = false;
  const diagnostics = new DiagnosticBuffer();
  const runtime = new SessionRuntime({ sessionId, projectPath: '/tmp', launchConfig: () => ({ workspace: '/tmp', trusted: true }), diagnostics,
    audioService: {
      getCapabilities: () => ({}), onEvent: () => () => undefined,
      executeAudioRequest: async () => ({ type: 'recording_started', handle: 'fake-recording' }),
      cancelAudioRequest: () => { throw new Error('synchronous cancellation failure'); },
      endAudioOwner: async () => { ownerCleanup = true; },
    } as any });
  const client = { sendCommand: () => { throw new Error('result transport closed'); } };
  (runtime as any).client = client;
  await (runtime as any).dispatchAudioRequest(client, (runtime as any).generation, request);
  assert.equal(ownerCleanup, true);
  assert.ok(diagnostics.snapshot().some((row) => row.message.includes('synchronous cancellation failure')));
  (runtime as any).client = null;
  (runtime as any).opts.audioService.cancelAudioRequest = async () => undefined;
  await runtime.dispose();
});
