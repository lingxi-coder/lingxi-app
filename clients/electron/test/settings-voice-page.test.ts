import { test } from 'node:test';
import assert from 'node:assert/strict';

import { activeVoiceProviderId, voicePageModel } from '../src/renderer/components/settings/pages/Voice';
import { defaultVoicePreferences, type VoicePreferences } from '../src/shared/voicePreferences';
import type { VoiceOption, VoicePlatformSnapshot } from '../src/renderer/audio/capabilities';

function prefs(overrides: Partial<VoicePreferences> = {}): VoicePreferences {
  return { ...defaultVoicePreferences(), ...overrides };
}

/** A raw voice shorthand — mirrors what `probePlatform` reads off `SpeechSynthesisVoice` before mapping it to a real `VoiceOption`. */
interface RawVoice { name: string; lang?: string; default?: boolean; localService?: boolean }

interface PlatformOverrides {
  voices?: RawVoice[];
  microphonePermission?: VoicePlatformSnapshot['microphonePermission'];
  providerConfigured?: boolean;
  providerTranscriptionCapable?: boolean;
  localeTag?: string;
}

/**
 * Builds a full, typed `VoicePlatformSnapshot`, converting the illustrative
 * `{ name, lang, default, localService }` voice shorthand into real
 * `VoiceOption`s the exact way `probePlatform` does — so a fixture never
 * drifts from what production actually produces (`clients/electron/test/`
 * is excluded from `npm run typecheck`, so this file is typed against the
 * real exported types on purpose, per the task's own warning).
 */
function platform(overrides: PlatformOverrides = {}): VoicePlatformSnapshot {
  const localeTag = overrides.localeTag ?? 'en-US';
  const systemVoices: VoiceOption[] = (overrides.voices ?? []).map((voice) => ({
    id: `system:${voice.name}`,
    label: voice.name,
    languageTag: voice.lang || localeTag,
    source: 'system',
    familyId: 'system',
    isDefault: voice.default,
    networkRequired: !voice.localService,
  }));
  const providerConfigured = overrides.providerConfigured ?? true;
  return {
    localeTag,
    microphonePermission: overrides.microphonePermission ?? 'granted',
    providerConfigured,
    // Mirrors `probePlatform`'s own defensive clamp: "capable" cannot be
    // claimed for a provider that is not even configured.
    providerTranscriptionCapable: providerConfigured && (overrides.providerTranscriptionCapable ?? true),
    systemVoices,
    defaultSystemVoiceId: systemVoices.find((voice) => voice.isDefault)?.id ?? systemVoices[0]?.id ?? '',
  };
}

test('the page renders the probed voices, never a hardcoded list', () => {
  const model = voicePageModel(prefs(), platform({
    voices: [{ name: 'Alex', lang: 'en-US', default: true, localService: true }],
  }));
  assert.deepEqual(model.voiceOptions.map((v) => v.label), ['Alex']);
});

test('a machine with several voices offers all of them, not just the default', () => {
  const model = voicePageModel(prefs(), platform({
    voices: [
      { name: 'Alex', default: true, localService: true },
      { name: 'Tingting', localService: false },
    ],
  }));
  assert.deepEqual(model.voiceOptions.map((v) => v.label), ['Alex', 'Tingting']);
  // A network-dependent voice is distinguishable from a local one — the
  // page labels this, per the brief's "网络嗓音标注" requirement.
  assert.equal(model.voiceOptions.find((v) => v.label === 'Tingting')?.networkRequired, true);
  assert.equal(model.voiceOptions.find((v) => v.label === 'Alex')?.networkRequired, false);
});

test('every blocking issue renders as a visible explanation', () => {
  const model = voicePageModel(prefs({ recognitionMode: 'localOnly' }), platform());
  assert.ok(
    model.notices.some((n) => /离线/.test(n)),
    'an unavailable mode must explain itself on screen, not just disable a control',
  );
});

test('the on-device option is offered but marked unavailable, not hidden', () => {
  const model = voicePageModel(prefs(), platform());
  const onDevice = model.recognitionOptions.find((o) => o.id === 'localOnly');
  assert.ok(onDevice, 'hiding the option would make the desktop look like it has no such concept');
  assert.equal(onDevice?.disabled, true);
  assert.ok(onDevice?.disabledReason, 'a disabled control with no visible reason is the exact failure this page exists to remove');
});

