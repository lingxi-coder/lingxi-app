/**
 * Fixture for the microphone-permission read, mounted in a REAL Electron
 * renderer by `microphone-permission-electron.mjs`.
 *
 * What is REAL here: the `Voice` settings page component itself, mounted with
 * real effects; `hostMicrophonePermissionReader` and its production IPC
 * channel; a real `contextBridge` preload
 * over real IPC to the production `readMicrophoneAccess` in the main process,
 * which reads the real `systemPreferences.getMediaAccessStatus('microphone')`;
 * and a real Chromium session carrying this app's own
 * `setPermissionCheckHandler` (copied verbatim into the driver), which is what
 * makes `navigator.permissions.query({name:'microphone'})` answer `granted`
 * for the app's own renderer no matter what macOS thinks.
 *
 * What is a stand-in: the narrow `audioRequest({type:'get_snapshot'})` adapter
 * that projects the real OS reader into the snapshot consumed by `Voice`.
 * Production uses NativeAudioManager for this request; this fixture does not
 * start that helper, and it does not fake the OS permission answer.
 *
 * Nothing about the microphone ANSWER is faked: that is the point. The defect
 * was invisible to unit tests precisely because "the page permission" and "the
 * OS grant" are only distinguishable against a real machine's real TCC state.
 */
import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';

import { hostMicrophonePermissionReader } from '../../src/shared/microphoneAccess';
import { Voice } from '@renderer/components/settings/pages/Voice';
import { Theme } from '@renderer/theme/ThemeContext';
import { tokens } from '@renderer/theme/tokens';
import { audioConfigurationDefaults } from '../../src/shared/generatedAudioConfiguration';
import { defaultNativeAudioSnapshot, type NativeAudioCommand, type NativeAudioResponse } from '../../src/shared/nativeAudio';
import type { PageContentProps } from '@renderer/components/settings/SettingsScreen';
import { setLanguagePreference } from '../../src/renderer/i18n/useI18n';

setLanguagePreference('zh-Hans');

declare global {
  interface Window {
    lingxi?: { microphoneAccess(): Promise<unknown> };
    __microphonePermissionTest?: {
      probe(): Promise<string>;
      permissionsApi(): Promise<string>;
      rowText(): string;
      openedSystemSettingsPanes(): string[];
      clickOpenSystemSettings(): boolean;
      refocus(): void;
    };
  }
}

const openedPanes: string[] = [];

/** Exactly `useBridge`'s own one-liner: the production reader over the production channel. */
const microphonePermission = () => hostMicrophonePermissionReader(window.lingxi)();

/**
 * Voice now reads the microphone value from the NativeAudioSnapshot returned
 * by `bridge.audioRequest({ type: 'get_snapshot' })`. Keep the current host
 * boundary in this fixture while sourcing the snapshot field from the real
 * production OS reader/IPC used by Electron. This is the fixture adapter for
 * NativeAudioManager's snapshot response; it does not stand in for the OS
 * answer or the page's focus refresh.
 */
async function microphoneSnapshot() {
  const snapshot = defaultNativeAudioSnapshot();
  snapshot.permissions.microphone = await microphonePermission();
  return snapshot;
}

async function audioRequest(command: NativeAudioCommand): Promise<NativeAudioResponse> {
  if (command.type !== 'get_snapshot') throw new Error(`unsupported microphone fixture audio request: ${command.type}`);
  return { type: 'snapshot', snapshot: await microphoneSnapshot() };
}

const bridge = {
  bootstrap: {
    settings: {
      model: 'anthropic/claude-opus-5',
      voice: audioConfigurationDefaults(),
      voiceRevision: 0,
    },
    providerCredentials: [{ providerId: 'anthropic', configured: true, encryptionAvailable: true }],
  },
  audioSnapshot: defaultNativeAudioSnapshot(),
  audioRequest,
  setVoicePreferences: async () => undefined,
  openSystemSettings: async (pane: string) => { openedPanes.push(pane); },
  microphonePermission,
};

function findOpenSystemSettingsButton(): HTMLButtonElement | null {
  return [...document.querySelectorAll('button')]
    .find((button) => button.textContent?.trim() === '打开系统设置') as HTMLButtonElement | undefined ?? null;
}

window.__microphonePermissionTest = {
  /** What the page's own probe reports, through the page's own dependency wiring. */
  probe: microphonePermission,
  /**
   * The value the page USED to show: the Permissions API, answered by this
   * app's own `setPermissionCheckHandler`. Measured, never asserted to be
   * right — it is the control in the comparison.
   */
  permissionsApi: async () => (await navigator.permissions.query({ name: 'microphone' as PermissionName })).state,
  rowText: () => document.body.innerText,
  openedSystemSettingsPanes: () => [...openedPanes],
  clickOpenSystemSettings: () => {
    const button = findOpenSystemSettingsButton();
    if (!button) return false;
    button.click();
    return true;
  },
  /** The real event macOS gives us when the user comes back from System Settings. */
  refocus: () => window.dispatchEvent(new Event('focus')),
};

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <Theme.Provider value={tokens(true)}>
      <Voice {...({ bridge } as unknown as PageContentProps)} />
    </Theme.Provider>
  </StrictMode>,
);
