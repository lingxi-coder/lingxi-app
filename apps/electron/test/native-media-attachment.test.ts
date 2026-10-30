import assert from 'node:assert/strict';
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { validateMediaDeviceResponse as validateWireMediaDeviceResponse } from '@lingxi/bridge-client';
import type { MediaDeviceRequestEnvelopeDto, MediaOwnerDto } from '@lingxi/bridge-client';
import { validateMediaDeviceRequestEnvelope } from '../../../packages/bridge-client/src/mediaProtocol.js';
import { DiagnosticBuffer } from '../src/main/host-utils.js';
import { NativeAudioManager } from '../src/main/audio/nativeAudioManager.js';
import { NativeMediaDevices } from '../src/main/media/nativeMediaDevices.js';
import { nativeMediaOwnerKey, validateMediaDeviceRequest, validateMediaDeviceResponse } from '../src/shared/nativeMedia.js';

const owner = {
  type: 'attached', attachment_host_id: '12345678-1234-4234-8234-123456789abc',
  attachment_generation: 1, logical_kind: 'session', logical_owner_id: 'session-a',
} as const;

function open(requestId: string, candidate: unknown): unknown {
  return { request_id: requestId, request: { type: 'open', request: {
    identity: { id: `op-${requestId}`, generation: 1, service_epoch: 10 }, owner: candidate,
    max_payload_bytes: 1024,
    request: { mode: 'audio', intent: 'conversation', audio_source: { id: 'media-runtime:microphone' } },
  } } };
}

test('wire and native validators reject malformed attachment owners', () => {
  const valid = open('valid', owner);
  assert.deepEqual(validateMediaDeviceRequestEnvelope(valid), valid);
  assert.deepEqual(validateMediaDeviceRequest(valid), valid);
  const invalid: unknown[] = [
    { ...owner, attachment_host_id: 'not-a-uuid' },
    { ...owner, attachment_host_id: `${owner.attachment_host_id}\n` },
    { ...owner, attachment_host_id: '12345678-1234-1234-8234-123456789abc' },
    ...[0, -1, 1.5, true, Number.MAX_SAFE_INTEGER + 1].map((attachment_generation) => ({ ...owner, attachment_generation })),
    { ...owner, logical_kind: 'attached' },
    { ...owner, logical_kind: ['session'] },
    { ...owner, logical_owner_id: '' },
    { ...owner, logical_owner_id: '\0' },
    { ...owner, logical_owner_id: '界'.repeat(86) },
    { ...owner, extra: 'ignored-by-nobody' },
    { ...owner, attachment_host_id: undefined },
  ];
  for (const [index, candidate] of invalid.entries()) {
    assert.throws(() => validateMediaDeviceRequestEnvelope(open(`wire-${index}`, candidate)));
    assert.throws(() => validateMediaDeviceRequest(open(`native-${index}`, candidate)));
  }
});

test('wire source references use the Rust DTO byte bound and permit runtime source IDs', () => {
  for (const id of ['media-runtime:microphone', 'a'.repeat(256), '\0']) {
    const request = open('source', owner) as any;
    request.request.request.request.audio_source.id = id;
    assert.doesNotThrow(() => validateMediaDeviceRequestEnvelope(request));
  }
  for (const id of ['', 'a'.repeat(257), '界'.repeat(86)]) {
    const request = open('source-invalid', owner) as any;
    request.request.request.request.audio_source.id = id;
    assert.throws(() => validateMediaDeviceRequestEnvelope(request));
  }
});

test('typed no-resource proof survives both device response validators', () => {
  const response = { request_id: 'request-a', result: { type: 'open_failed_without_resource',
    error: { kind: 'busy', message: 'not admitted' } } };
  assert.deepEqual(validateWireMediaDeviceResponse(response), response);
  assert.deepEqual(validateMediaDeviceResponse(response), response);
});

test('trusted attachment is matched exactly through the JSONL helper and rotates after cleanup', { timeout: 10_000 }, async () => {
  const seen: MediaDeviceRequestEnvelopeDto[] = [];
  let helper: ChildProcessWithoutNullStreams | undefined;
  let stderr = '';
  const manager = new NativeAudioManager({
    isPackaged: false, resourcesPath: '/unused', userDataPath: '/tmp/lingxi-attachment-test',
    helperPath: '/unused/fake-helper', diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => {
      helper = spawn(process.execPath, [fileURLToPath(new URL('./fixtures/native-media-attachment-helper.cjs', import.meta.url))], { stdio: 'pipe' });
      helper.stderr.on('data', (chunk: Buffer) => {
        stderr += chunk.toString('utf8');
        const lines = stderr.split('\n'); stderr = lines.pop()!;
        for (const line of lines) seen.push(JSON.parse(line) as MediaDeviceRequestEnvelopeDto);
      });
      return helper;
    },
  });
  const bare = new NativeMediaDevices({ audio: manager, owner: { type: 'session', session_id: owner.logical_owner_id } });
  const mutableBinding = { ...owner };
  const first = new NativeMediaDevices({ audio: manager, owner: mutableBinding });
  mutableBinding.attachment_generation = 2;
  const nextOwner = { ...owner, attachment_generation: 2 } as const;
  let second: NativeMediaDevices | undefined;
  try {
    assert.equal((await bare.request(open('self-assert', owner))).result.type, 'open_failed_without_resource');
    for (const [index, foreign] of [
      nextOwner,
      { ...owner, attachment_host_id: '12345678-1234-4234-8234-123456789abd' },
      { ...owner, logical_kind: 'ui' },
      { ...owner, logical_owner_id: 'session-b' },
      { type: 'session', session_id: owner.logical_owner_id },
    ].entries()) {
      assert.equal((await first.request(open(`foreign-${index}`, foreign))).result.type, 'open_failed_without_resource');
    }
    assert.equal(helper, undefined, 'rejected requests must not launch the native helper');
    assert.equal((await first.request(open('open-one', owner))).result.type, 'opened');
    await first.dispose(); // stop acknowledgement is the fence before installing a replacement
    second = new NativeMediaDevices({ audio: manager, owner: nextOwner });
    assert.equal((await first.request(open('old-closed', owner))).result.type, 'error');
    assert.equal((await second.request(open('old-generation', owner))).result.type, 'open_failed_without_resource');
    assert.equal((await second.request({ request_id: 'guess', request: { type: 'stop', lease_id: 'lease-1' } })).result.type, 'error');
    assert.equal((await second.request(open('open-two', nextOwner))).result.type, 'opened');
    await second.dispose();
    for (let n = 0; n < 20 && seen.length < 4; n++) await new Promise((resolve) => setTimeout(resolve, 10));
    assert.equal(seen.length, 4);
    assert.deepEqual(seen.map((item) => item.request.type), ['open', 'stop', 'open', 'stop']);
    assert.deepEqual((seen[0].request as { type: 'open'; request: { owner: MediaOwnerDto } }).request.owner, owner);
    assert.deepEqual((seen[2].request as { type: 'open'; request: { owner: MediaOwnerDto } }).request.owner, nextOwner);
    assert.notEqual(nativeMediaOwnerKey(owner), nativeMediaOwnerKey(nextOwner));
  } finally {
    await bare.dispose();
    await first.dispose();
    await second?.dispose();
    await manager.dispose();
    helper?.kill();
  }
});
