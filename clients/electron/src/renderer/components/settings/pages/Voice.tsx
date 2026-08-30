import { useEffect, useRef, useState } from 'react';
import { Card, Row } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import type { PageContentProps } from '../SettingsScreen';
import { modelReference } from '../../../bridge/modelCatalog';
import {
  browserProbeDeps,
  probePlatform,
  resolveCapabilities,
  subscribeMicrophoneGrantChanges,
  type VoiceBlockingIssue,
  type VoiceOption,
  type VoicePermissionStatus,
  type VoicePlatformSnapshot,
} from '../../../audio/capabilities';
import { defaultVoicePreferences, LANGUAGE_AUTO, type VoicePreferences } from '../../../../shared/voicePreferences';
import { ghostButtonStyle, inputStyle } from './ghostButton';

/**
 * Ruling 1 (Task 9 of the desktop-audio-capability plan): speech
 * RECOGNITION does not work on this desktop build at all, unconditionally —
 * see `renderer/audio/requests.ts`'s `DESKTOP_TRANSCRIPTION_UNAVAILABLE_MESSAGE`:
 * `serviceAudioOp`'s `'transcribe'` case always answers `failed`/`unavailable`,
 * regardless of microphone permission, `recognitionMode`, or which provider
 * is configured. The engine has no built-in recognizer, and the renderer
 * cannot call a hosted transcription API either — provider credentials live
 * only in the Rust engine (`main/host.ts` forwards a credential straight
 * through and keeps nothing), so there is no key in this process to
 * authenticate a transcription call with even when a transcription-capable
 * provider IS connected.
 *
 * This must be said PLAINLY and UNCONDITIONALLY — never gated behind
 * `resolveCapabilities`'s blocking-issue computation, which can legitimately
 * report zero recognition-blocking issues (mic granted, a transcription-
 * capable provider configured) while recognition still does not work,
 * because nothing in this build actually calls that provider's transcription
 * endpoint. Showing the itemized facts ONLY, with no issues present, would
 * read as "this should work" — exactly backwards.
 */
const RECOGNITION_UNAVAILABLE_BASE =
  '语音识别在当前桌面版本中尚不可用：这个客户端没有内置的语音识别引擎，也还没有能调用转写服务的通道——'
  + '即使已经连接了 Provider 也是如此，因为 Provider 凭据只保存在引擎一侧，桌面渲染进程本身拿不到它去调用转写接口。';

const MIC_PERMISSION_LABELS: Record<VoicePermissionStatus, string> = {
  granted: '已授权',
  denied: '未授权',
  prompt: '尚未询问',
  unavailable: '无法确定',
};

/**
 * Own, honest copy for each `VoiceBlockingIssue` — deliberately NOT
 * `resolveCapabilities`'s `fallbackReason` (`renderer/audio/capabilities.ts`).
 * Two of that field's branches ("connect one to enable it" / "connect a
 * different provider that can transcribe") were written by Task 6 before
 * Task 8 discovered the unconditional fact above; repeating them here would
 * be exactly the "implies a user action that would not actually help" copy
 * the controller's ruling forbids, since connecting or switching a provider
 * would not make recognition work today regardless. These messages are
 * deliberately non-imperative statements of fact instead — informative
 * (this is WHY `resolveActiveProviderVoiceCapability`'s join still has an
 * observable effect on this page, see `activeVoiceProviderId` below) without
 * ever claiming a fix would help. `Record<VoiceBlockingIssue, string>` is
 * total, so a future new issue variant fails typecheck here until it gets
 * its own honest sentence, rather than silently rendering nothing.
 */
const BLOCKING_ISSUE_MESSAGES: Record<VoiceBlockingIssue, string> = {
  MicrophonePermissionRequired: '尚未获得麦克风权限，录音功能无法使用。',
  OnDeviceUnsupportedOnDesktop: '桌面端没有内置的离线识别模型，"仅本设备"在这里暂不可用（不同于 iOS/Android）。',
  ProviderCredentialRequired: '当前没有连接任何 Provider。',
  ProviderCannotTranscribe: '当前连接的 Provider 不提供语音转写接口。',
  RequestedVoiceUnavailable: '之前选择的朗读音色已经不存在，目前使用的是其他可用音色。',
  PlaybackVoiceUnavailable: '这台设备上没有可用于朗读的系统音色。',
};

