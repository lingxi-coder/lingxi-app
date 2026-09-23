/**
 * Fixture for the microphone-permission read, mounted in a REAL Electron
 * renderer by `microphone-permission-electron.mjs`.
 *
 * What is REAL here: the `Voice` settings page component itself, mounted with
 * real effects; `hostMicrophonePermissionReader` — the exact production
 * reader the page calls; a real `contextBridge` preload
 * over real IPC to the production `readMicrophoneAccess` in the main process,
 * which reads the real `systemPreferences.getMediaAccessStatus('microphone')`;
 * and a real Chromium session carrying this app's own
 * `setPermissionCheckHandler` (copied verbatim into the driver), which is what
 * makes `navigator.permissions.query({name:'microphone'})` answer `granted`
 * for the app's own renderer no matter what macOS thinks.
 *
 * What is a stand-in: `bridge` (only the four members `Voice` touches) and the
 * `useBridge` hook around `hostMicrophonePermissionReader`, which is one line.
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
import type { PageContentProps } from '@renderer/components/settings/SettingsScreen';

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

const bridge = {
  bootstrap: {
    settings: {
      model: 'anthropic/claude-opus-5',
      voice: audioConfigurationDefaults(),
      voiceRevision: 0,
    },
    providerCredentials: [{ providerId: 'anthropic', configured: true, encryptionAvailable: true }],
  },
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
