/**
 * The desktop's voice platform capability probe.
 *
 * Desktop speech recognition runs through a configured provider's
 * transcription API (there is no on-device recognizer, unlike iOS/Android —
 * see `resolveCapabilities` in `capabilities.ts`'s sibling `voicePreferences`
 * doc for why `recognitionMode: 'localOnly'` is honestly unsupported here).
 * That means "can the microphone actually be used to talk to LingXi" depends
 * on facts from three different places: the OS's microphone-permission
 * grant, the browser's synthesis-voice list, and which provider (if any) is
 * configured — AND whether that specific provider exposes a transcription
 * endpoint at all. A configured Anthropic key, for example, is a fully
 * configured provider that still cannot transcribe anything, because
 * Anthropic's API has no transcription endpoint (see
 * `shared/providers.ts`'s `ProviderDefinition.transcriptionCapable`).
 *
 * This module only PROBES and REPORTS those facts as a `VoicePlatformSnapshot`
 * — it does not decide what to do about a denied permission or an incapable
 * provider. That decision (blocking issues, fallback reasons) is
 * `resolveCapabilities`'s job, consuming this snapshot alongside
 * `VoicePreferences`.
 *
 * Every real-world input is INJECTED (`ProbeDeps`) rather than read from
 * global browser objects directly, so every branch — permission
 * granted/denied/prompt/unavailable, zero voices, voices that only appear
 * after `voiceschanged` fires, a provider that's configured but can't
 * transcribe, a would-be-capable provider that isn't configured — can be
 * driven from a plain test fixture with no real browser involved.
 * `browserProbeDeps` at the bottom wires the injected shape to the real
 * globals for production use.
 */

/**
 * `'unavailable'` means the probe could not determine the permission state
 * at all (the Permissions API is unsupported for `'microphone'`, or the
 * query threw) — distinct from `'prompt'`, which means the browser CAN
 * answer and the answer is "the user hasn't been asked yet".
 */
export type VoicePermissionStatus = 'granted' | 'denied' | 'prompt' | 'unavailable';

export interface VoiceOption {
  id: string;
  label: string;
  languageTag: string;
  source: 'system' | 'sherpa';
  familyId: string;
  isDefault?: boolean;
  networkRequired?: boolean;
}

export interface VoicePlatformSnapshot {
  /** BCP-47 system locale tag, e.g. `navigator.language`. */
  localeTag: string;
  microphonePermission: VoicePermissionStatus;
  /**
   * Whether the currently active provider has ANY credential configured.
   * NOT sufficient on its own to say speech recognition will work — see
   * `providerTranscriptionCapable`. Kept as its own field because the UI
   * needs "nothing is configured" (→ connect a provider) told apart from
   * "something is configured but it can't transcribe" (→ connect a
   * DIFFERENT provider). Collapsing the two back into one boolean is
   * exactly the dishonesty this probe exists to remove: it would tell a
   * user who already connected Anthropic to "connect a provider", which is
   * both wrong and unactionable.
   */
  providerConfigured: boolean;
  /**
   * Whether the currently configured provider exposes a hosted
   * transcription endpoint reachable with that credential (see
   * `shared/providers.ts`'s `ProviderDefinition.transcriptionCapable`).
   * Always `false` when `providerConfigured` is `false` — there is no
   * configured provider to be capable of anything.
   */
  providerTranscriptionCapable: boolean;
  systemVoices: VoiceOption[];
  /**
   * The `system:<id>` of whichever enumerated voice the OS/browser flags as
   * its own default, or of the first enumerated voice when none is flagged.
   * `''` when `systemVoices` is empty — never a sentinel id that names no
   * real voice; see `probePlatform`'s doc comment on this exact point.
   */
  defaultSystemVoiceId: string;
}

/**
 * Structural subset of the real `SpeechSynthesis` Web API this module needs.
 * The real `window.speechSynthesis` satisfies this directly (see
 * `browserProbeDeps`); tests pass a small fake that can simulate the
 * `voiceschanged` race deliberately.
 */
export interface SpeechSynthesisLike {
  getVoices(): SpeechSynthesisVoice[];
  addEventListener(type: 'voiceschanged', listener: () => void): void;
  removeEventListener(type: 'voiceschanged', listener: () => void): void;
}

/** Default bound for `readSystemVoices`'s wait on a late `voiceschanged`. */
const DEFAULT_VOICE_LIST_TIMEOUT_MS = 1000;

/**
 * Reads the system voice list, working around Chromium's well-documented
 * race: on the very first call in a fresh renderer, `speechSynthesis.
 * getVoices()` returns `[]` synchronously, because the OS voice list loads
 * asynchronously and Chromium only fires `voiceschanged` once it is ready.
 * A naive one-shot read would report "no voices available" on a machine
 * that actually has dozens, and the settings page would confidently show a
 * wrong empty list.
 *
 * If the immediate read is non-empty, it is trusted as-is (some platforms —
 * notably macOS/Safari — never fire `voiceschanged` at all because the list
 * is already populated by the time a page can ask). Otherwise this waits
 * once for `voiceschanged`, bounded by `timeoutMs`, and re-reads
 * `getVoices()` when it fires OR when the timeout elapses — so a synth that
 * never populates (no voices installed at all) still resolves to `[]`
 * instead of hanging the caller forever.
 */
