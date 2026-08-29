/**
 * The desktop's voice platform capability probe.
 *
 * Desktop speech recognition runs through a configured provider's
 * transcription API (there is no on-device recognizer, unlike iOS/Android —
 * see `resolveCapabilities` in `capabilities.ts`'s sibling `voicePreferences`
 * doc for why `recognitionMode: 'localOnly'` is honestly unsupported here).
 * That means "can the microphone actually be used to talk to LingXi" depends
 * on facts from three different places: the OS's microphone-permission
 * grant (read in the MAIN process — see `shared/microphoneAccess.ts` for why
 * the renderer cannot read it itself), the browser's synthesis-voice list,
 * and which provider (if any) is configured — AND whether that specific provider exposes a transcription
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
 * `'unavailable'` means the probe could not determine the permission state at
 * all (no host bridge to ask, an IPC failure, a platform with no media-access
 * API) — distinct from `'prompt'`, which means the OS CAN answer and the
 * answer is "the user hasn't been asked yet".
 */
import { providerById } from '../../shared/providers';
import { isMicrophonePermissionStatus, type MicrophonePermissionStatus } from '../../shared/microphoneAccess';
import { DEFAULT_VOICE_SELECTION, LANGUAGE_AUTO, type VoicePreferences } from './preferences';

/**
 * The one microphone-permission vocabulary, declared once in `shared/` because
 * only the MAIN process can read the fact and only the renderer displays it —
 * see `shared/microphoneAccess.ts`.
 */
export type VoicePermissionStatus = MicrophonePermissionStatus;

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
 * A recognition/playback issue that keeps the running behaviour from
 * matching what the user asked for, named specifically enough that the UI
 * can render an honest, actionable explanation instead of a generic
 * failure. `ProviderCredentialRequired` and `ProviderCannotTranscribe` are
 * DIFFERENT issues, deliberately: the first means "nothing is connected",
 * the second means "something is connected but it cannot transcribe" — see
 * `resolveCapabilities`'s doc comment. Collapsing them would tell a user
 * who already connected a provider to "connect a provider", which is both
 * wrong and unactionable.
 */
export type VoiceBlockingIssue =
  | 'MicrophonePermissionRequired'
  | 'OnDeviceUnsupportedOnDesktop'
  | 'ProviderCredentialRequired'
  | 'ProviderCannotTranscribe'
  | 'RequestedVoiceUnavailable'
  | 'PlaybackVoiceUnavailable';

export interface VoiceCapabilitySnapshot {
  microphonePermission: VoicePermissionStatus;
  requestedRecognitionBackend: VoicePreferences['recognitionMode'];
  effectiveRecognitionBackend: 'provider' | 'unavailable';
  effectiveLanguage: string;
  voiceOptions: VoiceOption[];
  requestedVoice: VoiceOption | null;
  effectiveVoice: VoiceOption | null;
  blockingIssues: VoiceBlockingIssue[];
}

