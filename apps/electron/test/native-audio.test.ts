import { PassThrough } from 'node:stream';
import { EventEmitter } from 'node:events';
import assert from 'node:assert/strict';
import { test } from 'node:test';

import { DiagnosticBuffer } from '../src/main/host-utils';
import { NativeAudioManager } from '../src/main/audio/nativeAudioManager';
import { audioConfigurationDefaults } from '../src/shared/generatedAudioConfiguration';
import {
  defaultNativeAudioSnapshot,
  validateNativeAudioEvent,
  validateNativeAudioSnapshot,
  type NativeAudioSnapshot,
} from '../src/shared/nativeAudio';

class FakeHelperProcess extends EventEmitter {
  readonly stdout = new PassThrough();
  readonly stderr = new PassThrough();
  readonly stdin = new PassThrough();
  readonly pid = 4242;
  writes: string[] = [];
  killCount = 0;

  constructor() {
    super();
    this.stdin.write = ((chunk: string | Uint8Array) => {
      this.writes.push(Buffer.from(chunk).toString('utf8'));
      return true;
    }) as typeof this.stdin.write;
  }

  kill(): boolean {
    this.killCount += 1;
    this.emit('exit', 0, null);
    return true;
  }
}

function snapshot(overrides: Partial<NativeAudioSnapshot> = {}): NativeAudioSnapshot {
  return {
    helper: { state: 'running' },
    permissions: { microphone: 'granted', speech: 'authorized' },
    owner: null,
    activity: 'idle',
    audioOperations: [],
    activeOperationCount: 0,
    pendingOperationCount: 0,
    activeRecordingCount: 0,
    activePlaybackCount: 0,
    activeModelReferenceCount: 0,
    voices: [],
    models: [],
    ...overrides,
  };
}

test('input level events carry only a bounded real microphone amplitude', () => {
  const owner = { kind: 'dictation', id: 'dictation-1' } as const;
  assert.deepEqual(validateNativeAudioEvent({
    type: 'input_level',
    owner,
    level: 0.42,
  }), {
    type: 'input_level',
    owner,
    level: 0.42,
  });
  assert.throws(() => validateNativeAudioEvent({ type: 'input_level', owner, level: -0.01 }), /audio input level/);
  assert.throws(() => validateNativeAudioEvent({ type: 'input_level', owner, level: 1.01 }), /audio input level/);
});

test('snapshots and events accept every engine owner kind emitted by the Swift helper', () => {
  const owners = [
    { legacy: { kind: 'session', id: 'session-1' }, engine: { type: 'session', session_id: 'session-1' } },
    { legacy: { kind: 'local_app', id: 'app-1:7' }, engine: { type: 'local_app', app_id: 'app-1', runtime_generation: 7 } },
    { legacy: { kind: 'ui', id: 'voice-preview' }, engine: { type: 'ui', instance_id: 'voice-preview' } },
    { legacy: { kind: 'system', id: 'desktop' }, engine: { type: 'system', instance_id: 'desktop' } },
  ] as const;
  const identity = { id: '00000000-0000-4000-8000-000000000011', generation: 1, service_epoch: 9 };

  for (const { legacy, engine } of owners) {
    const activeSnapshot = snapshot({
      owner: legacy,
      activity: 'speaking',
      currentOperation: { identity, owner: engine },
    });
    assert.deepEqual(validateNativeAudioSnapshot(activeSnapshot).owner, legacy);
    assert.deepEqual(validateNativeAudioEvent({
      type: 'speech_state',
      snapshot: activeSnapshot,
      owner: legacy,
      state: 'speaking',
    }), {
      type: 'speech_state',
      snapshot: activeSnapshot,
      owner: legacy,
      state: 'speaking',
    });
    assert.deepEqual(validateNativeAudioEvent({ type: 'input_level', owner: legacy, level: 0.2 }), {
      type: 'input_level', owner: legacy, level: 0.2,
    });
  }
});

function lastEnvelope(process: FakeHelperProcess): { id: string; kind: string } {
  const raw = process.writes.at(-1);
  assert.ok(raw, 'expected helper request');
  return JSON.parse(raw.trim()) as { id: string; kind: string };
}

async function envelopeAt(process: FakeHelperProcess, index: number): Promise<{ id: string; kind: string; command?: { type?: string } }> {
  for (let attempt = 0; attempt < 10; attempt += 1) {
    const raw = process.writes[index];
    if (raw) return JSON.parse(raw.trim()) as { id: string; kind: string; command?: { type?: string } };
    await new Promise((resolve) => setImmediate(resolve));
  }
  assert.fail(`expected helper request at index ${index}`);
}

async function nextEnvelope(process: FakeHelperProcess): Promise<{ id: string; kind: string }> {
  for (let attempt = 0; attempt < 5; attempt += 1) {
    if (process.writes.length > 0) return lastEnvelope(process);
    await new Promise((resolve) => setImmediate(resolve));
  }
  return lastEnvelope(process);
}

