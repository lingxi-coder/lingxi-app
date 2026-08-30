import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  probePlatform,
  readSystemVoices,
  type ProbeDeps,
  type SpeechSynthesisLike,
  type VoicePermissionStatus,
} from '../src/renderer/audio/capabilities';

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/** A minimal, real `SpeechSynthesisVoice`-shaped object — no `as never`. */
function voice(overrides: Partial<SpeechSynthesisVoice> & { name: string; lang: string }): SpeechSynthesisVoice {
  return {
    default: false,
    localService: true,
    voiceURI: overrides.name,
    ...overrides,
  } as SpeechSynthesisVoice;
}

/**
 * A fake `SpeechSynthesisLike` whose voice list and `voiceschanged` firing
 * are both under the test's control, so the empty-then-populated race can
 * be simulated deliberately rather than waited out with a real timer.
 */
function fakeSynth(initial: SpeechSynthesisVoice[]): SpeechSynthesisLike & { setVoices(v: SpeechSynthesisVoice[]): void; fireVoicesChanged(): void } {
  let current = initial;
  const listeners = new Set<() => void>();
  return {
    getVoices: () => current,
    addEventListener: (_type, listener) => { listeners.add(listener); },
    removeEventListener: (_type, listener) => { listeners.delete(listener); },
    setVoices(v) { current = v; },
    fireVoicesChanged() { for (const l of [...listeners]) l(); },
  };
}

function neverGrantedPermission(): () => Promise<VoicePermissionStatus> {
  return () => Promise.resolve('prompt');
}