/**
 * Resolves a user's voice preferences against a measured platform snapshot
 * into an honest capability report: what will actually run
 * (`effectiveRecognitionBackend`/`effectiveVoice`), and — whenever that
 * differs from what was requested — exactly which named issue is
 * responsible, never a silent downgrade.
 *
 * Structure mirrors Android's `VoiceSettingsCapabilityResolver.resolve`
 * (`clients/android/.../settings/VoiceSettingsCapabilities.kt`): requested
 * and effective are kept separate, and every difference is explained via
 * `blockingIssues` rather than silently applied. Unlike Android's resolver,
 * this snapshot has no free-text `fallbackReason` sibling: an earlier
 * version did, but its "connect one to enable it" / "connect a different
 * provider that can transcribe" copy was exactly the forbidden imperative
 * this branch exists to remove (false for a user who already connected a
 * provider, and unactionable regardless — nothing in this build calls a
 * transcription endpoint yet, see `Voice.tsx`'s Ruling 1 doc comment), and
 * nothing ever read it (`Voice.tsx` renders its OWN
 * `BLOCKING_ISSUE_MESSAGES`, a `Record<VoiceBlockingIssue, string>` keyed
 * off `blockingIssues` below, specifically so a future new issue variant
 * fails typecheck until it earns its own honest, non-imperative sentence).
 * A free-text field that nothing reads is not a harmless leftover — it is a
 * landmine for the next person wiring up a fallback message, who would
 * reach for the obviously-named field and reintroduce the copy. Deleted
 * instead of "fixed", so there is no second, driftable representation of
 * facts `blockingIssues` + `BLOCKING_ISSUE_MESSAGES` already own.
 *
 * Desktop has no on-device recognizer at all (no Sherpa bindings, unlike
 * iOS/Android), so `recognitionMode: 'localOnly'` always resolves to
 * `'unavailable'` with `OnDeviceUnsupportedOnDesktop` — never a silent
 * fallback to the provider backend the user did not ask for. Desktop speech
 * recognition otherwise runs entirely through a configured provider's
 * transcription endpoint, which is why "no credential" and "credential
 * present but the provider can't transcribe" are reported as two distinct,
 * separately-actionable issues instead of one collapsed boolean.
 *
 * `blockingIssues` order is a deliberate precedence, not incidental
 * `if`-order (see the tests in `voice-capability-snapshot.test.ts` pinning
 * it under several simultaneous issues):
 *   1. `MicrophonePermissionRequired` — recognition cannot work at all
 *      without this, independent of every other setting, and it is the
 *      most immediately actionable (a single OS permission grant).
 *   2. `OnDeviceUnsupportedOnDesktop` — mutually exclusive with 3/4: when
 *      `localOnly` is requested, the reason recognition is unavailable is
 *      the requested MODE itself, so provider facts would be irrelevant
 *      noise and are not also reported.
 *   3. `ProviderCredentialRequired` / 4. `ProviderCannotTranscribe` —
 *      mutually exclusive with each other, only evaluated when
 *      `recognitionMode` is `'automatic'`.
 *   5. `RequestedVoiceUnavailable` / 6. `PlaybackVoiceUnavailable` —
 *      playback-voice problems never block recognition, so they always
 *      sort last.
 */
