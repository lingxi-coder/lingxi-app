import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  resolveCapabilities,
  resolveActiveProviderVoiceCapability,
  type ProviderConfiguredFact,
  type VoiceCapabilitySnapshot,
  type VoiceOption,
  type VoicePlatformSnapshot,
} from '../src/renderer/audio/capabilities';
import {
  DEFAULT_VOICE_SELECTION,
  defaultVoicePreferences,
  type VoicePreferences,
} from '../src/renderer/audio/preferences';

// ---------------------------------------------------------------------------
// Fixtures — typed against the REAL exported shapes, not `as never`.
// ---------------------------------------------------------------------------

function prefs(overrides: Partial<VoicePreferences> = {}): VoicePreferences {
  return { ...defaultVoicePreferences(), ...overrides };
}

function voiceOption(overrides: Partial<VoiceOption> & { id: string }): VoiceOption {
  return {
    label: overrides.id,
    languageTag: 'en-US',
    source: 'system',
    familyId: 'system',
    ...overrides,
  };
}

const ALEX = voiceOption({ id: 'system:Alex', label: 'Alex', isDefault: true });

function platform(overrides: Partial<VoicePlatformSnapshot> = {}): VoicePlatformSnapshot {
  const systemVoices = overrides.systemVoices ?? [ALEX];
  return {
    localeTag: 'en-US',
    microphonePermission: 'granted',
    providerConfigured: true,
    providerTranscriptionCapable: true,
    systemVoices,
    defaultSystemVoiceId: systemVoices.find((v) => v.isDefault)?.id ?? systemVoices[0]?.id ?? '',
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// Single-issue cases (adapted from the brief to the real field names:
// `microphonePermission`, split `providerConfigured`/`providerTranscriptionCapable`,
// and `recognitionMode: 'localOnly'` — not the brief's stale `'onDevice'`).
// ---------------------------------------------------------------------------

test('no microphone permission blocks recognition and says so', () => {
  const snap = resolveCapabilities(prefs(), platform({ microphonePermission: 'denied' }));
  assert.equal(snap.effectiveRecognitionBackend, 'unavailable');
  assert.ok(snap.blockingIssues.includes('MicrophonePermissionRequired'));
});

test('no provider credential blocks recognition on desktop specifically', () => {
  const snap = resolveCapabilities(
    prefs(),
    platform({ providerConfigured: false, providerTranscriptionCapable: false }),
  );
  assert.equal(snap.effectiveRecognitionBackend, 'unavailable');
  assert.ok(
    snap.blockingIssues.includes('ProviderCredentialRequired'),
    'desktop STT runs through a provider, so a missing credential must be named, not silently swallowed',
  );
});

test('localOnly is honestly unavailable on desktop and explains why', () => {
  const snap = resolveCapabilities(prefs({ recognitionMode: 'localOnly' }), platform());
  assert.equal(snap.effectiveRecognitionBackend, 'unavailable');
  assert.ok(snap.blockingIssues.includes('OnDeviceUnsupportedOnDesktop'));
  assert.match(
    snap.fallbackReason ?? '',
    /offline/i,
    'the reason must say there is no offline model on desktop, not just fail',
  );
});

test('the persisted mobile spelling "onDevice" is not a valid recognitionMode value here', () => {
  // Guards against ever regressing to the Swift *case name* — the actual
  // persisted/normalized value is 'localOnly' (see shared/voicePreferences.ts).
  const snap = resolveCapabilities(prefs({ recognitionMode: 'localOnly' }), platform());
  assert.equal(snap.requestedRecognitionBackend, 'localOnly');
});

test('a fully configured desktop resolves to the provider backend with no issues', () => {
  const snap = resolveCapabilities(prefs(), platform());
  assert.equal(snap.effectiveRecognitionBackend, 'provider');
  assert.deepEqual(
    snap.blockingIssues,
    [],
    'if issues were reported here, the tests around it would prove nothing',
  );
  assert.equal(snap.fallbackReason, null);
});

test('a requested voice that no longer exists falls back and says so', () => {
  const snap = resolveCapabilities(prefs({ voiceSelection: 'system:Ghost' }), platform());
  assert.notEqual(snap.effectiveVoice?.id, 'system:Ghost');
  assert.ok(snap.blockingIssues.includes('RequestedVoiceUnavailable'));
  assert.equal(snap.effectiveVoice?.id, ALEX.id);
});

test('requesting the default voice selection is never reported as unavailable', () => {
  const snap = resolveCapabilities(prefs({ voiceSelection: DEFAULT_VOICE_SELECTION }), platform());
  assert.ok(!snap.blockingIssues.includes('RequestedVoiceUnavailable'));
});

test('zero system voices reports PlaybackVoiceUnavailable, not a crash', () => {
  const snap = resolveCapabilities(prefs(), platform({ systemVoices: [], defaultSystemVoiceId: '' }));
  assert.equal(snap.effectiveVoice, null);
  assert.ok(snap.blockingIssues.includes('PlaybackVoiceUnavailable'));
  assert.ok(
    !snap.blockingIssues.includes('RequestedVoiceUnavailable'),
    'with nothing to fall back to, this is a playback-unavailable problem, not a "wrong voice" problem',
  );
});

// ---------------------------------------------------------------------------
// Ruling 2: "no credential" and "configured but cannot transcribe" are
// DIFFERENT, distinctly-actionable blocking issues.
// ---------------------------------------------------------------------------

test('a configured provider that cannot transcribe (e.g. Anthropic) is a DIFFERENT issue than missing credentials', () => {
  const snap = resolveCapabilities(
    prefs(),
    platform({ providerConfigured: true, providerTranscriptionCapable: false }),
  );
  assert.equal(snap.effectiveRecognitionBackend, 'unavailable');
  assert.ok(
    snap.blockingIssues.includes('ProviderCannotTranscribe'),
    'a connected-but-incapable provider must not be reported as ProviderCredentialRequired — the user already connected one',
  );
  assert.ok(
    !snap.blockingIssues.includes('ProviderCredentialRequired'),
    'telling a user who already connected a provider to "connect a provider" is unactionable and dishonest',
  );
  assert.match(
    snap.fallbackReason ?? '',
    /connect.*(different|another).*transcri|transcri.*connect/i,
    'the fallback reason must name the real remedy: connect a DIFFERENT provider that can transcribe',
  );
});

test('a provider that IS configured and capable reports neither provider issue', () => {
  const snap = resolveCapabilities(
    prefs(),
    platform({ providerConfigured: true, providerTranscriptionCapable: true }),
  );
  assert.ok(!snap.blockingIssues.includes('ProviderCredentialRequired'));
  assert.ok(!snap.blockingIssues.includes('ProviderCannotTranscribe'));
});

// ---------------------------------------------------------------------------
// Ruling 4: precedence between simultaneous blocking issues must be
// deliberate and tested, not an accident of `if`-statement order.
//
// Chosen order (mirrors Android's VoiceSettingsCapabilityResolver.resolve,
// adapted to desktop's issue vocabulary):
//   1. MicrophonePermissionRequired  — nothing about recognition works at
//      all without this, independent of every other setting; it is also
//      the most immediately actionable (a single OS permission grant).
//   2. OnDeviceUnsupportedOnDesktop  — mutually exclusive with 3/4 below:
//      when localOnly is requested, the reason recognition is unavailable
//      is the requested MODE itself, not provider state, so provider facts
//      are irrelevant noise and must not also be reported.
//   3. ProviderCredentialRequired / 4. ProviderCannotTranscribe — mutually
//      exclusive with each other (one requires providerConfigured=false,
//      the other requires it =true) and only apply when recognitionMode is
//      'automatic'.
//   5. RequestedVoiceUnavailable / 6. PlaybackVoiceUnavailable — playback
//      voice problems never block recognition and always sort last.
// ---------------------------------------------------------------------------

test('several issues live simultaneously: precedence is mic, then mode, then voice', () => {
  const snap = resolveCapabilities(
    prefs({ recognitionMode: 'localOnly', voiceSelection: 'system:Ghost' }),
    platform({ microphonePermission: 'denied' }),
  );
  assert.deepEqual(snap.blockingIssues, [
    'MicrophonePermissionRequired',
    'OnDeviceUnsupportedOnDesktop',
    'RequestedVoiceUnavailable',
  ]);
});

test('several issues live simultaneously: mic denied + incapable provider + missing voice, automatic mode', () => {
  const snap = resolveCapabilities(
    prefs({ voiceSelection: 'system:Ghost' }),
    platform({
      microphonePermission: 'denied',
      providerConfigured: true,
      providerTranscriptionCapable: false,
    }),
  );
  assert.deepEqual(snap.blockingIssues, [
    'MicrophonePermissionRequired',
    'ProviderCannotTranscribe',
    'RequestedVoiceUnavailable',
  ]);
});

test('several issues live simultaneously: no credential + zero voices, mic granted', () => {
  const snap = resolveCapabilities(
    prefs(),
    platform({
      providerConfigured: false,
      providerTranscriptionCapable: false,
      systemVoices: [],
      defaultSystemVoiceId: '',
    }),
  );
  assert.deepEqual(snap.blockingIssues, ['ProviderCredentialRequired', 'PlaybackVoiceUnavailable']);
});

test('localOnly and an incapable/unconfigured provider never co-occur: the mode reason wins alone', () => {
  const snap = resolveCapabilities(
    prefs({ recognitionMode: 'localOnly' }),
    platform({ providerConfigured: false, providerTranscriptionCapable: false }),
  );
  assert.deepEqual(snap.blockingIssues, ['OnDeviceUnsupportedOnDesktop']);
});

// ---------------------------------------------------------------------------
// Ruling 1: the join Task 5 left as a documented contract — "which provider
// is currently active" -> `providerById(...).transcriptionCapable` — must
// be real code, proven with cases that only pass if the real per-provider
// flag is actually consulted (not a stand-in that is always true/false, and
// not one that just mirrors `configured`).
// ---------------------------------------------------------------------------

test('join: an active, configured, transcription-capable provider (openai) reports capable', () => {
  const credentials: ProviderConfiguredFact[] = [{ providerId: 'openai', configured: true }];
  const result = resolveActiveProviderVoiceCapability('openai', credentials);
  assert.equal(result.providerConfigured, true);
  assert.equal(
    result.providerTranscriptionCapable,
    true,
    'openai.transcriptionCapable is true in shared/providers.ts — a join that never consults it would report false here',
  );
});

test('join: an active, configured, but NOT transcription-capable provider (anthropic) reports incapable', () => {
  const credentials: ProviderConfiguredFact[] = [{ providerId: 'anthropic', configured: true }];
  const result = resolveActiveProviderVoiceCapability('anthropic', credentials);
  assert.equal(result.providerConfigured, true);
  assert.equal(
    result.providerTranscriptionCapable,
    false,
    'anthropic.transcriptionCapable is false in shared/providers.ts — a join that hardcodes "configured implies capable" would wrongly report true here',
  );
});

test('join: a capable provider that is NOT actually configured must not be reported as capable', () => {
  const credentials: ProviderConfiguredFact[] = [{ providerId: 'openai', configured: false }];
  const result = resolveActiveProviderVoiceCapability('openai', credentials);
  assert.equal(result.providerConfigured, false);
  assert.equal(result.providerTranscriptionCapable, false);
});

test('join: no active provider at all reports both facts false without throwing', () => {
  const result = resolveActiveProviderVoiceCapability(null, []);
  assert.equal(result.providerConfigured, false);
  assert.equal(result.providerTranscriptionCapable, false);
});

test('join: an active provider id unknown to shared/providers.ts is treated as incapable, not thrown', () => {
  const credentials: ProviderConfiguredFact[] = [{ providerId: 'not-a-real-provider', configured: true }];
  const result = resolveActiveProviderVoiceCapability('not-a-real-provider', credentials);
  assert.equal(result.providerConfigured, true);
  assert.equal(result.providerTranscriptionCapable, false);
});

test('join: switching the active provider id changes the answer using the SAME credentials list', () => {
  const credentials: ProviderConfiguredFact[] = [
    { providerId: 'openai', configured: true },
    { providerId: 'anthropic', configured: true },
  ];
  const asOpenAi = resolveActiveProviderVoiceCapability('openai', credentials);
  const asAnthropic = resolveActiveProviderVoiceCapability('anthropic', credentials);
  assert.equal(
    asOpenAi.providerTranscriptionCapable,
    true,
    'this and the next assertion together can only both pass if the join actually looks up the ACTIVE provider id, not a fixed one',
  );
  assert.equal(asAnthropic.providerTranscriptionCapable, false);
});

// The full snapshot picks up the join's output as `platform.providerConfigured`
// / `.providerTranscriptionCapable` — exercised end to end here so a future
// caller that forgets to route through the join still gets caught by
// resolveCapabilities's own tests above.
test('the joined provider facts flow straight into the capability snapshot', () => {
  const joined = resolveActiveProviderVoiceCapability('anthropic', [
    { providerId: 'anthropic', configured: true },
  ]);
  const snap: VoiceCapabilitySnapshot = resolveCapabilities(
    prefs(),
    platform({
      providerConfigured: joined.providerConfigured,
      providerTranscriptionCapable: joined.providerTranscriptionCapable,
    }),
  );
  assert.ok(snap.blockingIssues.includes('ProviderCannotTranscribe'));
});