test('snapshot requests start the helper and return its real capabilities', async () => {
  const helper = new FakeHelperProcess();
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
  });
  try {
    const responsePromise = manager.request({ type: 'get_snapshot' });
    const envelope = await nextEnvelope(helper);
    assert.equal(envelope.kind, 'command');
    helper.stdout.write(`${JSON.stringify({
      id: envelope.id,
      type: 'response',
      result: { type: 'snapshot', snapshot: snapshot({ recognizerAvailable: true }) },
    })}\n`);
    const response = await responsePromise;
    assert.equal(response.type, 'snapshot');
    assert.equal(response.snapshot.recognizerAvailable, true);
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

for (const failure of ['verification', 'spawn'] as const) {
  test(`synchronous helper ${failure} failure leaves a failed snapshot and permits retry`, async () => {
    const helper = new FakeHelperProcess();
    let shouldFail = true;
    const manager = new NativeAudioManager({
      isPackaged: true,
      resourcesPath: '/resources',
      userDataPath: '/tmp/lingxi-audio-tests',
      helperPath: '/tmp/LingXiAudioHelper',
      diagnostics: new DiagnosticBuffer(),
      verifyPackagedHelper: () => {
        if (shouldFail && failure === 'verification') throw new Error('bad signature');
      },
      spawnHelper: () => {
        if (shouldFail && failure === 'spawn') throw new Error('spawn failed');
        return helper as any;
      },
    });
    try {
      assert.equal((await manager.request({ type: 'get_snapshot' })).type, 'error');
      assert.equal(manager.getSnapshot().helper.state, 'failed');
      shouldFail = false;
      const retry = manager.request({ type: 'get_snapshot' });
      const envelope = await nextEnvelope(helper);
      helper.stdout.write(`${JSON.stringify({ id: envelope.id, type: 'response', result: { type: 'snapshot', snapshot: snapshot() } })}\n`);
      assert.equal((await retry).type, 'snapshot');
    } finally {
      await manager.dispose();
      helper.stdin.end();
      helper.stdout.end();
      helper.stderr.end();
    }
  });
}

for (const failure of ['exit', 'input-error'] as const) {
  test(`helper ${failure} clears recording ownership so UI cleanup does not restart it`, async () => {
    const helper = new FakeHelperProcess();
    let spawnCount = 0;
    const manager = new NativeAudioManager({
      isPackaged: false,
      resourcesPath: '/resources',
      userDataPath: '/tmp/lingxi-audio-tests',
      helperPath: '/tmp/LingXiAudioHelper',
      diagnostics: new DiagnosticBuffer(),
      spawnHelper: () => { spawnCount += 1; return helper as any; },
    });
    try {
      await primeEngineCapabilities(manager, helper);
      const owner = { type: 'ui', instance_id: 'voice-panel' } as const;
      const identity = { id: '00000000-0000-4000-8000-000000000042', generation: 1, service_epoch: 9 };
      const started = manager.executeAudioRequest({
        identity, owner, max_payload_bytes: 8_000_000,
        operation: { type: 'start_recording', sample_rate_hz: 16_000, format: 'wav' },
      });
      const envelope = await envelopeAt(helper, 1);
      helper.stdout.write(`${JSON.stringify({
        id: envelope.id, type: 'response', result: {
          type: 'engine_result', result: { type: 'recording_started', handle: 'recording' },
          snapshot: snapshot({ owner: { kind: 'ui', id: 'voice-panel' }, activity: 'listening', capabilities: AUDIO_CAPABILITIES, currentOperation: { identity, owner } }),
        },
      })}\n`);
      assert.equal((await started).type, 'recording_started');
      if (failure === 'exit') helper.emit('exit', 1, null);
      else helper.stdin.emit('error', new Error('write EPIPE'));
      await manager.cancelUiAudioOperations('voice-panel');
      assert.equal(spawnCount, 1);
      assert.equal(manager.getSnapshot().helper.state, 'failed');
    } finally {
      await manager.dispose();
      helper.stdin.end();
      helper.stdout.end();
      helper.stderr.end();
    }
  });
}

test('a broken audio helper input pipe rejects requests without crashing the host', async () => {
  const helper = new FakeHelperProcess();
  helper.stdin.write = ((chunk: string | Uint8Array, encodingOrCallback?: unknown) => {
    helper.writes.push(Buffer.from(chunk).toString('utf8'));
    queueMicrotask(() => {
      const error = Object.assign(new Error('write EPIPE'), { code: 'EPIPE' });
      if (typeof encodingOrCallback === 'function') encodingOrCallback(error);
      helper.stdin.emit('error', error);
    });
    return false;
  }) as typeof helper.stdin.write;
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
  });
  try {
    const response = await manager.request({ type: 'get_snapshot' });
    assert.equal(response.type, 'error');
    if (response.type === 'error') assert.match(response.error.message, /EPIPE/);
    assert.equal(helper.killCount, 1);
    assert.equal(manager.getSnapshot().helper.state, 'failed');
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('an invalid audio helper envelope fails the helper instead of escaping its stdout handler', async () => {
  const helper = new FakeHelperProcess();
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
  });
  try {
    const responsePromise = manager.request({ type: 'get_snapshot' });
    await nextEnvelope(helper);
    assert.doesNotThrow(() => helper.stdout.write('{"type":"event","event":{}}\n'));
    const response = await responsePromise;
    assert.equal(response.type, 'error');
    assert.equal(helper.killCount, 1);
    assert.equal(manager.getSnapshot().helper.state, 'failed');
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('audio operation telemetry preserves bounded payload-free trace and resource counts', () => {
  const trace = {
    identity: { id: '00000000-0000-4000-8000-000000000001', generation: 4, service_epoch: 7 },
    owner: { type: 'local_app', app_id: 'app-1', runtime_generation: 19 },
    operation: 'synthesize',
    configurationRevision: 12,
    requestedSource: 'automatic',
    requestedVoiceId: 'legacy:unknown-explicit',
    effectiveSource: 'offline',
    effectiveModelId: 'sherpa.melo-zh-en',
    fallbackReason: 'system speech is unavailable',
  } as const;
  const validated = validateNativeAudioSnapshot(snapshot({
    audioOperations: [trace],
    activeOperationCount: 2,
    pendingOperationCount: 1,
    activeRecordingCount: 1,
    activePlaybackCount: 1,
    activeModelReferenceCount: 3,
  }));
  assert.deepEqual(validated.audioOperations, [trace]);
  assert.equal(validated.activeOperationCount, 2);
  assert.equal(validated.pendingOperationCount, 1);
  assert.equal(validated.activeRecordingCount, 1);
  assert.equal(validated.activePlaybackCount, 1);
  assert.equal(validated.activeModelReferenceCount, 3);

  assert.throws(() => validateNativeAudioSnapshot(snapshot({
    audioOperations: Array.from({ length: 65 }, () => trace),
  })), /invalid audio operation trace/);
  assert.throws(() => validateNativeAudioSnapshot(snapshot({ activeOperationCount: -1 })), /active audio operation count/);
  assert.throws(() => validateNativeAudioSnapshot(snapshot({ pendingOperationCount: Number.MAX_SAFE_INTEGER + 1 })), /pending audio operation count/);
  assert.throws(() => validateNativeAudioSnapshot({
    ...snapshot(),
    audioOperations: [{ ...trace, transcript: 'must never be in diagnostics' }],
  }), /invalid audio operation trace/);
});

test('native audio manager forwards input levels without replacing the capability snapshot', async () => {
  const helper = new FakeHelperProcess();
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
  });
  try {
    const responsePromise = manager.request({ type: 'get_snapshot' });
    const envelope = await nextEnvelope(helper);
    const currentSnapshot = snapshot({
      localeTag: 'en-US',
      audioOperations: [{
        identity: { id: '00000000-0000-4000-8000-000000000002', generation: 2, service_epoch: 7 },
        owner: { type: 'session', session_id: 'session-1' },
        operation: 'listen',
        configurationRevision: 3,
      }],
      activeOperationCount: 1,
      activeRecordingCount: 1,
    });
    helper.stdout.write(`${JSON.stringify({
      id: envelope.id,
      type: 'response',
      result: { type: 'snapshot', snapshot: currentSnapshot },
    })}\n`);
    await responsePromise;
    const callerSnapshot = manager.getSnapshot();
    callerSnapshot.audioOperations[0]!.identity.generation = 99;
    callerSnapshot.audioOperations.pop();
    assert.equal(manager.getSnapshot().audioOperations[0]?.identity.generation, 2);
    assert.equal(manager.getSnapshot().audioOperations.length, 1);
    assert.equal(manager.getSnapshot().activeRecordingCount, 1);

    const eventPromise = new Promise((resolve) => {
      const unsubscribe = manager.onEvent((event) => {
        unsubscribe();
        resolve(event);
      });
    });
    helper.stdout.write(`${JSON.stringify({
      type: 'event',
      event: {
        type: 'input_level',
        owner: { kind: 'dictation', id: 'dictation-1' },
        level: 0.37,
      },
    })}\n`);

    assert.deepEqual(await eventPromise, {
      type: 'input_level',
      owner: { kind: 'dictation', id: 'dictation-1' },
      level: 0.37,
    });
    assert.deepEqual(manager.getSnapshot(), currentSnapshot);
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('packaged permissions use the outer app for microphone and foreground helper for Speech', async () => {
  const helper = new FakeHelperProcess();
  const launches: Array<{ appPath: string; permissions: string[] }> = [];
  const microphoneRequests: string[] = [];
  const manager = new NativeAudioManager({
    isPackaged: true,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/resources/LingXiAudioHelper.app/Contents/MacOS/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
    verifyPackagedHelper: () => {},
    requestMicrophoneAccess: async () => {
      microphoneRequests.push('microphone');
      return 'granted';
    },
    launchPermissionHelper: async (appPath: string, permissions: string[]) => {
      launches.push({ appPath, permissions });
    },
  });
  try {
    const authorization = manager.request({
      type: 'request_authorization',
      permissions: ['microphone', 'speech'],
    });
    const before = await envelopeAt(helper, 0);
    assert.equal(before.command?.type, 'get_snapshot');
    helper.stdout.write(`${JSON.stringify({
      id: before.id,
      type: 'response',
      result: {
        type: 'snapshot',
        snapshot: snapshot({ permissions: { microphone: 'denied', speech: 'not_determined' } }),
      },
    })}\n`);

    const after = await envelopeAt(helper, 1);
    assert.deepEqual(launches, [{
      appPath: '/resources/LingXiAudioHelper.app',
      permissions: ['speech'],
    }]);
    assert.deepEqual(microphoneRequests, ['microphone']);
    assert.equal(after.command?.type, 'get_snapshot');
    helper.stdout.write(`${JSON.stringify({
      id: after.id,
      type: 'response',
      result: { type: 'snapshot', snapshot: snapshot() },
    })}\n`);

    assert.deepEqual(await authorization, { type: 'authorization', snapshot: snapshot() });
    assert.equal(helper.writes.length, 2, 'the background JSONL helper must not request TCC permission itself');
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('packaged microphone authorization also requests the helper app TCC grant when needed', async () => {
  const helper = new FakeHelperProcess();
  const launches: string[][] = [];
  const manager = new NativeAudioManager({
    isPackaged: true,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/resources/LingXiAudioHelper.app/Contents/MacOS/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
    verifyPackagedHelper: () => {},
    requestMicrophoneAccess: async () => 'granted',
    launchPermissionHelper: async (_appPath: string, permissions: string[]) => {
      launches.push(permissions);
    },
  });
  try {
    const authorization = manager.request({ type: 'request_authorization', permissions: ['microphone'] });
    const firstSnapshot = await envelopeAt(helper, 0);
    assert.equal(firstSnapshot.command?.type, 'get_snapshot');
    helper.stdout.write(`${JSON.stringify({
      id: firstSnapshot.id,
      type: 'response',
      result: {
        type: 'snapshot',
        snapshot: snapshot({ permissions: { microphone: 'prompt', speech: 'authorized' } }),
      },
    })}\n`);

    const afterGrant = await envelopeAt(helper, 1);
    assert.deepEqual(launches, [['microphone']]);
    assert.equal(afterGrant.command?.type, 'get_snapshot');
    helper.stdout.write(`${JSON.stringify({
      id: afterGrant.id,
      type: 'response',
      result: {
        type: 'snapshot',
        snapshot: snapshot({ permissions: { microphone: 'granted', speech: 'authorized' } }),
      },
    })}\n`);

    const response = await authorization;
    assert.equal(response.type, 'authorization');
    assert.equal(response.snapshot.permissions.microphone, 'granted');
    assert.deepEqual(launches, [['microphone']]);
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('packaged Listen requests Speech only for automatic/system recognition and raw recording stays microphone-only', async () => {
  const runScenario = async (source: string, operationKind: 'listen' | 'start_recording') => {
    const helper = new FakeHelperProcess();
    const launches: string[][] = [];
    const microphoneRequests: string[] = [];
    const manager = new NativeAudioManager({
      isPackaged: true,
      resourcesPath: '/resources',
      userDataPath: '/tmp/lingxi-audio-tests',
      helperPath: '/resources/LingXiAudioHelper.app/Contents/MacOS/LingXiAudioHelper',
      diagnostics: new DiagnosticBuffer(),
      spawnHelper: () => helper as any,
      verifyPackagedHelper: () => {},
      requestMicrophoneAccess: async () => {
        microphoneRequests.push('microphone');
        return 'granted';
      },
      launchPermissionHelper: async (_appPath: string, permissions: string[]) => {
        launches.push(permissions);
      },
      getAudioConfiguration: () => ({
        ...audioConfigurationDefaults(),
        recognition: { source, offlineModelId: null },
      }),
    });
    try {
      await primeEngineCapabilities(manager, helper);
      const owner = { type: 'session', session_id: `permission-${operationKind}-${source}` } as const;
      const request = {
        identity: { id: '00000000-0000-4000-8000-00000000000d', generation: 1, service_epoch: 9 },
        owner,
        max_payload_bytes: 8_000_000,
        operation: operationKind === 'listen'
          ? { type: 'listen', language: 'en-US' }
          : { type: 'start_recording', sample_rate_hz: 16_000, format: 'wav' },
      } as const;
      const operationResult = manager.executeAudioRequest(request);

      const microphoneSnapshot = await envelopeAt(helper, 1);
      assert.equal(microphoneSnapshot.command?.type, 'get_snapshot');
      helper.stdout.write(`${JSON.stringify({
        id: microphoneSnapshot.id,
        type: 'response',
        result: {
          type: 'snapshot',
          snapshot: snapshot({
            capabilities: AUDIO_CAPABILITIES,
            permissions: { microphone: 'granted', speech: 'not_determined' },
          }),
        },
      })}\n`);

      let operationEnvelopeIndex = 2;
      if (operationKind === 'listen' && (source === 'automatic' || source === 'system')) {
        const speechSnapshot = await envelopeAt(helper, 2);
        assert.equal(speechSnapshot.command?.type, 'get_snapshot');
        helper.stdout.write(`${JSON.stringify({
          id: speechSnapshot.id,
          type: 'response',
          result: {
            type: 'snapshot',
            snapshot: snapshot({
              capabilities: AUDIO_CAPABILITIES,
              permissions: { microphone: 'granted', speech: 'not_determined' },
            }),
          },
        })}\n`);
        const authorizedSnapshot = await envelopeAt(helper, 3);
        assert.equal(authorizedSnapshot.command?.type, 'get_snapshot');
        assert.deepEqual(launches, [['speech']]);
        helper.stdout.write(`${JSON.stringify({
          id: authorizedSnapshot.id,
          type: 'response',
          result: {
            type: 'snapshot',
            snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }),
          },
        })}\n`);
        operationEnvelopeIndex = 4;
      }

      const operationEnvelope = await envelopeAt(helper, operationEnvelopeIndex) as {
        id: string;
        kind: string;
        request?: { operation?: { type?: string } };
      };
      assert.equal(operationEnvelope.kind, 'engine_request');
      assert.equal(operationEnvelope.request?.operation?.type, operationKind);
      helper.stdout.write(`${JSON.stringify({
        id: operationEnvelope.id,
        type: 'response',
        result: {
          type: 'engine_result',
          snapshot: snapshot({
            capabilities: AUDIO_CAPABILITIES,
            ...(operationKind === 'start_recording'
              ? { owner: { kind: 'engine', id: owner.session_id }, activity: 'listening' as const }
              : {}),
            currentOperation: { identity: request.identity, owner },
          }),
          result: operationKind === 'listen'
            ? { type: 'transcript', text: 'hello', language: 'en-US' }
            : { type: 'recording_started', handle: 'raw-recording-handle' },
        },
      })}\n`);
      const result = await operationResult;
      assert.deepEqual(microphoneRequests, ['microphone']);
      return { launches, result };
    } finally {
      await manager.dispose();
      helper.stdin.end();
      helper.stdout.end();
      helper.stderr.end();
    }
  };

  for (const source of ['automatic', 'system']) {
    const outcome = await runScenario(source, 'listen');
    assert.deepEqual(outcome.launches, [['speech']], `${source} Listen should request undetermined Speech permission`);
    assert.equal(outcome.result.type, 'transcript');
  }
  for (const source of ['offline', 'legacy:unknown']) {
    const outcome = await runScenario(source, 'listen');
    assert.deepEqual(outcome.launches, [], `${source} Listen must not request Speech permission`);
    assert.equal(outcome.result.type, 'transcript');
  }
  const rawRecording = await runScenario('automatic', 'start_recording');
  assert.deepEqual(rawRecording.launches, [], 'raw capture is microphone-only even when saved recognition is automatic');
  assert.deepEqual(rawRecording.result, { type: 'recording_started', handle: 'raw-recording-handle' });
});

test('cancellation while packaged Speech permission is pending prevents later native Listen admission', async () => {
  const helper = new FakeHelperProcess();
  let finishSpeechPermission!: () => void;
  let speechPermissionStarted!: () => void;
  const started = new Promise<void>((resolve) => { speechPermissionStarted = resolve; });
  const manager = new NativeAudioManager({
    isPackaged: true,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/resources/LingXiAudioHelper.app/Contents/MacOS/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
    verifyPackagedHelper: () => {},
    requestMicrophoneAccess: async () => 'granted',
    launchPermissionHelper: async () => {
      speechPermissionStarted();
      await new Promise<void>((resolve) => { finishSpeechPermission = resolve; });
    },
    getAudioConfiguration: () => ({
      ...audioConfigurationDefaults(),
      recognition: { source: 'system', offlineModelId: null },
    }),
  });
  try {
    await primeEngineCapabilities(manager, helper);
    const request = {
      identity: { id: '00000000-0000-4000-8000-00000000000e', generation: 1, service_epoch: 9 },
      owner: { type: 'session', session_id: 'speech-permission-session' },
      max_payload_bytes: 8_000_000,
      operation: { type: 'listen', language: 'en-US' },
    } as const;
    const listenResult = manager.executeAudioRequest(request);
    const microphoneSnapshot = await envelopeAt(helper, 1);
    helper.stdout.write(`${JSON.stringify({
      id: microphoneSnapshot.id,
      type: 'response',
      result: {
        type: 'snapshot',
        snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES, permissions: { microphone: 'granted', speech: 'not_determined' } }),
      },
    })}\n`);
    const speechSnapshot = await envelopeAt(helper, 2);
    helper.stdout.write(`${JSON.stringify({
      id: speechSnapshot.id,
      type: 'response',
      result: {
        type: 'snapshot',
        snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES, permissions: { microphone: 'granted', speech: 'not_determined' } }),
      },
    })}\n`);
    await started;

    await manager.cancelAudioRequest(request.identity);
    assert.deepEqual(await listenResult, {
      type: 'failed', error: { kind: 'cancelled', message: 'the audio operation was cancelled' },
    });

    finishSpeechPermission();
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(helper.writes.length, 3, 'a late Speech grant must not refresh/admit a native Listen request');
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('Listen deadline includes packaged microphone permission and prevents late native admission', async () => {
  const helper = new FakeHelperProcess();
  let finishPermission!: (value: 'granted') => void;
  let permissionStarted!: () => void;
  const started = new Promise<void>((resolve) => { permissionStarted = resolve; });
  const manager = new NativeAudioManager({
    isPackaged: true,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/resources/LingXiAudioHelper.app/Contents/MacOS/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
    verifyPackagedHelper: () => {},
    requestMicrophoneAccess: () => {
      permissionStarted();
      return new Promise((resolve) => { finishPermission = resolve; });
    },
    getAudioConfiguration: () => ({
      ...audioConfigurationDefaults(),
      recognition: { source: 'system', offlineModelId: null },
    }),
  });
  try {
    await primeEngineCapabilities(manager, helper);
    const request = {
      identity: { id: '00000000-0000-4000-8000-00000000000f', generation: 1, service_epoch: 9 },
      owner: { type: 'session', session_id: 'listen-deadline-session' },
      timeout_budget_ms: 25,
      max_payload_bytes: 8_000_000,
      operation: { type: 'listen', language: 'en-US' },
    } as const;
    const resultPromise = manager.executeAudioRequest(request);
    await started;
    const result = await Promise.race([
      resultPromise,
      new Promise<'permission-dialog-still-blocking'>((resolve) => setTimeout(() => resolve('permission-dialog-still-blocking'), 250)),
    ]);
    assert.deepEqual(result, {
      type: 'failed', error: { kind: 'timeout', message: 'the audio operation deadline expired before native admission' },
    });

    finishPermission('granted');
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(helper.writes.length, 1, 'a late permission grant must not trigger helper snapshot or native operation requests');
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('native audio manager routes command responses and enforces single owner', async () => {
  const helper = new FakeHelperProcess();
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
  });
  try {
    const listening = manager.request({
      type: 'start_listening',
      owner: { kind: 'dictation', id: 'dictation-1' },
      recognitionMode: 'automatic',
      language: 'en-US',
    });
    const first = await nextEnvelope(helper);
    helper.stdout.write(`${JSON.stringify({
      id: first.id,
      type: 'response',
      result: {
        type: 'listening_started',
        snapshot: snapshot({
          owner: { kind: 'dictation', id: 'dictation-1' },
          activity: 'listening',
        }),
      },
    })}\n`);
    assert.equal((await listening).type, 'listening_started');

    const busy = await manager.request({
      type: 'start_listening',
      owner: { kind: 'flow', id: 'flow-1' },
      recognitionMode: 'automatic',
      language: 'en-US',
    });
    assert.deepEqual(busy, {
      type: 'error',
      snapshot: snapshot({
        owner: { kind: 'dictation', id: 'dictation-1' },
        activity: 'listening',
      }),
      error: { code: 'busy', message: 'another audio operation is already active on this device' },
    });
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

const AUDIO_CAPABILITIES = {
  service_epoch: 9,
  support_revision: 1,
  supported_operations: ['record', 'listen', 'synthesize', 'speak'],
  readiness: [],
  max_payload_bytes: 8_000_000,
};

async function primeEngineCapabilities(manager: NativeAudioManager, helper: FakeHelperProcess): Promise<void> {
  const responsePromise = manager.request({ type: 'get_snapshot' });
  const envelope = await nextEnvelope(helper);
  helper.stdout.write(`${JSON.stringify({
    id: envelope.id,
    type: 'response',
    result: { type: 'snapshot', snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }) },
  })}\n`);
  await responsePromise;
}

test('first UI audio use initializes helper capabilities before minting its operation identity', async () => {
  const helper = new FakeHelperProcess();
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
  });
  try {
    const operation = manager.executeUiAudioOperation({ type: 'synthesize', text: 'first use' }, 'voice-panel');
    const capabilitiesRequest = await envelopeAt(helper, 0);
    assert.equal(capabilitiesRequest.kind, 'command');
    assert.equal(capabilitiesRequest.command?.type, 'get_snapshot');
    helper.stdout.write(`${JSON.stringify({
      id: capabilitiesRequest.id,
      type: 'response',
      result: { type: 'snapshot', snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }) },
    })}\n`);

    const operationRequest = await envelopeAt(helper, 1) as {
      id: string;
      kind: string;
      request?: { identity?: { id: string; generation: number; service_epoch: number }; owner?: unknown };
    };
    assert.equal(operationRequest.kind, 'engine_request');
    assert.equal(operationRequest.request?.identity?.service_epoch, AUDIO_CAPABILITIES.service_epoch);
    assert.deepEqual(operationRequest.request?.owner, { type: 'ui', instance_id: 'voice-panel' });
    const identity = operationRequest.request!.identity!;
    const owner = operationRequest.request!.owner!;
    helper.stdout.write(`${JSON.stringify({
      id: operationRequest.id,
      type: 'response',
      result: {
        type: 'engine_result',
        snapshot: snapshot({
          capabilities: AUDIO_CAPABILITIES,
          currentOperation: { identity, owner: owner as any },
        }),
        result: { type: 'synthesized', pcm_base64: 'AQI=', sample_rate_hz: 24_000 },
      },
    })}\n`);
    assert.equal((await operation).result.type, 'synthesized');
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('UI cancellation during initial capability refresh prevents later operation admission', async () => {
  const helper = new FakeHelperProcess();
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
  });
  try {
    const operation = manager.executeUiAudioOperation({ type: 'synthesize', text: 'must be cancelled' }, 'voice-panel');
    const capabilityRequest = await envelopeAt(helper, 0);
    assert.equal(capabilityRequest.command?.type, 'get_snapshot');

    const cancellation = manager.cancelUiAudioOperations('voice-panel');
    helper.stdout.write(`${JSON.stringify({
      id: capabilityRequest.id,
      type: 'response',
      result: { type: 'snapshot', snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }) },
    })}\n`);

    assert.deepEqual((await operation).result, {
      type: 'failed',
      error: { kind: 'cancelled', message: 'the audio operation was cancelled' },
    });
    const teardown = await envelopeAt(helper, 1) as {
      id: string;
      kind: string;
      request?: { operation?: { type?: string } };
    };
    assert.equal(teardown.kind, 'engine_request');
    assert.equal(teardown.request?.operation?.type, 'end_owner',
      'cancellation may tear down the UI owner but must not admit the cancelled synthesis');
    helper.stdout.write(`${JSON.stringify({
      id: teardown.id,
      type: 'response',
      result: {
        type: 'engine_result',
        snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }),
        result: { type: 'owner_ended' },
      },
    })}\n`);
    await cancellation;
    assert.equal(helper.writes.length, 2, 'only capability refresh and owner teardown reach the helper');
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('cancelling idle UI audio does not start the helper', async () => {
  const helper = new FakeHelperProcess();
  let spawnCount = 0;
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => {
      spawnCount += 1;
      return helper as any;
    },
  });
  try {
    await manager.cancelUiAudioOperations('voice-panel');
    assert.equal(spawnCount, 0);
    assert.equal(helper.writes.length, 0);
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('engine cancellation during initial capability refresh tombstones identity before native admission', async () => {
  const helper = new FakeHelperProcess();
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
  });
  const request = {
    identity: { id: '00000000-0000-4000-8000-000000000018', generation: 1, service_epoch: 9 },
    owner: { type: 'session', session_id: 'cancel-during-capabilities' },
    max_payload_bytes: 8_000_000,
    operation: { type: 'listen', language: 'en-US' },
  } as const;
  try {
    const operation = manager.executeAudioRequest(request);
    const capabilityRequest = await envelopeAt(helper, 0);
    assert.equal(capabilityRequest.command?.type, 'get_snapshot');

    await manager.cancelAudioRequest(request.identity);
    helper.stdout.write(`${JSON.stringify({
      id: capabilityRequest.id,
      type: 'response',
      result: { type: 'snapshot', snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }) },
    })}\n`);

    assert.deepEqual((await operation), {
      type: 'failed',
      error: { kind: 'cancelled', message: 'the audio operation identity is stale' },
    });
    assert.equal(helper.writes.length, 1, 'the cancelled listen must not be sent to the helper');
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('engine cancellation arriving before its request prevents helper startup', async () => {
  const helper = new FakeHelperProcess();
  let spawnCount = 0;
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => { spawnCount += 1; return helper as any; },
  });
  const request = {
    identity: { id: '00000000-0000-4000-8000-000000000019', generation: 1, service_epoch: 9 },
    owner: { type: 'session', session_id: 'cancel-before-request' },
    max_payload_bytes: 8_000_000,
    operation: { type: 'listen', language: 'en-US' },
  } as const;
  try {
    await manager.cancelAudioRequest(request.identity);
    assert.deepEqual(await manager.executeAudioRequest(request), {
      type: 'failed',
      error: { kind: 'cancelled', message: 'the audio operation identity is stale' },
    });
    assert.equal(spawnCount, 0);
    assert.deepEqual(helper.writes, []);
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('finish requested during capability startup is latched and sent after listen admission', async () => {
  const helper = new FakeHelperProcess();
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
  });
  try {
    const listen = manager.executeUiAudioOperation({ type: 'listen', language: 'en-US' }, 'voice-panel');
    const capabilityRequest = await envelopeAt(helper, 0);
    assert.equal(capabilityRequest.command?.type, 'get_snapshot');

    await manager.finishUiAudioListen('voice-panel');
    assert.equal(helper.writes.length, 1, 'finish is latched until capability refresh mints the listen identity');
    helper.stdout.write(`${JSON.stringify({
      id: capabilityRequest.id,
      type: 'response',
      result: { type: 'snapshot', snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }) },
    })}\n`);

    const listenEnvelope = await envelopeAt(helper, 1) as {
      id: string;
      request?: { identity?: { id: string; generation: number; service_epoch: number }; owner?: unknown };
    };
    const finish = await envelopeAt(helper, 2) as {
      id: string;
      command?: { type?: string; owner?: unknown; identity?: { id: string; generation: number; service_epoch: number } };
    };
    assert.equal(finish.command?.type, 'finish_listening');
    assert.deepEqual(finish.command?.owner, { kind: 'ui', id: 'voice-panel' });
    assert.equal(finish.command?.identity?.service_epoch, AUDIO_CAPABILITIES.service_epoch);
    assert.deepEqual(listenEnvelope.request?.identity, finish.command?.identity,
      'the finish control follows admission and targets that exact listen');
    assert.deepEqual(listenEnvelope.request?.owner, { type: 'ui', instance_id: 'voice-panel' });
    helper.stdout.write(`${JSON.stringify({
      id: finish.id,
      type: 'response',
      result: { type: 'listening_finished', snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }) },
    })}\n`);
    helper.stdout.write(`${JSON.stringify({
      id: listenEnvelope.id,
      type: 'response',
      result: {
        type: 'engine_result',
        snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }),
        result: { type: 'transcript', text: 'finished transcript', language: 'en-US' },
      },
    })}\n`);
    assert.deepEqual((await listen).result, { type: 'transcript', text: 'finished transcript', language: 'en-US' });
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('finish requested during packaged microphone permission targets the pending listen identity', async () => {
  const helper = new FakeHelperProcess();
  let finishPermission!: (value: 'granted') => void;
  let permissionStarted!: () => void;
  const started = new Promise<void>((resolve) => { permissionStarted = resolve; });
  const manager = new NativeAudioManager({
    isPackaged: true,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/resources/LingXiAudioHelper.app/Contents/MacOS/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
    verifyPackagedHelper: () => {},
    requestMicrophoneAccess: () => {
      permissionStarted();
      return new Promise((resolve) => { finishPermission = resolve; });
    },
    getAudioConfiguration: () => ({
      ...audioConfigurationDefaults(),
      recognition: { source: 'offline', offlineModelId: null },
    }),
  });
  try {
    await primeEngineCapabilities(manager, helper);
    const listen = manager.executeUiAudioOperation({ type: 'listen', language: 'en-US' }, 'voice-panel');
    await started;

    const finishPromise = manager.finishUiAudioListen('voice-panel');
    await finishPromise;
    assert.equal(helper.writes.length, 1, 'finish is latched in main while microphone permission is pending');

    finishPermission('granted');
    const refreshedPermissions = await envelopeAt(helper, 1);
    assert.equal(refreshedPermissions.command?.type, 'get_snapshot');
    helper.stdout.write(`${JSON.stringify({
      id: refreshedPermissions.id,
      type: 'response',
      result: { type: 'snapshot', snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }) },
    })}\n`);
    const listenEnvelope = await envelopeAt(helper, 2) as {
      id: string;
      request?: { identity?: { id: string; generation: number; service_epoch: number } };
    };
    const finish = await envelopeAt(helper, 3) as {
      id: string;
      command?: { type?: string; identity?: { id: string; generation: number; service_epoch: number } };
    };
    assert.equal(finish.command?.type, 'finish_listening');
    assert.deepEqual(listenEnvelope.request?.identity, finish.command?.identity,
      'finish is sent only after admission and targets the permission-delayed identity');
    helper.stdout.write(`${JSON.stringify({
      id: finish.id,
      type: 'response',
      result: { type: 'listening_finished', snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }) },
    })}\n`);
    helper.stdout.write(`${JSON.stringify({
      id: listenEnvelope.id,
      type: 'response',
      result: {
        type: 'engine_result',
        snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }),
        result: { type: 'transcript', text: 'permission-delayed transcript', language: 'en-US' },
      },
    })}\n`);
    assert.equal((await listen).result.type, 'transcript');
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('overlapping UI listens retain their identities so finish can drain each in order', async () => {
  const helper = new FakeHelperProcess();
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
  });
  try {
    await primeEngineCapabilities(manager, helper);
    const olderListen = manager.executeUiAudioOperation({ type: 'listen', language: 'en-US' }, 'voice-panel');
    const olderRequest = await envelopeAt(helper, 1) as {
      id: string;
      request?: { identity?: { id: string; generation: number; service_epoch: number }; owner?: unknown };
    };
    const newerListen = manager.executeUiAudioOperation({ type: 'listen', language: 'en-US' }, 'voice-panel');
    const newerRequest = await envelopeAt(helper, 2) as {
      id: string;
      request?: { identity?: { id: string; generation: number; service_epoch: number }; owner?: unknown };
    };
    assert.notDeepEqual(olderRequest.request?.identity, newerRequest.request?.identity);

    const finishOlderPromise = manager.finishUiAudioListen('voice-panel');
    const finishOlder = await envelopeAt(helper, 3) as {
      id: string;
      command?: { type?: string; identity?: { id: string; generation: number; service_epoch: number } };
    };
    assert.equal(finishOlder.command?.type, 'finish_listening');
    assert.deepEqual(finishOlder.command?.identity, olderRequest.request?.identity,
      'the earliest active listen remains reachable after a newer listen starts');
    helper.stdout.write(`${JSON.stringify({
      id: finishOlder.id,
      type: 'response',
      result: {
        type: 'listening_finished',
        snapshot: snapshot({
          owner: { kind: 'ui', id: 'voice-panel' },
          activity: 'listening',
          capabilities: AUDIO_CAPABILITIES,
          currentOperation: {
            identity: newerRequest.request!.identity!,
            owner: newerRequest.request!.owner as any,
          },
        }),
      },
    })}\n`);
    await finishOlderPromise;
    helper.stdout.write(`${JSON.stringify({
      id: olderRequest.id,
      type: 'response',
      result: {
        type: 'engine_result',
        snapshot: snapshot({
          owner: { kind: 'ui', id: 'voice-panel' },
          activity: 'listening',
          capabilities: AUDIO_CAPABILITIES,
          currentOperation: {
            identity: newerRequest.request!.identity!,
            owner: newerRequest.request!.owner as any,
          },
        }),
        result: { type: 'transcript', text: 'older overlap transcript', language: 'en-US' },
      },
    })}\n`);
    assert.equal((await olderListen).result.type, 'transcript');

    const finishNewerPromise = manager.finishUiAudioListen('voice-panel');
    const finishNewer = await envelopeAt(helper, 4) as {
      id: string;
      command?: { type?: string; identity?: { id: string; generation: number; service_epoch: number } };
    };
    assert.equal(finishNewer.command?.type, 'finish_listening');
    assert.deepEqual(finishNewer.command?.identity, newerRequest.request?.identity,
      'finishing the older listen leaves the newer generation independently finishable');
    helper.stdout.write(`${JSON.stringify({
      id: finishNewer.id,
      type: 'response',
      result: { type: 'listening_finished', snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }) },
    })}\n`);
    await finishNewerPromise;
    helper.stdout.write(`${JSON.stringify({
      id: newerRequest.id,
      type: 'response',
      result: {
        type: 'engine_result',
        snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }),
        result: { type: 'transcript', text: 'newer overlap transcript', language: 'en-US' },
      },
    })}\n`);
    assert.equal((await newerListen).result.type, 'transcript');
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('a helper busy result for a second listen leaves the first listen finishable', async () => {
  const helper = new FakeHelperProcess();
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
  });
  try {
    await primeEngineCapabilities(manager, helper);
    const firstListen = manager.executeUiAudioOperation({ type: 'listen', language: 'en-US' }, 'voice-panel');
    const firstRequest = await envelopeAt(helper, 1) as {
      id: string;
      request?: { identity?: { id: string; generation: number; service_epoch: number }; owner?: unknown };
    };
    const secondListen = manager.executeUiAudioOperation({ type: 'listen', language: 'en-US' }, 'voice-panel');
    const secondRequest = await envelopeAt(helper, 2) as {
      id: string;
      request?: { identity?: { id: string; generation: number; service_epoch: number }; owner?: unknown };
    };
    helper.stdout.write(`${JSON.stringify({
      id: secondRequest.id,
      type: 'response',
      result: {
        type: 'engine_result',
        snapshot: snapshot({
          owner: { kind: 'ui', id: 'voice-panel' },
          activity: 'listening',
          capabilities: AUDIO_CAPABILITIES,
          currentOperation: {
            identity: firstRequest.request!.identity!,
            owner: firstRequest.request!.owner as any,
          },
        }),
        result: { type: 'failed', error: { kind: 'busy', message: 'another audio operation is already active' } },
      },
    })}\n`);
    assert.deepEqual((await secondListen).result, {
      type: 'failed', error: { kind: 'busy', message: 'another audio operation is already active' },
    });

    const finishFirstPromise = manager.finishUiAudioListen('voice-panel');
    const finishFirst = await envelopeAt(helper, 3) as {
      id: string;
      command?: { type?: string; identity?: { id: string; generation: number; service_epoch: number } };
    };
    assert.equal(finishFirst.command?.type, 'finish_listening');
    assert.deepEqual(finishFirst.command?.identity, firstRequest.request?.identity,
      'the helper-rejected listen must not replace the still-active listen identity');
    helper.stdout.write(`${JSON.stringify({
      id: finishFirst.id,
      type: 'response',
      result: { type: 'listening_finished', snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }) },
    })}\n`);
    await finishFirstPromise;
    helper.stdout.write(`${JSON.stringify({
      id: firstRequest.id,
      type: 'response',
      result: {
        type: 'engine_result',
        snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }),
        result: { type: 'transcript', text: 'first listen transcript', language: 'en-US' },
      },
    })}\n`);
    assert.equal((await firstListen).result.type, 'transcript');
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('a finish sent for an older UI listen cannot target or clear a subsequent generation', async () => {
  const helper = new FakeHelperProcess();
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
  });
  try {
    await primeEngineCapabilities(manager, helper);
    const olderListen = manager.executeUiAudioOperation({ type: 'listen', language: 'en-US' }, 'voice-panel');
    const olderRequest = await envelopeAt(helper, 1) as {
      id: string;
      request?: { identity?: { id: string; generation: number; service_epoch: number }; owner?: unknown };
    };
    assert.equal(olderRequest.request?.identity?.generation, 1);
    const olderFinishPromise = manager.finishUiAudioListen('voice-panel');
    const olderFinish = await envelopeAt(helper, 2) as {
      id: string;
      command?: { type?: string; identity?: { id: string; generation: number; service_epoch: number } };
    };
    assert.equal(olderFinish.command?.type, 'finish_listening');
    assert.deepEqual(olderFinish.command?.identity, olderRequest.request?.identity);

    const newerListen = manager.executeUiAudioOperation({ type: 'listen', language: 'en-US' }, 'voice-panel');
    const newerRequest = await envelopeAt(helper, 3) as {
      id: string;
      request?: { identity?: { id: string; generation: number; service_epoch: number }; owner?: unknown };
    };
    assert.equal(newerRequest.request?.identity?.generation, 2);
    assert.notDeepEqual(newerRequest.request?.identity, olderRequest.request?.identity);

    helper.stdout.write(`${JSON.stringify({
      id: olderFinish.id,
      type: 'response',
      result: {
        type: 'listening_finished',
        snapshot: snapshot({
          owner: { kind: 'ui', id: 'voice-panel' },
          activity: 'listening',
          capabilities: AUDIO_CAPABILITIES,
          currentOperation: {
            identity: newerRequest.request!.identity!,
            owner: { type: 'ui', instance_id: 'voice-panel' },
          },
        }),
      },
    })}\n`);
    await olderFinishPromise;
    helper.stdout.write(`${JSON.stringify({
      id: olderRequest.id,
      type: 'response',
      result: {
        type: 'engine_result',
        snapshot: snapshot({
          owner: { kind: 'ui', id: 'voice-panel' },
          activity: 'listening',
          capabilities: AUDIO_CAPABILITIES,
          currentOperation: {
            identity: newerRequest.request!.identity!,
            owner: { type: 'ui', instance_id: 'voice-panel' },
          },
        }),
        result: { type: 'transcript', text: 'older result', language: 'en-US' },
      },
    })}\n`);
    assert.equal((await olderListen).result.type, 'transcript');

    const newerFinishPromise = manager.finishUiAudioListen('voice-panel');
    const newerFinish = await envelopeAt(helper, 4) as {
      id: string;
      command?: { type?: string; identity?: { id: string; generation: number; service_epoch: number } };
    };
    assert.equal(newerFinish.command?.type, 'finish_listening');
    assert.deepEqual(newerFinish.command?.identity, newerRequest.request?.identity,
      'the active UI listen map must still point to the newer generation after the older result settles');
    helper.stdout.write(`${JSON.stringify({
      id: newerFinish.id,
      type: 'response',
      result: { type: 'listening_finished', snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }) },
    })}\n`);
    await newerFinishPromise;
    helper.stdout.write(`${JSON.stringify({
      id: newerRequest.id,
      type: 'response',
      result: {
        type: 'engine_result',
        snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }),
        result: { type: 'transcript', text: 'newer result', language: 'en-US' },
      },
    })}\n`);
    assert.equal((await newerListen).result.type, 'transcript');
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('an active UI listen propagates helper finish failures and allows retry', async () => {
  const helper = new FakeHelperProcess();
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
  });
  try {
    await primeEngineCapabilities(manager, helper);
    const listen = manager.executeUiAudioOperation({ type: 'listen', language: 'en-US' }, 'voice-panel');
    const listenEnvelope = await envelopeAt(helper, 1) as {
      id: string;
      request?: { identity?: { id: string; generation: number; service_epoch: number } };
    };

    const failedFinish = manager.finishUiAudioListen('voice-panel');
    const failedFinishEnvelope = await envelopeAt(helper, 2);
    helper.stdout.write(`${JSON.stringify({
      id: failedFinishEnvelope.id,
      type: 'error',
      error: { message: 'could not finalize this listen' },
    })}\n`);
    await assert.rejects(failedFinish, /could not finalize this listen/);

    const retriedFinish = manager.finishUiAudioListen('voice-panel');
    const retryEnvelope = await envelopeAt(helper, 3) as {
      id: string;
      command?: { type?: string; identity?: { id: string; generation: number; service_epoch: number } };
    };
    assert.equal(retryEnvelope.command?.type, 'finish_listening');
    assert.deepEqual(retryEnvelope.command?.identity, listenEnvelope.request?.identity,
      'retry keeps targeting the still-active listen identity');
    helper.stdout.write(`${JSON.stringify({
      id: retryEnvelope.id,
      type: 'response',
      result: { type: 'listening_finished', snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }) },
    })}\n`);
    await retriedFinish;

    helper.stdout.write(`${JSON.stringify({
      id: listenEnvelope.id,
      type: 'response',
      result: {
        type: 'engine_result',
        snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }),
        result: { type: 'transcript', text: 'retry transcript', language: 'en-US' },
      },
    })}\n`);
    assert.deepEqual((await listen).result, { type: 'transcript', text: 'retry transcript', language: 'en-US' });
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('helper restart refreshes capabilities and rejects a request carrying the old service epoch', async () => {
  const firstHelper = new FakeHelperProcess();
  const restartedHelper = new FakeHelperProcess();
  let spawnCount = 0;
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => (spawnCount++ === 0 ? firstHelper : restartedHelper) as any,
  });
  try {
    await primeEngineCapabilities(manager, firstHelper);
    firstHelper.kill();

    const nextCapabilities = { ...AUDIO_CAPABILITIES, service_epoch: 10 };
    const freshOperation = manager.executeUiAudioOperation({ type: 'synthesize', text: 'fresh epoch' }, 'voice-panel');
    const restartedCapabilities = await envelopeAt(restartedHelper, 0);
    assert.equal(restartedCapabilities.command?.type, 'get_snapshot');
    firstHelper.stdout.write('{"type":"event","event":');
    restartedHelper.stdout.write(`${JSON.stringify({
      id: restartedCapabilities.id,
      type: 'response',
      result: { type: 'snapshot', snapshot: snapshot({ capabilities: nextCapabilities }) },
    })}\n`);

    const admitted = await envelopeAt(restartedHelper, 1) as {
      id: string;
      kind: string;
      request?: { identity?: { id: string; generation: number; service_epoch: number }; owner?: unknown };
    };
    assert.equal(admitted.kind, 'engine_request');
    assert.equal(admitted.request?.identity?.service_epoch, nextCapabilities.service_epoch);
    restartedHelper.stdout.write(`${JSON.stringify({
      id: admitted.id,
      type: 'response',
      result: {
        type: 'engine_result',
        snapshot: snapshot({
          capabilities: nextCapabilities,
          currentOperation: {
            identity: admitted.request!.identity!,
            owner: admitted.request!.owner as any,
          },
        }),
        result: { type: 'synthesized', pcm_base64: 'AwQ=', sample_rate_hz: 24_000 },
      },
    })}\n`);
    assert.equal((await freshOperation).result.type, 'synthesized');
    firstHelper.stdout.write(`${JSON.stringify({
      type: 'event',
      event: { type: 'snapshot_changed', snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }) },
    })}\n`);
    assert.equal(manager.getCapabilities().service_epoch, nextCapabilities.service_epoch);

    const staleOperation = manager.executeAudioRequest({
      identity: { id: '00000000-0000-4000-8000-000000000017', generation: 1, service_epoch: 9 },
      owner: { type: 'session', session_id: 'stale-epoch-session' },
      max_payload_bytes: 8_000_000,
      operation: { type: 'speak', text: 'must not be admitted' },
    });
    assert.deepEqual(await staleOperation, {
      type: 'failed',
      error: { kind: 'cancelled', message: 'the audio service changed before this request started' },
    });
    assert.equal(restartedHelper.writes.length, 2, 'a stale identity must not reach the restarted helper');
  } finally {
    await manager.dispose();
    firstHelper.stdin.end();
    firstHelper.stdout.end();
    firstHelper.stderr.end();
    restartedHelper.stdin.end();
    restartedHelper.stdout.end();
    restartedHelper.stderr.end();
  }
});

test('window suspension tears down the active structured owner with end_owner', async () => {
  const helper = new FakeHelperProcess();
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
  });
  try {
    await primeEngineCapabilities(manager, helper);
    const owner = { type: 'ui', instance_id: 'voice-panel' } as const;
    const identity = { id: '00000000-0000-4000-8000-000000000012', generation: 4, service_epoch: 9 } as const;
    const readSnapshot = manager.request({ type: 'get_snapshot' });
    const snapshotRequest = await envelopeAt(helper, 1);
    helper.stdout.write(`${JSON.stringify({
      id: snapshotRequest.id,
      type: 'response',
      result: {
        type: 'snapshot',
        snapshot: snapshot({
          owner: { kind: 'ui', id: owner.instance_id },
          activity: 'speaking',
          capabilities: AUDIO_CAPABILITIES,
          currentOperation: { identity, owner },
        }),
      },
    })}\n`);
    await readSnapshot;

    const suspended = manager.suspend('window-hidden');
    const teardown = await envelopeAt(helper, 2) as {
      id: string;
      kind: string;
      request?: { owner?: unknown; operation?: { type?: string } };
    };
    assert.equal(teardown.kind, 'engine_request');
    assert.equal(teardown.request?.operation?.type, 'end_owner');
    assert.deepEqual(teardown.request?.owner, owner);
    helper.stdout.write(`${JSON.stringify({
      id: teardown.id,
      type: 'response',
      result: {
        type: 'engine_result',
        snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }),
        result: { type: 'owner_ended' },
      },
    })}\n`);
    await suspended;
    assert.equal(manager.getSnapshot().owner, null);
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('window suspension stops the helper when native owner teardown fails', async () => {
  const helper = new FakeHelperProcess();
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
  });
  try {
    await primeEngineCapabilities(manager, helper);
    const owner = { type: 'ui', instance_id: 'hidden-owner' } as const;
    const identity = { id: '00000000-0000-4000-8000-000000000078', generation: 1, service_epoch: 9 } as const;
    const readSnapshot = manager.request({ type: 'get_snapshot' });
    const snapshotRequest = await envelopeAt(helper, 1);
    helper.stdout.write(`${JSON.stringify({
      id: snapshotRequest.id,
      type: 'response',
      result: {
        type: 'snapshot',
        snapshot: snapshot({
          owner: { kind: 'ui', id: owner.instance_id },
          activity: 'speaking',
          capabilities: AUDIO_CAPABILITIES,
          currentOperation: { identity, owner },
        }),
      },
    })}\n`);
    await readSnapshot;

    const suspended = manager.suspend('window-hidden');
    const teardown = await envelopeAt(helper, 2) as {
      id: string;
      request?: { operation?: { type?: string } };
    };
    assert.equal(teardown.request?.operation?.type, 'end_owner');
    helper.stdout.write(`${JSON.stringify({
      id: teardown.id,
      type: 'response',
      result: {
        type: 'engine_result',
        snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }),
        result: { type: 'failed', error: { kind: 'native_failure', message: 'native stop failed' } },
      },
    })}\n`);
    await suspended;
    assert.equal(helper.killCount, 1);
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('window suspension bounds an unresponsive cancellation command', async () => {
  const helper = new FakeHelperProcess();
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
    suspendTeardownTimeoutMs: 30,
  });
  try {
    await primeEngineCapabilities(manager, helper);
    const identity = { id: '00000000-0000-4000-8000-000000000079', generation: 1, service_epoch: 9 } as const;
    const operation = manager.executeAudioRequest({
      identity,
      owner: { type: 'ui', instance_id: 'stalled-cancellation' },
      max_payload_bytes: 8_000_000,
      operation: { type: 'listen' },
    });
    await envelopeAt(helper, 1);
    const suspended = manager.suspend('window-hidden');
    const cancel = await envelopeAt(helper, 2);
    assert.equal(cancel.command?.type, 'cancel_operation');
    assert.strictEqual(manager.suspend('window-minimized'), suspended,
      'overlapping window events share one teardown and keep admission suspended');
    await suspended;
    assert.equal(helper.killCount, 1);
    assert.equal((await operation).type, 'failed');
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('suspension cancels a request waiting for helper capabilities before native admission', async () => {
  const helper = new FakeHelperProcess();
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
  });
  try {
    const result = manager.executeAudioRequest({
      identity: { id: '00000000-0000-4000-8000-000000000080', generation: 1, service_epoch: 9 },
      owner: { type: 'session', session_id: 'waiting-for-capabilities' },
      max_payload_bytes: 8_000_000,
      operation: { type: 'speak', text: 'hello' },
    });
    const capabilities = await envelopeAt(helper, 0);
    await manager.suspend('window-hidden');
    helper.stdout.write(`${JSON.stringify({
      id: capabilities.id,
      type: 'response',
      result: { type: 'snapshot', snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }) },
    })}\n`);
    assert.equal((await result).type, 'failed');
    assert.equal(helper.writes.length, 1, 'the hidden window must not admit the queued operation');
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('window suspension cancels pending owners and tears down owners beyond a stale helper snapshot', async () => {
  const helper = new FakeHelperProcess();
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
  });
  try {
    await primeEngineCapabilities(manager, helper);
    const ownerA = { type: 'ui', instance_id: 'owner-a' } as const;
    const ownerB = { type: 'ui', instance_id: 'owner-b' } as const;
    const identityA = { id: '00000000-0000-4000-8000-000000000014', generation: 1, service_epoch: 9 } as const;
    const identityB = { id: '00000000-0000-4000-8000-000000000015', generation: 2, service_epoch: 9 } as const;

    const snapshotRequest = manager.request({ type: 'get_snapshot' });
    const snapshotEnvelope = await envelopeAt(helper, 1);
    helper.stdout.write(`${JSON.stringify({
      id: snapshotEnvelope.id,
      type: 'response',
      result: {
        type: 'snapshot',
        snapshot: snapshot({
          owner: { kind: 'ui', id: ownerA.instance_id },
          activity: 'speaking',
          capabilities: AUDIO_CAPABILITIES,
          currentOperation: { identity: identityA, owner: ownerA },
        }),
      },
    })}\n`);
    await snapshotRequest;

    const ownerBOperation = manager.executeAudioRequest({
      identity: identityB,
      owner: ownerB,
      max_payload_bytes: 8_000_000,
      operation: { type: 'speak', text: 'owner B is playing without a start event' },
    });
    const ownerBEnvelope = await envelopeAt(helper, 2);
    assert.equal(ownerBEnvelope.kind, 'engine_request');

    const suspended = manager.suspend('window-hidden');
    const cancellation = await envelopeAt(helper, 3) as { id: string; command?: { type?: string; identity?: unknown } };
    assert.equal(cancellation.command?.type, 'cancel_operation');
    assert.deepEqual(cancellation.command?.identity, identityB);
    helper.stdout.write(`${JSON.stringify({
      id: cancellation.id,
      type: 'response',
      result: { type: 'cancelled', snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }) },
    })}\n`);

    const endedOwners = new Set<string>();
    for (const index of [4, 5]) {
      const teardown = await envelopeAt(helper, index) as {
        id: string;
        kind: string;
        request?: { owner?: { type?: string; instance_id?: string }; operation?: { type?: string } };
      };
      assert.equal(teardown.kind, 'engine_request');
      assert.equal(teardown.request?.operation?.type, 'end_owner');
      const instanceId = teardown.request?.owner?.instance_id;
      assert.ok(instanceId === ownerA.instance_id || instanceId === ownerB.instance_id);
      endedOwners.add(instanceId);
      const stillActiveB = instanceId === ownerA.instance_id;
      helper.stdout.write(`${JSON.stringify({
        id: teardown.id,
        type: 'response',
        result: {
          type: 'engine_result',
          snapshot: stillActiveB
            ? snapshot({
                owner: { kind: 'ui', id: ownerB.instance_id },
                activity: 'speaking',
                capabilities: AUDIO_CAPABILITIES,
                currentOperation: { identity: identityB, owner: ownerB },
              })
            : snapshot({ capabilities: AUDIO_CAPABILITIES }),
          result: { type: 'owner_ended' },
        },
      })}\n`);
    }
    await suspended;
    assert.deepEqual([...endedOwners].sort(), [ownerA.instance_id, ownerB.instance_id]);
    assert.deepEqual(await ownerBOperation, {
      type: 'failed', error: { kind: 'cancelled', message: 'the audio operation was cancelled' },
    });
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('a UI preview uses its normalized configuration override without pinning saved Flow configuration', async () => {
  const helper = new FakeHelperProcess();
  let savedConfigurationReads = 0;
  const savedConfiguration = audioConfigurationDefaults();
  const previewConfiguration = {
    ...audioConfigurationDefaults(),
    speech: {
      source: 'offline',
      offlineModelId: 'sherpa.melo-zh-en',
      voice: { source: 'offline', modelId: 'sherpa.melo-zh-en', id: 'voice-preview' },
    },
    language: 'zh-CN',
    rate: 1.25,
  } as const;
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
    getAudioConfiguration: () => {
      savedConfigurationReads += 1;
      return savedConfiguration;
    },
    getAudioConfigurationRevision: () => 42,
  });
  try {
    await primeEngineCapabilities(manager, helper);
    const preview = manager.executeUiAudioOperation(
      { type: 'synthesize', text: 'Preview' },
      'voice-settings',
      undefined,
      previewConfiguration,
    );
    const envelope = await envelopeAt(helper, 1) as {
      id: string;
      request?: { identity?: { id: string }; owner?: unknown; operation?: unknown };
      configuration?: { speech?: { source?: string; offlineModelId?: string | null }; language?: string; rate?: number };
      configurationRevision?: number;
    };
    assert.deepEqual(envelope.request?.owner, { type: 'ui', instance_id: 'voice-settings' });
    assert.equal(envelope.configuration?.speech?.source, 'offline');
    assert.equal(envelope.configuration?.speech?.offlineModelId, 'sherpa.melo-zh-en');
    assert.equal(envelope.configuration?.language, 'zh-CN');
    assert.equal(envelope.configuration?.rate, 1.25);
    assert.equal(envelope.configurationRevision, 0, 'a preview override does not claim a saved configuration revision');
    assert.equal(savedConfigurationReads, 0, 'preview must not fall back to saved Flow preferences');

    helper.stdout.write(`${JSON.stringify({
      id: envelope.id,
      type: 'response',
      result: {
        type: 'engine_result',
        snapshot: snapshot({
          capabilities: AUDIO_CAPABILITIES,
          currentOperation: {
            identity: { id: envelope.request?.identity?.id, generation: 1, service_epoch: 9 },
            owner: { type: 'ui', instance_id: 'voice-settings' },
          },
        }),
        result: { type: 'synthesized', pcm_base64: 'AQI=', sample_rate_hz: 24_000 },
      },
    })}\n`);
    const previewResponse = await preview;
    assert.equal(previewResponse.result.type, 'synthesized');

    const flowOperation = manager.executeUiAudioOperation({ type: 'synthesize', text: 'Saved' }, 'voice-settings', 42);
    const flowEnvelope = await envelopeAt(helper, 2) as {
      id: string;
      request?: { identity?: { id: string } };
      configuration?: { speech?: { source?: string }; language?: string };
      configurationRevision?: number;
    };
    assert.equal(flowEnvelope.configurationRevision, 42);
    assert.equal(flowEnvelope.configuration?.speech?.source, savedConfiguration.speech.source);
    assert.equal(flowEnvelope.configuration?.language, savedConfiguration.language);
    helper.stdout.write(`${JSON.stringify({
      id: flowEnvelope.id,
      type: 'response',
      result: {
        type: 'engine_result',
        snapshot: snapshot({
          capabilities: AUDIO_CAPABILITIES,
          currentOperation: {
            identity: { id: flowEnvelope.request?.identity?.id, generation: 2, service_epoch: 9 },
            owner: { type: 'ui', instance_id: 'voice-settings' },
          },
        }),
        result: { type: 'synthesized', pcm_base64: 'AwQ=', sample_rate_hz: 24_000 },
      },
    })}\n`);
    assert.equal((await flowOperation).result.type, 'synthesized');
    assert.equal(savedConfigurationReads, 1, 'a later Flow operation still reads saved settings');
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('UI speech keeps a long native response window without an engine timeout budget', async () => {
  const helper = new FakeHelperProcess();
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
  });
  try {
    await primeEngineCapabilities(manager, helper);
    const speech = manager.executeUiAudioOperation({ type: 'speak', text: 'long response' }, 'voice-panel');
    const envelope = await envelopeAt(helper, 1);
    const pending = (manager as unknown as { pending: Map<string, { timer: NodeJS.Timeout }> }).pending.get(envelope.id);
    assert.equal((pending?.timer as unknown as { _idleTimeout: number })._idleTimeout, 20 * 60_000);
    helper.stdout.write(`${JSON.stringify({
      id: envelope.id,
      type: 'response',
      result: {
        type: 'engine_result',
        snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }),
        result: { type: 'playback_completed', duration_ms: 1_000 },
      },
    })}\n`);
    assert.equal((await speech).result.type, 'playback_completed');
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('native audio manager dispatches a protocol v17 request through helper jsonl', async () => {
  const helper = new FakeHelperProcess();
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
  });
  try {
    await primeEngineCapabilities(manager, helper);
    const request = {
      identity: { id: '00000000-0000-4000-8000-000000000001', generation: 1, service_epoch: 9 },
      owner: { type: 'session', session_id: 'session-1' },
      max_payload_bytes: 8_000_000,
      operation: { type: 'synthesize', text: 'hello', voice: 'system:Alex' },
    } as const;
    const resultPromise = manager.executeAudioRequest(request);
    const envelope = await envelopeAt(helper, 1);
    assert.equal(envelope.kind, 'engine_request');
    helper.stdout.write(`${JSON.stringify({
      id: envelope.id,
      type: 'response',
      result: {
        type: 'engine_result',
        snapshot: snapshot({
          owner: { kind: 'engine', id: 'session-1' },
          activity: 'speaking',
          localeTag: 'fr-FR',
          capabilities: AUDIO_CAPABILITIES,
          currentOperation: { identity: request.identity, owner: request.owner },
        }),
        result: { type: 'synthesized', pcm_base64: 'AQI=', sample_rate_hz: 24_000 },
      },
    })}\n`);

    assert.deepEqual(await resultPromise, { type: 'synthesized', pcm_base64: 'AQI=', sample_rate_hz: 24_000 });
    assert.equal(manager.getSnapshot().localeTag, 'fr-FR', 'engine responses must retain the helper capability snapshot');
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('zero audio deadline expires before helper startup or microphone permission', async () => {
  const helper = new FakeHelperProcess();
  let spawnCount = 0;
  let permissionRequests = 0;
  const manager = new NativeAudioManager({
    isPackaged: true,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/resources/LingXiAudioHelper.app/Contents/MacOS/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => { spawnCount += 1; return helper as any; },
    verifyPackagedHelper: () => {},
    requestMicrophoneAccess: async () => { permissionRequests += 1; return 'granted'; },
  });
  try {
    const result = await manager.executeAudioRequest({
      identity: { id: '00000000-0000-4000-8000-00000000000a', generation: 1, service_epoch: 9 },
      owner: { type: 'session', session_id: 'zero-budget-session' },
      timeout_budget_ms: 0,
      max_payload_bytes: 8_000_000,
      operation: { type: 'start_recording', sample_rate_hz: 16_000, format: 'wav' },
    });

    assert.deepEqual(result, {
      type: 'failed', error: { kind: 'timeout', message: 'the audio operation deadline expired before native admission' },
    });
    assert.equal(permissionRequests, 0);
    assert.equal(spawnCount, 0);
    assert.deepEqual(helper.writes, []);
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('an audio deadline after native admission cancels the helper operation with the remaining budget', async () => {
  const helper = new FakeHelperProcess();
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
  });
  try {
    await primeEngineCapabilities(manager, helper);
    const identity = { id: '00000000-0000-4000-8000-000000000013', generation: 1, service_epoch: 9 } as const;
    const request = {
      identity,
      owner: { type: 'session', session_id: 'deadline-session' },
      timeout_budget_ms: 120,
      max_payload_bytes: 8_000_000,
      operation: { type: 'listen', language: 'en-US' },
    } as const;
    const resultPromise = manager.executeAudioRequest(request);
    const admitted = await envelopeAt(helper, 1) as {
      id: string;
      request?: { identity?: unknown; timeout_budget_ms?: number };
    };
    assert.deepEqual(admitted.request?.identity, identity);
    assert.equal(typeof admitted.request?.timeout_budget_ms, 'number');
    assert.ok(admitted.request.timeout_budget_ms! > 0 && admitted.request.timeout_budget_ms! <= request.timeout_budget_ms,
      'the helper receives only the request budget remaining at native admission');

    assert.deepEqual(await resultPromise, {
      type: 'failed',
      error: { kind: 'timeout', message: 'the audio operation deadline expired after native admission' },
    });
    const cancellation = await envelopeAt(helper, 2) as {
      id: string;
      command?: { type?: string; identity?: unknown };
    };
    assert.equal(cancellation.command?.type, 'cancel_operation');
    assert.deepEqual(cancellation.command?.identity, identity);
    helper.stdout.write(`${JSON.stringify({
      id: cancellation.id,
      type: 'response',
      result: { type: 'cancelled', snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }) },
    })}\n`);
    await new Promise((resolve) => setImmediate(resolve));
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('an inner helper timeout that wins first still sends targeted operation cancellation', async () => {
  const helper = new FakeHelperProcess();
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
  });
  try {
    await primeEngineCapabilities(manager, helper);
    const identity = { id: '00000000-0000-4000-8000-000000000016', generation: 1, service_epoch: 9 } as const;
    const originalSetTimeout = globalThis.setTimeout;
    let delayOuterDeadline = true;
    globalThis.setTimeout = ((handler: Parameters<typeof setTimeout>[0], timeout?: number, ...args: unknown[]) => {
      const delay = delayOuterDeadline ? ((delayOuterDeadline = false), (timeout ?? 0) + 100) : timeout;
      return originalSetTimeout(handler, delay, ...args);
    }) as typeof globalThis.setTimeout;
    let resultPromise: Promise<unknown>;
    try {
      resultPromise = manager.executeAudioRequest({
        identity,
        owner: { type: 'session', session_id: 'inner-timeout-session' },
        timeout_budget_ms: 80,
        max_payload_bytes: 8_000_000,
        operation: { type: 'listen', language: 'en-US' },
      });
      const admitted = await envelopeAt(helper, 1);
      assert.equal(admitted.kind, 'engine_request');
    } finally {
      globalThis.setTimeout = originalSetTimeout;
    }

    assert.deepEqual(await resultPromise, {
      type: 'failed',
      error: { kind: 'timeout', message: 'the audio operation deadline expired after native admission' },
    });
    const cancellation = await envelopeAt(helper, 2) as {
      id: string;
      command?: { type?: string; identity?: unknown };
    };
    assert.equal(cancellation.command?.type, 'cancel_operation');
    assert.deepEqual(cancellation.command?.identity, identity);
    helper.stdout.write(`${JSON.stringify({
      id: cancellation.id,
      type: 'response',
      result: { type: 'cancelled', snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }) },
    })}\n`);
    await new Promise((resolve) => setImmediate(resolve));
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('stopping a committed recording does not need a fresh saved configuration read', async () => {
  const helper = new FakeHelperProcess();
  let configurationUnavailable = false;
  let configurationReads = 0;
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
    getAudioConfiguration: () => {
      configurationReads += 1;
      if (configurationUnavailable) throw new Error('saved settings are unavailable');
      return audioConfigurationDefaults();
    },
    getAudioConfigurationRevision: () => 37,
  });
  try {
    await primeEngineCapabilities(manager, helper);
    const owner = { type: 'session', session_id: 'recording-session' } as const;
    const startRequest = {
      identity: { id: '00000000-0000-4000-8000-00000000000b', generation: 1, service_epoch: 9 },
      owner,
      max_payload_bytes: 8_000_000,
      operation: { type: 'start_recording', sample_rate_hz: 16_000, format: 'wav' },
    } as const;
    const startResult = manager.executeAudioRequest(startRequest);
    const startEnvelope = await envelopeAt(helper, 1) as { id: string; kind: string; configurationRevision?: number };
    assert.equal(startEnvelope.kind, 'engine_request');
    assert.equal(startEnvelope.configurationRevision, 37);
    helper.stdout.write(`${JSON.stringify({
      id: startEnvelope.id,
      type: 'response',
      result: {
        type: 'engine_result',
        snapshot: snapshot({
          owner: { kind: 'engine', id: owner.session_id },
          activity: 'listening',
          capabilities: AUDIO_CAPABILITIES,
          currentOperation: { identity: startRequest.identity, owner },
        }),
        result: { type: 'recording_started', handle: 'recording-handle' },
      },
    })}\n`);
    assert.deepEqual(await startResult, { type: 'recording_started', handle: 'recording-handle' });
    assert.equal(configurationReads, 1);

    configurationUnavailable = true;
    const stopRequest = {
      identity: { id: '00000000-0000-4000-8000-00000000000c', generation: 2, service_epoch: 9 },
      owner,
      max_payload_bytes: 8_000_000,
      operation: { type: 'stop_recording', handle: 'recording-handle' },
    } as const;
    const stopResult = manager.executeAudioRequest(stopRequest);
    const stopEnvelope = await envelopeAt(helper, 2) as { id: string; kind: string; configurationRevision?: number; request?: { operation?: { type?: string } } };
    assert.equal(stopEnvelope.kind, 'engine_request');
    assert.equal(stopEnvelope.request?.operation?.type, 'stop_recording');
    assert.equal(stopEnvelope.configurationRevision, 37, 'stop retains the committed capture revision');
    helper.stdout.write(`${JSON.stringify({
      id: stopEnvelope.id,
      type: 'response',
      result: {
        type: 'engine_result',
        snapshot: snapshot({
          capabilities: AUDIO_CAPABILITIES,
          currentOperation: { identity: stopRequest.identity, owner },
        }),
        result: { type: 'recording', audio_base64: 'AQI=', mime_type: 'audio/wav' },
      },
    })}\n`);
    assert.deepEqual(await stopResult, { type: 'recording', audio_base64: 'AQI=', mime_type: 'audio/wav' });
    assert.equal(configurationReads, 1, 'stop must not consult settings after the recording is committed');
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('failed completed-recording cancellation retains the origin for retry', async () => {
  const helper = new FakeHelperProcess();
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
  });
  try {
    await primeEngineCapabilities(manager, helper);
    const owner = { type: 'session', session_id: 'recording-owner' } as const;
    const identity = { id: '00000000-0000-4000-8000-000000000071', generation: 1, service_epoch: 9 } as const;
    const starting = manager.executeAudioRequest({
      identity,
      owner,
      max_payload_bytes: 8_000_000,
      operation: { type: 'start_recording', sample_rate_hz: 16_000, format: 'wav' },
    });
    const start = await envelopeAt(helper, 1);
    helper.stdout.write(`${JSON.stringify({
      id: start.id,
      type: 'response',
      result: {
        type: 'engine_result',
        snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }),
        result: { type: 'recording_started', handle: 'recording-handle' },
      },
    })}\n`);
    assert.equal((await starting).type, 'recording_started');

    const firstCancellation = manager.cancelAudioRequest(identity);
    const cancel = await envelopeAt(helper, 2);
    assert.equal(cancel.command?.type, 'cancel_operation');
    assert.deepEqual(cancel.command?.identity, identity);
    helper.stdout.write(`${JSON.stringify({
      id: cancel.id,
      type: 'response',
      result: { type: 'error', error: { code: 'native-error', message: 'native cancellation failed' }, snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }) },
    })}\n`);
    await firstCancellation;

    const retry = manager.cancelAudioRequest(identity);
    const retriedCancel = await envelopeAt(helper, 3);
    assert.equal(retriedCancel.command?.type, 'cancel_operation');
    assert.deepEqual(retriedCancel.command?.identity, identity);
    helper.stdout.write(`${JSON.stringify({
      id: retriedCancel.id,
      type: 'response',
      result: { type: 'cancelled', snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }) },
    })}\n`);
    await retry;
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('failed public owner teardown stops the helper and reports failure', async () => {
  const helper = new FakeHelperProcess();
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
  });
  try {
    await primeEngineCapabilities(manager, helper);
    const ending = manager.endAudioOwner({ type: 'session', session_id: 'closing-session' });
    const teardown = await envelopeAt(helper, 1);
    helper.stdout.write(`${JSON.stringify({
      id: teardown.id,
      type: 'response',
      result: {
        type: 'engine_result',
        snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }),
        result: { type: 'failed', error: { kind: 'native_failure', message: 'native stop failed' } },
      },
    })}\n`);
    await assert.rejects(ending, /could not be ended/);
    assert.equal(helper.killCount, 1);
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('identity cancellation settles once and ignores an old recording response after a new start', async () => {
  const helper = new FakeHelperProcess();
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
  });
  try {
    await primeEngineCapabilities(manager, helper);
    const owner = { type: 'session', session_id: 'session-1' } as const;
    const oldRequest = {
      identity: { id: '00000000-0000-4000-8000-000000000002', generation: 1, service_epoch: 9 }, owner,
      max_payload_bytes: 8_000_000,
      operation: { type: 'start_recording', sample_rate_hz: 16_000, format: 'wav' },
    } as const;
    const staleStart = manager.executeAudioRequest(oldRequest);
    const oldStartEnvelope = await envelopeAt(helper, 1);

    const cancel = manager.cancelAudioRequest(oldRequest.identity);
    const cancelEnvelope = await envelopeAt(helper, 2);
    assert.equal(cancelEnvelope.kind, 'command');
    helper.stdout.write(`${JSON.stringify({
      id: cancelEnvelope.id,
      type: 'response',
      result: { type: 'cancelled', snapshot: snapshot() },
    })}\n`);
    await cancel;
    assert.deepEqual(await staleStart, {
      type: 'failed', error: { kind: 'cancelled', message: 'the audio operation was cancelled' },
    });

    const currentRequest = {
      identity: { id: '00000000-0000-4000-8000-000000000003', generation: 2, service_epoch: 9 }, owner,
      max_payload_bytes: 8_000_000,
      operation: { type: 'start_recording', sample_rate_hz: 16_000, format: 'wav' },
    } as const;
    const currentStart = manager.executeAudioRequest(currentRequest);
    const newStartEnvelope = await envelopeAt(helper, 3);
    const currentSnapshot = snapshot({
      owner: { kind: 'engine', id: 'new-session' },
      activity: 'listening',
      capabilities: AUDIO_CAPABILITIES,
      currentOperation: { identity: currentRequest.identity, owner },
    });
    helper.stdout.write(`${JSON.stringify({
      id: newStartEnvelope.id,
      type: 'response',
      result: { type: 'engine_result', snapshot: currentSnapshot, result: { type: 'recording_started', handle: 'new-handle' } },
    })}\n`);
    assert.deepEqual(await currentStart, { type: 'recording_started', handle: 'new-handle' });

    // The helper can finish work after its cancellation response. Its old
    // snapshot must not undo the new recording that now owns the microphone.
    helper.stdout.write(`${JSON.stringify({
      id: oldStartEnvelope.id,
      type: 'response',
      result: { type: 'engine_result', snapshot: snapshot({ owner: { kind: 'engine', id: 'old' }, activity: 'listening' }), result: { type: 'recording_started', handle: 'old-handle' } },
    })}\n`);
    assert.deepEqual(manager.getSnapshot(), currentSnapshot);
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('cancellation while packaged permission is pending prevents later native admission', async () => {
  const helper = new FakeHelperProcess();
  let finishPermission!: (value: 'granted') => void;
  let permissionStarted!: () => void;
  const started = new Promise<void>((resolve) => { permissionStarted = resolve; });
  const manager = new NativeAudioManager({
    isPackaged: true,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/resources/LingXiAudioHelper.app/Contents/MacOS/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
    verifyPackagedHelper: () => {},
    requestMicrophoneAccess: () => {
      permissionStarted();
      return new Promise((resolve) => { finishPermission = resolve; });
    },
  });
  try {
    await primeEngineCapabilities(manager, helper);
    const request = {
      identity: { id: '00000000-0000-4000-8000-000000000004', generation: 1, service_epoch: 9 },
      owner: { type: 'session', session_id: 'permission-session' },
      max_payload_bytes: 8_000_000,
      operation: { type: 'start_recording', sample_rate_hz: 16_000, format: 'wav' },
    } as const;
    const resultPromise = manager.executeAudioRequest(request);
    await started;
    await manager.cancelAudioRequest(request.identity);
    assert.deepEqual(await resultPromise, {
      type: 'failed', error: { kind: 'cancelled', message: 'the audio operation was cancelled' },
    });

    finishPermission('granted');
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(helper.writes.length, 1,
      'a delayed permission callback must not make further helper requests after targeted cancellation');
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('unknown model ids are rejected before helper dispatch', async () => {
  const helper = new FakeHelperProcess();
  const manager = new NativeAudioManager({
    isPackaged: false,
    resourcesPath: '/resources',
    userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper',
    diagnostics: new DiagnosticBuffer(),
    spawnHelper: () => helper as any,
  });
  try {
    const response = await manager.request({ type: 'install_model', modelId: 'not-a-real-model' });
    assert.deepEqual(response, {
      type: 'error',
      snapshot: defaultNativeAudioSnapshot(),
      error: { code: 'invalid-request', message: 'unknown model id: not-a-real-model' },
    });
    assert.equal(helper.writes.length, 0, 'unknown model ids must be rejected before helper dispatch');
  } finally {
    await manager.dispose();
    helper.stdin.end();
    helper.stdout.end();
    helper.stderr.end();
  }
});

test('renderer-visible native audio snapshots reject path-bearing fields', () => {
  assert.throws(() => validateNativeAudioSnapshot({
    ...snapshot(),
    helper: { state: 'running', path: '/tmp/helper' },
  }), /invalid audio helper/);
  assert.throws(() => validateNativeAudioSnapshot({
    ...snapshot(),
    storageRoot: '/tmp/models',
  }), /invalid audio snapshot/);
  assert.throws(() => validateNativeAudioSnapshot({
    ...snapshot(),
    models: [{ modelId: 'sherpa.moonshine-tiny-en', state: { type: 'ready', rootPath: '/tmp/model' } }],
  }), /invalid audio model state/);
});

for (const error of [undefined, null, {}, { message: 7 }]) {
  test(`invalid helper error settles the request: ${JSON.stringify(error)}`, { timeout: 1_000 }, async () => {
    const helper = new FakeHelperProcess();
    const manager = new NativeAudioManager({
      isPackaged: false, resourcesPath: '/resources', userDataPath: '/tmp/lingxi-audio-tests',
      helperPath: '/tmp/helper', diagnostics: new DiagnosticBuffer(), spawnHelper: () => helper as any,
    });
    try {
      const pending = manager.request({ type: 'get_snapshot' });
      const envelope = await nextEnvelope(helper);
      helper.stdout.write(`${JSON.stringify({ id: envelope.id, type: 'error', error })}\n`);
      const response = await pending;
      assert.equal(response.type, 'error');
      if (response.type === 'error') assert.match(response.error.message, /invalid error response/);
    } finally {
      await manager.dispose();
      helper.stdin.end(); helper.stdout.end(); helper.stderr.end();
    }
  });
}

for (const budgets of [[1000, 10], [10, 1000]]) {
  test(`capability waiters keep independent deadlines: ${budgets}`, async () => {
    const helper = new FakeHelperProcess();
    const manager = new NativeAudioManager({
      isPackaged: false, resourcesPath: '/resources', userDataPath: '/tmp/lingxi-audio-tests',
      helperPath: '/tmp/helper', diagnostics: new DiagnosticBuffer(), spawnHelper: () => helper as any,
    });
    const request = (index: number) => ({
      identity: { id: `00000000-0000-4000-8000-00000000008${index}`, generation: 1, service_epoch: 9 },
      owner: { type: 'session', session_id: 'test' }, max_payload_bytes: 8_000_000,
      timeout_budget_ms: budgets[index], operation: { type: 'status' },
    });
    try {
      const first = manager.executeAudioRequest(request(0));
      const capability = await nextEnvelope(helper);
      const second = manager.executeAudioRequest(request(1));
      const [short, long] = budgets[0] === 10 ? [first, second] : [second, first];
      let longSettled = false;
      void long.then(() => { longSettled = true; });
      const shortResult = await short;
      assert.equal(shortResult.type, 'failed');
      if (shortResult.type === 'failed') assert.equal(shortResult.error.kind, 'timeout');
      assert.equal(longSettled, false);
      assert.equal(helper.writes.length, 1);
      helper.stdout.write(`${JSON.stringify({ id: capability.id, type: 'response', result: { type: 'snapshot', snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }) } })}\n`);
      const admitted = await envelopeAt(helper, 1);
      helper.stdout.write(`${JSON.stringify({ id: admitted.id, type: 'response', result: {
        type: 'engine_result', snapshot: snapshot({ capabilities: AUDIO_CAPABILITIES }), result: { type: 'status', status: { recording: false, playing: false } },
      } })}\n`);
      assert.equal((await long).type, 'status');
    } finally {
      await manager.dispose(); helper.stdin.end(); helper.stdout.end(); helper.stderr.end();
    }
  });
}

test('helper stdout preserves UTF-8 characters split across byte chunks', async () => {
  const helper = new FakeHelperProcess();
  const manager = new NativeAudioManager({
    isPackaged: false, resourcesPath: '/resources', userDataPath: '/tmp/lingxi-audio-tests',
    helperPath: '/tmp/LingXiAudioHelper', diagnostics: new DiagnosticBuffer(), spawnHelper: () => helper as any,
  });
  try {
    const responsePromise = manager.request({ type: 'get_snapshot' });
    const envelope = await nextEnvelope(helper);
    const message = '中文正常 😀 café';
    const bytes = Buffer.from(JSON.stringify({ id: envelope.id, type: 'response',
      result: { type: 'snapshot', snapshot: snapshot({ helper: { state: 'running', message } }) },
    }) + '\n');
    for (const byte of bytes) helper.stdout.write(Buffer.from([byte]));
    assert.equal((await responsePromise).snapshot.helper.message, message);
  } finally {
    await manager.dispose();
    helper.stdin.end(); helper.stdout.end(); helper.stderr.end();
  }
});