export function resolveCapabilities(
  prefs: VoicePreferences,
  platform: VoicePlatformSnapshot,
): VoiceCapabilitySnapshot {
  const effectiveLanguage = prefs.language === LANGUAGE_AUTO ? platform.localeTag : prefs.language;
  const micGranted = platform.microphonePermission === 'granted';

  const voiceOptions = platform.systemVoices;
  const requestedVoice = voiceOptions.find((voice) => voice.id === prefs.voiceSelection) ?? null;
  const effectiveVoice =
    requestedVoice
    ?? voiceOptions.find((voice) => voice.id === platform.defaultSystemVoiceId)
    ?? voiceOptions[0]
    ?? null;
  const requestedVoiceMissing =
    prefs.voiceSelection !== DEFAULT_VOICE_SELECTION && !requestedVoice && effectiveVoice != null;

  const blockingIssues: VoiceBlockingIssue[] = [];
  if (!micGranted) blockingIssues.push('MicrophonePermissionRequired');

  if (prefs.recognitionMode === 'localOnly') {
    blockingIssues.push('OnDeviceUnsupportedOnDesktop');
  } else if (!platform.providerConfigured) {
    blockingIssues.push('ProviderCredentialRequired');
  } else if (!platform.providerTranscriptionCapable) {
    blockingIssues.push('ProviderCannotTranscribe');
  }

  if (requestedVoiceMissing) blockingIssues.push('RequestedVoiceUnavailable');
  if (!effectiveVoice) blockingIssues.push('PlaybackVoiceUnavailable');

  const effectiveRecognitionBackend: 'provider' | 'unavailable' =
    micGranted
    && prefs.recognitionMode === 'automatic'
    && platform.providerConfigured
    && platform.providerTranscriptionCapable
      ? 'provider'
      : 'unavailable';

  return {
    microphonePermission: platform.microphonePermission,
    requestedRecognitionBackend: prefs.recognitionMode,
    effectiveRecognitionBackend,
    effectiveLanguage,
    voiceOptions,
    requestedVoice,
    effectiveVoice,
    blockingIssues,
  };
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
   * Reads the current microphone permission state — in production, the OS
   * grant, fetched from the main process (`hostMicrophonePermissionReader`).
   * May reject (the host bridge is IPC, and IPC can fail) — `probePlatform`
   * treats a rejection as `'unavailable'` rather than letting it propagate,
   * since "cannot answer right now" is a real, distinct, non-exceptional
   * state that the row can honestly render as 无法确定.
   */
  queryMicrophonePermission: () => Promise<VoicePermissionStatus>;
  /** BCP-47 system locale tag, e.g. `navigator.language`. */
  localeTag: string;
  /** Whether the currently active provider has a credential configured. */
  providerConfigured: boolean;
  /**
   * Whether that configured provider supports transcription. Production
   * callers get this from `browserProbeDeps`, which computes it via
   * `resolveActiveProviderVoiceCapability` (below) —
   * `providerById(activeProviderId)?.transcriptionCapable ?? false`
   * (see `shared/providers.ts`). Tests may still supply this raw.
   * `probePlatform` additionally clamps this to `false` whenever
   * `providerConfigured` is `false`, as a defensive floor — "capable but
   * not configured" must never reach the snapshot, even if a caller's join
   * logic gets that wrong.
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
 * A minimal structural fact about one provider's credential state — the
 * same shape `bridge/modelCatalog.ts`'s `resolveModelSelection` already
 * accepts (`{ providerId, configured }`), kept local here instead of
 * importing the IPC bridge's `ProviderCredentialMetadata` so this probe
 * module stays decoupled from it.
 */
export interface ProviderConfiguredFact {
  readonly providerId: string;
  readonly configured: boolean;
}

/**
 * The join from "which provider is currently active" to
 * `providerById(...).transcriptionCapable` (`shared/providers.ts`).
 *
 * This used to be a contract stated only in a doc comment on
 * `ProbeDeps.providerTranscriptionCapable`, asking a future caller to
 * compute `providerById(activeProviderId)?.transcriptionCapable ?? false`
 * by hand. Implemented here once instead, so no call site can silently
 * skip the lookup (e.g. by assuming "configured" implies "capable" —
 * exactly wrong for a configured Anthropic key, which has no transcription
 * endpoint at all).
 *
 * `providerConfigured` is derived from `credentials` rather than trusted
 * as a caller-supplied boolean, so "capable" can never be claimed for a
 * provider that in fact has no credential on file — the same defensive
 * posture `probePlatform` already enforces on its own inputs.
 */
export function resolveActiveProviderVoiceCapability(
  activeProviderId: string | null,
  credentials: readonly ProviderConfiguredFact[],
): { providerConfigured: boolean; providerTranscriptionCapable: boolean } {
  const providerConfigured =
    activeProviderId != null
    && credentials.some((entry) => entry.providerId === activeProviderId && entry.configured);
  const providerTranscriptionCapable =
    providerConfigured
    && (providerById(activeProviderId as string)?.transcriptionCapable ?? false);
  return { providerConfigured, providerTranscriptionCapable };
}

/** The one method of the preload bridge this module needs — `useBridge`'s `microphonePermission` is built on it. */
export interface MicrophoneAccessHost {
  microphoneAccess(): Promise<unknown>;
}

/**
 * The renderer's only honest source of the microphone grant: ask the main
 * process, which reads `systemPreferences.getMediaAccessStatus('microphone')`.
 *
 * There is deliberately NO browser fallback. The obvious one —
 * `navigator.permissions.query({name:'microphone'})` — is what this branch
 * removed: it reports the PAGE permission, which `main/index.ts`'s
 * `setPermissionCheckHandler` grants this app's own renderer unconditionally,
 * so it said `granted` on a machine where macOS had never granted anything
 * (measured; see `shared/microphoneAccess.ts`). Falling back to it when the
 * host is missing would restore exactly that lie in exactly the situation
 * where we know least. With no host (a plain browser dev run) the honest
 * answer is `'unavailable'` — the row then says 无法确定.
 *
 * An answer that is not one of the four known states is `'unavailable'` too:
 * a value this renderer cannot interpret is not evidence of a grant.
 */
export function hostMicrophonePermissionReader(
  host: MicrophoneAccessHost | undefined,
): () => Promise<VoicePermissionStatus> {
  return async () => {
    if (typeof host?.microphoneAccess !== 'function') return 'unavailable';
    const answer = await host.microphoneAccess();
    return isMicrophonePermissionStatus(answer) ? answer : 'unavailable';
  };
}

/** Structural subset of an `EventTarget` this module subscribes to; the real `window`/`document` satisfy it. */
export interface GrantChangeTarget {
  addEventListener(type: string, listener: () => void): void;
  removeEventListener(type: string, listener: () => void): void;
}

/**
 * Subscribes to the moments the OS microphone grant can have changed under a
 * running app, and calls `onChange` for each.
 *
 * The grant is not a value the app owns: the user can flip it in System
 * Settings → Privacy & Security → Microphone at any time, including from the
 * very button this page renders. A row that reads it once at mount is wrong
 * from that moment on — and wrong in the direction that matters, since the
 * user changing it is usually the user acting on this page's own advice.
 *
 * macOS emits no event for a TCC change, so the trigger is the user coming
 * BACK: a window `focus`, or the document becoming visible again. Both are
 * needed — switching apps and returning fires `focus`; a window revealed
 * without taking focus fires only `visibilitychange`. `hidden` is ignored so
 * leaving does not spend an IPC round trip.
 */
export function subscribeMicrophoneGrantChanges(
  onChange: () => void,
  targets: { window: GrantChangeTarget; document: GrantChangeTarget & { visibilityState?: string } },
): () => void {
  const onFocus = () => onChange();
  const onVisibilityChange = () => { if (targets.document.visibilityState !== 'hidden') onChange(); };
  targets.window.addEventListener('focus', onFocus);
  targets.document.addEventListener('visibilitychange', onVisibilityChange);
  return () => {
    targets.window.removeEventListener('focus', onFocus);
    targets.document.removeEventListener('visibilitychange', onVisibilityChange);
  };
}

/**
 * Builds `ProbeDeps` from the real browser globals, for production use.
 *
 * The two provider facts are not derivable from any browser API, so they
 * are computed here via `resolveActiveProviderVoiceCapability` from
 * whichever provider the caller currently treats as active and the
 * desktop's known credential list — see that function's doc comment for
 * why this join lives in code, not in a comment.
 *
 * The microphone grant is not derivable from a browser API either — not
 * honestly — so it is a required PARAMETER rather than something read here:
 * the caller supplies `useBridge`'s `microphonePermission`, which asks the
 * main process. Requiring it means no call site can silently fall back to
 * the Permissions API answer that made the 麦克风权限 row lie.
 */
export function browserProbeDeps(
  activeProviderId: string | null,
  credentials: readonly ProviderConfiguredFact[],
  queryMicrophonePermission: () => Promise<VoicePermissionStatus>,
): ProbeDeps {
  const { providerConfigured, providerTranscriptionCapable } = resolveActiveProviderVoiceCapability(
    activeProviderId,
    credentials,
  );
  return {
    synth: window.speechSynthesis,
    queryMicrophonePermission,
    localeTag: navigator.language,
    providerConfigured,
    providerTranscriptionCapable,
  };
}