/**
 * The recognition-unavailable notice's second half — the claim about
 * RECORDING — genuinely depends on `microphonePermission`, unlike the
 * recognition claim above it: `capture.ts` classifies a `getUserMedia`
 * `NotAllowedError` as `permission_denied`, and `requests.ts`'s
 * `failureFrom` turns that into an outright `failed` response, so recording
 * really does not work whenever permission is not `'granted'`. Speech
 * OUTPUT (synthesis) never touches the microphone, so ITS claim stays true
 * and present unconditionally — collapsing the two back into one sentence
 * (the bug this function fixes) would either overclaim recording works when
 * it does not, or, if written the other way, wrongly imply playback is
 * broken too.
 *
 * Uses the exact same `microphonePermission === 'granted'` predicate as
 * `resolveCapabilities`'s `MicrophonePermissionRequired` check (and reuses
 * that issue's own message text for the "blocked" branch) so this banner
 * can never end up disagreeing with the Status card rendered below it in
 * the same page — see `settings-voice-page.test.ts`'s
 * "does not contradict the Status card" tests.
 */
function recognitionUnavailableNotice(microphonePermission: VoicePermissionStatus): string {
  const recordingClaim = microphonePermission === 'granted'
    ? '录音与语音朗读功能不受影响，可以正常使用。'
    : `${BLOCKING_ISSUE_MESSAGES.MicrophonePermissionRequired}语音朗读不需要使用麦克风，不受影响，可以正常使用。`;
  return `${RECOGNITION_UNAVAILABLE_BASE}${recordingClaim}`;
}

export interface VoiceRecognitionOption {
  id: VoicePreferences['recognitionMode'];
  label: string;
  disabled: boolean;
  disabledReason: string | null;
}

export interface VoicePageModel {
  recognitionOptions: VoiceRecognitionOption[];
  recognitionMode: VoicePreferences['recognitionMode'];
  recognitionUnavailableNotice: string;
  language: string;
  effectiveLanguage: string;
  microphonePermission: VoicePermissionStatus;
  microphonePermissionLabel: string;
  microphoneActionable: boolean;
  voiceOptions: VoiceOption[];
  voiceSelection: string;
  effectiveVoiceId: string | null;
  rate: number;
  notices: string[];
}

/**
 * Ruling 3 (Task 9): what "the active provider for voice" means when
 * several are configured. There is no dedicated "active provider for
 * voice" concept anywhere in this codebase — the only existing notion of
 * "which provider is active" is the provider prefix of the currently
 * selected CHAT model (`PublicSettings.model`; `bridge/modelCatalog.ts`'s
 * `resolveModelSelection` already keys the provider-connect flow off the
 * exact same parse). Reusing that instead of inventing a second "active"
 * concept means:
 *   - it can never be settled by accident, e.g. by whichever provider
 *     happens to sort first in a credentials list — an absent/unqualified
 *     model resolves to `null` (no active provider), not a guess;
 *   - a `'builtin'` model is Anthropic under the hood, so it is normalized
 *     exactly the way `resolveModelSelection` already normalizes it,
 *     rather than a second place that could silently drift from that
 *     mapping (see `modelCatalog.ts`'s own handling of `'builtin'`).
 * If desktop voice ever gets a model/provider picker independent of chat,
 * this is the one function to change. Pinned by
 * `settings-voice-page.test.ts`.
 */
export function activeVoiceProviderId(model: string | undefined): string | null {
  const parsed = modelReference(model ?? '');
  if (!parsed.providerId) return null;
  return parsed.providerId === 'builtin' ? 'anthropic' : parsed.providerId;
}