test('the on-device option stays disabled no matter which mode is currently selected', () => {
  // Ruling 2: unconditional, never a silent downgrade that depends on the
  // CURRENT preference. If this were computed from `prefs.recognitionMode`
  // instead of being a fixed fact about this desktop build, selecting
  // "automatic" would make the on-device option look selectable again.
  const whenAutomatic = voicePageModel(prefs({ recognitionMode: 'automatic' }), platform());
  const whenLocalOnly = voicePageModel(prefs({ recognitionMode: 'localOnly' }), platform());
  assert.equal(whenAutomatic.recognitionOptions.find((o) => o.id === 'localOnly')?.disabled, true);
  assert.equal(whenLocalOnly.recognitionOptions.find((o) => o.id === 'localOnly')?.disabled, true);
});

test('the automatic option is offered and selectable, unlike on-device', () => {
  const model = voicePageModel(prefs(), platform());
  const automatic = model.recognitionOptions.find((o) => o.id === 'automatic');
  assert.equal(automatic?.disabled, false);
});

test('the recognition-unavailable notice is present even when every capability fact looks fully healthy', () => {
  // Ruling 1: this must be UNCONDITIONAL, never gated behind
  // `resolveCapabilities`'s blocking-issue computation — which legitimately
  // reports zero issues here (mic granted, mode automatic, a configured AND
  // transcription-capable provider). If the notice were derived from
  // "blockingIssues.length > 0" it would vanish in exactly this case, which
  // is the single worst case for it to go silent: recognition still does
  // not work, because nothing in this build calls a transcription endpoint
  // at all yet.
  const healthy = platform({
    microphonePermission: 'granted',
    providerConfigured: true,
    providerTranscriptionCapable: true,
    voices: [{ name: 'Alex', default: true, localService: true }],
  });
  const model = voicePageModel(prefs({ recognitionMode: 'automatic' }), healthy);
  assert.equal(model.notices.length, 0, 'the itemized issue list is genuinely empty in this scenario');
  assert.ok(model.recognitionUnavailableNotice.length > 0, 'but the unconditional notice must still say recognition does not work');
});

test('the recognition-unavailable notice never tells the user to connect a provider', () => {
  const model = voicePageModel(prefs(), platform());
  // The exact phrasing the controller's ruling forbids ("connect a
  // provider") is both false for someone who already has one and would not
  // fix anything even if it were true, since nothing in this build calls a
  // transcription endpoint regardless of provider state.
  assert.ok(!/请连接|去连接|请先连接/.test(model.recognitionUnavailableNotice));
  // The copy explicitly refutes the wrong implication instead of leaving it
  // unaddressed — proven by mutation: deleting this clause from the notice
  // does not fail this assertion, so it is pinned by CONTENT, not merely by
  // absence of a forbidden phrase.
  assert.match(model.recognitionUnavailableNotice, /即使.*连接.*Provider/);
});

// --- Defect 1: the recording claim must depend on the REAL permission
// state, not be flattened together with the (genuinely unconditional)
// recognition claim. `capture.ts` classifies a `getUserMedia`
// `NotAllowedError` as `permission_denied`, and `requests.ts`'s
// `failureFrom` turns that into an outright `failed` response — so
// recording does not "work normally" whenever `platform.microphonePermission`
// is not `'granted'`. Every test below that predates this one used the
// default `platform()` fixture (`microphonePermission: 'granted'`), which is
// exactly why none of them caught the unconditional claim being wrong for
// every OTHER permission state.

const RECORDING_UNAFFECTED_CLAUSE = '录音与语音朗读功能不受影响，可以正常使用。';

test('when microphone permission is granted, the notice says recording is genuinely unaffected', () => {
  const model = voicePageModel(prefs(), platform({ microphonePermission: 'granted' }));
  assert.ok(
    model.recognitionUnavailableNotice.includes(RECORDING_UNAFFECTED_CLAUSE),
    'this is the one state where the claim is actually true',
  );
});

for (const status of ['denied', 'prompt', 'unavailable'] as const) {
  test(`when microphone permission is '${status}', the notice must not claim recording works normally`, () => {
    const model = voicePageModel(prefs(), platform({ microphonePermission: status }));
    assert.ok(
      !model.recognitionUnavailableNotice.includes(RECORDING_UNAFFECTED_CLAUSE),
      `a user with microphonePermission '${status}' who presses the composer mic button gets an immediate `
      + 'permission_denied failure — the banner must not have just told them recording is unaffected',
    );
    // Speech OUTPUT never touches the microphone — that half of the claim
    // is genuinely independent of permission state and must survive.
    assert.match(
      model.recognitionUnavailableNotice,
      /语音朗读.*(不受影响|正常使用)/,
      'speech synthesis really is unaffected in every state — only the recording half of the old sentence was wrong',
    );
  });

  test(`when microphone permission is '${status}', the notice does not contradict the Status card's own honest message`, () => {
    const model = voicePageModel(prefs(), platform({ microphonePermission: status }));
    assert.ok(
      model.notices.some((n) => n.includes('尚未获得麦克风权限，录音功能无法使用')),
      'sanity check: the Status card below is expected to report recording as blocked in this same render',
    );
    assert.ok(
      !model.recognitionUnavailableNotice.includes(RECORDING_UNAFFECTED_CLAUSE),
      'the top-of-page notice must not say the opposite of what the Status card says three rows below, in the same render',
    );
  });
}