function baseDeps(overrides: Partial<ProbeDeps> = {}): ProbeDeps {
  return {
    synth: fakeSynth([]),
    queryMicrophonePermission: neverGrantedPermission(),
    localeTag: 'en-US',
    providerConfigured: false,
    providerTranscriptionCapable: false,
    voiceListTimeoutMs: 5,
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// System voices: enumeration, tagging, default selection, network flag
// ---------------------------------------------------------------------------

test('system voices are read from speechSynthesis and tagged system:<id>', async () => {
  const synth = fakeSynth([
    voice({ name: 'Alex', lang: 'en-US', default: true, localService: true }),
    voice({ name: 'Tingting', lang: 'zh-CN', default: false, localService: false }),
  ]);
  const snapshot = await probePlatform(baseDeps({ synth, localeTag: 'zh-CN' }));

  assert.deepEqual(
    snapshot.systemVoices.map((v) => v.id), ['system:Alex', 'system:Tingting'],
    'every enumerated voice must carry the shared system:<id> selection grammar',
  );
  assert.equal(snapshot.defaultSystemVoiceId, 'system:Alex');
  assert.equal(
    snapshot.systemVoices[1].networkRequired, true,
    'a non-local voice must be marked as needing the network, as Android does',
  );
  assert.equal(
    snapshot.systemVoices[0].networkRequired, false,
    'a local voice must not be marked as needing the network',
  );
  assert.equal(snapshot.systemVoices[1].languageTag, 'zh-CN');
});

test('no voices enumerated yields an empty list, not a fabricated default', async () => {
  const snapshot = await probePlatform(baseDeps({
    synth: fakeSynth([]),
    queryMicrophonePermission: () => Promise.resolve('denied'),
    providerConfigured: false,
    localeTag: 'en-US',
  }));
  assert.deepEqual(snapshot.systemVoices, []);
  assert.equal(snapshot.microphonePermission, 'denied');
  assert.equal(snapshot.providerConfigured, false);
  assert.equal(
    snapshot.defaultSystemVoiceId, '',
    'with zero voices there is nothing to default to — must not be a sentinel id naming no real voice',
  );
});

test('when no voice is flagged default, the first ENUMERATED voice is used, not a fabricated sentinel', async () => {
  const synth = fakeSynth([
    voice({ name: 'Milena', lang: 'ru-RU', default: false }),
    voice({ name: 'Yuna', lang: 'ko-KR', default: false }),
  ]);
  const snapshot = await probePlatform(baseDeps({ synth }));
  assert.equal(snapshot.defaultSystemVoiceId, 'system:Milena');
  assert.notEqual(
    snapshot.defaultSystemVoiceId, 'system:default',
    'a made-up "system:default" id would name a voice that does not exist in systemVoices',
  );
});

// ---------------------------------------------------------------------------
// The voiceschanged race: Chromium reports [] synchronously, then fires late
// ---------------------------------------------------------------------------

test('voices arriving only after voiceschanged fires are still reported, not lost', async () => {
  const synth = fakeSynth([]); // simulates Chromium's first-call []
  const probe = probePlatform(baseDeps({ synth, voiceListTimeoutMs: 2000 }));

  // Give the probe a turn to register its voiceschanged listener before the
  // voices "arrive" — mirrors the real race where the browser populates the
  // list and fires the event some time after the page first asks.
  await new Promise((resolve) => setTimeout(resolve, 5));
  synth.setVoices([voice({ name: 'Alex', lang: 'en-US', default: true })]);
  synth.fireVoicesChanged();

  const snapshot = await probe;
  assert.deepEqual(
    snapshot.systemVoices.map((v) => v.id), ['system:Alex'],
    'a naive single read would have reported [] permanently for this machine',
  );
});

test('readSystemVoices resolves to [] instead of hanging when voiceschanged never fires', async () => {
  const synth = fakeSynth([]);
  const result = await readSystemVoices(synth, 5);
  assert.deepEqual(result, []);
});

test('readSystemVoices trusts a non-empty immediate read without waiting for an event at all', async () => {
  const synth = fakeSynth([voice({ name: 'Daniel', lang: 'en-GB' })]);
  // No listener is ever registered by the test, and voiceschanged never
  // fires; if the implementation waited unconditionally this would hang
  // until the (very long) default timeout instead of resolving immediately.
  const result = await readSystemVoices(synth, 60_000);
  assert.deepEqual(result.map((v) => v.name), ['Daniel']);
});

// ---------------------------------------------------------------------------
// Microphone permission: all four branches
// ---------------------------------------------------------------------------

for (const status of ['granted', 'denied', 'prompt'] as const) {
  test(`microphone permission '${status}' passes through unchanged`, async () => {
    const snapshot = await probePlatform(baseDeps({ queryMicrophonePermission: () => Promise.resolve(status) }));
    assert.equal(snapshot.microphonePermission, status);
  });
}

test('a queryMicrophonePermission that throws is reported as unavailable, not propagated', async () => {
  const snapshot = await probePlatform(baseDeps({
    queryMicrophonePermission: () => Promise.reject(new Error('Permissions API does not support "microphone" in this engine')),
  }));
  assert.equal(snapshot.microphonePermission, 'unavailable');
});

test('a queryMicrophonePermission that resolves unavailable directly is passed through too', async () => {
  const snapshot = await probePlatform(baseDeps({ queryMicrophonePermission: () => Promise.resolve('unavailable') }));
  assert.equal(snapshot.microphonePermission, 'unavailable');
});

// ---------------------------------------------------------------------------
// Provider axes: configured vs. capable are independent facts
// ---------------------------------------------------------------------------

test('provider configured AND transcription-capable: both facts reported true', async () => {
  const snapshot = await probePlatform(baseDeps({ providerConfigured: true, providerTranscriptionCapable: true }));
  assert.equal(snapshot.providerConfigured, true);
  assert.equal(snapshot.providerTranscriptionCapable, true);
});

test('provider configured but INCAPABLE of transcription (e.g. Anthropic): configured stays true, capable is false', async () => {
  const snapshot = await probePlatform(baseDeps({ providerConfigured: true, providerTranscriptionCapable: false }));
  assert.equal(
    snapshot.providerConfigured, true,
    'a connected provider must not be reported as unconfigured — that would tell the user to do something already done',
  );
  assert.equal(
    snapshot.providerTranscriptionCapable, false,
    'the provider being configured does not make it able to transcribe',
  );
});

test('no provider configured at all: both facts false', async () => {
  const snapshot = await probePlatform(baseDeps({ providerConfigured: false, providerTranscriptionCapable: false }));
  assert.equal(snapshot.providerConfigured, false);
  assert.equal(snapshot.providerTranscriptionCapable, false);
});

test('a caller wrongly claiming "capable" with nothing configured is clamped to false, not trusted', async () => {
  // This combination should never happen if the caller's join logic is
  // correct, but it must never reach the snapshot even if it does — there
  // is no configured provider for "capable" to describe.
  const snapshot = await probePlatform(baseDeps({ providerConfigured: false, providerTranscriptionCapable: true }));
  assert.equal(snapshot.providerConfigured, false);
  assert.equal(
    snapshot.providerTranscriptionCapable, false,
    'capability with no configured provider is incoherent and must be clamped, not passed through',
  );
});

// ---------------------------------------------------------------------------
// Locale passthrough
// ---------------------------------------------------------------------------

test('the locale tag is reported as given, and used as a per-voice language fallback', async () => {
  const synth = fakeSynth([voice({ name: 'NoLang', lang: '' })]);
  const snapshot = await probePlatform(baseDeps({ synth, localeTag: 'fr-FR' }));
  assert.equal(snapshot.localeTag, 'fr-FR');
  assert.equal(
    snapshot.systemVoices[0].languageTag, 'fr-FR',
    'a voice with no reported lang falls back to the system locale, not an empty string',
  );
});