/** Pure computation of everything the page renders — probed voices in, never a hardcoded list; see `settings-voice-page.test.ts`. */
export function voicePageModel(prefs: VoicePreferences, platform: VoicePlatformSnapshot): VoicePageModel {
  const capability = resolveCapabilities(prefs, platform);

  const recognitionOptions: VoiceRecognitionOption[] = [
    { id: 'automatic', label: '自动', disabled: false, disabledReason: null },
    {
      id: 'localOnly',
      label: '仅本设备（离线）',
      // Ruling 2: UNCONDITIONALLY disabled — there are no Sherpa bindings
      // on this desktop build no matter what `prefs.recognitionMode`
      // currently is. Offered, never hidden (hiding it would make the
      // desktop look like it has no such concept at all), and never
      // silently downgraded to automatic — the disabled control plus this
      // visible reason is the whole point.
      disabled: true,
      disabledReason: BLOCKING_ISSUE_MESSAGES.OnDeviceUnsupportedOnDesktop,
    },
  ];

  return {
    recognitionOptions,
    recognitionMode: prefs.recognitionMode,
    recognitionUnavailableNotice: recognitionUnavailableNotice(platform.microphonePermission),
    language: prefs.language,
    effectiveLanguage: capability.effectiveLanguage,
    microphonePermission: platform.microphonePermission,
    microphonePermissionLabel: MIC_PERMISSION_LABELS[platform.microphonePermission],
    // Only `denied` is something "open System Settings" can fix — `prompt`
    // means the OS has not been asked yet (nothing to open), and
    // `unavailable` means this browser cannot even report the state.
    microphoneActionable: platform.microphonePermission === 'denied',
    voiceOptions: capability.voiceOptions,
    voiceSelection: prefs.voiceSelection,
    effectiveVoiceId: capability.effectiveVoice?.id ?? null,
    rate: prefs.rate,
    notices: capability.blockingIssues.map((issue) => BLOCKING_ISSUE_MESSAGES[issue]),
  };
}

/**
 * The voice settings page. `needsEngine: false` / `layered: false`
 * (`nav.ts`) — like `appearance`/`diagnostics`, this reads and writes only
 * the device-level Electron store (`PublicSettings.voice`, Task 4), never
 * an engine settings layer, so there is no `LayerSwitcher` and no
 * `snapshot` dependency.
 */
