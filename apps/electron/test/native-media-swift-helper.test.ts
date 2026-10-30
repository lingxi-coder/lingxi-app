import assert from 'node:assert/strict';
import { test } from 'node:test';
import { DiagnosticBuffer } from '../src/main/host-utils.js';
import { NativeAudioManager } from '../src/main/audio/nativeAudioManager.js';

const helperPath = process.env['MEDIA_SWIFT_HELPER_PATH'];
test('real Swift JSONL helper proves an invalid source failed before resource admission',
  { skip: !helperPath, timeout: 10_000 }, async () => {
    const manager = new NativeAudioManager({ isPackaged: false, resourcesPath: '/unused', userDataPath: '/tmp/lingxi-media-swift-test',
      helperPath, diagnostics: new DiagnosticBuffer() });
    try {
      const result = await manager.executeMediaDevice({ request_id: 'real-swift-invalid-source', request: { type: 'open', request: {
        identity: { id: 'operation-real-swift', generation: 1, service_epoch: 10 },
        owner: { type: 'attached', attachment_host_id: '12345678-1234-4234-8234-123456789abc',
          attachment_generation: 1, logical_kind: 'session', logical_owner_id: 'session-a' },
        timeout_budget_ms: 5_000, max_payload_bytes: 1024,
        request: { mode: 'audio', intent: 'conversation', audio_source: { id: 'not-the-native-microphone' } },
      } } });
      assert.equal(result.result.type, 'open_failed_without_resource');
      await manager.drainMediaOpenFlights();
    } finally { await manager.dispose(); }
  });