test('a provider-related blocking issue is described as a fact, never as an instruction to connect', () => {
  const noProvider = voicePageModel(prefs(), platform({ providerConfigured: false }));
  assert.ok(noProvider.notices.some((n) => /没有连接任何 Provider/.test(n)));
  assert.ok(!noProvider.notices.some((n) => /请连接|去连接/.test(n)));

  const incapableProvider = voicePageModel(prefs(), platform({ providerConfigured: true, providerTranscriptionCapable: false }));
  assert.ok(incapableProvider.notices.some((n) => /不提供语音转写接口/.test(n)));
  assert.ok(!incapableProvider.notices.some((n) => /请连接|去连接/.test(n)));
});

test('a fully capable, configured, granted setup has no itemized blocking issues', () => {
  const model = voicePageModel(
    prefs({ recognitionMode: 'automatic' }),
    platform({ providerConfigured: true, providerTranscriptionCapable: true, voices: [{ name: 'Alex', default: true, localService: true }] }),
  );
  assert.deepEqual(model.notices, []);
});

test('microphone permission is actionable only when denied', () => {
  assert.equal(voicePageModel(prefs(), platform({ microphonePermission: 'denied' })).microphoneActionable, true);
  assert.equal(voicePageModel(prefs(), platform({ microphonePermission: 'granted' })).microphoneActionable, false);
  assert.equal(voicePageModel(prefs(), platform({ microphonePermission: 'prompt' })).microphoneActionable, false);
  assert.equal(voicePageModel(prefs(), platform({ microphonePermission: 'unavailable' })).microphoneActionable, false);
});

test('the model passes through the persisted rate, voice selection, and auto-play preference untouched', () => {
  const model = voicePageModel(prefs({ rate: 1.5, voiceSelection: 'system:Alex', autoPlayReplies: true }), platform());
  assert.equal(model.rate, 1.5);
  assert.equal(model.voiceSelection, 'system:Alex');
  assert.equal(model.autoPlayReplies, true);
});

test('language falls back to the platform locale only for display, never mutating the stored "auto" preference', () => {
  const model = voicePageModel(prefs({ language: 'auto' }), platform({ localeTag: 'zh-CN' }));
  assert.equal(model.language, 'auto', 'the raw preference must round-trip untouched — normalization belongs to shared/voicePreferences.ts, not this page');
  assert.equal(model.effectiveLanguage, 'zh-CN');
});

// --- Ruling 3: what "the active provider for voice" means ---

test('activeVoiceProviderId reads the provider prefix off the currently selected chat model', () => {
  assert.equal(activeVoiceProviderId('anthropic/claude-opus-5'), 'anthropic');
  assert.equal(activeVoiceProviderId('openai/gpt-5.4'), 'openai');
});

test('activeVoiceProviderId normalizes a "builtin/..." model to anthropic, matching modelCatalog\'s own provider-connect resolution', () => {
  // A BARE `'builtin'` (no `/model`) has no qualified provider prefix at
  // all — `modelReference` reports `providerId: null` for it, the same as
  // any other unqualified string, and `resolveModelSelection` never reaches
  // its own `'builtin' → 'anthropic'` line for that shape either. The
  // normalization only applies to a QUALIFIED `builtin/<model>` reference.
  assert.equal(activeVoiceProviderId('builtin/claude-sonnet'), 'anthropic');
  assert.equal(activeVoiceProviderId('builtin'), null);
});

test('activeVoiceProviderId never guesses: an absent or unqualified model has no active provider', () => {
  // This is the decision itself: rather than falling back to "whichever
  // provider sorts first" (the outcome the controller's ruling explicitly
  // forbids settling for by accident), an unqualified model name resolves
  // to `null` — an honest "we do not know", not a guess.
  assert.equal(activeVoiceProviderId(undefined), null);
  assert.equal(activeVoiceProviderId(''), null);
  assert.equal(activeVoiceProviderId('sonnet'), null);
});