export function Voice({ bridge }: PageContentProps) {
  const t = useT();
  const prefs = bridge.bootstrap?.settings?.voice ?? defaultVoicePreferences();
  const credentials = bridge.bootstrap?.providerCredentials ?? [];
  const activeProviderId = activeVoiceProviderId(bridge.bootstrap?.settings?.model);

  // Only which providers are CONFIGURED can change
  // `resolveActiveProviderVoiceCapability`'s answer (see its own doc) — so the
  // probe effect below depends on a stable key of that set rather than on the
  // whole `credentials` array, which is a fresh reference on every bootstrap
  // patch (including ones unrelated to providers, e.g. a new diagnostic log
  // line). Depending on the array directly would re-probe the microphone
  // permission and voice list on every unrelated settings change while this
  // page happens to be open.
  const configuredProviderIds = credentials
    .filter((entry) => entry.configured)
    .map((entry) => entry.providerId)
    .sort()
    .join(',');

  // Always the LATEST credentials at probe time, without being a probe
  // dependency itself — same "ref for latest value, narrow effect deps"
  // idiom `SettingsScreen.tsx` uses for `onCloseRef`.
  const credentialsRef = useRef(credentials);
  credentialsRef.current = credentials;

  const [platform, setPlatform] = useState<VoicePlatformSnapshot | null>(null);
  // Bumped whenever the OS microphone grant may have changed while this page
  // is open — see the subscription effect below. Re-probing everything rather
  // than patching one field keeps a single code path for what the page shows:
  // `readSystemVoices` returns its already-populated list immediately, so the
  // extra work is one IPC round trip.
  const [grantGeneration, setGrantGeneration] = useState(0);
  // Same "ref for latest value, narrow effect deps" idiom as `credentialsRef`
  // above, and for a sharper reason: `useBridge` returns a stable
  // `microphonePermission`, but a caller that rebuilt it per render would
  // otherwise make this effect re-run on the very state update it causes —
  // an endless probe loop rather than a wrong value.
  const readMicrophonePermissionRef = useRef(bridge.microphonePermission);
  readMicrophonePermissionRef.current = bridge.microphonePermission;
  useEffect(() => {
    let cancelled = false;
    // `probePlatform` never rejects (permission/voice-list failures are
    // caught internally and reported as honest states, not exceptions —
    // see its own doc), so no `.catch` is needed here.
    void probePlatform(browserProbeDeps(
      activeProviderId,
      credentialsRef.current,
      () => readMicrophonePermissionRef.current(),
    )).then((snapshot) => {
      if (!cancelled) setPlatform(snapshot);
    });
    return () => { cancelled = true; };
  }, [activeProviderId, configuredProviderIds, grantGeneration]);

  // The microphone grant lives in System Settings, not in this app, and the
  // user can change it while this page is open — most often by acting on the
  // 「打开系统设置」 button this very page renders. macOS emits no event for
  // that, so the trigger is the user coming back to the window.
  useEffect(
    () => subscribeMicrophoneGrantChanges(
      () => setGrantGeneration((generation) => generation + 1),
      { window, document },
    ),
    [],
  );

  // `speechSynthesis.getVoices()` is frequently empty on the very first
  // call until `voiceschanged` fires (`capabilities.ts`'s `readSystemVoices`
  // already handles the wait) — showing the form before the probe resolves
  // would risk confidently rendering an empty voice list on a machine that
  // actually has dozens.
  const [languageDraft, setLanguageDraft] = useState(() => (prefs.language === LANGUAGE_AUTO ? '' : prefs.language));

  const persist = (next: VoicePreferences) => {
    // `bridge.setVoicePreferences` sets the global bridge error AND
    // rethrows on failure (the same `capture` every settings page's write
    // depends on — see `bridge-error-reaches-callers.test.ts`). This page
    // has no dedicated inline error row for a voice-preference write —
    // unlike the custom API base URL, a voice write never restarts the
    // bridge and there is nothing further for a local error state to add —
    // so it relies on the shell's own global `<ErrorBanner>`, the same
    // choice `Appearance.tsx` makes for `setThemePreference`.
    void bridge.setVoicePreferences(next).catch(() => undefined);
  };

  if (!platform) {
    return (
      <Card title="语音">
        <div style={{ padding: '24px 18px', color: t.text4, fontSize: 12.5 }}>正在检测设备的语音能力…</div>
      </Card>
    );
  }

  const model = voicePageModel(prefs, platform);
  const disabledRecognitionOption = model.recognitionOptions.find((option) => option.disabled) ?? null;

  const setMode = (recognitionMode: VoicePreferences['recognitionMode']) => {
    const option = model.recognitionOptions.find((candidate) => candidate.id === recognitionMode);
    if (option?.disabled) return; // Defensive: a disabled control should never dispatch a click at all.
    persist({ ...prefs, recognitionMode });
  };
  const commitLanguage = () => {
    const trimmed = languageDraft.trim();
    persist({ ...prefs, language: trimmed === '' ? LANGUAGE_AUTO : trimmed });
  };
  const setVoiceSelection = (voiceSelection: string) => persist({ ...prefs, voiceSelection });
  const setRate = (rate: number) => persist({ ...prefs, rate });

  return (
    <>
      <Card title="识别">
        <div role="status" data-testid="voice-recognition-unavailable-notice" style={{ padding: '14px 18px', fontSize: 12.5, color: t.text2, lineHeight: 1.6 }}>
          {model.recognitionUnavailableNotice}
        </div>
        <Row title="识别模式" align="center">
          {/* Visible text, not a `title=` tooltip: Chromium does not
              dispatch the pointer events a native tooltip needs on a
              DISABLED control, and a tooltip is invisible to keyboard/
              screen-reader users regardless — same reasoning, and same
              "pill plus a real, visible, id-bearing line under it" shape,
              as `SettingsScreen.tsx`'s `LayerSwitcher`. A `display:none`
              node would be dropped from the accessibility tree entirely,
              which would make `aria-describedby` point at nothing — this
              text has to actually render for that attribute to mean
              anything. */}
          <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'flex-end', gap: 4 }}>
            <div style={{ display: 'inline-flex', padding: 3, gap: 2, borderRadius: 9, background: t.sidebarBg, border: `0.5px solid ${t.border}` }}>
              {model.recognitionOptions.map((option) => {
                const active = model.recognitionMode === option.id;
                return (
                  <button
                    key={option.id}
                    type="button"
                    data-recognition-mode={option.id}
                    disabled={option.disabled}
                    aria-pressed={active}
                    aria-describedby={option.disabled ? 'voice-recognition-mode-disabled-reason' : undefined}
                    onClick={() => setMode(option.id)}
                    style={{
                      padding: '5px 14px', borderRadius: 7, border: 'none', fontFamily: 'inherit',
                      cursor: option.disabled ? 'not-allowed' : 'pointer',
                      background: active ? t.surface : 'transparent',
                      color: active ? t.text : option.disabled ? t.text4 : t.text3,
                      fontSize: 12.5, fontWeight: active ? 600 : 500,
                    }}
                  >
                    {option.label}
                  </button>
                );
              })}
            </div>
            {disabledRecognitionOption?.disabledReason && (
              <div id="voice-recognition-mode-disabled-reason" data-testid="voice-recognition-mode-disabled-reason" style={{ fontSize: 11, color: t.text4, maxWidth: 320, textAlign: 'right' }}>
                {disabledRecognitionOption.disabledReason}
              </div>
            )}
          </div>
        </Row>
        <Row title="识别语言" desc={`当前生效：${model.effectiveLanguage}${prefs.language === LANGUAGE_AUTO ? '（跟随系统）' : ''}`} align="center">
          <input
            value={languageDraft}
            onChange={(event) => setLanguageDraft(event.target.value)}
            onBlur={commitLanguage}
            placeholder="auto"
            aria-label="识别语言"
            style={inputStyle(t)}
          />
        </Row>
        <Row title="麦克风权限" desc={`当前状态：${model.microphonePermissionLabel}`} align="center">
          {model.microphoneActionable && (
            <button
              type="button"
              onClick={() => void bridge.openSystemSettings('microphone')}
              style={ghostButtonStyle(t)}
            >
              打开系统设置
            </button>
          )}
        </Row>
      </Card>

      <Card title="朗读">
        <Row title="音色" desc={model.voiceOptions.length === 0 ? '这台设备上没有可用的系统嗓音。' : '来自这台设备实际安装的系统嗓音；需要联网的嗓音已标注。'} align="center">
          <select
            value={model.voiceSelection}
            onChange={(event) => setVoiceSelection(event.target.value)}
            aria-label="朗读音色"
            style={inputStyle(t)}
          >
            {model.voiceOptions.length === 0 && <option value={model.voiceSelection}>{model.voiceSelection}</option>}
            {model.voiceOptions.map((voice) => (
              <option key={voice.id} value={voice.id}>
                {voice.label}{voice.networkRequired ? '（需要网络）' : ''}
              </option>
            ))}
          </select>
        </Row>
        <Row title="语速" desc="0.5× – 2.0×" align="center">
          <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
            <input
              type="range"
              min={0.5}
              max={2}
              step={0.05}
              value={model.rate}
              onChange={(event) => setRate(Number(event.target.value))}
              aria-label="朗读语速"
            />
            <span style={{ fontSize: 12.5, color: t.text3, minWidth: 40 }}>{model.rate.toFixed(2)}×</span>
          </div>
        </Row>
      </Card>

      <Card title="状态">
        <div data-testid="voice-notices" style={{ padding: '14px 18px', display: 'flex', flexDirection: 'column', gap: 10 }}>
          {model.notices.length === 0
            ? <span style={{ color: t.text4, fontSize: 12.5 }}>没有需要额外说明的问题。</span>
            : model.notices.map((notice, index) => (
              <div key={index} style={{ fontSize: 12.5, color: t.text2, lineHeight: 1.6 }}>· {notice}</div>
            ))}
        </div>
      </Card>
    </>
  );
}