export function readSystemVoices(
  synth: SpeechSynthesisLike,
  timeoutMs: number = DEFAULT_VOICE_LIST_TIMEOUT_MS,
): Promise<SpeechSynthesisVoice[]> {
  const immediate = synth.getVoices();
  if (immediate.length > 0) return Promise.resolve(immediate);

  return new Promise((resolve) => {
    let settled = false;
    const finish = () => {
      if (settled) return;
      settled = true;
      synth.removeEventListener('voiceschanged', onVoicesChanged);
      clearTimeout(timer);
      resolve(synth.getVoices());
    };
    const onVoicesChanged = () => finish();
    const timer = setTimeout(finish, timeoutMs);
    synth.addEventListener('voiceschanged', onVoicesChanged);
  });
}

export interface ProbeDeps {
  /** Speech-synthesis voice source; see `readSystemVoices`. */
  synth: SpeechSynthesisLike;
  /**
   * Reads the current microphone permission state. May reject (e.g. the
   * Permissions API throws for an unsupported name in some browsers) —
   * `probePlatform` treats a rejection as `'unavailable'` rather than
   * letting it propagate, since "this browser cannot answer" is a real,
   * distinct, non-exceptional state.
   */
  queryMicrophonePermission: () => Promise<VoicePermissionStatus>;
  /** BCP-47 system locale tag, e.g. `navigator.language`. */
  localeTag: string;
  /** Whether the currently active provider has a credential configured. */
  providerConfigured: boolean;
  /**
   * Whether that configured provider supports transcription. Callers
   * should compute this as
   * `providerById(activeProviderId)?.transcriptionCapable ?? false`
   * (see `shared/providers.ts`). `probePlatform` additionally clamps this
   * to `false` whenever `providerConfigured` is `false`, as a defensive
   * floor — "capable but not configured" must never reach the snapshot,
   * even if a caller's join logic gets that wrong.
   */
  providerTranscriptionCapable: boolean;
  /** Overrides `readSystemVoices`'s wait bound; tests use a short value. */
  voiceListTimeoutMs?: number;
}

export async function probePlatform(deps: ProbeDeps): Promise<VoicePlatformSnapshot> {
  const [voices, microphonePermission] = await Promise.all([
    readSystemVoices(deps.synth, deps.voiceListTimeoutMs),
    resolveMicrophonePermission(deps.queryMicrophonePermission),
  ]);

  const systemVoices: VoiceOption[] = voices.map((voice) => ({
    id: `system:${voice.name}`,
    label: voice.name,
    languageTag: voice.lang || deps.localeTag,
    source: 'system',
    familyId: 'system',
    isDefault: voice.default,
    networkRequired: !voice.localService,
  }));

  // Never fabricate an id that names no real voice: prefer the OS-flagged
  // default, else the first ENUMERATED voice (a real id), else '' when
  // there is nothing to default to at all.
  const defaultSystemVoiceId =
    systemVoices.find((voice) => voice.isDefault)?.id
    ?? systemVoices[0]?.id
    ?? '';

  return {
    localeTag: deps.localeTag,
    microphonePermission,
    providerConfigured: deps.providerConfigured,
    providerTranscriptionCapable: deps.providerConfigured && deps.providerTranscriptionCapable,
    systemVoices,
    defaultSystemVoiceId,
  };
}

async function resolveMicrophonePermission(
  query: () => Promise<VoicePermissionStatus>,
): Promise<VoicePermissionStatus> {
  try {
    return await query();
  } catch {
    return 'unavailable';
  }
}

/**
 * Builds `ProbeDeps` from the real browser globals, for production use.
 * The two provider facts are not derivable from any browser API, so the
 * caller (whoever knows which provider is currently active) supplies them
 * — see `ProbeDeps.providerConfigured`/`.providerTranscriptionCapable`.
 */
export function browserProbeDeps(provider: {
  configured: boolean;
  transcriptionCapable: boolean;
}): ProbeDeps {
  return {
    synth: window.speechSynthesis,
    queryMicrophonePermission: queryBrowserMicrophonePermission,
    localeTag: navigator.language,
    providerConfigured: provider.configured,
    providerTranscriptionCapable: provider.transcriptionCapable,
  };
}

async function queryBrowserMicrophonePermission(): Promise<VoicePermissionStatus> {
  if (!navigator.permissions?.query) return 'unavailable';
  const status = await navigator.permissions.query({ name: 'microphone' });
  return status.state;
}
