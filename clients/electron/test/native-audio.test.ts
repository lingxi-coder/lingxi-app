import { PassThrough } from 'node:stream';
import { EventEmitter } from 'node:events';
import assert from 'node:assert/strict';
import { test } from 'node:test';

import { DiagnosticBuffer } from '../src/main/host-utils';
import { NativeAudioManager } from '../src/main/audio/nativeAudioManager';
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

  constructor() {
    super();
    this.stdin.write = ((chunk: string | Uint8Array) => {
      this.writes.push(Buffer.from(chunk).toString('utf8'));
      return true;
    }) as typeof this.stdin.write;
  }

  kill(): boolean {
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
    const currentSnapshot = snapshot({ localeTag: 'en-US' });
    helper.stdout.write(`${JSON.stringify({
      id: envelope.id,
      type: 'response',
      result: { type: 'snapshot', snapshot: currentSnapshot },
    })}\n`);
    await responsePromise;

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

test('native audio manager routes engine requests through helper jsonl', async () => {
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
    const resultPromise = manager.executeEngineRequest('session-1', { type: 'synthesize', text: 'hello', voice: 'system:Alex' });
    const envelope = await nextEnvelope(helper);
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
        }),
        result: { type: 'audio', pcm_base64: '', sample_rate_hz: 0 },
      },
    })}\n`);

    assert.deepEqual(await resultPromise, { type: 'audio', pcm_base64: '', sample_rate_hz: 0 });
    assert.equal(manager.getSnapshot().localeTag, 'fr-FR', 'engine responses must retain the helper capability snapshot');
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
